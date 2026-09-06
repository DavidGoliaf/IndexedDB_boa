# M7-B — наряд на доработку по итогам приёмки

**Статус:** `REWORK REQUIRED`  
**Дата ревью:** 2026-09-05  
**Ветка:** `task/m7b-optimizations-reliability`  
**Норматив:** `tasks/09_TASK_M7_PERFORMANCE_RELIABILITY.md`, §§3–8; `AGENTS.md`.

M7-B содержит полезные и локально подтверждённые оптимизации (SQLite keyset
cursor, FS quota/frame packing, SQLite blob-sweep fast path), но **не может
быть принят как завершённый этап**. Handoff сам имеет статус `in progress`, а
несколько прямых условий ТЗ остаются `PARTIAL` или нарушены. До повторной
сдачи не начинать следующий milestone.

## Что уже подтверждено ревью

- `sqlite_cursor_paging_tests`: 5/5 PASS;
- `quota_delta_tests`: 3/3 PASS;
- короткие allocation-overhead tests: 2/2 PASS;
- `cargo check --manifest-path fuzz/Cargo.toml`: PASS;
- keyset/FS quota изменения имеют адресные regression tests;
- nightly workflow и fuzz targets добавлены, но их **первый реальный запуск
  ещё не является evidence**.

Это не отменяет требования ниже.

## P1-1. Убрать или явно согласовать `unsafe`

### Факт

`crates/boa_idb/tests/memory_gates_tests.rs` вводит file-wide
`#![allow(unsafe_code)]`, `unsafe impl GlobalAlloc` и unsafe calls. Правило
репозитория — «No `unsafe` in code; escalate and stop». Запись в
`QUESTIONS.md` уведомляет о риске, но не является согласием заказчика и не
отменяет stop-condition.

### Требуемый результат

Выбрать и документировать один вариант до дальнейшей реализации:

1. **Предпочтительно:** удалить custom global allocator и заменить portable
   in-process измерение безопасной методикой. Возможны: отдельный
   process/RSS gate, Linux `valgrind massif`, или поддерживаемый готовый
   инструмент. Новая dependency требует ADR до добавления.
2. Если без данного shim невозможно выполнить требование, остановиться и
   получить **явное** разрешение заказчика именно на этот ограниченный
   test-only `unsafe` scope. До такого разрешения не маркировать memory gate
   PASS и не расширять unsafe code.

В обоих вариантах production crates должны оставаться без unsafe, а handoff
должен перестать утверждать отсутствие отклонений без согласования.

## P1-2. Закрыть полный cursor memory scope

### Факт

Текущий gate не соответствует §4 и §5 ТЗ:

- store walk выполняется только в `Direction::Next`;
- index walks выполняются только для memory/SQLite и на 100 000 rows;
- FS index вообще не проверяется;
- FS store scale снижен до 200 000 вместо 1 000 000.

Нельзя считать «та же cursor contract» доказательством отсутствия range
materialization для непроверенных backend/source/direction combinations.

### Требуемый результат

1. Добавить full 1M gates для **store и index**, `Next` и `Prev`, на
   memory, SQLite и FS.
2. Каждый вариант обязан проверять: точное число строк, порядок/отсутствие
   дублей, live-allocation/RSS bound, и отсутствие роста, пропорционального
   размеру range.
3. Если full FS matrix не укладывается в обычный PR budget, вынести именно
   её в dedicated nightly job с достаточным timeout. Но по умолчанию scale
   остаётся 1M; 100k/200k могут быть только smoke/diagnostic, не evidence.
4. Добавить receipt/artifact с machine, command, scale, bound и фактическими
   результатами каждого варианта.

## P1-3. Достичь нормативных coverage thresholds

### Факт

ТЗ требует `boa_idb_core ≥90 %` и `boa_idb ≥80 %`. В handoff указано
80.5 % и 75.5 % соответственно; nightly workflow устанавливает только
ratchet 78 % / 73 %. Ratchet полезен, но не заменяет абсолютное условие.

### Требуемый результат

1. Запустить воспроизводимую команду coverage с tests, bins/examples и тремя
   WPT backend runs; сохранить LCOV/HTML artifact.
2. Довести реальные line coverage до `boa_idb_core ≥90 %`, `boa_idb ≥80 %`
   без broad `#[cfg(coverage)]`, `#[ignore]` или artificial exclusions.
3. Заменить nightly floors на требуемые 90/80 (допустим extra ratchet выше
   них, но не ниже).
4. Для каждого исключённого из coverage source — либо удалить genuinely dead
   code отдельным review, либо добавить meaningful tests. Нельзя удалять
   живой API только ради процента.
5. Обновить traceability R13.2 на `PASS` только после artifact с фактическими
   процентами и успешного enforcing workflow.

## P1-4. Получить реальное nightly evidence

### Факт

Workflow wiring недостаточен для статуса PASS. Handoff прямо сообщает, что
первый запуск pending: fuzz 4h, massif, coverage ratchet и long lifecycle.
Кроме того, §5/§7 требуют nightly 10k differential и 200 crash iterations.

### Требуемый результат

1. Выполнить/дождаться одного успешного запуска каждого relevant workflow:

   - M7 coverage + LCOV artifact;
   - 10 000 seeded differential + `BOA_IDB_LONG_LIFECYCLE=1`;
   - 200 seeded FS crash iterations;
   - Linux massif gate;
   - минимум 4 часа fuzzing суммарно, с не менее четырьмя required target
     classes (SCF, key, WAL recovery, stateful transaction/segment).

2. В handoff указать run URL/ID, commit SHA, длительность, target list,
   artifacts и точные replay commands. Для fuzz/crash failure обязательно
   сохранить seed/corpus/artifact и replay.
3. Если CI/репозиторий недоступны исполнителю, это внешний blocker: не
   объявлять M7-B complete, а запросить запуск у владельца CI.

## P1-5. Реально включить blocking >10 % performance comparison

### Факт

§3 требует baseline artifact и блокирование регрессии >10 % на pinned/
labelled host. Текущий `bench-smoke` честно diagnostic, но authoritative
comparison оставлен долгом.

### Требуемый результат

1. Выбрать/зафиксировать labelled self-hosted benchmark host и его immutable
configuration (OS, CPU, storage, Rust/profile).
2. Сохранить baseline artifact с full commit/configuration provenance.
3. Добавить job, который сравнивает все семь scenarios с baseline и завершает
build ошибкой при regression >10 %, с понятным исключением только для
заранее утверждённого baseline reset.
4. Не использовать heterogeneous GitHub-hosted smoke measurements как
release evidence.

## P2. Укрепить методику JS↔core target

Сейчас F3 объявлен PASS как разность двух разных измерений (`21.5 - 6.9 µs`).
Это полезная диагностика, но не прямой criterion result сценария. Добавить
парный benchmark в одном harness/iteration: identical setup and drain path,
с request и без request, затем публиковать raw samples и derived delta с
variance. Пока target тонкий, отдельный loaded outlier и формула должны быть
в receipt видны как limitation, а не скрыты средним.

## Ограничения rework

- Не менять IndexedDB JS semantics, error names, SCF/KEY/WAL/segment formats,
  strict durability или WPT expectations.
- Не снижать 1M scope, coverage targets либо nightly duration под видом
  оптимизации CI. Более короткие варианты допустимы только как smoke.
- Новая dependency — только после ADR.
- Не продолжать M8/следующую задачу до закрытия P1 или формального external
  blocker с владельцем и датой повторной проверки.

## Команды повторной локальной приёмки

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test -p boa_idb_sqlite --test sqlite_cursor_paging_tests
cargo test -p boa_idb_fs --test quota_delta_tests
cargo test -p boa_idb --test memory_gates_tests
cargo check --manifest-path fuzz/Cargo.toml
$env:RUSTDOCFLAGS = '-D warnings'; cargo doc --workspace --no-deps
```

Перед повторной сдачей обновить `docs/reviews/M7B-handoff.md` со статусом
`complete` только при закрытии всех P1; добавить реальные nightly receipts и
изменить traceability на PASS лишь там, где gate действительно существует и
выполнен.

## Критерии повторной приёмки

- [ ] `unsafe` удален либо имеет явное разрешение заказчика и точный scope.
- [ ] 1M memory matrix покрывает store/index × Next/Prev × все backends.
- [ ] Coverage actuals и enforced floors достигают core 90 / boa_idb 80.
- [ ] Есть successful artifacts всех nightly requirements, включая 200 crash
  и ≈4h fuzz.
- [ ] Labelled host блокирует performance regression >10 %.
- [ ] Handoff больше не имеет `in progress`; все claims подкреплены
  командой/artifact/commit provenance.
