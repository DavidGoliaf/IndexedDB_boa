# M7-B — наряд на доработку № 3

**Статус:** `REWORK REQUIRED`
**Основание:** повторное ревью от 2026-09-07
**Ветка:** `task/m7b-optimizations-reliability`
**Норматив:** `tasks/09_TASK_M7_PERFORMANCE_RELIABILITY.md`, §3 и §7; `AGENTS.md`.

В rework №2 добавлены PR trigger, fail-closed проверка interim baseline,
unit-тесты comparator и локальные coverage/deny receipts. Это полезные
исправления. M7-B всё ещё не сдаётся: benchmark gate не подтверждает
идентичность машины, а handoff расходится с traceability по R13.2.

## P1-1. Связать labelled baseline с конкретным исполняющим runner

### Факт

`scripts/bench_compare.py::load_baseline` проверяет только наличие полей
`git_sha`, `os`, `cpu`, `arch`, `rustc`, `profile`. Он не сравнивает их с
текущей машиной. Labels `[self-hosted, bench]` в workflow не являются
неизменяемым fingerprint: тот же label может оказаться на другой машине,
после замены CPU/диска или изменения образа.

Значит, baseline с одного хоста можно сравнить с результатом другого, и
формально «зелёный» результат перестанет быть доказательством отсутствия
регрессии. Для требования «pinned/labelled host» это P1.

### Требуемая реализация

1. Ввести обязательный стабильный идентификатор benchmark host:
   `BOA_IDB_BENCH_HOST_ID`. Его значение задаёт владелец CI в защищённой
   конфигурации self-hosted runner, а не workflow/PR.
2. При `write` сохранить `host_id` в `provenance`. При `compare` до запуска
   `cargo bench` fail-closed, если:
   - `BOA_IDB_BENCH_HOST_ID` отсутствует;
   - `provenance.host_id` отсутствует;
   - значения не совпадают;
   - baseline не `host_role == "labelled"` или не содержит уже обязательные
     поля provenance.
3. Дополнительно записывать диагностический host fingerprint (OS release,
   arch, CPU, toolchain, profile; storage/image version, если CI может их
   задать). Эти поля нужны для расследования, но именно защищённый `host_id`
   является критерием допуска к сравнению.
4. Не принимать host ID из PR-controlled файла или обычной workflow env.
   В README указать, где владелец runner устанавливает переменную и как
   безопасно выполнить initial baseline capture.
5. Расширить `scripts/test_bench_compare.py` минимум кейсами:
   labelled baseline + совпадающий ID PASS; отсутствующий env ID rejected;
   отсутствующий baseline ID rejected; несовпадающие ID rejected. Во всех
   reject случаях `run_bench` не вызывается.

### Критерий приёмки

`python scripts/test_bench_compare.py` подтверждает все четыре случая. На
подменённом host ID comparator возвращает non-zero до benchmark. В artefact
реального labelled baseline есть `host_id`, а handoff приводит его
нечувствительный идентификатор или безопасный hash, а не секрет.

## P1-2. Устранить противоречие статусов R13.2

### Факт

В `docs/traceability.md` R13.2 имеет корректный статус `PARTIAL`: local
coverage есть, inaugural CI artifact отсутствует. В
`docs/reviews/M7B-handoff.md`, раздел `Traceability deltas`, тот же R13.2
назван `PASS`. Это нарушает правило единственного доказуемого статуса и
вводит принимающего в заблуждение.

### Требуемый результат

1. До появления CI run URL/ID изменить R13.2 в handoff на `PARTIAL` с тем
же основанием, что в traceability: local 90.10% / 80.91% и enforcing
workflow существуют, но CI artifact ещё не получен.
2. Проверить все R12/R13 в handoff, evidence table и traceability: каждый
requirement имеет ровно один итоговый status. `PASS` разрешён только там,
где evidence table даёт проверяемый local receipt или CI artifact.
3. После успешного nightly coverage run добавить URL/ID, SHA, command,
package totals и дату в evidence table; только затем одновременно сменить
R13.2 на `PASS` во всех документах.

### Критерий приёмки

Поиск `R13.2` в `M7B-handoff.md` и `docs/traceability.md` не выдаёт
конфликтующих статусов. Ни один `PASS` не ссылается на «pending inaugural
run» как на единственное доказательство.

## Внешние блокеры, не выдавать за готовую сдачу

После исправления P1-1/P1-2 кодовая часть rework может быть принята условно,
но итоговый M7-B остаётся `EXTERNAL BLOCKER` до выполнения владельцем CI:

1. Provision pinned runner с labels `self-hosted, bench`, защищённым
   `BOA_IDB_BENCH_HOST_ID` и неизменяемой конфигурацией.
2. Capture отдельного labelled baseline с новым host ID; interim JSON
   остаётся только diagnostic.
3. Добавления `bench-regression` в required checks и первого успешного PR
   run URL/ID.
4. Inaugural nightly evidence: coverage artifact, 1M matrix log, Massif,
   4-hour fuzz statistics и crash/differential receipts, с run URLs.

Отсутствие доступа к GitHub Actions или runner не является поводом ставить
`PASS`; в handoff должен оставаться `EXTERNAL BLOCKER` с владельцем и
условием повторной проверки.

## P3. Чистота diff

Перед следующей сдачей устранить замечания `git diff --check`: trailing
whitespace в review Markdown и в приложенном terminal log. Если raw log
сохраняется как неизменяемый evidence, нормализовать его при экспорте либо
объяснить исключение в handoff; не оставлять красный `diff --check` без
объяснения.

## Команды повторной проверки

```powershell
python scripts/test_bench_compare.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --summary
git diff --check ac3284f..HEAD
```

На labelled runner:

```powershell
# BOA_IDB_BENCH_HOST_ID задаётся защищённо владельцем runner.
$env:BOA_IDB_BASELINE_ROLE = 'labelled'
python scripts/bench_compare.py write --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
python scripts/bench_compare.py compare --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
```

Следующая сдача должна содержать commit/diff, результаты этих команд и,
если доступны, URLs CI runs. Один лишь обновлённый handoff недостаточен.
