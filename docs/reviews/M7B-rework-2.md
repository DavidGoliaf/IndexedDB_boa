# M7-B — наряд на доработку № 2

**Статус:** `REWORK REQUIRED`
**Основание:** повторная приёмка от 2026-09-06
**Ветка:** `task/m7b-optimizations-reliability`
**Норматив:** `tasks/09_TASK_M7_PERFORMANCE_RELIABILITY.md`, §3 и §7; `AGENTS.md`.

Предыдущие технические блокеры частично устранены: unsafe allocator заменён
на `dhat`, полный cursor harness и локальные receipts добавлены, coverage
threshold в nightly повышен до 90/80. Новая DOM-regression поправка также
покрыта тестами. Однако M7-B нельзя объявлять завершённым, пока отсутствует
работающий blocking performance gate и документация не отделяет фактически
полученные evidence от запланированной CI-инфраструктуры.

## P1-1. Сделать performance comparison настоящим PR-blocking gate

### Подтверждённый факт

`.github/workflows/bench-regression.yml` запускается только по `schedule` и
`workflow_dispatch`; он не создаёт check для pull request. В README/workflow
и handoff сказано, что проверку сделают required только после provisioning
runner. Следовательно, на текущем состоянии ни один PR не блокируется при
регрессии более 10%.

Проверяемый baseline
`crates/boa_idb/benches/baselines/m7b-label-ref.json` имеет
`"host_role": "interim"`. `scripts/bench_compare.py` использует scenario
means, но не отвергает baseline с ролью, отличной от `labelled`, и не
проверяет fingerprint машины. Поэтому interim Windows-измерение может быть
ошибочно использовано как норматив для self-hosted runner.

### Результат, который должен поставить агент

1. Реализовать `pull_request` trigger для `bench-regression`, ограничив его
   теми ветками, для которых gate действительно должен быть required. Job
   обязан публиковать status именно в проверяемый PR SHA.
2. В `scripts/bench_compare.py` сделать жёсткий отказ до запуска сравнения,
   если baseline не содержит `host_role == "labelled"`, обязательный
   provenance (commit, OS, CPU, architecture, toolchain/profile) или набор
   сценариев не совпадает с ожидаемым набором harness.
3. Сохранить отдельный **настоящий** baseline, снятый на pinned labelled
   host. Нельзя переименовывать существующий interim JSON или менять
   `host_role` вручную. Interim baseline может остаться только diagnostic и
   не должен быть аргументом `compare` в blocking workflow.
4. Добавить tests для comparator: labelled baseline PASS; interim baseline
   rejected; missing scenario rejected; искусственная регрессия 10.01%
   возвращает non-zero.
5. Передать владельцу CI точные labels runner, требование добавить
   `bench-regression` в required checks и URL/run ID первого успешного PR
   запуска. Это внешняя операция: без неё агент помечает пункт `EXTERNAL
   BLOCKER`, а не `PASS`.

### Критерий приёмки

- На labelled runner нормальный PR run создаёт check и проходит.
- Подмена результата любого scenario на `> 1.10 × baseline` делает PR check
  красным.
- Interim baseline не может быть принят comparator’ом.
- Handoff содержит baseline commit SHA, runner fingerprint, run URL/ID и
  replay command.

## P1-2. Привести handoff и traceability к доказанному состоянию

### Подтверждённый факт

`M7B-handoff.md` заявляет «all four acceptance blockers are closed», но в
нём же указывает, что labelled runner и inaugural nightly runs ещё pending.
`docs/traceability.md` одновременно содержит `R13.2 PASS` в строке таблицы
и старое утверждение «R13.2 PARTIAL with ratchet» в итоговом индексе. R12.1,
R13.1 и fuzz evidence также являются `PARTIAL`, пока нет реальных CI
artifacts.

### Требуемый результат

1. До фактического выполнения внешних шагов статус handoff должен быть
   `REWORK REQUIRED` либо `EXTERNAL BLOCKER`, а не delivery/complete.
2. Для каждого R12/R13 оставить ровно один непротиворечивый статус:
   - `R12.1`: `PARTIAL`, пока performance PR gate не является реально
     required и не имеет labelled evidence;
   - `R13.1` / `R13.1-fuzz`: `PARTIAL`, пока нет 4-hour Linux fuzz run;
   - `R13.2`: `PASS` только после успешного CI coverage artifact с
     command/SHA/процентами; иначе `PARTIAL`;
   - `R12.2`: локальные full-matrix receipts можно считать evidence, но
     nightly run URL обязателен, если status ссылается на nightly.
3. Внести в handoff таблицу evidence: requirement, commit SHA, команда,
   host/runner, artifact/run URL, дата и результат. Не заменять URL фразой
   «wired and dispatchable».
4. Удалить из traceability старое резюме про 78/73 ratchet и проверить, что
   все итоговые строки согласованы с таблицей.

### Критерий приёмки

В `M7B-handoff.md` и `docs/traceability.md` нет взаимоисключающих статусов,
а каждое `PASS` опирается на проверяемый локальный receipt или CI artifact.

## P2. Подтвердить coverage и cargo-deny воспроизводимым evidence

### Факт

Новая nightly логика действительно ставит 90/80, но локальный независимый
`cargo llvm-cov` прогон не был завершён: инструментированные memory-gates
значительно дольше smoke. Кроме того, `cargo deny check` требует
`CARGO_DENY_DB_PATH`; с указанным cache path текущая среда не смогла
обновить RustSec advisory DB из-за сетевой недоступности. Это не дефект
реализации, но не позволяет заявлять fresh green result без artifact.

### Требуемый результат

1. Выполнить CI coverage workflow в точной конфигурации workflow (tests,
   bins/examples и три WPT backend runs), приложить LCOV + JSON и строки
   package totals. Порог должен быть рассчитан тем же скриптом, который
   fail-closed работает в workflow.
2. Приложить результат `cargo deny check` с доступной актуальной advisory
   DB, указав commit и дату DB. Нельзя считать отсутствие доступа к RustSec
   базам успешной проверкой.
3. Если оба запуска делает владелец CI, отметить их external evidence в
   handoff, не имитировать результат локальным текстовым receipt.

## Уже проверено повторным ревью

- `cargo fmt --all -- --check` — PASS;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — PASS;
- `cargo test -p boa_idb --test dom_api_tests` — 24/24 PASS;
- `cargo test -p boa_idb --test observer_tests --features tracing` — 4/4 PASS;
- `cargo test -p boa_idb --test memory_gates_tests continue_scaling_subquadratic` — PASS;
- WPT memory — 482/482 PASS;
- WPT SQLite — 482/482 PASS;
- `git diff --check 62f32db..HEAD` — PASS.

## Ограничения

- Не снижать 10% threshold, не разрешать interim baseline для release gate и
  не заменять labelled benchmark heterogeneous GitHub-hosted measurements.
- Не выдавать workflow wiring за completed external evidence.
- Не продолжать следующий milestone до P1-1 и P1-2; P2 должен иметь либо
  artifact, либо честный `EXTERNAL BLOCKER` с владельцем и датой повторной
  проверки.

## Команды, которые агент обязан приложить к повторной сдаче

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --summary
$env:CARGO_DENY_DB_PATH = 'target/cargo-deny-advisories'; cargo deny check
```

Для labelled host additionally:

```powershell
$env:BOA_IDB_BASELINE_ROLE = 'labelled'
python scripts/bench_compare.py write --baseline <new-labelled-baseline.json>
python scripts/bench_compare.py compare --baseline <new-labelled-baseline.json>
```

Повторная сдача допустима только с diff, тестами и ссылками на CI evidence;
не только с обновлённым handoff.
