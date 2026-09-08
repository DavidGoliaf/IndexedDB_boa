# M7-A — наряд на доработку по итогам приёмки

**Статус:** `REWORK REQUIRED`  
**Дата ревью:** 2026-09-05  
**Ветка:** `task/m7-performance-reliability`  
**Нормативный документ:** `tasks/09_TASK_M7_PERFORMANCE_RELIABILITY.md`, §§2, 3, 6–8.

M7-A нельзя сдавать до закрытия трёх P1 ниже. Цель доработки — сделать
наблюдаемость действительно невмешивающейся, восстановить честность
нормативной benchmark evidence и вернуть зелёный обязательный gate. Не
начинать задачи M7-B и не менять семантику IndexedDB, storage formats,
durability или WPT expectations.

## P1-1. Не вызывать observer под mutex engine/driver

### Факт

В `process_opens` для `OpenStep::StartUpgrade` одновременно удерживаются
`engine_guard` и `DriverState` guard. Вызванная оттуда `start_upgrade_txn`
вызывает `crate::observer::emit` для `TransactionBegun` до возврата в
`process_opens`.

Это нарушает собственный контракт:

```rust
// crates/boa_idb/src/observer.rs
// on_event runs ... with no engine/driver locks held.
```

Файлы и ориентиры:

- `crates/boa_idb/src/driver.rs`: `process_opens`, ветка `StartUpgrade`;
- `crates/boa_idb/src/driver.rs`: `start_upgrade_txn`, эмиссия
  `IdbEvent::TransactionBegun`;
- `crates/boa_idb/src/observer.rs`: module-level discipline и
  `ObserverState::emit`.

### Требуемая доработка

1. `start_upgrade_txn` не должна вызывать observer или JS callbacks.
   Пусть она возвращает идентификатор транзакции и plain-data payload события
   (либо данные, достаточные для его построения).
2. В `process_opens` полностью освободить оба guards, обновить нужное состояние
   драйвера, затем отправить `TransactionBegun`.
3. Сохранить порядок событий: `TransactionBegun` должен наблюдаться до
   `upgradeneeded` и до terminal event этой транзакции.
4. Просмотреть все новые пути `observer::emit` в M7-A. Для каждого подтвердить
   scoped drop guards до вызова callback; не ограничиваться upgrade-path.

### Обязательный тест

Добавить регрессионный тест, который доказывает порядок upgrade lifecycle и
не допускает возвращения эмиссии под lock. Тест должен покрыть как минимум:

- один `TransactionBegun` для versionchange;
- `TransactionBegun` раньше JS `onupgradeneeded` / его terminal event;
- успешное завершение lifecycle без deadlock.

Если возможно выразить это без ненадёжных таймаутов, observer в тесте должен
синхронно выполнить безопасную read-only проверку runtime state. Не передавать
и не удерживать `Context` или IDB objects внутри observer.

## P1-2. Не выдавать 20k cursor scan за нормативный 1M scenario

### Факт

`SCAN_FILL` установлен в 20 000, хотя M7 §4 требует проверку cursor на
1 000 000 записей. При этом:

- `benches/receipts/README.md` называет default scale нормативным и говорит о
  cursor fixture до 1M;
- receipt показывает 20k rate как `PASS / PASS`;
- raw log уже показывает, что SQLite на 1M имеет сверхлинейный cliff.

Результат на 20k не является evidence достижения target full cursor scan при
известной зависимости времени от размера range.

### Требуемая доработка

1. Вернуть 1 000 000 как **default normative** scale для полного cursor scan.
   `20k` допускается только под отдельным diagnostic benchmark id или через
   явно named diagnostic env/config; он не должен подменять normative scenario.
2. Сделать нормативное измерение завершамым и воспроизводимым при текущем
   miss. Допускается отдельный one-pass benchmark/runner с зафиксированным
   timeout, `sample_size = 1` или эквивалентом, если Criterion не может
   собрать статистику за разумное время.
3. Receipt должен содержать фактическое время/throughput 1M запуска или
   явный timeout с командой, timeout value и доказательством незавершения.
   В таком случае verdict — `MISS`, не `PASS`.
4. 20k таблицу переименовать в diagnostic result и исключить из подсчёта
   backend targets. Обновить handoff и `docs/traceability.md`, чтобы R12.1
   оставался `PARTIAL` и не создавал ложного PASS.
5. Не оптимизировать SQLite cursor в этой доработке: profiling и исправление
   асимптотики остаются M7-B. Разрешены лишь изменения harness/receipt,
   необходимые для честного измерения.

### Обязательные проверки

- default command действительно выбирает 1M cursor case;
- smoke command по-прежнему остаётся малым и diagnostic:
  `BOA_IDB_BENCH_SMOKE=1 cargo bench -p boa_idb --bench backends`;
- нормативный 1M runner воспроизводимо сообщает result или controlled timeout;
- receipt содержит platform, commit, Rust, profile, точные параметры и raw
  output/путь к нему.

## P1-3. Вернуть зелёный `cargo test --workspace`

### Факт

На текущей ветке команда падает:

```text
boa_idb_sqlite --test sqlite_migration_tests
test_old_schema_version_is_migrated
assertion failed: left 0, right 1
```

Handoff называет это pre-existing, но Definition of Done требует зелёный
workspace и CI. Констатация старого дефекта не является исключением из gate.

### Требуемая доработка

Выбрать один из двух путей и явно отразить его в handoff:

1. **Предпочтительно:** исправить migration test/production migration в
   отдельном, минимальном commit на текущей ветке, с объяснением root cause и
   дополнительным regression test. Не менять M7 scope сверх минимально
   необходимого repair.
2. Если исправление признано отдельным work order, остановить сдачу M7-A и
   запросить у заказчика формальное исключение с владельцем и трекером долга.
   Без такого согласования M7-A не принимается.

## P2. Документационный gate должен исполняться в CI

Добавить в `.github/workflows/ci.yml`:

```sh
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

M7-A исправляет broken intra-doc links и заявляет strict doc command в
handoff; gate должен быть проверяемым на PR, а не только локальной практикой.

## Ограничения

- Не начинать M7-B optimizations, dhat/valgrind, coverage/fuzz/nightly work.
- Не менять public JS API, error mapping, WAL/segment/SCF/key formats,
  durability guarantees или WPT expectations.
- Новая dependency не требуется и не допускается без ADR.
- Не переписывать benchmark targets: miss следует фиксировать как miss.

## Команды повторной приёмки

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test -p boa_idb --features tracing --test observer_tests
$env:RUSTDOCFLAGS = '-D warnings'; cargo doc --workspace --no-deps
$env:BOA_IDB_BENCH_SMOKE = '1'; cargo bench -p boa_idb --bench backends
```

Для `cargo deny check` использовать workspace-local advisory cache:

```powershell
$env:CARGO_DENY_DB_PATH = (Join-Path $PWD 'target/cargo-deny-advisories')
cargo deny check
```

Если cache отсутствует и сеть недоступна, приложить точный вывод как
environmental limitation; не маркировать это как passing gate.

Перед повторной сдачей обновить `docs/reviews/M7A-handoff.md`: commits,
все команды и их реальные результаты, normative/diagnostic benchmark boundary,
raw receipt, remaining M7-B debt и отсутствие отклонений без согласования.

## Критерии приёмки rework

- [ ] Ни один `IdbObserver::on_event` не вызывается при удержании engine или
  driver mutex; upgrade path покрыт тестом.
- [ ] 1M full cursor scan остаётся нормативным сценарием; 20k не объявлен
  evidence target и не может дать ложный PASS.
- [ ] Есть воспроизводимый 1M result либо controlled timeout с verdict `MISS`.
- [ ] `cargo test --workspace` проходит либо есть формально согласованное
  исключение с владельцем долга.
- [ ] Strict docs command включена в CI и проходит.
- [ ] Handoff и traceability честно отражают статус M7-A/M7-B.
