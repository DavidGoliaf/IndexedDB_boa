# ТЕХНИЧЕСКОЕ ЗАДАНИЕ: TASK-07 / M6-A

## Файловый backend — фундамент WAL, recovery и блокировка базы

| Метаданные | Значение |
|---|---|
| **Нормативный этап** | M6 «Файловый бэкенд» из `TZ_boa_idb_IndexedDB.md`, §8.3 и §8.5 |
| **Идентификатор work order** | `TASK-07-M6A-FILESYSTEM-BACKEND-FOUNDATION` |
| **Ветка** | `task/m6-fs-foundation`, от принятого M6 hardening revision |
| **Целевой crate** | `crates/boa_idb_fs` |
| **Результат этого наряда** | Надёжный минимальный persistent backend с WAL/recovery/lock; **не** приёмка полного нормативного M6 |

## 1. Почему M6 разделён на два наряда

Нормативный M6 включает WAL, сегменты, манифест, compaction, MVCC snapshots,
мультипроцессные lock и crash matrix. Это превышает ограничение рабочего
контракта на один diff (~3000 строк) и создаёт риск смешать независимые
инварианты durability и производительности.

Этот work order реализует фундамент, без которого нельзя безопасно начать
M6-B: дисковую топологию, журнал, атомарный commit/recovery и lock. M6-B
отдельно реализует immutable segments, O(1)/O(log n) readonly snapshots,
compaction, полный fault/crash matrix и подключение FS к WPT/differential
runner. В handoff запрещено называть M6 целиком завершённым.

## 2. Нормативная цель и границы

Нужно реализовать `BackendFactory`, `Storage`, `Database`, `BackendTxn` и
`BackendCursor` из `boa_idb_core::backend::traits` для `boa_idb_fs`, сохраняя
семантику memory/SQLite backend: schema operations, savepoints, записи,
индексы, курсоры, key generators, commit и abort.

Файловая топология должна следовать §8.3:

```text
<root>/<storage_key_hash>/<db_name_hash>/
  LOCK
  CURRENT
  MANIFEST-<seq>
  meta.scf
  wal/<seq>.log
```

`seg/` и `blob/` могут быть созданы только если они нужны для согласованной
топологии, но не должны стать фиктивной реализацией M6-B. Не реализовывать
compaction или snapshot, обозначая их как «готовые».

В scope:

1. детерминированное hashing/naming storage key и database name, без path
   traversal;
2. `LOCK` с эксклюзивной advisory-блокировкой при open;
3. versioned `CURRENT` + `MANIFEST` и метаданные базы;
4. WAL frame из R8.3.1 с `IWAL`, length, txn sequence, flags, payload и
   CRC32C;
5. recovery: применить только валидный prefix WAL и отбросить torn/corrupt
   tail без panic;
6. один атомарный transaction commit: ни abort, ни незавершённый frame не
   меняют восстановленное состояние;
7. `Durability::{Relaxed,Strict}` согласно R8.3.2; strict выполняет sync WAL
   до успешного `commit`;
8. конфигурация `max_keys_in_memory`, default 5 000 000, с
   `QuotaExceededError` при превышении;
9. README с расчётом `keys × (key_len + 48)` и рекомендацией SQLite для
   больших баз.

Вне scope M6-A:

- immutable `seg/*.seg` и блоковые индексы;
- compaction и refcount старых сегментов (R8.3.3);
- O(1)/O(log n) MVCC readonly snapshots (R8.3.4);
- externalized blobs;
- WPT runner support для FS и критерий M6 ≥92 %;
- 200 kill/fault iterations nightly (подготовить test seams, выполнить в M6-B);
- benchmark targets M7.

## 3. Архитектурные обязательства

### 3.1. ADR до новой зависимости

Решение для persistent map/snapshot, file locking, encoding и atomic replace
должно быть зафиксировано до использования. Каждая новая dependency требует
абзац в `docs/DECISIONS.md`: поддерживаемость, популярность и permissive
license. Не добавлять `unsafe`; не писать platform-specific FFI вручную.

Если portable advisory lock нельзя реализовать с текущим безопасным API и
без неподходящей зависимости, остановиться и задать вопрос в `QUESTIONS.md`.

### 3.2. WAL и recovery

- Encode/decode WAL вынести в отдельный тестируемый модуль. Проверять magic,
  length overflow/limit, CRC и sequence.
- После первой некорректной записи recovery прекращает чтение и не применяет
  ни байта последующего хвоста. Хвост физически truncate только после
  успешной валидации и с обработкой ошибки IO; иначе открыть read-only
  recovery state либо вернуть нормализованную backend ошибку.
- Коммит с несколькими frame использует `CONTINUES`/`COMMIT`; recovery не
  применяет неполную транзакцию.
- Никаких `unwrap`, `expect`, `panic!` или silent data loss в production path.
  Все ошибки filesystem переводятся в подходящий `BackendError` и далее в
  правильный DOMException path.

### 3.3. Manifest и sync ordering

`CURRENT` указывает только на полностью записанный manifest. Любая замена
делается в порядке: temporary file → write → file sync → rename → directory
sync, если платформа предоставляет безопасный API. Порядок и platform
ограничения документировать и покрыть fault-oriented tests. Не перезаписывать
актуальный manifest на месте.

### 3.4. Transaction model

Разрешён временный in-memory ordered index для M6-A, но он должен
детерминированно строиться из manifest/meta + WAL при reopen. Write
transactions используют savepoints как остальные backend. Readonly snapshot в
M6-A может быть documentable copy-on-write только как переходное решение;
отдельно измерить и написать в handoff, что R8.3.4 ещё не закрыт. Нельзя
объявлять O(1)/O(log n) без измерения и design M6-B.

## 4. Обязательные тесты

Добавить `crates/boa_idb_fs/tests/` и unit/property tests для WAL.

- полный CRUD/schema/index/cursor/key-generator parity минимум с базовыми
  сценариями memory backend;
- persistence после close/reopen;
- abort и request savepoint не попадают в WAL/recovered state;
- torn WAL: порча последних N байт каждого типа frame; open не паникует,
  восстановленное состояние соответствует только committed prefix;
- случайные malformed length/CRC/magic (property test) не приводят к panic;
- strict и relaxed различаются через injectable file-sync observer, без
  зависящей от timing проверки;
- второй open того же DB возвращает `UnknownError` пока LOCK удерживается;
  после штатного close база открывается;
- subprocess crash-style test: worker успевает записать WAL, процесс
  завершается принудительно, новый процесс открывает базу и валидирует prefix.
  На Windows вместо `SIGKILL` использовать platform-equivalent termination;
  если это невозможно стабильно в CI, подготовить worker и описать точное
  ограничение для M6-B, не выдавая test за пройденный R8.3.6;
- лимит `max_keys_in_memory`: значение на границе разрешено, следующее
  добавление отклоняется без частично записанной транзакции.

## 5. Критерии приёмки M6-A

- [ ] `boa_idb_fs` больше не placeholder и безопасно реализует backend traits.
- [ ] WAL/recovery имеют адресные tests; corrupted tail не вызывает panic и
  не меняет committed prefix.
- [ ] Strict commit синхронизирует WAL до возврата success; relaxed не
  обещает durability.
- [ ] Lock защищает от одновременного открытия и корректно освобождается при
  штатном close.
- [ ] Memory limit и документация R8.3.5 выполнены.
- [ ] Добавлен ADR для persistent data structure и всех новых crates.
- [ ] `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace`, `cargo doc --workspace --no-deps` и
  `cargo deny check` проходят.
- [ ] В `docs/traceability.md` R8.3.1, R8.3.2, R8.3.5 и реализованная часть
  R8.3.6 имеют точные tests; R8.3.3/R8.3.4/R8.5 остаются явно `PARTIAL` до
  M6-B, а не `PASS`.
- [ ] Создан `docs/reviews/M6A-handoff.md`: commits, команды, результаты,
  known gaps и план передачи в M6-B.

## 6. Условия остановки

Остановиться и запросить решение до изменения формата SCF/KEY, публичного
IDB API, добавления dependency с непермиссивной лицензией, внедрения `unsafe`,
изменения expectations/WPT snapshot либо когда прогноз diff превышает 3000
строк. В последнем случае предложить ещё один ограниченный наряд, а не
смешивать M6-A и M6-B.

## 7. Рекомендуемый исполнитель

Владелец: `gpt-5.6-sol` с reasoning `high` или `xhigh`. Нужны знания Rust IO,
durability ordering, crash recovery и свойства транзакций. Модель среднего
уровня может подготовить изолированные codec/property tests после ADR, но не
должна самостоятельно выбирать формат WAL, locking или recovery semantics.
