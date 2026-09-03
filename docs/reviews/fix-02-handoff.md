# Handoff: fix-02-engine-memory

Branch: `task/fix-02-engine-memory`, base: `master@f2d0833` (fix-01 merged).
Scope: ревью-дефекты TASK-02 (ядро `boa_idb_core::engine`, `boa_idb_memory`).
Трейт `BackendTxn` **не менялся** (совместимость с будущим fix-04).

## Что исправлено

### Ядро (`boa_idb_core::engine`)

- **D1 — индексное обслуживание в ядре (R8.1.2).** `ops_store.rs`:
  `put` декодирует старое значение и через `sync_indexes_on_put` удаляет
  устаревшие индексные записи и вставляет новые (неизменные пропускаются —
  нет ложных `unique`-нарушений при self-overwrite); `delete`/`clear`
  сначала собирают первичные ключи сканом диапазона (`collect_range_keys`),
  затем чистят индексы через `index_delete_by_primary`.
- **D3 — персист генератора.** `put` в конце пишет
  `txn.key_gen_set(store, keygen.current())` — идёт через savepoint-механику
  бэкенда, откат запроса восстанавливает и генератор.
- **D9 — multiEntry dedup.** `index_keys_for_value`: раскрытие только при
  `multi_entry`, dedup закодированных ключей с сохранением порядка.
- **D10 — маппинг ошибок.** `backend_err`: `Constraint→Constraint`,
  остальное → `Data` (раньше всё глоталось в `Data`).
- **D8 — keygen off-by-one.** `generate`: `>= MAX` → `> MAX`
  (`2^53-1` выдаётся, ошибка только выше).
- **D13 (keygen)** — `commit/rollback_request` возвращают
  `Result<(), IdbError>` (`InvalidState` на несбалансированном стеке).
- **D15** — убран `unwrap()` из `scheduler::poll_ready`.
- **D11** — `engine::connection` подключён (`pub mod connection`).

### Memory-бэкенд (`boa_idb_memory::txn`)

- **D2 — merge вложенных savepoint.** `commit_request` сливает undo-опсы в
  родительский уровень вместо отбрасывания.
- **D13 (backend)** — `commit/rollback_request` на пустом стеке →
  `BackendError::Internal` (было молчаливое no-op).
- **D4 — `*unique` dedup.** Index-скан: сортировка `(idx,pkey)`,
  collapse групп до первой записи (наименьший pkey), reverse для `prev*`.
- **D5 — pending-merge для индексов.** `count(Index)`, `scan(Index)` и
  `find_index_entries_by_primary` видят незакоммиченные вставки/удаления.
- **D6 — значения в индексном скане.** Не-key-only курсоры резолвят записи
  через merged view (`read-your-writes`).
- **D7 — undo для pending-only.** `delete_range`/`clear` логируют undo и
  считают только живые записи (раньше уже-удалённые tombstones
  пересчитывались, откат их не восстанавливал).
- **Найдено дифференциальным тестом сверх ревью: неверная семантика undo.**
  `UndoOp` хранил *merged*-значение; вложенный rollback воскрешал записи
  (tombstone внешнего уровня терялся). Введён `PendingSlot::Absent /
  Deleted / Value` — undo хранит *pending*-состояние. Плюс `clear`
  переписан через union ключей + tombstones (старый код снимал tombstone
  внешнего уровня до чтения committed-значения).

### Тесты

- Новые: `boa_idb_core/tests/{scheduler,keygen,open_queue}_tests.rs`
  (сценарные, поверх inline), `boa_idb_memory/tests/engine_ops_tests.rs`
  (13 тестов: stale-индексы, unique self-overwrite/cross-record,
  multiEntry dedup, delete/clear чистка, персист/откат keygen, nested merge,
  unbalanced savepoint, unique-курсоры, pending-видимость, pending-undo).
- `differential_model_tests.rs`: операции `SavepointBegin/Commit/Rollback`,
  снапшоты эталонной модели, lockstep-инвариант стеков, двунаправленная
  сверка состояния после rollback и в конце (10 000 кейсов).

## Верификация

- `cargo fmt --all -- --check` — чисто
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — 0
- `cargo test --workspace --no-fail-fast` — все сьюты ok, 0 failed
  (в т.ч. differential 10k, engine_ops 13)
- `cargo check -p boa_idb_core --target wasm32-unknown-unknown` — ok
- grep `unwrap/expect/panic/unreachable/todo/unsafe` в `engine/*` и
  `boa_idb_memory/src` — только `#[cfg(test)]`

## Известные ограничения / передано дальше

- **D12/D14** — осознанно не закрыты: `open_queue` без рассылки
  `versionchange` (нет такого варианта в `OpenQueueAction`;
  запрос вынимается до успеха), `request`/`transaction`/`registry`/
  `connection` — скелеты без семантики TZ §7 (Inactive, автокоммит,
  close-pending, версии соединений). Это работа fix-03 (L1-драйвер).
- `MemoryCursor::seek(Key)` на reversed-курсорах ищет `>=` без учёта
  направления — зафиксировать семантику directional seek в fix-03
  (L1-курсоры) вместе с живым позиционированием §6.7.
- Store-ветка `scan`/`count` перечитывает `committed_exists` под замком на
  каждую pending-запись — корректно, но O(n) локов; оптимизация при
  необходимости (общий merged-snapshot helper).
- `delete`/`clear` доверяют `delete_range`/`clear` бэкенда (чистят индексы
  по собранным ключам): контракт «бэкенд удаляет ровно диапазон» обязателен
  для fix-04 (sqlite) — там покрыть тестом.

## Demo

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --no-fail-fast
cargo test -p boa_idb_memory --test differential_model_tests
cargo test -p boa_idb_memory --test engine_ops_tests
```
