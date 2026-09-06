# ТЕХНИЧЕСКОЕ ЗАДАНИЕ: TASK-10 / M7-C

## M7-C — CI evidence, labelled benchmark gate и финальная сдача M7

| Метаданные | Значение |
|---|---|
| **Нормативный этап** | завершение M7 из `TZ_boa_idb_IndexedDB.md`, §12 и R13.1/R13.2/R13.4.1/R13.6 |
| **Идентификатор work order** | `TASK-10-M7C-CI-EVIDENCE-FINAL-ACCEPTANCE` |
| **Базовая ветка** | текущий tip `task/m7b-optimizations-reliability` после commit `3d29f8b` |
| **Предшественники** | M7-A и M7-B code scope; `docs/reviews/M7B-handoff.md` |
| **Результат** | проверяемые CI run URLs/artifacts, labelled baseline, final `M7-handoff.md`, M7 traceability `PASS` только по доказанным требованиям |

## 1. Цель и граница

M7-A/M7-B уже поставили code-side harness, оптимизации, local receipts,
absolute coverage gate, nightly workflows и fail-closed comparator с pinned
host ID. M7-C **не открывает новую оптимизацию и не меняет IndexedDB
семантику**. Его единственная цель — получить настоящее CI evidence и закрыть
M7 документально, либо честно зафиксировать внешний blocker.

Не включать в M7-C:

- новый backend/refactor, изменение SCF/KEY/WAL/segment format или WPT
  expectations;
- понижение target/coverage/fuzz duration/memory scope;
- переделку M6/M8 задач;
- замены required gate локальным receipt или handoff-текстом.

Точечная правка workflow/comparator разрешена **только**, если её требует
фактически запущенный CI job. Она должна сопровождаться regression test и
повторным успешным run URL.

## 2. Внешняя подготовка — владелец CI

Это действия владельца CI, а не повод объявлять агентскую работу завершённой
без evidence.

1. Provision выделенного Linux x64 self-hosted runner с labels
   `self-hosted`, `bench`: стабильная OS image, CPU governor, storage,
   toolchain, без co-tenants, с разумным benchmark timeout.
2. В защищённой runner/repository/environment configuration задать
   `BOA_IDB_BENCH_HOST_ID`. Его не добавлять в workflow YAML, baseline из PR
   или обычный repo file. Это inventory ID, не runner registration token.
3. Добавить GitHub required check `bench-regression` для PR в `main` после
   успешного первого run. Сохранить URL настройки/check и PR run URL.
4. Обеспечить GitHub Actions права на artifacts и cache. Runner token,
   credentials и любые секреты не помещать в репозиторий или handoff.

Если хотя бы один пункт недоступен, задача получает `EXTERNAL BLOCKER` с
владельцем, причиной и датой повторной проверки; `M7` не переводить в PASS.

## 3. Labelled baseline и blocking benchmark evidence

На provisioned runner, в clean tree текущего M7 SHA:

```sh
BOA_IDB_BASELINE_ROLE=labelled \
python3 scripts/bench_compare.py write \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json

python3 scripts/bench_compare.py compare \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
```

Требования:

1. Baseline должен иметь `host_role: labelled`, `host_id`, commit SHA,
   OS/CPU/arch/Rust/profile, diagnostic fingerprint и полный набор scenario
   means. Нельзя переименовывать или hand-edit `m7b-label-ref.json`:
   он остаётся interim/diagnostic.
2. Закоммитить labelled baseline отдельным логичным commit и сохранить SHA.
3. Открыть PR, запускающий `bench-regression`; приложить URL/run ID и
   `bench-compare.log`. Он обязан PASS на том же host ID.
4. Получить отрицательное доказательство без реальной регрессии production
   кода: unit test comparator уже проверяет 10.01% failure; в handoff дать
   его command/result. Не подменять baseline или benchmark source в PR для
   искусственного красного run.
5. После required-check настройки показать, что PR cannot merge при
   отсутствующем/failed check. Если политикой GitHub это проверяется вне
   репозитория, зафиксировать owner и URL/скриншот настройки в handoff.

## 4. Inaugural nightly evidence

Запустить `nightly-m7.yml` на SHA, содержащем M7-C baseline commit. Сохранить
одну URL/ID верхнего workflow и URLs/IDs jobs, точные команды и artifacts:

| Job | Обязательное evidence |
|---|---|
| `coverage` | `lcov.info`, coverage JSON, totals core ≥90%, `boa_idb` ≥80%, exit 0; tests/bins/examples и WPT memory+SQLite+FS |
| `differential-and-lifecycle` | success log: 10 000 seeded scenarios и `BOA_IDB_LONG_LIFECYCLE=1`; при failure — seed + replay |
| `cursor-matrix-1m` | `matrix-1m.log`, `receipt-matrix-1m.txt`, 12 combinations store/index × Next/Prev × memory/SQLite/FS, scale 1M, bound/result |
| `memory-massif` | `massif.out`, Valgrind version, peak heap и gate result |
| `fuzz` | пять target jobs по 48 min (минимум четыре required classes, суммарно ≥4 h), final stats, corpus/cache identity; crash artifacts + replay if any |

Отдельно запустить `nightly-fs-crash.yml` с M7 nightly scale
`BOA_IDB_FS_CRASH_ITERS=200`; приложить run URL/ID, seed/replay output и
подтверждение зелёной M6 fault matrix.

Если hosted CI не может выполнить job, не редактировать workflow чтобы
ослабить scope. Оставить status `PARTIAL` и оформить точную инфраструктурную
причину, owner и recheck date.

## 5. Документация и финальный handoff

1. Создать `docs/reviews/M7-handoff.md` (это обязательный deliverable §8
   исходного M7): commits, platform receipts, labelled baseline provenance,
   target deltas/profiles, coverage/fuzz/crash/memory results, known
   limitations, CI URLs/IDs, artifact names and replay commands.
2. Обновить `docs/reviews/M7B-handoff.md`: добавить ссылку на final M7
   handoff и снять `EXTERNAL BLOCKER` только после evidence.
3. Обновить `docs/traceability.md` синхронно. `R12.1`, `R12.2`, `R13.1`,
   `R13.1-fuzz`, `R13.2`, `R13.4.1`, `R13.6` получают `PASS` лишь если
   соответствующий artifact/run URL реально доступен. Не оставлять разные
   статусы для одного R-ID в handoff и traceability.
4. Для каждого failed target сохранить actual value, profile/reproducible
   profiler output, объяснение и ограниченный follow-up. Запрещено объявлять
   miss PASS или ослаблять target.

## 6. Локальная и финальная проверка

Перед handoff выполнить и приложить результаты:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test -p boa_idb --features tracing --test observer_tests
cargo check --manifest-path fuzz/Cargo.toml
$env:RUSTDOCFLAGS = '-D warnings'; cargo doc --workspace --no-deps
$env:CARGO_DENY_DB_PATH = 'target/cargo-deny-advisories'; cargo deny check
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend fs --summary
python scripts/test_bench_compare.py
git diff --check
```

`cargo deny` при недоступной advisory DB — не PASS. Нужны CI artifact или
локальный cache с указанными freshness/commit базы.

## 7. Приёмка M7-C и всего M7

- [ ] Labelled runner и protected host ID provisioned; labelled baseline
  создан на нём и закоммичен.
- [ ] `bench-regression` является actual required PR check и успешный run
  подтверждает baseline comparison; comparator fail-closed cases tested.
- [ ] Есть successful nightly evidence coverage, 1M matrix, Massif,
  differential/lifecycle и ≥4h fuzz.
- [ ] Есть successful `nightly-fs-crash` evidence на 200 iterations.
- [ ] M7 targets/misses имеют receipts/profiles/follow-ups без подмены
  requirements.
- [ ] `M7-handoff.md`, M7B handoff и traceability согласованы; каждая
  отметка PASS имеет URL/artifact/command/SHA.
- [ ] Полный локальный quality suite и memory/SQLite/FS WPT зелёные.

Если все пункты выполнены, M7-C сдаётся вместе с M7. Если остаётся хотя бы
один CI-инфраструктурный пункт, M7-C остаётся `EXTERNAL BLOCKER`; следующий
milestone не маркировать как зависимый от принятого M7.

## 8. Рекомендуемый исполнитель

Нужен сильный агент уровня `gpt-6-astra` с доступом к GitHub Actions и
настройкам self-hosted runner. Обычный coding agent может подготовить
baseline/handoff и диагностировать workflow, но не должен имитировать
внешние CI runs или менять branch protection без авторизации владельца.
