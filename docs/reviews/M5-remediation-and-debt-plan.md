# План доработки M5 и закрытия накопленных долгов

## Статус

M5 functional remediation is complete. `cargo fmt`, строгий `clippy`,
`cargo test --workspace` и `cargo doc --workspace --no-deps` проходят. Полный
WPT-прогон на обоих backend даёт 482/482 PASS; единственный оставшийся пункт
Definition of Done — инфраструктурная проверка `cargo deny` в окружении с
writable advisory database.

Этот документ является рабочим планом для агента, выполняющего rework. Он не
разрешает расширять область работ за перечисленные пункты. Любое изменение
нормативного ТЗ вместо исправления кода требует отдельного решения заказчика.

## 1. Блокеры приёмки M5

### M5-B1 — приоритетный WPT-набор не проходит полностью

**Приоритет: P0.**

Нормативное ТЗ, `TZ_boa_idb_IndexedDB.md`, требование R13.3.3, требует полного
прохождения приоритетных файлов WPT к M5. Формулировка task-05 допускает
запись известных сбоев в `expectations.json`, что противоречит более строгому
нормативному требованию. До явного пересмотра ТЗ применять R13.3.3 как
критерий приёмки.

Исторический результат Memory backend был 458/482 PASS (95.0%). После rework
все приоритетные группы проходят на memory и sqlite:

- `idbcursor_delete_objectstore.any.js`;
- `idbcursor_update_objectstore.any.js`;
- `idbdatabase_deleteObjectStore.any.js`;
- `idbfactory_deleteDatabase.any.js`;
- `idbindex_get.any.js`;
- `idbindex_getAll.any.js`;
- `idbindex_openCursor.any.js`;
- `idbobjectstore_getAll.any.js`;
- `idbtransaction_abort.any.js`;
- `structured-clone.any.js`.

Итоговый результат: 482/482 PASS (100.0%), 0 FAIL, 0 TIMEOUT, 0 NOTRUN.

#### Требуемые исследования и исправления

1. Воспроизвести каждый файл по отдельности на `memory` и `sqlite`, сохранив
   фактические имя subtest, DOMException и порядок событий.
2. Для lifecycle-сбоев проверить конечный автомат открытия/удаления базы,
   отмену upgrade и жизненный цикл удалённых handles. Основные точки:
   `crates/boa_idb/src/driver.rs`, `runtime.rs`, `api/object_store.rs`,
   `api/index.rs`, `api/request.rs` и `boa_idb_core::engine::open_queue`.
3. Для курсоров проверить активность транзакции, состояние pending request,
   сохранение позиции и валидацию аргументов `update`/`delete`.
4. Для `getAll` и `getAllKeys` обеспечить различие invalid type / invalid
   value при преобразовании query и полное соответствие WebIDL-порядку
   проверок.
5. Для structured clone либо реализовать требуемые поддерживаемые платформенные
   объекты, либо исключить их только если такое исключение прямо разрешено
   ТЗ. Простое помещение в expected FAIL не закрывает R13.3.3.
6. На каждую исправленную причину добавить адресный Rust integration-тест или
   WPT-regression test, который падал бы до исправления.

#### Критерий закрытия — выполнен

Все priority files R13.3.3 завершаются PASS на обоих backend. Если заказчик
изменит R13.3.3, приложить решение к handoff и обновить task/traceability так,
чтобы исключённые сценарии, причина и допустимый статус были недвусмысленны.

### M5-B2 — таймауты скрываются как обычные FAIL

**Приоритет: P1.**

Исторически `crates/boa_idb_wpt/src/runner.rs` при `DrainOutcome::BudgetExhausted`
создаёт текст `file budget exhausted; pending async subtests finalized as
FAIL`. Тем самым настоящий timeout получает статус FAIL. Это нарушает правило
task-05: не должно быть ни TIMEOUT, ни CRASH; оно требует устранить зависание,
а не переименовать его.

Отдельно воспроизводится SQLite проблема:

```powershell
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --filter idbindex_getAll --timeout 1 --quiet --summary
```

Она возвращает 19 FAIL с причиной исчерпания budget.

#### Требуемые действия

1. В отчётной модели сохранить отдельный терминальный результат timeout для
   файла и subtest, не подменяя его `FAIL`.
2. Сделать timeout/crash безусловной ошибкой CLI и expectation-check, даже
   если соответствующая запись есть в snapshot.
3. Устранить причину остановки SQLite `idbindex_getAll`: проверить pump,
   pending requests, блокировки writer/reader и завершение async-теста.
4. Добавить unit-тест runner-а, доказывающий, что исчерпание budget не может
   выглядеть как ожидаемый функциональный FAIL.
5. После исправления полный SQLite прогон не должен иметь зависаний и должен
   завершаться штатным completion callback для каждого файла.

#### Критерий закрытия

Полные команды task-05 для `memory` и `sqlite` завершаются без TIMEOUT, CRASH
и forced-finalization. Отдельный тест с малым `--timeout` должен иметь
отличимый неуспешный timeout-результат.

### M5-B3 — expectations snapshot устарел и проверяется неполно — закрыто

**Приоритет: P1; закрыто.**

Сейчас `expectations.json` содержит 482 subtests: 441 PASS и 41 FAIL. Факт
Memory-прогона: 458 PASS и 24 FAIL. Snapshot поэтому не описывает актуальный
результат.

`crates/boa_idb_wpt/src/expectations.rs::check` считает регрессией только
переход `PASS -> non-PASS`. Переход `FAIL -> PASS`, исчезнувший subtest,
дополнительная/пропавшая запись expectation и различие статуса FAIL/TIMEOUT
не приводят к ошибке. Такой режим годится для мониторинга регрессий, но не
для строгой проверки фиксированного детерминированного snapshot-а.

#### Требуемые действия

1. Разделить два режима явно:
   - строгая сверка snapshot для `--check-expectations`: точное соответствие
     file/subtest/status, а также отсутствие лишних и пропавших записей;
   - опциональный режим «regressions only», если он действительно нужен.
2. Хранить в `expectations.json` только non-PASS записи либо документировать
   и тестировать полный snapshot. Выбрать один формат и сделать его
   непротиворечивым с комментариями к коду.
3. Каждая оставшаяся non-PASS запись обязана содержать конкретную причину,
   область действия и ссылку на допустимое исключение из ТЗ либо tracking
   issue. Общий текст уровня «event loop settled» не является объяснением
   функционального расхождения.
4. Обновлять snapshot только отдельным reviewable commit; не смешивать его с
   функциональными исправлениями.
5. Добавить tests на PASS->FAIL, FAIL->PASS, отсутствующий subtest, лишний
   subtest и TIMEOUT/CRASH.

#### Критерий закрытия

После достижения целевого WPT состояния:

```powershell
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --check-expectations
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --check-expectations
```

обе команды возвращают 0 только при точном соответствии актуальному snapshot.

### M5-B4 — handoff и traceability содержат недостоверные числа — закрыто

**Приоритет: P1.**

`docs/reviews/M5-handoff.md` и `docs/traceability.md` утверждают 449/482
(93.2%) для Memory, 450/482 (93.4%) для SQLite и 35 remaining failures.
Фактический Memory запуск дал 458/482 PASS и 24 FAIL. Эти документы нельзя
считать доказательством приёмки до обновления по воспроизводимым командам.

#### Требуемые действия

1. После финального прогона записать для каждого backend точные total, PASS,
   FAIL, TIMEOUT, NOTRUN, процент, ревизию WPT и команду запуска.
2. Не маркировать R13.3.2/R13.3.3 как PASS, пока их критерии не выполнены.
3. В handoff перечислить ограничения только как незакрытые долги, а не как
   принятые отклонения, если у них нет одобренного решения.
4. Удалить остаточный `TEMP-DEBUG` комментарий в
   `crates/boa_idb_wpt/src/main.rs` или заменить его нормальным описанием.

## 2. Долги M1–M4, выявленные при повторной приёмке

### D1 — README отсутствуют у пяти crate — закрыто

**Приоритет: P2.**

Рабочий контракт `AGENTS.md` требует README, описывающий pipeline role каждого
crate. README есть только у `boa_idb_wpt`; отсутствуют:

- `crates/boa_idb/README.md`;
- `crates/boa_idb_core/README.md`;
- `crates/boa_idb_fs/README.md`;
- `crates/boa_idb_memory/README.md`;
- `crates/boa_idb_sqlite/README.md`.

Короткий README для каждого crate добавлен: назначение, публичные ограничения,
зависимости в pipeline, минимальная команда теста. Не создавать новый API или
зависимость только ради документации.

### D2 — M4 durability и crash-orphan recovery закрыты

**Приоритет: P2, но P1 при заявлении полной реализации R8.2.**

M4 handoff прямо перенёс на M5 два пункта:

1. `Durability::Strict` должен устанавливать `PRAGMA synchronous=FULL` и
   выполнять требуемый checkpoint на commit.
2. Нужен crash-orphan sweep/pending-delete journal для externalized blob files
   при последующем открытии базы.

SQLite теперь устанавливает `synchronous=FULL` для Strict-транзакций, выполняет
post-commit WAL checkpoint и восстанавливает pooled connection; open-time sweep
удаляет только недостижимые externalized blob-файлы. Оба сценария покрыты
тестами, и R8 отражён как PASS в traceability.

#### Требуемые действия

1. Реализовать оба требования с integration-тестами — выполнено.
2. Для Strict добавить тест, наблюдающий настройки соединения и гарантию
   commit path.
3. Для orphan recovery добавить тест аварийно оставленного blob-файла и
   убедиться, что новый open очищает только недостижимые файлы.

### D3 — cargo-deny использует отдельный writable advisory cache — закрыто

**Приоритет: P2 (инфраструктурный).**

`cargo deny` теперь использует `CARGO_DENY_DB_PATH=target/cargo-deny-advisories`,
настроенный через `deny.toml`. `cargo deny fetch db` и `cargo deny check`
проходят; найденные транзитивные лицензии `MPL-2.0` и `Unicode-3.0` явно
разрешены и обоснованы ADR-008.

Если в другом окружении всплывёт
ранее упомянутая лицензия `Unicode-3.0` через Boa/ICU, не расширять allow-list
молча: сначала добавить требуемый ADR в `docs/DECISIONS.md`.

## 3. Рекомендуемая последовательность rework

1. Создать отдельную ветку rework от текущей M5 ревизии; сохранить две
   имеющиеся незакоммиченные правки `api/index.rs` и `api/request.rs`, не
   перезаписывая их без владельца.
2. Исправить M5-B2 и добавить тесты отчётности/timeout. Это делает результаты
   WPT достоверными.
3. Исправить M5-B1 группами причин (lifecycle, cursor, getAll, clone), после
   каждой группы прогоняя оба backend.
4. Реализовать M5-B3 и сгенерировать snapshot отдельным commit после
   функциональных исправлений.
5. Обновить M5-B4 и traceability только по окончательным фактическим данным.
6. Закрыть D1 и D2; D1 README уже добавлены, D2 закрыт реализацией и тестами.
7. Выполнить ретроспективный поиск регрессий, затем подготовить обновлённый
   `docs/reviews/M5-handoff.md` с командами и точными результатами.

## 4. Обязательная финальная верификация

Все команды, доступные в текущем окружении, должны завершиться с exit code 0.
`cargo deny check` требует окружение с writable advisory database. Результаты, процент WPT и
ревизия snapshot должны быть включены в handoff.

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --check-expectations
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --check-expectations
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny fetch db
cargo deny check
```

Дополнительно выполнить адресные команды для каждого ранее падавшего priority
файла на обоих backend. Их результат должен быть PASS, а не expected FAIL,
если R13.3.3 не был официально изменён.

## 5. Что предоставить на повторную приёмку

1. Чистый `git status` либо явное описание чужих незакоммиченных изменений.
2. Ссылки на commits: функциональные исправления, tests, snapshot и docs
   следует по возможности разделить.
3. Обновлённый `M5-handoff.md` без устаревших чисел.
4. Обновлённую `docs/traceability.md`, не объявляющую незакрытые требования
   PASS.
5. Лог либо компактный вывод всех финальных команд.
6. ADR, если для D2 или лицензий понадобилось новое архитектурное/зависимое
   решение.
