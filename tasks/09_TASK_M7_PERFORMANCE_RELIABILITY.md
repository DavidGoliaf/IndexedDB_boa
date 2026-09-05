# ТЕХНИЧЕСКОЕ ЗАДАНИЕ: TASK-09 / M7

## Производительность, наблюдаемость и reliability gate

| Метаданные | Значение |
|---|---|
| **Нормативный этап** | M7 из `TZ_boa_idb_IndexedDB.md`, §12, R13.1, R13.2, R13.4.1 и R13.6 |
| **Идентификатор work order** | `TASK-09-M7-PERFORMANCE-RELIABILITY` |
| **Базовая ветка** | новый `task/m7-performance-reliability` от принятого tip M6-B; не включать незакоммиченные изменения предыдущей приёмки без отдельного commit |
| **Основные crates** | `boa_idb_core`, `boa_idb_memory`, `boa_idb_sqlite`, `boa_idb_fs`, `boa_idb`, `boa_idb_wpt` только при необходимости |
| **Результат** | Воспроизводимые метрики и benchmark gate, устранённые доказанные узкие места, наблюдаемость, memory/reliability evidence |

## 1. Граница этапа

M6 уже закрыл корректность filesystem backend: WAL/segments/MVCC, fault matrix,
crash worker и FS-WPT. M7 не меняет семантику IndexedDB, формат SCF/KEY/WAL/
segments или expectations без отдельного review. Его задача — измерить фактическое
поведение, исправить только доказанные bottleneck/regression и поставить gates,
которые не позволят производительности и памяти незаметно деградировать.

Нельзя объявлять целевые цифры достигнутыми на произвольной машине. Любая
метрика без platform/commit/configuration/provenance — диагностическая, а не
release evidence.

## 2. Обязательный scope gate

До кода записать в `QUESTIONS.md` оценку по пакетам A–D. Если прогноз превышает
~3000 changed LOC, остановиться до реализации и предложить минимум два
последовательных наряда: `M7-A` (harness, baselines, observability) и `M7-B`
(оптимизации, memory/reliability gates). Нельзя совместить массовый рефакторинг
backends с созданием benchmark infrastructure в одном commit или скрыть
изменение поведения под видом оптимизации.

## 3. Benchmark harness и baseline — §12.1

1. Добавить criterion benchmarks в release-профиле с `lto = "thin"`; каждый
   benchmark использует изолированный temp-root, фиксированный seed, numeric
   keys, values около 200 B и указанную durability. Не включать setup/teardown
   в измеряемый участок, если сценарий явно не измеряет open/close.
2. Для **SQLite** и **FS** реализовать ровно эти сценарии:

   | Сценарий | SQLite | FS |
   |---|---:|---:|
   | `put` batch 10 000, ops/s | ≥60 000 | ≥120 000 |
   | primary-key `get`, ops/s | ≥100 000 | ≥300 000 |
   | полный cursor scan, records/s | ≥150 000 | ≥400 000 |
   | пустая readwrite transaction, ops/s | ≥5 000 | ≥10 000 |
   | JS↔core request без IO, µs | ≤15 | ≤15 |
   | open DB с 1 000 000 records, s | ≤0.2 | ≤3.0 |
   | одиночный strict commit, commits/s | ≥200 | ≥200 |

3. `memory` benchmark запускается как контроль, но не подменяет цели SQLite/FS.
   Для каждой метрики в JSON/Markdown receipt зафиксировать commit, Rust,
   OS/CPU/storage, profile, command, параметры и raw criterion output.
4. Сравнение с baseline допускается только на закреплённом benchmark host или
   c явно равной конфигурацией. CI обязан хранить baseline artifact и блокировать
   регресс >10 %; на обычных heterogeneous runners допустим лишь smoke/format
   run, а authoritative comparison переносится на labelled/self-hosted job.
5. Если цель не достигнута, handoff обязан содержать профиль (flamegraph или
   эквивалентный reproducible profiler output), величину отклонения, объяснение
   и ограниченный follow-up. Нельзя ослаблять target, удалять benchmark или
   выдавать локальную цифру за эталон.

## 4. Направленные оптимизации и асимптотика

Оптимизация разрешена только после baseline/profile и только в горячем пути.
До изменения записать гипотезу и ожидаемый выигрыш; после — сравнить
same-machine receipt до/после.

- Cursor для всех backends не материализует полный диапазон. Проверить 1 000 000
  records с ограничением памяти/счётчиком allocation: потребление остаётся
  O(1) относительно размера range; допустимы logarithmic seek/reopen.
- Полный `continue()` на 100 000 records не имеет квадратичной сложности.
- Readonly FS snapshot сохраняет O(1)/O(log n) start, установленный M6-B.
- Open/recovery не перечитывает superseded WAL/segments и не делает лишнего
  полного copy; compaction не разрушает throughput последующих commits.
- Не оптимизировать ценой ослабления strict fsync, snapshot isolation,
  atomicity, quota, tracing или test coverage.

## 5. Memory и долгоживущая надёжность — §12.2, §13.1

1. Добавить repeatable Linux gate c `dhat` или `valgrind` и долгий lifecycle
   test: open/close, transactions, cursor, compaction, DB delete. Он проверяет
   отсутствие unbounded RSS/live-allocation growth; результаты и tool version
   сохраняются artifact-ом.
2. Измерить и зафиксировать overhead open connection ≤64 KiB и live transaction
   ≤8 KiB (исключая declared backend caches). Если нельзя измерить портативно,
   сделать Linux authoritative test и явно документировать platform limitation.
3. `getAll` без лимита — единственная разрешённая large-memory операция;
   README/API docs должны явно это говорить. Cursor memory test должен покрывать
   store и index, прямое и обратное направления.
4. Nightly выполняет уже созданные M6 crash 200 iterations, расширенный
   differential generator на **10 000** seeded scenarios и выбранные memory
   gates. На failure выводятся seed и replay command.

## 6. Observability — §12.3

1. Добавить feature `tracing` (по умолчанию выключена, zero/near-zero cost when
   disabled) со spans `idb.open`, `idb.txn`, `idb.request`; атрибуты: database
   (без утечки key/value), mode, scope size, transaction sequence, duration и
   bytes read/written. Не логировать user data, storage keys или hashes без
   явной privacy review.
2. Реализовать/завершить `IdbObserver` из §4.3: counters begun/committed/
   aborted transactions, requests, read/written bytes, bounded latency
   histogram. Observer не должен изменять порядок event loop и не должен
   удерживать IDB objects/contexts.
3. Добавить dev-only `boa-idb-cli` (examples или binary): `ls`, `dump`,
   `verify`, `compact`, `stats`. `dump` по умолчанию выводит только metadata и
   key counts; раскрытие values требует явного `--values`. CLI не становится
   новым production public API без ADR.
4. Tests подтверждают span names/required fields, bounded histograms,
   observer lifecycle и отсутствие data leakage в default output.

## 7. Quality/coverage gates

1. Подключить `cargo-llvm-cov` gate: `boa_idb_core` ≥90 %, `boa_idb` ≥80 %.
   Порог, команда, exclusions и raw LCOV/HTML artifact фиксируются; запрещены
   broad `#[cfg(coverage)]`/ignore для обхода порога.
2. Завершить и nightly-run четыре fuzz targets: SCF decoder, key decoder,
   WAL recovery и один stateful transaction/segment decoder target. Суммарно
   не менее 4 часов; corpus и crash artifacts versioned без copyrighted data.
3. Обновить `docs/traceability.md` c адресными tests/commands для R12.*, R13.1,
   R13.2, R13.4.1 и R13.6. Статус `PASS` только при реально доступном gate;
   infrastructure, не запущенная в CI, остаётся `PARTIAL`.

## 8. Приёмка и handoff

- [ ] Criterion suite измеряет все семь сценариев §12.1 для SQLite и FS,
  формирует reproducible receipts и baseline comparison.
- [ ] Любой miss target имеет profile + письменное обоснование + follow-up;
  регресс >10 % над authenticated baseline блокируется.
- [ ] Cursor/memory tests доказывают отсутствие range materialization и
  lifecycle gates не показывают unbounded growth.
- [ ] 10 000 differential scenarios и 200 crash iterations запускаются nightly
  с seeded replay; fault matrix M6 остаётся зелёной.
- [ ] tracing, observer и privacy-safe dev CLI работают и покрыты tests.
- [ ] Coverage thresholds и четыре fuzz targets имеют CI/nightly evidence.
- [ ] `cargo fmt --all -- --check`, strict clippy, `cargo test --workspace`,
  `cargo doc --workspace --no-deps` **без warnings**, `cargo deny check` и
  memory/SQLite/FS WPT проходят.
- [ ] Создан `docs/reviews/M7-handoff.md`: commits, platform receipts,
  baseline artifacts, benchmark deltas, profiles, coverage/fuzz/crash results,
  known limitations и точные replay commands.

## 9. Стоп-условия и рекомендуемый исполнитель

Остановиться при необходимости `unsafe`, FFI, изменения форматов/JS API,
dependency с непермиссивной лицензией, правки WPT expectations, недостоверной
platform comparison или превышении scope gate. New dependency требует ADR.

Рекомендуемый исполнитель: `gpt-5.6-sol`, reasoning **xhigh**. Нужны Rust
profiling/criterion, filesystem and allocator diagnostics, CI provenance и
понимание IDB correctness. Модель среднего класса уместна лишь для
изолированных benchmark/test scaffolds после утверждения interfaces.
