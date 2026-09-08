# План доработки M6-A — WAL atomicity и multi-frame commits

**Наряд:** `tasks/07_TASK_M6A_FILESYSTEM_BACKEND_FOUNDATION.md`  
**Ветка rework:** продолжать `task/m6-fs-foundation`  
**Статус:** M6-A не принят; требуются два исправления P1.  
**Граница:** не начинать M6-B (segments, MVCC, compaction, полный crash matrix).

## 1. Результат, который должен быть достигнут

После rework WAL обязан обеспечивать следующее свойство:

> После любого crash/reopen состояние базы равно состоянию после некоторого
> префикса **целиком закоммиченных транзакций**. Ни одна операция из
> незавершённой или смешанной транзакции не применяется.

Одна транзакция с sequence `S` может состоять из frame-цепочки:

```text
CONTINUES(S) → CONTINUES(S) → ... → COMMIT(S)
```

Только полный такой набор применим. Frame с иным `txn_seq`, некорректными
flags, порчей CRC/length или внезапным EOF завершает recovery на последнем
ранее закоммиченном prefix. Последующие байты не интерпретируются.

## 2. P1-1 — не смешивать `txn_seq` при recovery

### Дефект

В `crates/boa_idb_fs/src/wal.rs::recover_committed_frames` текущая логика
добавляет каждый успешно decoded frame в единый `pending`, а при первом frame
с `COMMIT` переносит весь `pending` в `committed`. Она не проверяет, что
`CONTINUES` и завершающий `COMMIT` имеют одинаковый `txn_seq`.

Например, этот поток сейчас ошибочно считается двумя committed frames:

```text
frame A: txn_seq=41, flags=CONTINUES, op=Put(a)
frame B: txn_seq=42, flags=COMMIT,     op=Put(b)
```

`Put(a)` относится к неполной транзакции 41 и не имеет права пережить
recovery. Применение обеих операций — нарушение R8.3.1 и crash consistency.

### Инструкция исправления

1. Хранить sequence текущей pending-транзакции отдельно, например
   `Option<u64>`.
2. Когда pending пуст, принимать frame только как:
   - single-frame transaction (`COMMIT`, без `CONTINUES`), либо
   - начало multi-frame transaction (`CONTINUES`, без `COMMIT`).
3. Когда pending не пуст, следующий frame обязан:
   - иметь тот же `txn_seq`;
   - быть `CONTINUES` или финальным `COMMIT` согласно определённой таблице
     flags;
   - не открывать новую sequence, пока старая не завершена.
4. При sequence mismatch или недопустимой комбинации flags очистить pending,
   завершить scan и оставить `valid_prefix_len` на конце последней уже
   закоммиченной транзакции.
5. Не "пропускать" дефектный frame в поисках следующего `COMMIT`: это могло
   бы склеить несвязанные транзакции и скрыть corruption.

### Обязательные тесты

Добавить в `wal.rs` unit-тесты, а минимум один — integration test с фактическим
reopen базы:

- `CONTINUES(41)` + `COMMIT(42)` → нет применённых операций 41 и 42, valid
  prefix заканчивается до frame 41 (либо до предыдущей корректной транзакции);
- `COMMIT(41)` + `CONTINUES(42)` + `COMMIT(43)` → recovery оставляет только
  транзакцию 41;
- цепочка `CONTINUES(41)` + `COMMIT(41)` → применяется целиком;
- некорректные комбинации flags (`0`, `CONTINUES|COMMIT`, если их семантика не
  определена) имеют явный ожидаемый результат и не паникуют.

## 3. P1-2 — реализовать multi-frame commit

### Дефект

`FsTxn::commit` в `crates/boa_idb_fs/src/txn.rs` строит ровно один
`WalFrame { flags: FLAG_COMMIT, ops }`. Если payload превышает
`MAX_FRAME_PAYLOAD`, `encode_frame` возвращает ошибку, хотя R8.3.1 и M6-A
требуют использовать `CONTINUES` для продолжения одной транзакции.

### Инструкция исправления

1. До записи WAL разбить `ops` на непустые группы, каждая из которых после
   полного encoding укладывается в `MAX_FRAME_PAYLOAD`.
2. Все созданные frames имеют один `txn_seq`.
3. Каждый frame кроме последнего получает **только** `FLAG_CONTINUES`; последний
   получает **только** `FLAG_COMMIT`.
4. Не делить отдельную operation на части. Если единственная operation сама
   превышает лимит, вернуть нормализованный `QuotaExceeded`/`ValueTooLarge`
   до записи первого frame; не оставлять частичную транзакцию в WAL.
5. Сформировать все bytes frames до первого append. Если append/sync любой
   части цепочки завершается ошибкой, откатить WAL строго к длине до
   транзакции. Ошибка rollback не должна маскировать исходную IO ошибку;
   последующий recovery всё равно должен отбрасывать partial suffix.
6. При schema commit сохранять порядок durability: WAL с финальным COMMIT
   синхронизирован до публикации `meta.scf`. Для `Relaxed` это исключение
   остаётся, если schema metadata публикуется после WAL.
7. Не менять формат `ScValue`, публичный IDB API или `expectations.json`.

### Обязательные тесты

Для testability разрешается вынести размер frame в локальную тестовую helper
или собрать достаточно много допустимых `WalOp`; не снижать production
`MAX_FRAME_PAYLOAD`.

- encoding большой transaction даёт минимум два frames с одинаковым sequence
  и ожидаемыми flags;
- normal recovery применяет все операции multi-frame transaction;
- torn tail после первого `CONTINUES` не применяет ни одной операции этой
  transaction;
- ошибка SyncHooks на втором append/sync откатывает WAL к исходной длине и
  reopen не видит частичных данных;
- operation, превышающая лимит в одиночку, не меняет WAL и состояние базы;
- старый single-frame commit продолжает иметь один `COMMIT` frame.

## 4. Не расширять scope

В rework запрещено добавлять:

- segment files, compaction или refcount;
- persistent immutable map / O(1) snapshots;
- blob persistence;
- новый WPT snapshot или смягчение expectations;
- несвязанные обновления Boa, Cargo.lock или dependency versions.

Новая dependency, `unsafe`, изменение формата SCF/KEY или прогноз нового diff
свыше ~3000 строк требуют остановки и явного вопроса заказчику. Существующий
общий diff уже 3778 строк; исправление должно быть локальным и не увеличивать
объём работ за пределы P1.

## 5. Обновления документации и повторная сдача

После исправления:

1. Обновить `docs/reviews/M6A-handoff.md` с точным описанием multi-frame
   protocol и результатами новых тестов.
2. Обновить `docs/traceability.md`: R8.3.1 можно оставить PASS только если
   sequence isolation и multi-frame recovery покрыты тестами. R8.3.3, R8.3.4
   и R8.5 остаются PARTIAL до M6-B.
3. Сохранить ADR-009, если выбранный формат flags не меняется. Если его
   семантика уточняется, дополнить ADR кратким правилами sequence/flags.
4. Сдать отдельный commit rework и чистый `git status`.

## 6. Обязательная повторная верификация

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p boa_idb_fs
cargo test --workspace
cargo doc --workspace --no-deps
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny check
```

В handoff приложить краткий лог с числом новых WAL regression tests и
подтвердить, что тесты невозможно было бы пройти до исправления. Повторная
приёмка отдельно проверит crafted sequence mismatch, incomplete multi-frame
tail и failure посередине append/sync.
