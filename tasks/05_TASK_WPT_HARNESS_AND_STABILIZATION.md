# ТЕХНИЧЕСКОЕ ЗАДАНИЕ НА ИСПОЛНЕНИЕ: TASK-05
## Conformance-тестирование WPT, раннер `boa_idb_wpt`, стабилизация API и матрица трассируемости

| Метаданные | Значение |
|---|---|
| **Идентификатор задачи** | `TASK-05-WPT-CONFORMANCE-AND-STABILIZATION` |
| **Этап ТЗ** | **M5 (WPT и стабилизация)** |
| **Целевые крейты** | `crates/boa_idb_wpt`, `crates/boa_idb`, `crates/boa_idb_core`, `crates/boa_idb_sqlite`, `crates/boa_idb_memory` |
| **Нормативное ТЗ** | `TZ_boa_idb_IndexedDB.md` (разделы 1.4, 5.0, 13.2, 13.3, 14.3, 15, Приложения A, E) |
| **Пререквизиты** | Принятые `TASK-01`, `TASK-02`, `TASK-03`, `TASK-04` (SQLite-бэкенд с 88 тестами) |
| **Роль архитектора** | Спроектированы архитектура раннера WPT, схема изоляции среды `testharness.js`, протокол сбора отчетов JSON, правила стабилизации порядка ошибок и шаблон `docs/traceability.md`. |
| **Роль исполнителя** | Строгая реализация раннера, интеграция тестового набора `wpt/IndexedDB/**`, устранение расхождений WebIDL/Спеки, фиксация `expectations.json` и заполнение `docs/traceability.md`. |

---

## 1. Архитектурный контекст и жесткие правила (Guardrails)

1. **Автономность раннера (`R13.3.1`):**
   - Раннер `boa_idb_wpt` работает автономно в среде Rust + Boa без браузера и без Node.js.
   - Поддерживает выполнение тестов формата `*.any.js`, а также скриптов, извлеченных из `*.htm`/`*.html`.
2. **Изоляция и полифилы окружения:**
   - Для каждого теста создается чистый экземпляр `Context` с изолированным временным хранилищем (`tempfile`).
   - Регистрируются полифилы браузерного окружения: `self = globalThis`, `location` (origin `http://localhost`), таймеры (`setTimeout`, `clearTimeout`), `EventTarget`, `Event`, `DOMException`, `structuredClone`.
   - Загружаются официальные скрипты `testharness.js` и `testharnessreport.js` с мостом `add_completion_callback` для передачи отчета из JS в Rust через нативную функцию.
3. **Целевой уровень прохождения (R13.3.2):**
   - **≥ 80 % subtests PASS** на этапе M5 (для обоих бэкендов: `memory` и `sqlite`).
   - Исключения (Out of scope согласно §1.4 ТЗ): Blob/File API, Web Workers, Storage Buckets, ImageBitmap.
4. **Стабильность ожиданий (`expectations.json`):**
   - Все не прошедшие субтесты (FAIL/SKIP) обязаны быть детерминированно зафиксированы в `expectations.json` с обоснованием причины. Ни одного `CRASH` или `TIMEOUT`!
5. **Матрица трассируемости (`docs/traceability.md`, R13.2):**
   - Каждый пункт требований ТЗ (`R2.1`..`R13.7`) обязан быть сопоставлен минимум с одним unit-, property- или WPT-тестом в файле `docs/traceability.md`.

---

## 2. Полное дерево файлов задачи

```
crates/boa_idb_wpt/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── environment.rs       # Инициализация Context, полифилы (timers, location, globalThis)
│   ├── harness.rs           # Загрузка testharness.js, testharnessreport.js, нативный мост отчетов
│   ├── report.rs            # Структуры отчетов (TestFileResult, SubtestResult, Status)
│   ├── expectations.rs      # Загрузка, проверка и обновление expectations.json
│   ├── runner.rs            # Параллельный / последовательный запуск тестов по бэкендам
│   └── main.rs              # CLI бинарник boa-idb-wpt
├── wpt/
│   ├── resources/           # testharness.js, testharnessreport.js, idbharness.js
│   └── IndexedDB/           # Официальные тесты W3C IndexedDB (commit 82a84e1)
├── expectations.json        # Базовый снимок ожиданий (PASS / FAIL с причинами)
└── tests/
    └── runner_tests.rs      # Тесты самого раннера WPT
docs/
└── traceability.md          # Матрица покрытия требований ТЗ тестами
```

---

## 3. Манифест `crates/boa_idb_wpt/Cargo.toml`

```toml
[package]
name = "boa_idb_wpt"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "W3C Web Platform Tests runner for boa_idb"

[[bin]]
name = "boa-idb-wpt"
path = "src/main.rs"

[dependencies]
boa_idb = { path = "../boa_idb" }
boa_idb_core = { path = "../boa_idb_core" }
boa_idb_memory = { path = "../boa_idb_memory" }
boa_idb_sqlite = { path = "../boa_idb_sqlite" }
boa_engine = "~0.22.0"
boa_gc = "~0.22.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
clap = { version = "4.5", features = ["derive"] }
walkdir = "2.5"
tempfile = "3"
colored = "2.1"
thiserror = { workspace = true }
parking_lot = "0.12"

[dev-dependencies]
tempfile = "3"

[lints]
workspace = true
```

---

## 4. Спецификация WPT-раннера

### 4.1. Мост `testharness.js` ↔ Rust (`harness.rs`)
Файл: `crates/boa_idb_wpt/src/harness.rs`

Раннер внедряет в глобальное пространство JS нативный коллбек `__wpt_report_completion`:
```rust
use boa_engine::native_function::NativeFunction;
use boa_engine::{Context, JsResult, JsValue, js_string};
use parking_lot::Mutex;
use std::sync::Arc;
use crate::report::WptRunResult;

pub fn install_wpt_reporter(context: &mut Context, result_sink: Arc<Mutex<Option<WptRunResult>>>) -> JsResult<()> {
    let sink = result_sink.clone();
    
    let reporter = NativeFunction::from_copy_closure(move |_this, args, _ctx| {
        if let Some(json_val) = args.first() {
            if let Some(s) = json_val.as_string() {
                let json_str = s.to_std_string_escaped();
                if let Ok(report) = serde_json::from_str::<WptRunResult>(&json_str) {
                    *sink.lock() = Some(report);
                }
            }
        }
        Ok(JsValue::undefined())
    });

    context.global_object().set(
        js_string!("__wpt_report_completion"),
        JsValue::from(reporter.to_js_function(context.realm())),
        false,
        context,
    )?;

    // Инъекция JS-кода привязки add_completion_callback
    context.eval(boa_engine::Source::from_bytes(
        r#"
        if (typeof add_completion_callback === 'function') {
            add_completion_callback(function(tests, status) {
                var results = {
                    status: status.status,
                    message: status.message,
                    subtests: tests.map(function(t) {
                        return {
                            name: t.name,
                            status: t.status,
                            message: t.message
                        };
                    })
                };
                __wpt_report_completion(JSON.stringify(results));
            });
        }
        "#,
    ))?;

    Ok(())
}
```

---

### 4.2. Инициализация окружения теста (`environment.rs`)
Файл: `crates/boa_idb_wpt/src/environment.rs`

Для каждого теста подготавливается изолированный контекст:
1. Создание чистого `Context::default()`.
2. Регистрация `IndexedDbExtension` (на `MemoryBackend` или `SqliteBackend` во временной папке).
3. Установка полифилов:
   - `globalThis.self = globalThis;`
   - `globalThis.location = { href: "http://localhost/IndexedDB/", origin: "http://localhost", protocol: "http:", host: "localhost", hostname: "localhost", port: "", pathname: "/IndexedDB/", search: "", hash: "" };`
   - Таймеры: реализация `setTimeout`/`clearTimeout` через сбор макрозадач и их выполнение в event loop.
4. Выполнение скриптов `testharness.js` и `testharnessreport.js`.

---

### 4.3. Структуры отчетов (`report.rs`) и Сверка ожиданий (`expectations.rs`)
Файлы: `crates/boa_idb_wpt/src/report.rs`, `expectations.rs`

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SubtestStatus {
    Pass,
    Fail,
    Timeout,
    NotRun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtestResult {
    pub name: String,
    pub status: SubtestStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WptRunResult {
    pub status: u8,
    pub message: Option<String>,
    pub subtests: Vec<SubtestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileExpectation {
    pub subtests: BTreeMap<String, SubtestStatus>,
    pub reason: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Expectations {
    pub files: BTreeMap<String, FileExpectation>,
}
```

---

## 5. Приоритетные тест-сьюты WPT и устранение расхождений (§13.3.3)

Исполнитель **ОБЯЗАН** стабилизировать и обеспечить прохождение следующих файлов WPT:

| Группа WPT | Целевые файлы тестов | Ключевые проверки соответствия Спеке |
|---|---|---|
| **Factory** | `idbfactory-open.any.js`, `idbfactory-deleteDatabase.any.js`, `idbfactory_cmp.any.js`, `get-databases.any.js` | `version === 0` -> `TypeError`, удаление занятой базы, порядок версий |
| **Database** | `idbdatabase_createObjectStore.any.js`, `idbdatabase_deleteObjectStore.any.js`, `idbdatabase_transaction.any.js`, `close-in-upgradeneeded.any.js` | Создание store только в upgrade, валидация keyPath, закрытие соединений |
| **ObjectStore** | `idbobjectstore_put.any.js`, `idbobjectstore_add.any.js`, `idbobjectstore_get.any.js`, `idbobjectstore_getAll.any.js`, `idbobjectstore_delete.any.js`, `idbobjectstore_clear.any.js`, `idbobjectstore_count.any.js` | Порядок ошибок: `TransactionInactiveError` vs `DataError`, inline keyPath, autoIncrement |
| **Index** | `idbindex_get.any.js`, `idbindex_getAll.any.js`, `idbindex_count.any.js`, `idbindex_openCursor.any.js`, `idbindex-multientry.any.js` | `multiEntry` распаковка массивов, `unique` проверки с `ConstraintError` |
| **KeyRange** | `idbkeyrange.any.js`, `idbkeyrange_includes.any.js` | `only`, `lowerBound`, `upperBound`, `bound`, `lower > upper` -> `DataError` |
| **Cursor** | `idbcursor_advance.any.js`, `idbcursor_continue.any.js`, `idbcursor-continuePrimaryKey.any.js`, `idbcursor_update.any.js`, `idbcursor_delete.any.js` | 4 направления (`next`, `nextunique`, `prev`, `prevunique`), живое обновление позиции |
| **Transactions** | `transaction-lifetime.any.js`, `transaction-abort.any.js`, `idb-explicit-commit.any.js`, `abort-in-initial-upgradeneeded.any.js` | Границы активности транзакций, автокоммит, откат генератора ключей |
| **Cloning & Keys** | `keyorder.any.js`, `keypath.any.js`, `structured-clone.any.js`, `idb-binary-key.any.js`, `key-conversion-exceptions.any.js` | Сравнение ключей по типам, непарные суррогаты, ArrayBuffer ключи |

---

## 6. Матрица трассируемости требований `docs/traceability.md`

Исполнитель **ОБЯЗАН** сформировать и поддерживать файл `docs/traceability.md` в следующем формате:

```markdown
# Матрица трассируемости требований спецификации IndexedDB 3.0

| ID требования | Формулировка требования | Модуль реализации | Тест(ы) верификации | Статус |
|---|---|---|---|---|
| **R2.1** | Rust 1.91.0, 100% Safe Rust, wasm32-unknown-unknown | `boa_idb_core` | `cargo check --target wasm32`, `cargo clippy` |  PASS |
| **R5.0.1** | Конвертация WebIDL, [EnforceRange] для version/count | `boa_idb::convert::webidl` | `tests/integration_tests.rs::test_idb_factory_open_version_zero_fails` |  PASS |
| **R5.1** | IDBFactory: open, deleteDatabase, databases, cmp | `boa_idb::api::factory` | `wpt/IndexedDB/idbfactory-*.any.js` |  PASS |
| **R5.2** | IDBDatabase: name, version, storeNames, transaction, close | `boa_idb::api::database` | `wpt/IndexedDB/idbdatabase-*.any.js` |  PASS |
| **R5.3** | IDBObjectStore: CRUD, keyPath, autoIncrement, index | `boa_idb::api::object_store` | `wpt/IndexedDB/idbobjectstore-*.any.js` |  PASS |
| **R5.4** | IDBIndex: get, getAll, count, multiEntry, unique | `boa_idb::api::index` | `wpt/IndexedDB/idbindex-*.any.js` |  PASS |
| **R5.5** | IDBKeyRange: only, lowerBound, upperBound, bound, includes | `boa_idb::api::key_range` | `wpt/IndexedDB/idbkeyrange.any.js` |  PASS |
| **R5.6** | IDBCursor: 4 направления, advance, continue, continuePrimaryKey | `boa_idb::api::cursor` | `wpt/IndexedDB/idbcursor-*.any.js` |  PASS |
| **R5.7** | IDBTransaction: mode, durability, commit, abort | `boa_idb::api::transaction` | `wpt/IndexedDB/transaction-*.any.js` |  PASS |
| **R5.11** | DOM-шим: EventTarget, Event, DOMException, DOMStringList, dispatch | `boa_idb::dom` | `tests/integration_tests.rs`, `wpt/IndexedDB/` |  PASS |
| **R6.1** | Представление Key, Utf16String, полный порядок §2.4 | `boa_idb_core::key` | `crates/boa_idb_core/tests/key_tests.rs` |  PASS |
| **R6.3** | Кодек KEY-v1, float order-preserving, CESU-8 экранирование | `boa_idb_core::key::encode` | `crates/boa_idb_core/tests/key_proptests.rs` |  PASS |
| **R6.4** | Сериализатор SCF-v1, memo-таблица, CRC32C, лимиты | `boa_idb_core::clone` | `crates/boa_idb_core/tests/scf_tests.rs`, `scf_proptests.rs` |  PASS |
| **R6.5** | KeyPath: валидация IdentifierName, extract, inject | `boa_idb_core::key::path` | `crates/boa_idb_core/tests/keypath_tests.rs` |  PASS |
| **R7.1** | Реестр и FSM очереди открытия (blocked, versionchange, upgrade) | `boa_idb_core::engine::open_queue` | `crates/boa_idb_core/tests/open_queue_tests.rs` |  PASS |
| **R7.2** | Планировщик транзакций, FIFO справедливость, автокоммит | `boa_idb_core::engine::scheduler` | `crates/boa_idb_core/tests/scheduler_tests.rs` |  PASS |
| **R7.4** | Курсоры ядра, живая позиция, итерация §6.7 | `boa_idb_core::engine::cursor` | `crates/boa_idb_core/tests/cursor_tests.rs` |  PASS |
| **R8.2** | Бэкенд SQLite: WAL, пул 1W+NR, SAVEPOINT r<seq>, Blobs > 256KB | `boa_idb_sqlite` | `crates/boa_idb_sqlite/tests/*.rs` (88 тестов) |  PASS |
| **R8.4** | In-Memory бэкенд на BTreeMap с Undo-логами | `boa_idb_memory` | `crates/boa_idb_memory/tests/*.rs` (26 тестов) |  PASS |
| **R13.3** | WPT Conformance ≥ 80% pass rate | `boa_idb_wpt` | `cargo run --bin boa-idb-wpt` |  PASS |
```

---

## 7. Пошаговый план работ для исполнителя (Execution Steps)

1. **Шаг 1. Настройка крейта `crates/boa_idb_wpt`**
   - Наполнить `Cargo.toml` (`clap`, `serde`, `serde_json`, `walkdir`, `colored`).
   - Добавить официальные ресурсы WPT (`testharness.js`, `testharnessreport.js`, тесты `wpt/IndexedDB/`).
2. **Шаг 2. Реализация полифилов окружения и моста отчетов**
   - Реализовать `environment.rs` (контекст, `self`, `location`, таймеры `setTimeout`/`clearTimeout`).
   - Реализовать `harness.rs` (`install_wpt_reporter`, `__wpt_report_completion`).
3. **Шаг 3. Реализация раннера и CLI**
   - Реализовать `report.rs`, `expectations.rs`, `runner.rs`, `main.rs`.
   - Поддержать флаги CLI: `--backend memory|sqlite`, `--filter <pattern>`, `--update-expectations`.
4. **Шаг 4. Стабилизация и устранение расхождений IDB API**
   - Прогнать WPT тесты на `MemoryBackend` и `SqliteBackend`.
   - Устранить выявленные расхождения (порядок проверок исключений, граничные случаи курсоров, `[SameObject]` свойства).
   - Добиться показателя **≥ 80 % PASS** от общего числа поддерживаемых субтестов.
5. **Шаг 5. Фиксация `expectations.json` и `docs/traceability.md`**
   - Сгенерировать актуальный `expectations.json`.
   - Заполнить и проверить матрицу `docs/traceability.md`.
6. **Шаг 6. Финальная верификация**
   - Запустить все команды из Раздела 8.

---

## 8. Команды валидации и критерии приёмки

Исполнитель сдает работу только тогда, когда **ВСЕ** команды выполняются со статусом SUCCESS (EXIT CODE 0):

```powershell
# 1. Проверка форматирования
cargo fmt --all -- --check

# 2. Строгий линтинг clippy
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 3. Полный прогон всех существующих unit и integration тестов
cargo test --workspace

# 4. Запуск WPT раннера на Memory-бэкенде (должно быть ≥ 80% PASS)
cargo run --package boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary

# 5. Запуск WPT раннера на SQLite-бэкенде (должно быть ≥ 80% PASS)
cargo run --package boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --summary

# 6. Сверка с зафиксированным expectations.json (0 непредвиденных регрессий)
cargo run --package boa_idb_wpt --bin boa-idb-wpt -- --backend memory --check-expectations

# 7. Проверка генерации документации
cargo doc --workspace --no-deps
```

### Чек-лист соответствия ТЗ:
- [ ] `R13.3.1`: Реализован полноценный раннер WPT `boa_idb_wpt` без внешних зависимостей от браузера/Node.js.
- [ ] `R13.3.2`: Достигнут уровень прохождения **≥ 80 % PASS** субтестов WPT на бэкендах Memory и SQLite.
- [ ] `R13.3.3`: Все приоритетные тесты (`idbfactory-*`, `idbdatabase-*`, `idbobjectstore-*`, `idbindex-*`, `idbcursor-*`, `transaction-*`, `keyorder`, `structured-clone`) проходят или зафиксированы в `expectations.json`.
- [ ] `R13.2`: Файл `docs/traceability.md` полностью заполнен и сопоставляет каждое требование ТЗ с тестами.
- [ ] 100% Safe Rust (`#![deny(unsafe_code)]`), 0 предупреждений компилятора.
