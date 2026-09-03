# Техническое задание

## Разработка крейта `boa-idb` — реализация IndexedDB API для JS-движка Boa

| | |
|---|---|
| **Документ** | ТЗ на разработку программного компонента |
| **Продукт** | `boa-idb` — реализация W3C Indexed Database API 3.0 для встраиваемого JS-движка Boa |
| **Версия ТЗ** | 1.0 |
| **Дата** | 2026-09-01 |
| **Целевая платформа** | Rust 1.91.0 (edition 2024), `boa_engine` 0.22.x |
| **Нормативная спецификация** | [Indexed Database API 3.0, W3C Working Draft, 13 August 2025](https://www.w3.org/TR/IndexedDB/) |
| **Статус** | к исполнению |

---

## Оглавление

1. [Общие сведения](#1-общие-сведения)
2. [Целевая платформа, зависимости и структура проекта](#2-целевая-платформа-зависимости-и-структура-проекта)
3. [Архитектура](#3-архитектура)
4. [Публичный Rust API крейта (host-side API)](#4-публичный-rust-api-крейта-host-side-api)
5. [Поверхность JavaScript API](#5-поверхность-javascript-api)
6. [Ядро: ключи, кодирование, сериализация значений](#6-ядро-ключи-кодирование-сериализация-значений)
7. [Ядро: транзакции, соединения, запросы, курсоры](#7-ядро-транзакции-соединения-запросы-курсоры)
8. [Слой хранения (backends)](#8-слой-хранения-backends)
9. [Интеграция с event loop Boa](#9-интеграция-с-event-loop-boa)
10. [Обработка ошибок и DOMException](#10-обработка-ошибок-и-domexception)
11. [Безопасность, приватность, квоты](#11-безопасность-приватность-квоты)
12. [Нефункциональные требования и производительность](#12-нефункциональные-требования-и-производительность)
13. [Тестирование и приёмка качества](#13-тестирование-и-приёмка-качества)
14. [Документация, сборка, поставка](#14-документация-сборка-поставка)
15. [Этапы работ и трудоёмкость](#15-этапы-работ-и-трудоёмкость)
16. [Критерии приёмки](#16-критерии-приёмки)
17. [Риски и открытые вопросы](#17-риски-и-открытые-вопросы)
18. [Приложения](#18-приложения)

---

## 1. Общие сведения

### 1.1. Назначение и цель

Разработать самостоятельный крейт (набор крейтов в рамках одного Cargo workspace), который добавляет в среду исполнения на базе движка **Boa** полнофункциональную реализацию **IndexedDB API** в объёме спецификации W3C Indexed Database API 3.0 (далее — «Спека»).

Крейт предназначен для встраивания в хост-приложения на Rust, которые:

* исполняют недоверенный или полудоверенный JS-код (плагины, автоматизации, серверless-обработчики, CLI-скрипты);
* хотят предоставить этому коду привычный браузерный API персистентного хранилища без запуска браузера/Node.js;
* требуют полного контроля над тем, где и как данные лежат на диске.

**Ключевое требование:** JS-код, написанный для браузера и использующий `indexedDB`, должен работать без модификаций (с точностью до документированного списка исключений — раздел 1.4).

### 1.2. Термины и сокращения

| Термин | Определение |
|---|---|
| **Спека** | W3C Indexed Database API 3.0, WD от 13.08.2025 |
| **Движок / Boa** | `boa_engine` 0.22.x и связанные крейты (`boa_gc`, `boa_runtime`, `boa_macros`) |
| **Хост** | Rust-приложение, встраивающее Boa и данный крейт |
| **Storage key** | Ключ изоляции хранилища (аналог origin в браузере); в нашем случае — задаваемая хостом строка |
| **Backend / бэкенд** | Реализация физического хранения (SQLite, файлы на диске, память) |
| **SCF** | Structured Clone Format — внутренний бинарный формат сериализации JS-значений (раздел 6.4) |
| **Ключ (key)** | Значение типа `number` \| `date` \| `string` \| `binary` \| `array` согласно §2.4 Спеки |
| **Транзакция IDB** | Логическая транзакция уровня Спеки (`readonly` \| `readwrite` \| `versionchange`) |
| **Транзакция бэкенда** | Физическая транзакция уровня хранилища |
| **WPT** | web-platform-tests, набор `wpt/IndexedDB/**` |
| **Задача (job)** | Единица работы в `boa_engine::job` (`NativeJob`, `NativeAsyncJob`, `TimeoutJob`, `PromiseJob`) |

### 1.3. Область работ (в объёме ТЗ)

Реализуются в полном объёме:

1. Все интерфейсы Спеки: `IDBFactory`, `IDBDatabase`, `IDBObjectStore`, `IDBIndex`, `IDBKeyRange`, `IDBRecord`, `IDBCursor`, `IDBCursorWithValue`, `IDBTransaction`, `IDBRequest`, `IDBOpenDBRequest`, `IDBVersionChangeEvent`.
2. Все алгоритмы §5–§7 Спеки, включая жизненный цикл транзакций, connection queue, upgrade-транзакции, key generators, multiEntry/unique индексы, курсоры всех четырёх направлений.
3. Минимальный DOM-слой, необходимый для работы IDB (`EventTarget`, `Event`, `DOMException`, `DOMStringList`) — с возможностью отключения, если хост предоставляет свои реализации.
4. Два production-бэкенда: **SQLite** и **файловый (log-structured)**, плюс in-memory бэкенд для тестов.
5. Собственный формат структурного клонирования значений с персистентностью и версионированием.
6. Интеграция с системой задач Boa (`JobExecutor`), включая корректную деактивацию транзакций на границах задач.

### 1.4. Вне области работ (out of scope для версии 1.0)

| Что | Причина / решение |
|---|---|
| `Blob`, `File`, `FileList`, `ImageData`, `ImageBitmap` в значениях | В Boa нет File API. При попытке сохранить — `DataCloneError`. Формат SCF **обязан** зарезервировать теги под эти типы (раздел 6.4.6) для v1.1. |
| Storage Buckets API, `navigator.storage` | Отдельная спецификация. Квоты предоставляются через Rust API хоста. |
| Мультипроцессный доступ к одной базе | Реализуется только защита (advisory-локи, раздел 8.5), а не координация. Второй процесс получает `UnknownError` при попытке открыть занятую базу. |
| Web Workers / многопоточный JS-доступ к одной `IDBFactory` | Один `Context` = один поток. Несколько `Context` в разных потоках, работающих с одним storage key, — сценарий v1.1 (раздел 17). |
| `IDBTransaction.durability = "strict"` с гарантией на уровне ОС для файлового бэкенда на всех ФС | Реализуется через `fsync`/`fdatasync`; поведение на сетевых ФС не гарантируется, документируется. |
| Шифрование данных на диске | Опциональная фича `encryption` вынесена в бэклог; в v1.0 — только точка расширения в trait `ValueCodec`. |

### 1.5. Нормативные и справочные ссылки

1. W3C Indexed Database API 3.0 — https://www.w3.org/TR/IndexedDB/ (основной источник требований; все ссылки вида «§2.4» — на этот документ).
2. WHATWG DOM Standard — `EventTarget`, `Event`, dispatch, `DOMException`.
3. WHATWG HTML Standard — `StructuredSerializeForStorage`, `StructuredDeserialize`, event loop, task sources.
4. WHATWG Infra Standard — `code unit less than`, `byte less than`, list/set semantics.
5. Web IDL Standard — конвертации типов аргументов, `EnforceRange`, имена `DOMException`.
6. ECMA-262 — `IdentifierName` (валидация key path), `Date`, `TypedArray`, `BigInt`.
7. Документация `boa_engine` 0.22: модули `class`, `job`, `interop`, `object`, `value`, `error`.
8. web-platform-tests, каталог `IndexedDB/`.

---

## 2. Целевая платформа, зависимости и структура проекта

### 2.1. Toolchain

| Параметр | Значение |
|---|---|
| MSRV | **1.91.0** (совпадает с MSRV `boa_engine` 0.22.0) |
| Edition | **2024** |
| `rust-toolchain.toml` | канал `1.91.0`, компоненты `rustfmt`, `clippy` |
| Целевые платформы CI | `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` |
| Дополнительно (best effort) | `aarch64-unknown-linux-gnu`, `x86_64-unknown-freebsd` |
| `wasm32-*` | Не поддерживается в v1.0 (файловый и SQLite-бэкенд требуют ФС). Крейт `boa_idb_core` **должен** компилироваться под `wasm32-unknown-unknown` (без бэкендов) — проверяется в CI. |

Требования к коду:

* `#![deny(unsafe_code)]` во всех крейтах, кроме явно перечисленных модулей (при необходимости — `unsafe` только в кодеке SCF для zero-copy чтения, с обоснованием и `# Safety`-комментарием; предпочтителен полностью safe вариант через `bytemuck`).
* `#![warn(missing_docs)]`, `clippy::pedantic` включён, отклонения — точечно через `#[expect(...)]` с обоснованием.
* Запрещены `unwrap()`/`expect()`/`panic!` на путях, достижимых из JS. Паника внутри IO-потока обязана транслироваться в `UnknownError`, а не «убивать» процесс хоста (ловится через `catch_unwind` на границе IO-потока).

### 2.2. Состав workspace

```
boa-idb/
├── Cargo.toml                  # workspace
├── crates/
│   ├── boa_idb/                # фасад: JS-биндинги, регистрация, DOM-шим
│   ├── boa_idb_core/           # ядро без зависимости от boa_engine
│   ├── boa_idb_sqlite/         # бэкенд SQLite
│   ├── boa_idb_fs/             # бэкенд «файлы на диске»
│   ├── boa_idb_memory/         # in-memory бэкенд (тесты, эфемерные среды)
│   └── boa_idb_wpt/            # dev-only: раннер web-platform-tests
├── examples/
├── benches/
├── fuzz/
└── docs/
```

**Принципиальное разделение:** `boa_idb_core` **не зависит** от `boa_engine`. В нём живут: типы ключей и их кодирование, SCF-кодек (работающий с промежуточным представлением `ScValue`, а не с `JsValue`), планировщик транзакций, реализация алгоритмов §5–§6 Спеки, трейты бэкендов, типы ошибок. Это даёт: (а) быстрые unit- и property-тесты без поднятия JS-контекста; (б) возможность повторного использования ядра в других движках/раннерах; (в) чистую границу «нет JS-объектов в IO-потоке» (см. 3.4).

### 2.3. Зависимости

`boa_idb_core`:

| Крейт | Версия* | Назначение |
|---|---|---|
| `thiserror` | 2 | типы ошибок |
| `smallvec` | 1 | буферы ключей без аллокаций для коротких ключей |
| `indexmap` | 2 | детерминированный порядок свойств при клонировании |
| `hashbrown` / `rustc-hash` | — | внутренние карты |
| `crc32fast` | 1 | контрольные суммы фреймов (нужно для fs-бэкенда и валидации SCF) |
| `num-bigint` | 0.4 | сериализация `BigInt` (совместимо с `boa_engine`) |
| `bytemuck` | 1 | safe-приведения байтовых представлений |
| `tracing` | 0.1 | опциональная трассировка (фича `tracing`) |

`boa_idb`:

| Крейт | Версия* | Назначение |
|---|---|---|
| `boa_engine` | 0.22 | движок; фичи по умолчанию, без `intl` |
| `boa_gc` | 0.22 | `Trace`/`Finalize` |
| `boa_runtime` | 0.22 | опционально (фича `runtime-interop`): переиспользование `structuredClone`-семантики и таймеров |
| `crossbeam-channel` | 0.5 | каналы «JS-поток ↔ IO-поток» |
| `futures-channel`, `futures-lite` | 0.3 / 2 | `oneshot`/`mpsc` для `NativeAsyncJob` |
| `parking_lot` | 0.12 | мьютексы/условные переменные |

`boa_idb_sqlite`: `rusqlite` (фичи `bundled` за фичей `bundled-sqlite`, `blob`, `functions` не требуются), `lru` (кэш prepared statements — либо собственный).

`boa_idb_fs`: `fs4`/`fd-lock` (advisory-локи), `tempfile` (dev), `memmap2` (опционально, за фичей `mmap`).

\* Мажорные версии фиксируются на старте проекта; точные минорные версии подбирает исполнитель и фиксирует в `Cargo.lock` + `deny.toml`. Обязателен `cargo-deny` в CI (лицензии: разрешены MIT/Apache-2.0/BSD/Unlicense/ISC/Zlib; запрещены GPL/AGPL/копилефт).

### 2.4. Cargo-фичи крейта `boa_idb`

| Фича | По умолчанию | Описание |
|---|---|---|
| `sqlite` | ✔ | бэкенд SQLite |
| `bundled-sqlite` | ✔ | статически собирать SQLite (иначе — системная библиотека) |
| `fs` | ✔ | файловый бэкенд |
| `memory` | ✔ | in-memory бэкенд |
| `dom-shim` | ✔ | регистрировать собственные `EventTarget`/`Event`/`DOMException`/`DOMStringList` |
| `runtime-interop` | ✗ | интеграция с `boa_runtime` (переиспользование `DOMException`, если появится; паритет с `structuredClone`) |
| `tracing` | ✗ | трассировка операций |
| `blob` | ✗ | (заглушка под v1.1) |
| `encryption` | ✗ | (заглушка под v1.1) |

Требование: любая комбинация фич должна собираться и проходить тесты; проверяется через `cargo hack --feature-powerset --depth 2`.

---

## 3. Архитектура

### 3.1. Слои

```
┌──────────────────────────────────────────────────────────────────────┐
│ JS-код в Boa: indexedDB.open(...), tx.objectStore("s").put(...)      │
└───────────────────────────────┬──────────────────────────────────────┘
                                │ вызовы native-методов классов
┌───────────────────────────────▼──────────────────────────────────────┐
│ L1. Binding layer  (crate boa_idb)                                   │
│  • классы Boa (impl Class / #[boa_class]) для 12 интерфейсов IDB      │
│  • WebIDL-конвертации аргументов, генерация исключений               │
│  • DOM-шим: EventTarget, Event, DOMException, DOMStringList          │
│  • JsValue ⇄ ScValue (структурное клонирование), JsValue ⇄ Key       │
│  • доставка результатов: NativeAsyncJob / NativeJob, dispatch событий│
└───────────────────────────────┬──────────────────────────────────────┘
                                │ команды/результаты, только POD-типы
┌───────────────────────────────▼──────────────────────────────────────┐
│ L2. Core / Engine  (crate boa_idb_core)                              │
│  • Registry: storage key → базы, connection queues                   │
│  • TransactionScheduler: scope/mode/порядок создания, старт, commit  │
│  • RequestQueue: FIFO per-transaction, savepoint per-request         │
│  • Алгоритмы §5–§6: store/retrieve/delete/count/clear/iterate       │
│  • Ключи: Key, compare, encode/decode; key path extract/inject       │
│  • SCF: encode/decode ScValue ⇄ bytes                                │
│  • Ошибки: IdbError → DOMException name                              │
└───────────────────────────────┬──────────────────────────────────────┘
                                │ trait StorageBackend / BackendTxn / BackendCursor
┌───────────────┬───────────────▼──────────────┬───────────────────────┐
│ L3a. SQLite   │ L3b. Files (log-structured)  │ L3c. Memory (BTreeMap)│
│ rusqlite, WAL │ WAL + сегменты + компакция    │ тесты, эфемерные среды│
└───────────────┴──────────────────────────────┴───────────────────────┘
```

### 3.2. Ключевые архитектурные решения (обязательные к исполнению)

**AD-1. Ядро не знает о JS.** Через границу L1↔L2 передаются только `Send`-типы: закодированные ключи (`Vec<u8>`/`SmallVec`), закодированные значения (`Vec<u8>`), метаданные схемы, коды ошибок. `JsValue`, `JsObject`, `Context` за границу не проходят никогда.

**AD-2. Структурное клонирование выполняется в JS-потоке, синхронно, в момент вызова API.** Это прямое требование Спеки (§5.11 «Clone a value» выполняется до постановки запроса в очередь и до возврата из метода; исключение `DataCloneError` бросается синхронно). Результат — байты SCF, которые далее живут вне JS.

**AD-3. Извлечение ключа из значения (§7.1) и инжекция (§7.2) выполняются в JS-потоке** — они требуют вызова геттеров и `[[Get]]` на JS-объекте.

**AD-4. Один поток ввода-вывода на одну открытую IDB-базу** («database thread»). Поток владеет объектом бэкенда, обрабатывает команды из канала строго последовательно и не имеет доступа к `Context`. Пул потоков (`IoRuntime`) конфигурируется хостом; допустимо схлопывание в один общий поток при большом числе баз (лимит настраиваемый, по умолчанию — поток на базу, максимум 32, далее — шардирование).

**AD-5. Доставка результатов через `NativeAsyncJob`, один драйвер на транзакцию.** Для каждой IDB-транзакции L1 создаёт одну задачу-драйвер, которая в цикле: (1) берёт первый незавершённый запрос из своей очереди, (2) `await`-ит `oneshot` от IO-потока, (3) устанавливает `result`/`error` запроса, (4) активирует транзакцию, диспатчит `success`/`error`, деактивирует транзакцию, (5) повторяет. Это гарантирует требуемый Спекой порядок «результаты возвращаются в порядке постановки запросов» (§2.7.1 п.2) без дополнительных блокировок и без гонок между несколькими задачами.

**AD-6. Транзакция бэкенда живёт ровно столько, сколько IDB-транзакция.** Для `readonly` это даёт требуемый Спекой snapshot (§2.7.2: «данные, возвращаемые readonly-транзакцией, остаются постоянными»). Для SQLite это означает отдельное соединение на каждую активную IDB-транзакцию (пул соединений, WAL-режим).

**AD-7. Savepoint на каждый запрос.** Требование §2.11 (откат key generator при неуспешной операции) и §5.6 реализуется вложенной точкой сохранения бэкенда: `begin_request()` / `commit_request()` / `rollback_request()`.

**AD-8. Активность транзакции определяется границами задач.** Аналог «cleanup Indexed Database transactions» (§2.7.1). Реализуется обёрткой над `JobExecutor` хоста (раздел 9.3): после выполнения каждой задачи все транзакции, созданные/активированные в ней, переводятся в `inactive`, после чего проверяется условие автокоммита.

### 3.3. Объектная модель и GC

* Каждый JS-объект IDB-интерфейса реализуется как класс Boa с native-данными: `#[derive(Trace, Finalize, JsData)]` + `impl Class` (или атрибут `#[boa_class]` из `boa_macros`, если он покрывает нужный набор геттеров/методов — выбор фиксируется на этапе M1 и применяется единообразно).
* Native-данные хранят **идентификаторы** (`ConnectionId`, `TransactionId`, `StoreId`, `IndexId`, `RequestId`), а не `Rc` на состояние ядра. Само состояние живёт в `IdbRuntime`, помещённом в `Context` через `HostDefined` (`context.insert_data`/`get_data`).
* Ссылки, которые обязаны быть достижимы для GC (обработчики событий, `source`, `transaction`, `result`), хранятся как `JsObject`/`JsValue` внутри native-данных и корректно трассируются (`#[unsafe_ignore_trace]` — только для `Send`-полей без JS-указателей, с обоснованием).
* Требование: наличие незавершённой транзакции или запроса **не должно** течь памятью. `IdbRuntime` хранит слабые дескрипторы; при сборке `IDBRequest` без слушателей задача-драйвер обязана продолжить работу (транзакция должна закоммититься даже если скрипт потерял ссылку на запрос) — но native-состояние обязано освобождаться при `finished`.
* Обязателен тест: 10 000 циклов «открыть базу → 100 транзакций → закрыть» без роста RSS более чем на согласованный порог (раздел 13.6).

### 3.4. Потоковая модель и структура сообщений

```rust
// boa_idb_core::proto (упрощённо, окончательные сигнатуры — за исполнителем)
pub enum Command {
    Open { name: String, version: Option<u64>, reply: Reply<OpenOutcome> },
    Delete { name: String, reply: Reply<DeleteOutcome> },
    ListDatabases { reply: Reply<Vec<DatabaseInfo>> },
    BeginTxn { txn: TxnId, scope: Vec<StoreId>, mode: TxnMode, durability: Durability,
               reply: Reply<()> },
    Exec { txn: TxnId, seq: u64, op: Operation, reply: Reply<OpOutcome> },
    Commit { txn: TxnId, reply: Reply<()> },
    Abort { txn: TxnId, reply: Reply<()> },
    // схема
    CreateStore { .. }, DeleteStore { .. }, RenameStore { .. },
    CreateIndex { .. }, DeleteIndex { .. }, RenameIndex { .. },
    Close { .. },
}

pub enum Operation {
    Put { store: StoreId, key: Option<KeyBytes>, value: ValueBytes, no_overwrite: bool },
    Get { source: Source, range: KeyRangeBytes },
    GetKey { .. }, GetAll { .. }, GetAllKeys { .. }, GetAllRecords { .. },
    Delete { store: StoreId, range: KeyRangeBytes },
    Clear { store: StoreId },
    Count { source: Source, range: KeyRangeBytes },
    OpenCursor { source: Source, range: KeyRangeBytes, direction: Direction, key_only: bool },
    CursorAdvance { cursor: CursorId, op: CursorOp },
    CursorUpdate { cursor: CursorId, value: ValueBytes },
    CursorDelete { cursor: CursorId },
}
```

Требования к каналам:

* Команды — `crossbeam_channel::Sender<Command>`, ответы — `futures_channel::oneshot`.
* Ограничение на размер очереди команд (backpressure): по умолчанию 1024 команды на транзакцию; при превышении — запрос всё равно принимается (Спека это требует), но включается предупреждение в `tracing` и (опционально) синхронное подтормаживание постановки. Отказ ставить запрос **недопустим**.
* IO-поток обязан завершаться корректно при `Drop` рантайма: дренаж канала, откат незакоммиченных транзакций, закрытие файлов, `join` с таймаутом (по умолчанию 5 с), после таймаута — `abort` + лог.

---

## 4. Публичный Rust API крейта (host-side API)

### 4.1. Регистрация в контексте

```rust
use boa_engine::Context;
use boa_idb::{IndexedDbExtension, StorageKey, DomShim};
use boa_idb_sqlite::SqliteBackendFactory;

let extension = IndexedDbExtension::builder()
    .storage_key(StorageKey::new("https://example.com"))      // обязательный
    .backend(SqliteBackendFactory::new("/var/lib/myapp/idb"))  // обязательный
    .quota_bytes(Some(256 * 1024 * 1024))                      // None = без лимита
    .default_durability(boa_idb::Durability::Relaxed)
    .max_key_len(1024)
    .max_value_len(64 * 1024 * 1024)
    .dom_shim(DomShim::Register)   // Register | Reuse | Skip
    .io_threads(boa_idb::IoThreads::PerDatabase { max: 32 })
    .build()?;

extension.register(&mut context)?;   // реализует boa_runtime::RuntimeExtension
```

Требования:

* `IndexedDbExtension` реализует трейт `boa_runtime::extensions::RuntimeExtension` (за фичей `runtime-interop`) **и** имеет собственный метод `register(&self, &mut Context) -> JsResult<()>`, не требующий `boa_runtime`.
* Регистрация идемпотентна в пределах контекста: повторный вызов возвращает ошибку `AlreadyRegistered`, а не перезаписывает глобальные объекты.
* Свойство `indexedDB` на глобальном объекте: `writable: false`, `enumerable: true`, `configurable: false` (как `[SameObject] readonly attribute`). Дополнительно регистрируются конструкторы всех интерфейсов IDB на глобальном объекте (`IDBKeyRange`, `IDBRequest`, … — они `[Exposed]`, значит должны быть видимы и проходить `instanceof`).
* `Symbol.toStringTag` для каждого интерфейса, корректные имена/`length` конструкторов, «неконструируемость» тех интерфейсов, которые не имеют `constructor` в IDL (вызов `new IDBRequest()` → `TypeError`).

### 4.2. Трейт бэкенда

Трейты **синхронные** и `dyn`-совместимые: они исполняются внутри IO-потока, асинхронность обеспечивается на уровне выше. Это сознательное решение (упрощает реализацию бэкендов и делает `rusqlite` естественным).

```rust
pub trait BackendFactory: Send + Sync + 'static {
    /// Открывает/создаёт физическое хранилище для storage key.
    fn open_storage(&self, key: &StorageKey) -> Result<Box<dyn Storage>, BackendError>;
}

pub trait Storage: Send + 'static {
    fn list_databases(&self) -> Result<Vec<DatabaseInfo>, BackendError>;
    /// Открыть базу; создаёт с version=0, если не существует.
    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError>;
    fn delete_database(&self, name: &str) -> Result<(), BackendError>;
    fn usage_bytes(&self) -> Result<u64, BackendError>;
}

pub trait Database: Send + 'static {
    fn metadata(&self) -> &DatabaseMeta;             // version, stores, indexes, key gens
    fn begin(&mut self, mode: TxnMode, scope: &[StoreId], durability: Durability)
        -> Result<Box<dyn BackendTxn + '_>, BackendError>;
    fn flush(&mut self) -> Result<(), BackendError>;
    fn close(self: Box<Self>) -> Result<(), BackendError>;
}

pub trait BackendTxn {
    // --- вложенные точки сохранения (одна на IDB-запрос) ---
    fn begin_request(&mut self) -> Result<(), BackendError>;
    fn commit_request(&mut self) -> Result<(), BackendError>;
    fn rollback_request(&mut self) -> Result<(), BackendError>;

    // --- схема (только для versionchange) ---
    fn create_store(&mut self, spec: &StoreSpec) -> Result<StoreId, BackendError>;
    fn delete_store(&mut self, id: StoreId) -> Result<(), BackendError>;
    fn rename_store(&mut self, id: StoreId, new_name: &str) -> Result<(), BackendError>;
    fn create_index(&mut self, store: StoreId, spec: &IndexSpec) -> Result<IndexId, BackendError>;
    fn delete_index(&mut self, id: IndexId) -> Result<(), BackendError>;
    fn rename_index(&mut self, id: IndexId, new_name: &str) -> Result<(), BackendError>;
    fn set_version(&mut self, v: u64) -> Result<(), BackendError>;

    // --- данные ---
    fn put(&mut self, store: StoreId, key: &[u8], value: &[u8], no_overwrite: bool)
        -> Result<(), BackendError>;
    fn get(&mut self, store: StoreId, range: &EncodedRange)
        -> Result<Option<(KeyBytes, ValueBytes)>, BackendError>;
    fn delete_range(&mut self, store: StoreId, range: &EncodedRange) -> Result<(), BackendError>;
    fn clear(&mut self, store: StoreId) -> Result<(), BackendError>;
    fn count(&mut self, src: SourceRef, range: &EncodedRange) -> Result<u64, BackendError>;
    fn scan(&mut self, src: SourceRef, range: &EncodedRange, dir: Direction,
            unique: bool, limit: Option<u64>, key_only: bool)
        -> Result<Box<dyn BackendCursor + '_>, BackendError>;

    // --- key generator ---
    fn key_gen_next(&mut self, store: StoreId) -> Result<Option<f64>, BackendError>;
    fn key_gen_bump(&mut self, store: StoreId, to: f64) -> Result<(), BackendError>;

    // --- индексы (обслуживаются ядром либо бэкендом — см. 8.1.2) ---
    fn index_put(&mut self, index: IndexId, key: &[u8], primary: &[u8], unique: bool)
        -> Result<(), BackendError>;
    fn index_delete_by_primary(&mut self, index: IndexId, primary: &[u8])
        -> Result<(), BackendError>;

    fn commit(self: Box<Self>) -> Result<(), BackendError>;
    fn abort(self: Box<Self>) -> Result<(), BackendError>;
}

pub trait BackendCursor {
    /// Позиционирование: см. §6.7. Возвращает false, если записи исчерпаны.
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError>;
    fn step(&mut self, count: u64) -> Result<bool, BackendError>;
    fn key(&self) -> &[u8];
    fn primary_key(&self) -> &[u8];
    fn value(&self) -> Option<&[u8]>;
}
```

Требования к трейтам:

1. Все методы обязаны быть безопасны при вызове в любом порядке: некорректный порядок — `BackendError::Internal`, а не паника.
2. Семантика `no_overwrite = true` (соответствует `add()`): если ключ существует — вернуть `BackendError::Constraint`.
3. `scan` обязан поддерживать все 4 направления и режим `unique` (`nextunique`/`prevunique`), а также ленивую итерацию (не материализовать весь диапазон).
4. Бэкенд **обязан** гарантировать атомарность `commit`: либо все изменения транзакции видимы, либо ни одно.
5. Бэкенд **обязан** обеспечивать изоляцию `readonly`-транзакции от параллельных коммитов (snapshot). Если бэкенд не может — он обязан объявить `Capabilities { snapshot_reads: false }`, и тогда ядро сериализует такие транзакции (деградация производительности, но не корректности).

### 4.3. Прочий host-API

* `IdbRuntimeHandle` — получение из `Context`; методы: `usage_bytes()`, `close_all()`, `flush_all()`, `set_quota()`, `stats()` (счётчики: транзакций начато/закоммичено/откачено, запросов, байт прочитано/записано, гистограммы задержек).
* `boa_idb::testing::MemoryBackendFactory` — для тестов хоста.
* Хук аудита: `trait IdbObserver { fn on_event(&self, ev: &IdbEvent); }` — регистрируется в билдере, получает события `DatabaseOpened`, `TransactionCommitted { duration, bytes_written }`, `QuotaExceeded`, `Corruption { .. }` и т. п. Нужен для интеграции с метриками хоста и для тестов.
* Все публичные типы ошибок — `#[non_exhaustive]`.

---

## 5. Поверхность JavaScript API

Общие требования ко всему разделу:

* **R5.0.1.** Конвертация аргументов выполняется строго по Web IDL: порядок операций (сначала конвертации в порядке объявления, затем проверки состояния — если Спека не требует иного), `EnforceRange` для `unsigned long long version` и `unsigned long count`, `[Clamp]` отсутствует, `DOMString` — через `ToString` (кроме `any`).
* **R5.0.2.** Все геттеры и методы обязаны проверять brand (`this` — экземпляр нужного класса), иначе `TypeError`.
* **R5.0.3.** Порядок проверок исключений должен точно соответствовать Спеке (WPT проверяет именно порядок: например, `TransactionInactiveError` vs `DataError` в `put()`).
* **R5.0.4.** Методы, возвращающие `IDBRequest`, обязаны возвращать **новый** объект и ставить запрос в очередь транзакции синхронно.
* **R5.0.5.** Прототипы: методы и геттеры находятся на `.prototype`, `enumerable: true`, `configurable: true`, `writable: true` для методов; геттеры — accessor properties. Проверяется отдельным тестом на shape объекта.

### 5.1. `IDBFactory` (глобальный `indexedDB`)

| Член | Требования |
|---|---|
| `open(name, version?)` | `version === 0` → `TypeError`; `version` конвертируется с `EnforceRange`; возвращает `IDBOpenDBRequest`; запуск алгоритма §5.1 «в параллель»; события `blocked`, `upgradeneeded`, `success`, `error` |
| `deleteDatabase(name)` | §5.3; `success` — это `IDBVersionChangeEvent` с `oldVersion` = прежняя версия, `newVersion = null` |
| `databases()` | Возвращает `Promise<sequence<IDBDatabaseInfo>>`; базы с версией 0 не включаются; результат — снапшот; реализуется через `NativeAsyncJob` + `JsPromise` |
| `cmp(a, b)` | Синхронно; конвертация value→key; невалидный ключ → `DataError`; возвращает `-1|0|1` |

Дополнительно: имена баз/хранилищ/индексов — произвольные последовательности UTF-16 code unit, включая пустую строку и непарные суррогаты. Требование: бэкенд обязан хранить их без потерь (см. 8.1.4 — экранирование имён при использовании в путях ФС).

### 5.2. `IDBDatabase`

| Член | Требования |
|---|---|
| `name`, `version` | значения, зафиксированные при создании соединения; при откате upgrade — версия возвращается к предыдущей |
| `objectStoreNames` | `DOMStringList`, отсортирован по «code unit less than» (§2, `create a sorted name list`) |
| `transaction(storeNames, mode?, options?)` | `storeNames`: `DOMString` или `sequence<DOMString>`; пустой список → `InvalidAccessError`; неизвестное имя → `NotFoundError`; активная upgrade-транзакция → `InvalidStateError`; `close pending` → `InvalidStateError`; `options.durability` |
| `createObjectStore(name, options?)` | только внутри live upgrade-транзакции, иначе `InvalidStateError`/`TransactionInactiveError`; невалидный keyPath → `SyntaxError`; `autoIncrement` + keyPath === `""` или keyPath — массив → `InvalidAccessError`; дубликат имени → `ConstraintError` |
| `deleteObjectStore(name)` | аналогично; отсутствие → `NotFoundError` |
| `close()` | §5.2; выставляет `close pending`; после — `transaction()` бросает `InvalidStateError`; уже начатые транзакции доигрываются |
| `onabort`, `onerror`, `onclose`, `onversionchange` | event handler IDL attributes |

Требование к событию `close`: генерируется только при аварийном закрытии (потеря доступа к ФС, повреждение, принудительное закрытие хостом), не при `close()`.

### 5.3. `IDBObjectStore`

| Член | Особые требования |
|---|---|
| `name` (get/**set**) | Сеттер работает только в live upgrade-транзакции; переименование в существующее имя → `ConstraintError`; обновляет `objectStoreNames` соединения |
| `keyPath` | `[SameObject]` в IDL отсутствует → для массивного keyPath при каждом обращении возвращается **новый** `Array` (проверяется WPT `idbobjectstore-keyPath*`); `null` при out-of-line ключах |
| `indexNames`, `transaction`, `autoIncrement` | по Спеке |
| `put(value, key?)`, `add(value, key?)` | Порядок: (1) проверка mode/state, (2) конвертация key→Key, (3) `keyPath` + explicit key → `DataError`, (4) отсутствие ключа без keygen → `DataError`, (5) клонирование значения (`DataCloneError`), (6) постановка запроса. `add` = `no_overwrite` |
| `delete(query)`, `clear()` | `readonly` → `ReadOnlyError` |
| `get`, `getKey`, `getAll`, `getAllKeys`, `getAllRecords`, `count` | `getAll`/`getAllKeys` принимают либо `query` + `count`, либо `IDBGetAllOptions` — по алгоритму `is a potentially valid key range` (§2.9); `getAllRecords(options)` возвращает массив `IDBRecord` (§4.8, §5.12) |
| `openCursor`, `openKeyCursor` | направление из `IDBCursorDirection`; результат запроса — `IDBCursorWithValue`/`IDBCursor` или `null` |
| `index(name)` | отсутствие → `NotFoundError`; кэширование index handle в пределах транзакции (одна и та же ссылка при повторных вызовах — требование §2.6.1 «только один index handle на индекс в транзакции») |
| `createIndex(name, keyPath, options?)`, `deleteIndex(name)` | только upgrade; `multiEntry` + массивный keyPath → `InvalidAccessError`; создание индекса обязано просканировать существующие записи и наполнить индекс, а при нарушении `unique` — прервать запрос с `ConstraintError` (и, как следствие, транзакцию) |

Важное требование к `IDBGetAllOptions`: поля `query`, `count`, `direction`; `count = 0` трактуется как «без лимита»; лимит по умолчанию — конфигурируемый (`max_get_all` по умолчанию 1 000 000 записей; при превышении — `UnknownError` с осмысленным сообщением, чтобы не OOM-ить хост).

### 5.4. `IDBIndex`

`name` (get/set), `objectStore`, `keyPath`, `multiEntry`, `unique`, `get`, `getKey`, `getAll`, `getAllKeys`, `getAllRecords`, `count`, `openCursor`, `openKeyCursor` — по §4.6. Требование: после удаления индекса (`deleteIndex`) или отката upgrade-транзакции все существующие `IDBIndex`-обёртки обязаны бросать `InvalidStateError` при любом обращении (флаг «deleted»). Аналогично для `IDBObjectStore`.

### 5.5. `IDBKeyRange`

Статические `only`, `lowerBound`, `upperBound`, `bound`; геттеры `lower`, `upper`, `lowerOpen`, `upperOpen`; метод `includes(key)`. Требования: `bound` с `lower > upper` → `DataError`; `lower === upper` при любом open-флаге допустим (пустой диапазон, если хоть один флаг открыт); конструктор недоступен (`new IDBKeyRange()` → `TypeError` «Illegal constructor»).

### 5.6. `IDBCursor` / `IDBCursorWithValue`

| Член | Требования |
|---|---|
| `source`, `direction`, `key`, `primaryKey`, `request`, `value` | `value` только у `IDBCursorWithValue`; `key`/`primaryKey` возвращают новое JS-значение при каждом чтении (кэширование значения — по §4.9, `[SameObject]` отсутствует; но `value` кэшируется до следующей итерации) |
| `advance(count)` | `count = 0` → `TypeError`; `got value flag = false` → `InvalidStateError` |
| `continue(key?)` | ключ не в направлении итерации → `DataError`; равный текущему → `DataError` |
| `continuePrimaryKey(key, primaryKey)` | только для индексных курсоров с направлением `next`/`prev` (не `*unique`), иначе `InvalidAccessError` |
| `update(value)` | `key only flag` → `InvalidStateError`; `readonly` → `ReadOnlyError`; при in-line keyPath изменённый ключ ≠ текущий → `DataError`; клонирование до постановки запроса |
| `delete()` | аналогично; `key only` → `InvalidStateError` |

Требование к переиспользованию запроса: итерация курсора возвращает результат в **том же** `IDBRequest`, что открыл курсор (§2.8, NOTE), с корректным сбросом `done flag` в `false` и обратно в `true`.

### 5.7. `IDBTransaction`

`objectStoreNames`, `mode`, `durability`, `db`, `error`, `objectStore(name)`, `abort()`, `commit()`, `onabort`, `oncomplete`, `onerror`.

Требования:

* `objectStore(name)` вне scope → `NotFoundError`; после `finished` → `InvalidStateError`; возвращает **тот же** handle при повторных вызовах (§2.2.1).
* `commit()` в состоянии `inactive`/`active` — переводит в `committing`; в `committing`/`finished` → `InvalidStateError`. После `commit()` новые запросы → `TransactionInactiveError`.
* `abort()` в `committing`/`finished` → `InvalidStateError`; иначе — откат, `error = null`, событие `abort`.
* `error` — `DOMException` или `null`; доступен только после `abort`/сбоя.
* Событие `complete` — тип `Event`, `bubbles = false`; `abort` — `bubbles = true` (по DOM-дереву IDB: transaction → connection).

### 5.8. `IDBRequest` / `IDBOpenDBRequest`

`result`, `error`, `source`, `transaction`, `readyState`, `onsuccess`, `onerror` (+ `onblocked`, `onupgradeneeded`). Требования: чтение `result`/`error` при `readyState === "pending"` → `InvalidStateError`; `result` после ошибки → `undefined`.

### 5.9. `IDBRecord` (§4.8, новое в 3.0)

Интерфейс с геттерами `key`, `primaryKey`, `value`. Создаётся только реализацией (конструктор недоступен). Используется в результате `getAllRecords()`.

### 5.10. `IDBVersionChangeEvent`

Конструктор доступен из JS (`new IDBVersionChangeEvent(type, init)`), поля `oldVersion`, `newVersion`, наследует `Event`.

### 5.11. DOM-шим (фича `dom-shim`)

Минимально необходимый, но корректный DOM-слой:

* **`EventTarget`**: `addEventListener(type, cb, options)` (`capture`, `once`, `passive` — принимаются; `signal` — поддержать, если хост зарегистрировал `AbortSignal` из `boa_runtime::abort`), `removeEventListener`, `dispatchEvent`. Поддержка объектов-слушателей с методом `handleEvent`. Порядок вызова — порядок добавления; дубликаты (same type/callback/capture) не добавляются.
* **`Event`**: `type`, `target`, `currentTarget`, `eventPhase`, `bubbles`, `cancelable`, `defaultPrevented`, `composed`, `isTrusted`, `timeStamp`, `preventDefault()`, `stopPropagation()`, `stopImmediatePropagation()`, конструктор с `EventInit`.
* **Dispatch**: реализуется полный алгоритм DOM «dispatch» на упрощённом дереве: `get the parent` = для запроса → транзакция, для транзакции → соединение, для соединения → null (§2.1.1, §2.7, §2.8). Обязательны фазы capturing → at target → bubbling и флаг `legacyOutputDidListenersThrowFlag` (используется в §5.1 и §4.2 Спеки).
* **`DOMException`**: `name`, `message`, `code` (legacy-коды по WebIDL-таблице), `stack` (best effort), наследует `Error`, корректный `instanceof Error`, `toString()` вида `"ConstraintError: ..."`.
* **`DOMStringList`**: `length`, `item(i)`, `contains(s)`, индексный доступ (`[[GetOwnProperty]]` для индексов), итерируемость (`Symbol.iterator`) — браузеры её предоставляют, WPT местами использует `Array.from`.

Режимы фичи: `DomShim::Register` (регистрировать), `Reuse` (взять существующие глобальные `EventTarget`/`Event`/`DOMException` — валидировать их наличие и минимальную пригодность), `Skip` (не трогать; классы IDB всё равно наследуют от того, что лежит в глобальном `EventTarget`).

**R5.11.1.** Обработка необработанной ошибки запроса: если событие `error` дошло до конца распространения и `preventDefault()` не вызван — транзакция обязана быть прервана с этой ошибкой (§5.10). Если слушатель бросил исключение — `legacyOutputDidListenersThrowFlag = true` → транзакция прерывается с `AbortError`. Исключение обязано быть доставлено хосту как «uncaught error» (через хук `IdbObserver` и/или `console.error`, но **не** через возврат ошибки из `run_jobs`, чтобы не ломать event loop).

---

## 6. Ядро: ключи, кодирование, сериализация значений

### 6.1. Представление ключа

```rust
pub enum Key {
    Number(f64),           // не NaN
    Date(f64),             // мс от epoch, не NaN
    String(Utf16String),   // произвольная последовательность UTF-16 code unit
    Binary(Vec<u8>),
    Array(Vec<Key>),
}
```

**R6.1.1.** Строки хранятся как последовательности UTF-16 code unit (тип `Utf16String` поверх `boa_string::JsString` на уровне L1 и поверх `Vec<u16>`/собственного типа в `boa_idb_core`). Использование `String`/`&str` (валидный UTF-8) **запрещено**: непарные суррогаты обязаны переживать round-trip.

**R6.1.2.** `Ord` для `Key` реализуется строго по §2.4 «compare two keys»: `number < date < string < binary < array`; строки — по code unit; бинарные — по беззнаковым байтам; массивы — лексикографически, затем по длине.

**R6.1.3.** Глубина вложенности массивов ограничена (по умолчанию 32; настраивается). Превышение при конвертации → `DataError`. Циклический массив → `DataError` (§7.4 использует «seen» set).

### 6.2. Конвертация JS-значения в ключ (§7.4)

**R6.2.1.** Реализуется алгоритм «convert a value to a key» с результатами: `Key`, `invalid type`, `invalid value`. Различие между «invalid type» и «invalid value» критично для `getAll()`/`getAllKeys()` (см. `is a potentially valid key range`) и обязано быть отражено в типе результата.

**R6.2.2.** Обработка:

| Вход | Результат |
|---|---|
| `Number` (не NaN) | `Key::Number` |
| `NaN` | invalid value |
| `Date` с валидным `[[DateValue]]` | `Key::Date` |
| `Date` с NaN | invalid value |
| `String` | `Key::String` |
| `ArrayBuffer` / `SharedArrayBuffer`? | `ArrayBuffer` → `Key::Binary` (копия байт); `SharedArrayBuffer` → invalid type |
| detached `ArrayBuffer` / view | invalid value |
| `TypedArray` / `DataView` | `Key::Binary` по видимому окну (byteOffset..byteLength) |
| `Array` (exotic, `[[Class]] === "Array"`, не Proxy) | рекурсивно; `length` через `[[Get]]`; дыры/`undefined` → invalid value |
| `Proxy` вокруг массива | invalid type (Спека проверяет internal slot) |
| прочее | invalid type |

**R6.2.3.** Обратная конвертация «convert a key to a value» (§7.3): `Number` → number, `Date` → новый `Date`, `String` → строка, `Binary` → **новый `ArrayBuffer`** (не view), `Array` → новый `Array`. Все объекты создаются в текущем realm.

### 6.3. Кодирование ключей в байты (order-preserving)

Ключевое требование: **лексикографический порядок закодированных байт обязан совпадать с порядком `Key::cmp`**. Это позволяет любому бэкенду хранить ключи как `BLOB` и использовать нативную сортировку (`ORDER BY key`, `BTreeMap<Vec<u8>, _>`).

#### 6.3.1. Формат `KEY-v1`

| Тип | Тег | Тело |
|---|---|---|
| `number` | `0x10` | 8 байт: IEEE-754 big-endian с преобразованием знака |
| `date` | `0x20` | то же |
| `string` | `0x30` | CESU-8 (каждый UTF-16 code unit кодируется независимо), байт `0x00` экранируется как `0x00 0xFF`, терминатор `0x00 0x00` |
| `binary` | `0x40` | байты с экранированием `0x00` → `0x00 0xFF`, терминатор `0x00 0x00` |
| `array` | `0x50` | конкатенация закодированных элементов, терминатор `0x00` |

Преобразование знака для `f64`:

```rust
let bits = value.to_bits();
let ord = if bits & (1 << 63) != 0 { !bits } else { bits | (1 << 63) };
out.extend_from_slice(&ord.to_be_bytes());
```

Обоснование корректности:

* Теги монотонны и совпадают с порядком типов из §2.4.
* Преобразование знака даёт полный порядок на `f64` без NaN, включая `-Infinity < ... < +Infinity` и `-0.0 == 0.0` — **внимание**: `-0.0` и `+0.0` дают разные битовые паттерны. Требование: перед кодированием выполнять нормализацию `if v == 0.0 { v = 0.0 }` (превращает `-0.0` в `+0.0`), иначе нарушается требование `cmp(-0, 0) === 0`.
* CESU-8 кодирует каждый UTF-16 code unit (включая непарные суррогаты) независимо в 1–3 байта и сохраняет порядок code unit: `0x7F` → `7F` < `0x80` → `C2 80`; суррогаты `D800–DFFF` → `ED A0 80 … ED BF BF` < `U+E000` → `EE 80 80`. **Важно:** «модифицированный UTF-8» с `U+0000` → `C0 80` использовать нельзя — он ломает порядок (`C0 …` сортируется выше всех остальных code unit). Поэтому `U+0000` кодируется как `00`, а байт `0x00` экранируется тем же правилом, что и в `binary`.
* Терминатор `0x00 0x00` меньше любого экранированного нуля (`0x00 0xFF`) и любого непустого продолжения, поэтому «префикс сортируется раньше»; терминатор массива `0x00` меньше любого тега (`≥ 0x10`) — то же свойство для массивов.

**R6.3.2.** Кодирование обязано быть **инъективным и обратимым**: `decode(encode(k)) == k` для любого валидного `k`. Проверяется property-тестом.

**R6.3.3.** Property-тест (обязателен): для 10⁶ случайных пар ключей `cmp(a,b) == encode(a).cmp(&encode(b))`. Генератор обязан покрывать: `±0.0`, `±Infinity`, денормализованные числа, пустые строки/массивы/буферы, непарные суррогаты, строки, различающиеся только суррогатами vs BMP-символами ≥ U+E000, вложенные массивы, ключи-префиксы друг друга.

**R6.3.4.** Кодирование диапазонов: `EncodedRange { lower: Option<(Vec<u8>, bool /*open*/)>, upper: Option<(Vec<u8>, bool)> }`. Для «открытой» границы бэкенд использует строгое сравнение; допускается преобразование в полуинтервал через `successor(bytes)` (добавление байта `0x00`) — реализация обязана быть задокументирована и покрыта тестами.

**R6.3.5.** Ограничение размера: закодированный ключ ≤ `max_key_len` (по умолчанию 1 KiB). Превышение → `DataError` («Key too large»). Для индексных ключей — тот же лимит.

### 6.4. Структурное клонирование значений (SCF)

#### 6.4.1. Общие требования

**R6.4.1.** Реализуется `StructuredSerializeForStorage` в объёме, необходимом для IDB: **без** передачи transferable-объектов, **с** обработкой циклов и разделяемых ссылок (memo-таблица), **с** отклонением несериализуемых значений (`DataCloneError`).

**R6.4.2.** Сериализация — двухфазная: `JsValue → ScValue` (в JS-потоке, использует `Context`) и `ScValue → bytes` (может быть в любом потоке). Десериализация — зеркально. Промежуточное представление `ScValue` определяется в `boa_idb_core` и не зависит от Boa. Это позволяет тестировать кодек без движка и переиспользовать его.

Допускается оптимизация: прямое `JsValue → bytes` без материализации `ScValue` (single pass) — при условии сохранения побайтовой идентичности формата и наличия обоих путей в тестах.

#### 6.4.2. Поддерживаемые типы

| Категория | Типы |
|---|---|
| Примитивы | `undefined`, `null`, `true`, `false`, `Number` (i32-оптимизация + f64), `BigInt`, `String` |
| Объекты-обёртки | `Boolean`, `Number`, `String`, `BigInt` (boxed) |
| Даты | `Date` (в т. ч. invalid date) |
| Регулярные выражения | `RegExp` (source + flags; `lastIndex` не сохраняется — по HTML) |
| Коллекции | `Array` (в т. ч. разреженные — дыры сохраняются), `Object` (собственные enumerable string-ключи в порядке `OrdinaryOwnPropertyKeys`), `Map`, `Set` |
| Ошибки | `Error`, `EvalError`, `RangeError`, `ReferenceError`, `SyntaxError`, `TypeError`, `URIError`, `AggregateError` (`name`, `message`, `cause`, `errors`; `stack` — не сохраняется) |
| Бинарные | `ArrayBuffer` (в т. ч. resizable: `maxByteLength`), `DataView`, все `TypedArray` (включая `Float16Array`, если включён в сборке Boa) |

**Отклоняются** (`DataCloneError`): функции, `Symbol`, `WeakMap`/`WeakSet`/`WeakRef`, `Promise`, `Proxy` (любой), `SharedArrayBuffer`, detached `ArrayBuffer`, объекты платформенных классов (в т. ч. сами IDB-объекты), `arguments`, генераторы, любой объект с host-data, для которого не зарегистрирован сериализатор.

**R6.4.3.** Точка расширения: `trait ScSerializerHook` — хост может зарегистрировать сериализацию собственных классов (нужно для будущего `Blob`). Незарегистрированный host-объект → `DataCloneError`.

#### 6.4.3. Бинарный формат `SCF-v1`

```
Header:
  magic       : 4 байта  "IDB1"
  format_ver  : u8       = 1
  flags       : u8       bit0 = compressed (зарезервировано, в v1 всегда 0)
  reserved    : u16      = 0
Body:
  value       : Value    (см. ниже)
Trailer:
  crc32c      : u32      (по Header+Body, LE)   -- обязателен

Value := tag:u8 payload
tags:
  0x00 undefined      0x01 null           0x02 false          0x03 true
  0x04 int32   i32(zigzag varint)         0x05 double  f64(LE 8B)
  0x06 string  len:varint + UTF-16LE code units (или UTF-8 при flag)
  0x07 bigint  sign:u8 + len:varint + LE-байты величины
  0x08 date    f64
  0x09 regexp  source:string + flags:varint
  0x0A array   length:varint + count:varint + count×(index:varint, Value) + props
  0x0B object  count:varint + count×(key:string|int, Value)
  0x0C map     count:varint + count×(Value, Value)
  0x0D set     count:varint + count×Value
  0x0E error   kind:u8 + message:Value + cause:Value + errors:Value
  0x0F arraybuffer  max_byte_len:varint(0=не resizable) + len:varint + bytes
  0x10 typedarray   kind:u8 + byte_offset:varint + length:varint + Value(buffer|ref)
  0x11 dataview     byte_offset:varint + byte_length:varint + Value(buffer|ref)
  0x12 boxed_bool/num/str/bigint (subtag:u8 + Value)
  0x13 ref     index:varint            -- ссылка на ранее встреченный объект (memo)
  0x14 hole    -- дыра в разреженном массиве
  0x20..0x2F  зарезервировано: blob, file, filelist, imagedata, imagebitmap
  0xF0..0xFF  зарезервировано под host-hooks
```

**R6.4.4.** Каждый объектный узел (`array`, `object`, `map`, `set`, `error`, `arraybuffer`, `typedarray`, `dataview`, boxed) при сериализации получает порядковый номер в memo-таблице; повторная встреча → `ref`. При десериализации объект **обязан** быть зарегистрирован в memo **до** сериализации/десериализации своих детей (иначе циклы не восстановятся).

**R6.4.5.** `TypedArray`/`DataView`, разделяющие один `ArrayBuffer`, обязаны после round-trip разделять один и тот же буфер (проверяется WPT-тестами structured clone).

**R6.4.6.** Совместимость: декодер обязан отвергать `format_ver > 1` с `NotReadableError` (не паниковать, не читать мусор). Резервные диапазоны тегов при встрече в v1 → `NotReadableError`. При добавлении новых типов в v1.1 повышается `format_ver`; старый декодер корректно откажет, новый — прочитает оба.

**R6.4.7.** Ограничения: максимальная глубина вложенности при сериализации/десериализации — 512 (защита от stack overflow; реализация десериализации **обязана** быть либо итеративной с явным стеком, либо иметь проверку глубины до рекурсии). Максимальный размер значения — `max_value_len` (по умолчанию 64 MiB); превышение → `QuotaExceededError` при записи.

**R6.4.8.** CRC проверяется при каждом чтении. Несовпадение → `NotReadableError` + событие `Corruption` в `IdbObserver`.

### 6.5. Key path (§2.5, §7.1, §7.2)

**R6.5.1.** Валидация: пустая строка; `IdentifierName` по ECMA-262 (полная поддержка Unicode ID_Start/ID_Continue, `$`, `_`, escape-последовательности **не** допускаются); последовательность идентификаторов через `.`; непустой массив строк, каждая из которых удовлетворяет предыдущим правилам. Невалидный → `SyntaxError`. Массив key path запрещён для `multiEntry`-индексов и для `autoIncrement`-хранилищ.

**R6.5.2.** Извлечение (§7.1): последовательные `[[Get]]`; для строк и массивов допускается `length`; `undefined`-промежуточное звено → «failure» (не ошибка!) → трактуется вызывающим как отсутствие ключа. Для массива key path результат — `Key::Array` из подключей; любой сбой подключа → failure.

**R6.5.3.** Инжекция (§7.2): создаёт недостающие промежуточные объекты; попытка задать свойство на примитиве → `DataError`. Инжекция выполняется **в клон значения** (на уровне `ScValue`), а не в исходный JS-объект — это важно: скрипт не должен видеть изменения. Реализация обязана уметь инжектировать ключ в `ScValue`-дерево, включая создание промежуточных `object`-узлов.

**R6.5.4.** «Check that a key could be injected into a value» (используется в `add`/`put` до генерации ключа) реализуется отдельной функцией без побочных эффектов.

### 6.6. Индексные ключи

**R6.6.1.** При записи/обновлении/удалении записи объектного хранилища ядро обязано атомарно пересчитать записи всех индексов этого хранилища (§6.1 «store a record into an object store», шаги по индексам).

**R6.6.2.** `multiEntry`: если извлечённый ключ — массив, из него берутся подключи; невалидные подключи пропускаются; дубликаты подключей удаляются (после чего, если `unique`, проверка уникальности идёт по оставшимся). Если ключ не массив — одна запись.

**R6.6.3.** `unique`: нарушение → `ConstraintError` для конкретного запроса; savepoint запроса откатывается, key generator не двигается, транзакция прерывается, если ошибка не обработана скриптом.

---

## 7. Ядро: транзакции, соединения, запросы, курсоры

### 7.1. Реестр и connection queue

**R7.1.1.** `Registry` хранит по каждому `(storage_key, db_name)`: текущую версию, список открытых соединений (`Connection`), очередь открытых запросов (connection queue) и флаг «идёт upgrade/delete».

**R7.1.2.** Open-запросы обрабатываются строго по одному на базу и строго в порядке постановки (§2.8.2). Реализация — конечный автомат, шагающий при событиях: «соединение закрыто», «upgrade завершён», «истёк таймер blocked» (таймера нет — только события).

**R7.1.3.** Алгоритм §5.1 «opening a database connection» реализуется полностью, включая:
* сравнение версий, `VersionError` при `requested < current`;
* рассылку `versionchange` всем прочим соединениям (с `oldVersion`/`newVersion`);
* событие `blocked`, если после рассылки остались открытые соединения (в т. ч. с `close pending = false`);
* ожидание закрытия всех соединений перед `upgradeneeded`;
* создание upgrade-транзакции, её активность внутри обработчика `upgradeneeded`;
* корректный порядок `upgradeneeded` → (транзакция `complete`) → `success`.

**R7.1.4.** §5.3 «deleting a database» — аналогично, с рассылкой `versionchange` и `blocked`, финальное событие — `IDBVersionChangeEvent` с `newVersion = null`.

**R7.1.5.** §5.7/§5.8 (upgrade и abort upgrade): при откате upgrade-транзакции обязаны откатиться версия базы, создание/удаление/переименование хранилищ и индексов, а версия соединения — вернуться к предыдущей.

### 7.2. Планировщик транзакций

**R7.2.1.** Реализуются правила §2.7.2:
* `readonly` стартует, когда нет более ранних незавершённых `readwrite` с пересекающимся scope;
* `readwrite` стартует, когда нет более ранних незавершённых транзакций (любых) с пересекающимся scope;
* `versionchange` эксклюзивна на всю базу.

**R7.2.2.** Планировщик обязан быть FIFO-справедливым: не допускать голодания `readwrite` при потоке `readonly`. Формально это следует из правил (более ранние транзакции блокируют более поздние), но реализация обязана явным тестом подтверждать отсутствие reordering.

**R7.2.3.** Транзакция создаётся в состоянии `active` синхронно в вызове `db.transaction()`; старт (физический `BEGIN`) — асинхронный. Запросы, поставленные до старта, копятся в очереди и исполняются после.

**R7.2.4.** Автокоммит: после того как (а) все поставленные запросы завершены и их события обработаны, (б) транзакция `inactive`, (в) не было `abort` — планировщик обязан инициировать commit. Проверка выполняется в конце каждой задачи (раздел 9.3).

**R7.2.5.** Явный `commit()` переводит транзакцию в `committing` немедленно; ожидающие запросы доигрываются, новые — отклоняются.

**R7.2.6.** Откат: `abort` обязан откатить физическую транзакцию бэкенда, включая изменения key generators, и выставить `error` (для `abort()` — `null`, для сбоя — соответствующий `DOMException`).

**R7.2.7.** Таймаут «залипших» транзакций: конфигурируемый (по умолчанию отключён). Если включён и транзакция не завершилась за N секунд — принудительный abort с `TimeoutError`-подобным `UnknownError` и записью в лог. Обязательно документировать как расширение (в браузерах такого нет).

### 7.3. Очередь запросов и исполнение

**R7.3.1.** Каждый запрос получает монотонный `seq` в рамках транзакции. Бэкенду команды отправляются в порядке `seq`; ответы обрабатываются в том же порядке (см. AD-5).

**R7.3.2.** Алгоритм §5.6 «asynchronously executing a request» реализуется буквально: постановка → выполнение «в параллель» → `queue a database task` для доставки результата → `fire a success/error event` (§5.9/§5.10) с активацией/деактивацией транзакции вокруг диспатча.

**R7.3.3.** Если транзакция прервана, все ещё не выполненные запросы обязаны завершиться с `AbortError` без диспатча событий (§5.5: их `done flag` устанавливается, событие `error` не выстреливает).

### 7.4. Курсоры

**R7.4.1.** Позиция курсора хранится как ключ (и `object store position` для индексных курсоров), а не как индекс (§2.10). Итерация после изменения данных внутри той же транзакции обязана видеть изменения (курсор «живой»).

**R7.4.2.** Реализация `iterate a cursor` (§6.7) — полностью, включая параметры `key`, `primaryKey`, `count`, режимы `*unique`, и корректную обработку удаления текущей записи.

**R7.4.3.** Курсор бэкенда может «протухнуть» после записи в ту же таблицу; реализация обязана либо использовать курсор с поддержкой переоткрытия (seek к сохранённой позиции), либо явно переоткрывать после каждой мутации в scope. Требование к производительности: полный проход 100 000 записей с `continue()` — без квадратичной сложности (переоткрытие с seek допустимо, если seek логарифмический).

**R7.4.4.** `nextunique`/`prevunique` для источника-хранилища эквивалентны `next`/`prev`. Для индекса `nextunique` возвращает первую запись каждого индексного ключа; `prevunique` — **тоже первую** (наименьший primary key) при обратной итерации по ключам (частая ошибка реализаций; покрыть тестом).

---

## 8. Слой хранения (backends)

### 8.1. Общие требования ко всем бэкендам

**R8.1.1 (атомарность).** Коммит IDB-транзакции атомарен относительно сбоя процесса и питания в пределах гарантий, объявленных `Durability`.

**R8.1.2 (индексы).** Обслуживание индексных записей выполняется **в ядре** (`boa_idb_core`), а не в каждом бэкенде: ядро вычисляет наборы «добавить/удалить индексные записи» и вызывает `index_put`/`index_delete_by_primary`. Исключение допускается для бэкенда, объявившего `Capabilities { native_indexes: true }`, — тогда он обязан обеспечить идентичную семантику (`multiEntry`, `unique`). Для v1.0 все три бэкенда используют ядро.

**R8.1.3 (изоляция).** `readonly` видит снапшот на момент старта. `readwrite` видит собственные незакоммиченные изменения (read-your-writes).

**R8.1.4 (имена).** Имена баз/хранилищ/индексов — произвольные UTF-16 последовательности. Правило: имена **никогда** не используются как имена файлов напрямую. Файловое имя = `base32(sha256(utf16le(name)))[..26]`, а оригинальное имя хранится внутри метаданных (в UTF-16 или WTF-8). Это снимает вопросы регистра, длины путей, зарезервированных имён Windows (`CON`, `NUL`, …), непарных суррогатов и path traversal.

**R8.1.5 (журналирование).** Все ошибки уровня хранилища логируются с контекстом (база, хранилище, операция) без содержимого пользовательских данных (только длины/хэши).

**R8.1.6 (проверка целостности).** У каждого бэкенда — метод `verify()` (dev/CLI): полный обход, проверка CRC, соответствия индексных записей записям хранилища, монотонности ключей.

### 8.2. Бэкенд SQLite (`boa_idb_sqlite`)

#### 8.2.1. Топология файлов

```
<root>/
  <storage_key_hash>/
    registry.sqlite            # реестр баз: name(BLOB utf16), version, file, created_at
    db-<db_name_hash>.sqlite   # одна SQLite-БД на одну IndexedDB-базу
    db-<db_name_hash>.sqlite-wal / -shm
    blobs/<db_name_hash>/xx/<sha256>.bin   # значения-переростки (см. 8.2.5)
```

Обоснование: `deleteDatabase` = удаление файла (быстро, без вакуума); повреждение одной базы не роняет остальные; блокировки SQLite не создают ложных конфликтов между независимыми базами.

Альтернатива «одна SQLite-БД на storage key» — **отклонена** (единый writer-лок на все базы противоречит §2.7.2, где транзакции к разным базам полностью независимы).

#### 8.2.2. Схема

```sql
PRAGMA page_size = 4096;          -- до создания
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;      -- FULL для durability = strict
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -8000;        -- 8 MiB, конфигурируемо
PRAGMA wal_autocheckpoint = 1000;

CREATE TABLE meta (
  k TEXT PRIMARY KEY,
  v BLOB
) WITHOUT ROWID;
-- k: 'schema_version' (u32), 'idb_version' (u64), 'name' (BLOB utf16le),
--    'scf_version' (u32), 'created_at', 'key_format' (u32)

CREATE TABLE object_stores (
  id             INTEGER PRIMARY KEY,
  name           BLOB NOT NULL,          -- UTF-16LE
  key_path       BLOB,                   -- NULL | SCF-закодированный string|array<string>
  auto_increment INTEGER NOT NULL DEFAULT 0,
  key_gen        REAL    NOT NULL DEFAULT 1,   -- current number
  UNIQUE(name)
);

CREATE TABLE indexes (
  id          INTEGER PRIMARY KEY,
  store_id    INTEGER NOT NULL REFERENCES object_stores(id) ON DELETE CASCADE,
  name        BLOB NOT NULL,
  key_path    BLOB NOT NULL,
  is_unique   INTEGER NOT NULL DEFAULT 0,
  multi_entry INTEGER NOT NULL DEFAULT 0,
  UNIQUE(store_id, name)
);

CREATE TABLE records (
  store_id INTEGER NOT NULL REFERENCES object_stores(id) ON DELETE CASCADE,
  key      BLOB NOT NULL,      -- KEY-v1
  value    BLOB,               -- SCF-v1, NULL если вынесено в blobs
  ext      TEXT,               -- имя внешнего файла, если value IS NULL
  vlen     INTEGER NOT NULL,   -- логическая длина значения (для квот)
  PRIMARY KEY (store_id, key)
) WITHOUT ROWID;

CREATE TABLE index_records (
  index_id INTEGER NOT NULL REFERENCES indexes(id) ON DELETE CASCADE,
  key      BLOB NOT NULL,      -- KEY-v1 (индексный ключ)
  pkey     BLOB NOT NULL,      -- KEY-v1 (первичный ключ записи)
  PRIMARY KEY (index_id, key, pkey)
) WITHOUT ROWID;

CREATE INDEX idx_index_records_pkey ON index_records(index_id, pkey);
```

Замечания:

* `WITHOUT ROWID` + составной PK даёт кластеризацию по `(store_id, key)` — сканы диапазонов идут по B-дереву без дополнительной сортировки. Это ключевое требование производительности.
* Уникальность индекса (`is_unique`) **не** выражается схемой (она зависит от строки), а проверяется ядром запросом существования перед вставкой в рамках той же транзакции. Это корректно, т. к. writer в базе один.
* `ON DELETE CASCADE` при `deleteObjectStore` может быть дорогим на больших хранилищах — допускается замена на явное пакетное удаление с прогрессом.

#### 8.2.3. Отображение транзакций

| IDB | SQLite |
|---|---|
| `readonly` старт | отдельное соединение из пула, `BEGIN DEFERRED` + первый `SELECT` (фиксация read-snapshot WAL) |
| `readwrite`/`versionchange` старт | соединение-писатель, `BEGIN IMMEDIATE` |
| `begin_request` | `SAVEPOINT r<seq>` |
| `commit_request` | `RELEASE r<seq>` |
| `rollback_request` | `ROLLBACK TO r<seq>; RELEASE r<seq>` |
| `commit` | `COMMIT` (+ `PRAGMA synchronous=FULL` и `wal_checkpoint(TRUNCATE)` при `durability = strict`) |
| `abort` | `ROLLBACK` |

**R8.2.1.** Пул соединений: по умолчанию 1 писатель + N читателей (`N = io_readers`, по умолчанию 4). Соединения переиспользуются; кэш prepared statements — на соединение, LRU на 64 запроса.

**R8.2.2.** `busy_timeout` + повтор при `SQLITE_BUSY` не должны маскировать логические ошибки; после 3 повторов — `UnknownError`.

**R8.2.3.** Запросы диапазонов формируются шаблонами и **обязаны** использовать индекс. Обязателен тест, проверяющий планы (`EXPLAIN QUERY PLAN`) на отсутствие `SCAN TABLE` для операций get/count/cursor по диапазону.

Примеры:

```sql
-- курсор next по хранилищу
SELECT key, value, ext FROM records
 WHERE store_id = ?1 AND key > ?2 AND key < ?3
 ORDER BY key ASC LIMIT ?4;

-- курсор prev по индексу с primaryKey
SELECT ir.key, ir.pkey, r.value, r.ext
  FROM index_records ir JOIN records r
    ON r.store_id = ?1 AND r.key = ir.pkey
 WHERE ir.index_id = ?2 AND (ir.key < ?3 OR (ir.key = ?3 AND ir.pkey < ?4))
 ORDER BY ir.key DESC, ir.pkey DESC LIMIT ?5;

-- count по диапазону индекса
SELECT COUNT(*) FROM index_records WHERE index_id = ?1 AND key >= ?2 AND key <= ?3;
```

#### 8.2.4. Миграции схемы бэкенда

`meta.schema_version` — целое. При открытии: если версия меньше текущей — выполнить миграции по цепочке в одной транзакции; если больше — `NotReadableError` («база создана более новой версией»). Миграции обязаны быть покрыты тестами с фикстурами (файлы старых версий хранятся в репозитории).

#### 8.2.5. Крупные значения

Значения > `inline_value_threshold` (по умолчанию 256 KiB) сохраняются во внешний файл `blobs/<hash>.bin` (содержимое SCF + CRC), в `records.value` пишется `NULL`, в `ext` — имя файла. Требования: запись во временный файл + `fsync` + `rename` **до** коммита SQLite-транзакции; удаление осиротевших файлов — фоновой сборкой (GC при открытии базы + после каждых N коммитов), с журналом «pending delete» внутри SQLite, чтобы не удалить нужное при откате.

### 8.3. Файловый бэкенд (`boa_idb_fs`)

Назначение: среды, где SQLite нежелателен (лицензионные/сборочные ограничения, отсутствие C-тулчейна, требование «чистого Rust»), а также сценарии, где важна простая ручная инспекция/бэкап данных.

#### 8.3.1. Топология

```
<root>/<storage_key_hash>/<db_name_hash>/
  LOCK                       # advisory-лок файла
  MANIFEST-000007            # актуальный манифест (последний по номеру)
  CURRENT                    # текстовый файл с именем актуального манифеста
  meta.scf                   # метаданные базы (имя, версия, хранилища, индексы, key gens)
  wal/000042.log             # журнал транзакций (фреймы)
  seg/000031.seg             # неизменяемые отсортированные сегменты (snapshot)
  blob/<sha256>.bin          # крупные значения
```

#### 8.3.2. Модель данных

Log-structured storage с in-memory упорядоченным индексом:

* В памяти: на каждое хранилище — `BTreeMap<KeyBytes, Location>`, где `Location = Inline(Bytes) | Segment { file, offset, len } | Blob(hash)`. На каждый индекс — `BTreeMap<(IndexKeyBytes, PrimaryKeyBytes), ()>`.
* На диске: сегменты (отсортированные, неизменяемые, с блочным индексом в хвосте) + WAL с фреймами последних транзакций.
* Открытие базы: чтение `CURRENT`/`MANIFEST` → загрузка индексных блоков сегментов (не всех данных!) → проигрывание WAL → готовность.

**R8.3.1.** Формат фрейма WAL:

```
frame := magic:u32("IWAL") | len:u32 | txn_seq:u64 | flags:u8 | payload | crc32c:u32
payload := count:varint × op
op := kind:u8 | store_or_index_id:varint | key:len+bytes | value?:len+bytes
```

Фрейм записывается целиком одним `write`; коммит транзакции = запись **одного** фрейма (или нескольких с флагом `CONTINUES`, последний — с флагом `COMMIT`). Валидным считается только префикс журнала до первого фрейма с битым CRC/длиной — остальное отбрасывается при восстановлении.

**R8.3.2.** Durability:
* `relaxed` — `write` без `fsync` (данные в page cache ОС);
* `strict` — `fsync`/`fdatasync` файла WAL до подтверждения коммита;
* `default` — конфигурация хоста (по умолчанию = `relaxed`).

**R8.3.3.** Компакция: при превышении WAL порога (по умолчанию 64 MiB или 10 000 фреймов) — фоновая (в том же IO-потоке, между транзакциями) запись нового сегмента, обновление манифеста (write temp → `fsync` → `rename` → `fsync` каталога), удаление устаревших WAL/сегментов. Компакция обязана быть прерываемой и не блокировать открытые readonly-снапшоты (старые сегменты удаляются по refcount).

**R8.3.4.** Снапшоты для `readonly`: MVCC на уровне «версия индекса». `BTreeMap` клонируется структурно (`im`/`rpds`-подобная персистентная структура **или** копирование карты) на старт readonly-транзакции. Требование: старт readonly-транзакции — O(1) или O(log n), но **не** O(n) по количеству записей. Рекомендуется персистентное (immutable) дерево; выбор библиотеки/собственной реализации — за исполнителем, с обоснованием в ADR.

**R8.3.5.** Ограничение: весь набор ключей одной базы должен помещаться в память. Требование к документации: явно указать оценку (≈ `keys × (key_len + 48)` байт) и рекомендовать SQLite для больших наборов. Обязателен конфигурируемый лимит `max_keys_in_memory` (по умолчанию 5 000 000) с `QuotaExceededError` при превышении.

**R8.3.6.** Мультипроцессная защита: `LOCK`-файл с эксклюзивным advisory-локом при открытии. Занято → `UnknownError` с понятным сообщением. Отдельно — тест, что после `kill -9` лок освобождается ОС и база открывается с восстановлением WAL.

### 8.4. In-memory бэкенд (`boa_idb_memory`)

Полная реализация трейтов на `BTreeMap`. Требования: те же семантики транзакций (savepoints через стек undo-логов), snapshot для readonly через persistent-структуру или copy-on-write. Используется как эталон в дифференциальном тестировании (раздел 13.4) и в WPT-прогонах (быстро).

### 8.5. Требования к crash-safety (для всех дисковых бэкендов)

**R8.5.1.** Тест «crash consistency»: отдельный бинарь-воркер выполняет сценарий записи, родительский процесс убивает его `SIGKILL` в случайный момент; затем база открывается и проверяется инвариант: набор данных соответствует состоянию после какого-то префикса закоммиченных транзакций (никаких «половинчатых» транзакций, никакой рассинхронизации индексов). Минимум 200 итераций в nightly CI.

**R8.5.2.** Тест «torn write»: искусственная порча последних N байт WAL/файла → база обязана открыться, отбросив незавершённое, либо вернуть `NotReadableError`, но не паниковать и не отдавать мусор.

**R8.5.3.** Все дисковые операции — через слой `trait FileSystem` (реальная ФС + `FaultInjectingFs` для тестов: ошибки `ENOSPC`, `EIO`, короткие записи, задержки).

---

## 9. Интеграция с event loop Boa

### 9.1. Модель задач

Boa 0.22 предоставляет `boa_engine::job::{Job, JobExecutor, NativeJob, NativeAsyncJob, TimeoutJob, PromiseJob}`; `JobExecutor` принимает `Rc<Self>`, `enqueue_job(Job, &mut Context)`, `run_jobs`/`run_jobs_async(&RefCell<&mut Context>)`.

**R9.1.1.** Крейт **не навязывает** свой исполнитель задач, но требует от хоста «настоящий» исполнитель — умеющий выполнять `NativeAsyncJob` без блокировки потока (в терминах документации Boa — не `IdleJobExecutor` и не блокирующий на каждом future). В `boa_idb` поставляется:
* `IdbJobExecutor<E>` — декоратор над любым `JobExecutor` хоста, добавляющий обязательный хук конца задачи (см. 9.3);
* `SimpleIdbExecutor` — готовый исполнитель на `futures_lite` + `FutureGroup` для приложений без собственного event loop (аналог `SimpleJobExecutor`, но не блокирующий на async-задачах).

**R9.1.2.** Если хост зарегистрировал IDB, но не установил совместимый исполнитель, `register()` обязан вернуть понятную ошибку (проверка через попытку постановки тестовой async-задачи или через явный флаг билдера `assume_executor_ok(true)`).

### 9.2. Отображение task sources

Спека вводит «database access task source». Отображение:

| Сущность Спеки | Реализация |
|---|---|
| «run these steps in parallel» | отправка `Command` в IO-поток |
| «queue a database task» | завершение `oneshot` → пробуждение `NativeAsyncJob`-драйвера |
| порядок задач одной транзакции | гарантируется единственным драйвером (AD-5) |
| порядок между транзакциями | не специфицирован Спекой; реализация — FIFO по готовности |
| микрозадачи (промисы) | штатный механизм Boa; IDB не вмешивается |

**R9.2.1.** События IDB **никогда** не диспатчатся синхронно внутри вызова API. Даже мгновенно доступный результат (например, ошибка) доставляется через задачу.

### 9.3. Деактивация транзакций на границе задачи

**R9.3.1.** Реализуется эквивалент «cleanup Indexed Database transactions» (§2.7.1): после завершения каждой задачи (job) все транзакции с `cleanup event loop == current` переводятся в `inactive`, затем проверяются условия автокоммита (R7.2.4).

**R9.3.2.** Реализация — в `IdbJobExecutor::run_jobs*`: после каждого `job.call(context)` вызывается `IdbRuntime::end_of_task(context)`. Дополнительно предоставляется публичная функция `boa_idb::end_of_task(&mut Context)` для хостов с собственным исполнителем — её вызов обязателен и документируется как контракт.

**R9.3.3.** Транзакция активна:
* синхронно, от возврата `db.transaction()` до конца текущей задачи;
* во время диспатча событий `success`/`error` её запросов (активируется до dispatch, деактивируется после — §5.9/§5.10);
* во время диспатча `upgradeneeded` (для upgrade-транзакции).

Вне этих окон любой запрос → `TransactionInactiveError`. Обязателен тест: `setTimeout(() => store.get(1), 0)` внутри обработчика → `TransactionInactiveError`.

### 9.4. Завершение работы

**R9.4.1.** При `Drop` контекста/рантайма: незавершённые транзакции откатываются, соединения закрываются, IO-потоки останавливаются. Данные закоммиченных транзакций обязаны быть на диске (с учётом durability).

**R9.4.2.** `IdbRuntimeHandle::flush_all()` — принудительный `fsync` всех бэкендов; используется хостом перед завершением процесса.

---

## 10. Обработка ошибок и DOMException

### 10.1. Внутренний тип ошибки

```rust
#[non_exhaustive]
pub enum IdbError {
    Abort, Constraint(String), DataClone(String), Data(String),
    InvalidAccess(String), InvalidState(String), NotFound(String),
    NotReadable(String), Syntax(String), ReadOnly, TransactionInactive,
    Unknown(String), Version(String), QuotaExceeded { needed: u64, available: u64 },
}
```

### 10.2. Таблица соответствия

| Внутренняя причина | `DOMException.name` | Где возникает |
|---|---|---|
| явный `abort()` | — (`transaction.error = null`) | §5.5 |
| прерывание из-за необработанной ошибки запроса | ошибка запроса | §5.10 |
| исключение в слушателе события | `AbortError` | §5.10 |
| нарушение `unique`-индекса, `add()` на существующий ключ, дубликат имени store/index, исчерпание key generator | `ConstraintError` | §6.1, §4.4 |
| несериализуемое значение | `DataCloneError` | §5.11 |
| невалидный ключ/диапазон/значение keyPath, слишком большой ключ | `DataError` | §7.1, §7.4 |
| пустой список хранилищ в `transaction()`, `multiEntry` + array keyPath, `autoIncrement` + array/empty keyPath, `continuePrimaryKey` на неподходящем курсоре | `InvalidAccessError` | §4.4–§4.9 |
| операция на удалённом store/index, доступ к `result` до готовности, `transaction()` при `close pending`, `commit()`/`abort()` в `finished` | `InvalidStateError` | везде |
| неизвестное имя store/index | `NotFoundError` | §4.4, §4.5 |
| ошибка чтения с диска, битый CRC, неизвестная версия формата | `NotReadableError` | бэкенд |
| невалидный key path | `SyntaxError` | §2.5 |
| мутация в `readonly` | `ReadOnlyError` | §4.5 |
| запрос к неактивной/завершённой транзакции | `TransactionInactiveError` | §2.7.1 |
| исчерпание квоты, превышение лимита размера значения | `QuotaExceededError` | §3 |
| `open(name, v)` при `v < current` | `VersionError` | §5.1 |
| прочие сбои (IO, паника в IO-потоке, `SQLITE_BUSY` после повторов, недостижимый бэкенд) | `UnknownError` | — |

**R10.2.1.** Все сообщения `message` — на английском, информативные, **без** пользовательских данных (никаких значений/ключей в тексте; допустимы длины и имена хранилищ).

**R10.2.2.** Ошибки бэкенда обязаны нести причину в цепочке `source()` для логов хоста, но в JS уходит только `name` + краткое сообщение.

**R10.2.3.** Паника в IO-потоке: перехватывается `catch_unwind`, поток помечается «отравленным», база принудительно закрывается (`close` с `forced = true` → событие `close` на соединениях), все ожидающие запросы получают `UnknownError`. Процесс хоста не падает. Событие `Corruption`/`Panic` уходит в `IdbObserver`.

---

## 11. Безопасность, приватность, квоты

**R11.1 (изоляция).** Разделение по storage key обязательно и обеспечивается структурой каталогов + проверкой на уровне `Registry`. JS-код не имеет способа обратиться к данным другого storage key.

**R11.2 (path traversal).** Имена, приходящие из JS, никогда не попадают в пути ФС (R8.1.4). Дополнительно: канонизация корневого каталога при старте (`fs::canonicalize`) и проверка, что все создаваемые пути лежат под ним.

**R11.3 (квоты).** Учёт объёма — по сумме `vlen` + служебных накладных расходов (оценка) на storage key. При превышении — `QuotaExceededError`. Проверка обязана быть до записи (по оценке размера) и после (по факту). Хост может задать `quota_bytes = None` (без лимита).

**R11.4 (лимиты для защиты от DoS).** Все конфигурируемы, значения по умолчанию:

| Лимит | По умолчанию |
|---|---|
| размер закодированного ключа | 1 KiB |
| размер значения | 64 MiB |
| глубина вложенности при клонировании | 512 |
| глубина массивных ключей | 32 |
| число хранилищ в базе | 1024 |
| число индексов на хранилище | 128 |
| число одновременно открытых баз на контекст | 64 |
| число одновременно живых транзакций | 512 |
| число записей в результате `getAll*` | 1 000 000 |
| число одновременно открытых курсоров на транзакцию | 256 |

**R11.5 (утечки через тайминги/размер).** Не является угрозой в модели «доверенный хост, недоверенный скрипт», но требуется: не возвращать в сообщениях об ошибках информацию о наличии данных другого storage key.

**R11.6 (стирание данных).** `deleteDatabase` обязана удалять все файлы базы, включая внешние blob-файлы и WAL. Опция билдера `secure_delete(true)` — перезапись нулями перед удалением (для SQLite — `PRAGMA secure_delete=ON`).

**R11.7 (fsync-дисциплина).** Метаданные (`meta.scf`, `MANIFEST`, `registry.sqlite`) обновляются только атомарно (temp + rename + fsync каталога). Порядок: данные → fsync → метаданные → fsync.

---

## 12. Нефункциональные требования и производительность

### 12.1. Целевые показатели

Эталонная конфигурация замеров: 4 vCPU, NVMe SSD, Linux, release-сборка с `lto = "thin"`, значения ~200 байт, ключи-числа, durability = `relaxed`.

| Сценарий | SQLite-бэкенд | FS-бэкенд |
|---|---|---|
| `put` в одной транзакции, ops/s (батч 10 000) | ≥ 60 000 | ≥ 120 000 |
| `get` по первичному ключу, ops/s (одна транзакция) | ≥ 100 000 | ≥ 300 000 |
| Полный проход курсором, записей/с | ≥ 150 000 | ≥ 400 000 |
| Пустая `readwrite`-транзакция (создание+коммит), ops/s | ≥ 5 000 | ≥ 10 000 |
| Оверхед на один запрос (JS↔ядро, без IO), µs | ≤ 15 | ≤ 15 |
| Открытие базы с 1 000 000 записей, с | ≤ 0.2 | ≤ 3.0 |
| `durability = strict`, коммитов/с (одиночные) | ≥ 200 | ≥ 200 |

Показатели — целевые ориентиры; при недостижении требуется письменное обоснование с профилем (flamegraph) и предложением по оптимизации. Регресс более 10 % относительно предыдущего релиза блокирует мёрж (benchmark-gate в CI на `criterion` с сохранением baseline).

### 12.2. Память

* Оверхед на открытое соединение — ≤ 64 KiB (без учёта кэшей бэкенда).
* Оверхед на живую транзакцию — ≤ 8 KiB.
* Курсор не материализует диапазон: чтение 10⁶ записей курсором обязано идти при постоянном потреблении памяти (проверяется тестом с лимитом RSS).
* `getAll` без лимита на 10⁶ записей — единственное место, где допускается большой пик; документируется.

### 12.3. Наблюдаемость

* Фича `tracing`: спаны `idb.open`, `idb.txn`, `idb.request` с атрибутами (база, режим, scope, seq, длительность, байты).
* Метрики через `IdbObserver` (раздел 4.3).
* Dev-инструмент `boa-idb-cli` (в `examples/` или отдельный бинарь, dev-only): `ls`, `dump`, `verify`, `compact`, `stats` для отладки хранилищ.

---

## 13. Тестирование и приёмка качества

### 13.1. Уровни тестирования

| Уровень | Объём | Инструменты |
|---|---|---|
| Unit (ядро) | ключи, кодеки, key path, планировщик, конечные автоматы | `cargo test`, `rstest`, `test-case` |
| Property-based | порядок ключей, round-trip SCF, инварианты планировщика | `proptest` |
| Дифференциальные | три бэкенда на одном сценарии дают одинаковый результат | собственный раннер |
| Интеграционные (JS) | сценарии на JS через `Context` | `indoc` + встроенные ассерты |
| Conformance | web-platform-tests `IndexedDB/**` | `boa_idb_wpt` |
| Crash/fault | SIGKILL, torn writes, `ENOSPC`, `EIO` | воркер-процесс + `FaultInjectingFs` |
| Fuzzing | SCF-декодер, декодер ключей, WAL-восстановление | `cargo-fuzz` (libFuzzer) |
| Perf | бенчмарки из 12.1 | `criterion` |
| Memory | отсутствие утечек и роста RSS | долгие циклы + `dhat`/`valgrind` (Linux) |

### 13.2. Покрытие

* Обязательное покрытие строк для `boa_idb_core` — **≥ 90 %**, для `boa_idb` — **≥ 80 %** (`cargo-llvm-cov`), измеряется в CI, регресс блокирует мёрж.
* Каждый идентификатор требования из этого ТЗ (`Rx.y.z`) обязан быть сопоставлен минимум одному тесту. Ведётся файл `docs/traceability.md` (таблица «требование → тест(ы)»). Это условие приёмки.

### 13.3. Conformance: web-platform-tests

**R13.3.1.** Разрабатывается раннер `boa_idb_wpt`, который:
* берёт `wpt/IndexedDB/**` из зафиксированного коммита (git submodule или vendored архив с указанием SHA);
* поднимает окружение: `testharness.js` + `testharnessreport.js`, `self`/`globalThis`, `setTimeout`/`clearTimeout` (из `boa_runtime::interval` или собственные), `EventTarget`/`Event`/`DOMException` (DOM-шим), `structuredClone`, `location` (заглушка), `add_completion_callback`;
* поддерживает форматы `*.any.js` (варианты `window`/`worker` сводятся к одному) и `*.htm`/`*.html` — для HTML-тестов допускается минимальный shim либо перевод в скрипт (список исключений фиксируется);
* выдаёт машинно-читаемый отчёт (JSON) с PASS/FAIL/SKIP по каждому subtest и сравнивает с зафиксированным `expectations.json`.

**R13.3.2.** Целевой уровень прохождения (subtests, исключая объявленные вне области работ разделы — Blob/File, workers, storage buckets, `ImageBitmap`, `nested-cloning` с Blob):

| Этап | Целевой % |
|---|---|
| M4 | ≥ 60 % |
| M5 | ≥ 80 % |
| M6 (release 1.0) | **≥ 92 %** |

Любой FAIL, не внесённый в `expectations.json` с обоснованием, блокирует релиз. Изменение `expectations.json` требует отдельного code review.

**R13.3.3.** Приоритетные файлы WPT (обязаны проходить полностью к M5): `idbfactory-*`, `idbdatabase-*`, `idbobjectstore-*`, `idbindex-*`, `idbcursor-*`, `idbtransaction-*`, `idbkeyrange*`, `key-conversion-exceptions`, `keyorder`, `keypath*`, `transaction-lifetime*`, `transaction-abort*`, `abort-in-initial-upgradeneeded`, `close-in-upgradeneeded`, `value*`, `structured-clone*`, `get-databases`, `idb-explicit-commit*`, `getall*`, `idb-binary-key*`.

### 13.4. Дифференциальное тестирование бэкендов

**R13.4.1.** Генератор случайных сценариев (последовательности операций: create/delete store, put/add/delete/clear, курсоры, abort/commit, upgrade) прогоняется на трёх бэкендах; итоговые состояния (полный дамп всех хранилищ и индексов) обязаны совпадать байт в байт, ответы каждой операции — совпадать. Минимум 10 000 сценариев в nightly.

**R13.4.2.** Модельное тестирование: простая эталонная модель IDB на `BTreeMap` без транзакций как «оракул» для проверки семантики (в том числе key generators и multiEntry).

### 13.5. Тестирование конкурентности и порядка

**R13.5.1.** Тесты на планировщик: пересекающиеся/непересекающиеся scope, порядок старта, отсутствие reordering, `versionchange`-эксклюзивность, `blocked`, живучесть при 100 параллельных транзакциях.

**R13.5.2.** Тест детерминированного порядка событий: последовательность 50 запросов в одной транзакции обязана дать события `success` строго в порядке постановки (проверяется журналом).

### 13.6. Тесты GC и утечек

**R13.6.1.** 10 000 итераций «открыть/закрыть базу + 100 транзакций»: рост RSS ≤ 5 % от плато и отсутствие роста числа живых native-объектов (счётчик в `IdbRuntime`, доступный в dev-сборке).

**R13.6.2.** Тест: потеря скриптом ссылки на `IDBRequest` и `IDBTransaction` не мешает завершению транзакции; после `complete` native-состояние освобождено.

### 13.7. CI

Обязательные джобы: `fmt`, `clippy -D warnings`, `test` (все ОС), `feature-powerset`, `msrv 1.91.0`, `llvm-cov`, `wpt`, `deny`, `doc` (`RUSTDOCFLAGS="-D warnings"`), `wasm32-check` (только `boa_idb_core`). Nightly: `fuzz` (30 мин на цель), `crash-consistency`, `differential`, `bench`.

---

## 14. Документация, сборка, поставка

**R14.1.** `rustdoc`: 100 % публичных элементов документированы; в корне крейта — обзор архитектуры и полный рабочий пример; примеры в doc-комментариях компилируются и проходят как doctests.

**R14.2.** `README.md`: назначение, быстрый старт, матрица поддержки (что реализовано / что нет / известные отличия от браузеров), выбор бэкенда, требования к event loop.

**R14.3.** `docs/`:
* `architecture.md` — слои, потоки, диаграммы последовательностей (open, put, cursor, upgrade, abort);
* `formats.md` — точные спецификации `KEY-v1` и `SCF-v1` (с байтовыми примерами) и схемы бэкендов;
* `adr/` — Architecture Decision Records по всем решениям AD-1…AD-8 и по выбору структур данных FS-бэкенда;
* `compat.md` — отличия от браузеров и список известных ограничений;
* `traceability.md` — требование → тесты;
* `operations.md` — эксплуатация: где лежат данные, как бэкапить, как восстанавливать, как читать метрики.

**R14.4.** Версионирование: SemVer. Формат данных версионируется отдельно (`schema_version`, `key_format`, `scf_version`); повышение мажорной версии крейта не обязано ломать формат, ломающее изменение формата обязано сопровождаться миграцией.

**R14.5.** Лицензия: MIT OR Apache-2.0 (совместимо с окружением Boa: Unlicense OR MIT). Все зависимости — с совместимыми лицензиями (`cargo-deny`).

**R14.6.** Публикация: `crates.io` (все крейты workspace, синхронные версии), `CHANGELOG.md` в формате Keep a Changelog, теги git, CI-релиз по тегу.

---

## 15. Этапы работ и трудоёмкость

Оценка в человеко-неделях (ч-нед) для инженера уровня senior, знакомого с Rust; предполагается 1–2 исполнителя.

| Этап | Содержание | Результат (артефакты) | Оценка |
|---|---|---|---|
| **M0. Проектирование** | ADR по AD-1…AD-8, окончательные сигнатуры трейтов, формат `KEY-v1`/`SCF-v1` на бумаге, скелет workspace, CI | `docs/adr/*`, `docs/formats.md`, каркас, зелёный CI | 2 |
| **M1. Ядро: ключи и кодеки** | `Key`, cmp, `KEY-v1` (+property-тесты), key path (валидация/extract/inject), `ScValue`, `SCF-v1` encode/decode, fuzz-цели | `boa_idb_core` (ключи+кодеки) с покрытием ≥ 90 % | 4 |
| **M2. Ядро: движок** | Registry, connection queue, планировщик, состояния транзакций, очередь запросов, алгоритмы §6, трейты бэкендов, memory-бэкенд | Ядро + memory-бэкенд, дифф-тесты против модели | 5 |
| **M3. JS-биндинги** | DOM-шим (EventTarget/Event/DOMException/DOMStringList), 12 классов IDB, WebIDL-конвертации, `IdbJobExecutor`, `end_of_task`, доставка результатов | Работающий `indexedDB` на memory-бэкенде; примеры из §1 Спеки исполняются | 6 |
| **M4. SQLite-бэкенд** | Схема, миграции, пул соединений, savepoints, курсоры, крупные значения, `EXPLAIN`-тесты | `boa_idb_sqlite`, WPT ≥ 60 % | 4 |
| **M5. WPT и стабилизация** | Раннер WPT, `expectations.json`, исправление расхождений (порядок ошибок, edge cases курсоров, upgrade-сценарии) | WPT ≥ 80 %, traceability-таблица | 5 |
| **M6. Файловый бэкенд** | WAL, сегменты, манифест, компакция, MVCC-снапшоты, локи, восстановление | `boa_idb_fs`, crash-тесты, WPT ≥ 92 % на обоих бэкендах | 5 |
| **M7. Производительность и надёжность** | Бенчмарки, профилирование, оптимизации, fault-injection, memory/GC-тесты, benchmark-gate | Показатели §12.1 достигнуты или обоснованы | 3 |
| **M8. Документация и релиз 1.0** | rustdoc, README, docs/, CHANGELOG, публикация, `boa-idb-cli` | Релиз 1.0.0 на crates.io | 2 |
| | | **Итого** | **36 ч-нед** |

Контрольные точки: демонстрация после каждого этапа; переход к следующему этапу — только при зелёном CI и выполненных критериях этапа.

Порядок этапов допускает распараллеливание: M4 (SQLite) может идти параллельно с M3 после завершения M2; M6 — параллельно с M5.

---

## 16. Критерии приёмки

Работа считается принятой при одновременном выполнении:

1. **Функциональность.** Все интерфейсы и алгоритмы разделов 5–9 реализованы; отклонения только те, что перечислены в 1.4 и продублированы в `docs/compat.md`.
2. **Conformance.** WPT `IndexedDB/**`: ≥ 92 % subtests PASS на бэкендах SQLite, FS и memory; все FAIL зафиксированы в `expectations.json` с обоснованием; ни одного CRASH/TIMEOUT.
3. **Совместимость платформы.** Сборка и зелёные тесты на `rustc 1.91.0` (без warnings) на трёх целевых ОС; `boa_idb_core` собирается под `wasm32-unknown-unknown`.
4. **Качество кода.** `cargo clippy --all-targets --all-features -D warnings` чист; `cargo fmt --check` чист; `unsafe` отсутствует либо обоснован в ADR; `cargo deny check` чист.
5. **Покрытие.** ≥ 90 % (`boa_idb_core`), ≥ 80 % (`boa_idb`); `docs/traceability.md` покрывает все требования `Rx.y.z`.
6. **Надёжность.** 200 итераций crash-consistency и 10 000 дифференциальных сценариев без расхождений; 4 fuzz-цели без падений за 4 часа суммарно; тесты fault-injection проходят.
7. **Производительность.** Показатели §12.1 достигнуты, либо каждое отклонение обосновано письменно с профилем.
8. **Память.** Тесты 13.6 проходят.
9. **Документация.** Разделы 14.1–14.3 выполнены; `cargo doc` без warnings; примеры компилируются.
10. **Демонстрация.** Приёмочный сценарий: реальное JS-приложение (например, обёртка типа `idb`/`Dexie`-подобный минимальный слой, либо WPT-независимый сценарий «каталог книг» из §1 Спеки + миграция версий 1→3 + курсоры + индексы + abort) исполняется в CLI-хосте на всех трёх бэкендах, данные переживают перезапуск процесса.

---

## 17. Риски и открытые вопросы

| # | Риск | Влияние | Митигация |
|---|---|---|---|
| R1 | Boa 0.22 может не иметь всех необходимых точек расширения (например, наследование от нативного класса, host-defined данные в объектах) | Высокое | На M0 — техническая проверка (spike) всех критичных приёмов: наследование `IDBOpenDBRequest → IDBRequest → EventTarget`, `Symbol.toStringTag`, брендирование, `HostDefined`. При пробелах — патч в upstream Boa (крейт лицензионно совместим) или обходной путь через прототипную цепочку вручную |
| R2 | Отсутствие в Boa DOM-инфраструктуры → нужно реализовать корректный dispatch | Среднее | Заложено в объём (5.11); риск в деталях (`legacyOutputDidListenersThrowFlag`, порядок фаз) снимается WPT |
| R3 | WPT сильно опирается на браузерное окружение (`document`, iframes, workers) | Среднее | Фильтрация набора, документированный список исключений, приоритетные файлы (13.3.3) |
| R4 | FS-бэкенд с in-memory индексом не масштабируется на большие наборы | Среднее | Явное ограничение и документация; SQLite как рекомендуемый бэкенд по умолчанию; в v1.1 — дисковый B-tree/LSM |
| R5 | Требование snapshot-изоляции для readonly в FS-бэкенде тянет persistent-структуры | Среднее | Решение фиксируется ADR на M0; допускается copy-on-write карты с ограничением по размеру, если производительность приемлема |
| R6 | Расхождения в порядке проверок исключений с браузерами | Низкое | WPT покрывает; порядок брать буквально из Спеки |
| R7 | Изменение Спеки (WD, не REC) во время разработки | Низкое | Фиксация конкретной ревизии (WD 13.08.2025 + указанный git-SHA editor's draft) в `docs/`; изменения — отдельным согласованием |
| R8 | `-0.0`, NaN, непарные суррогаты, resizable ArrayBuffer — источники тонких багов | Среднее | Property-тесты (R6.3.3) и целевые unit-тесты обязательны |
| R9 | Многопоточный доступ (несколько `Context`) в будущем | Низкое (v1.0) | Ядро уже потокобезопасно по конструкции (IO-поток + `Registry` под мьютексом); в v1.1 — общий `Registry` на процесс с межпоточной рассылкой `versionchange` |

**Открытые вопросы, требующие решения заказчика до M1:**

1. Какой бэкенд считать бэкендом по умолчанию в документации и примерах (предложение: SQLite).
2. Нужна ли поддержка нескольких storage key в одном контексте одновременно (предложение: один storage key на `IndexedDbExtension`, но несколько расширений в одном контексте недопустимы; при необходимости — API `switch_storage_key`).
3. Требуется ли `boa-idb-cli` как поставляемый бинарь или достаточно dev-инструмента.
4. Целевой процент WPT: 92 % — предложение исполнителя; при требовании выше 95 % нужен бюджет на реализацию Blob/File (≈ +4 ч-нед).

---

## 18. Приложения

### Приложение A. Полный WebIDL (нормативный объём реализации)

```webidl
[Exposed=(Window,Worker)]
interface IDBRequest : EventTarget {
  readonly attribute any result;
  readonly attribute DOMException? error;
  readonly attribute (IDBObjectStore or IDBIndex or IDBCursor)? source;
  readonly attribute IDBTransaction? transaction;
  readonly attribute IDBRequestReadyState readyState;
  attribute EventHandler onsuccess;
  attribute EventHandler onerror;
};
enum IDBRequestReadyState { "pending", "done" };

[Exposed=(Window,Worker)]
interface IDBOpenDBRequest : IDBRequest {
  attribute EventHandler onblocked;
  attribute EventHandler onupgradeneeded;
};

[Exposed=(Window,Worker)]
interface IDBVersionChangeEvent : Event {
  constructor(DOMString type, optional IDBVersionChangeEventInit eventInitDict = {});
  readonly attribute unsigned long long oldVersion;
  readonly attribute unsigned long long? newVersion;
};
dictionary IDBVersionChangeEventInit : EventInit {
  unsigned long long oldVersion = 0;
  unsigned long long? newVersion = null;
};

[Exposed=(Window,Worker)]
interface IDBFactory {
  [NewObject] IDBOpenDBRequest open(DOMString name,
                                    optional [EnforceRange] unsigned long long version);
  [NewObject] IDBOpenDBRequest deleteDatabase(DOMString name);
  Promise<sequence<IDBDatabaseInfo>> databases();
  short cmp(any first, any second);
};
dictionary IDBDatabaseInfo { DOMString name; unsigned long long version; };

[Exposed=(Window,Worker)]
interface IDBDatabase : EventTarget {
  readonly attribute DOMString name;
  readonly attribute unsigned long long version;
  readonly attribute DOMStringList objectStoreNames;
  [NewObject] IDBTransaction transaction((DOMString or sequence<DOMString>) storeNames,
                                         optional IDBTransactionMode mode = "readonly",
                                         optional IDBTransactionOptions options = {});
  undefined close();
  [NewObject] IDBObjectStore createObjectStore(
      DOMString name, optional IDBObjectStoreParameters options = {});
  undefined deleteObjectStore(DOMString name);
  attribute EventHandler onabort;
  attribute EventHandler onclose;
  attribute EventHandler onerror;
  attribute EventHandler onversionchange;
};
dictionary IDBObjectStoreParameters {
  (DOMString or sequence<DOMString>)? keyPath = null;
  boolean autoIncrement = false;
};
dictionary IDBTransactionOptions { IDBTransactionDurability durability = "default"; };

[Exposed=(Window,Worker)]
interface IDBObjectStore {
  attribute DOMString name;
  readonly attribute any keyPath;
  readonly attribute DOMStringList indexNames;
  [SameObject] readonly attribute IDBTransaction transaction;
  readonly attribute boolean autoIncrement;
  [NewObject] IDBRequest put(any value, optional any key);
  [NewObject] IDBRequest add(any value, optional any key);
  [NewObject] IDBRequest delete(any query);
  [NewObject] IDBRequest clear();
  [NewObject] IDBRequest get(any query);
  [NewObject] IDBRequest getKey(any query);
  [NewObject] IDBRequest getAll(optional (IDBGetAllOptions or any) queryOrOptions,
                                optional [EnforceRange] unsigned long count);
  [NewObject] IDBRequest getAllKeys(optional (IDBGetAllOptions or any) queryOrOptions,
                                    optional [EnforceRange] unsigned long count);
  [NewObject] IDBRequest getAllRecords(optional IDBGetAllOptions options = {});
  [NewObject] IDBRequest count(optional any query);
  [NewObject] IDBRequest openCursor(optional any query,
                                    optional IDBCursorDirection direction = "next");
  [NewObject] IDBRequest openKeyCursor(optional any query,
                                       optional IDBCursorDirection direction = "next");
  IDBIndex index(DOMString name);
  [NewObject] IDBIndex createIndex(DOMString name,
                                   (DOMString or sequence<DOMString>) keyPath,
                                   optional IDBIndexParameters options = {});
  undefined deleteIndex(DOMString name);
};
dictionary IDBIndexParameters { boolean unique = false; boolean multiEntry = false; };
dictionary IDBGetAllOptions {
  any query = null;
  [EnforceRange] unsigned long count;
  IDBCursorDirection direction = "next";
};

[Exposed=(Window,Worker)]
interface IDBIndex {
  attribute DOMString name;
  [SameObject] readonly attribute IDBObjectStore objectStore;
  readonly attribute any keyPath;
  readonly attribute boolean multiEntry;
  readonly attribute boolean unique;
  [NewObject] IDBRequest get(any query);
  [NewObject] IDBRequest getKey(any query);
  [NewObject] IDBRequest getAll(optional (IDBGetAllOptions or any) queryOrOptions,
                                optional [EnforceRange] unsigned long count);
  [NewObject] IDBRequest getAllKeys(optional (IDBGetAllOptions or any) queryOrOptions,
                                    optional [EnforceRange] unsigned long count);
  [NewObject] IDBRequest getAllRecords(optional IDBGetAllOptions options = {});
  [NewObject] IDBRequest count(optional any query);
  [NewObject] IDBRequest openCursor(optional any query,
                                    optional IDBCursorDirection direction = "next");
  [NewObject] IDBRequest openKeyCursor(optional any query,
                                       optional IDBCursorDirection direction = "next");
};

[Exposed=(Window,Worker)]
interface IDBKeyRange {
  readonly attribute any lower;
  readonly attribute any upper;
  readonly attribute boolean lowerOpen;
  readonly attribute boolean upperOpen;
  [NewObject] static IDBKeyRange only(any value);
  [NewObject] static IDBKeyRange lowerBound(any lower, optional boolean open = false);
  [NewObject] static IDBKeyRange upperBound(any upper, optional boolean open = false);
  [NewObject] static IDBKeyRange bound(any lower, any upper,
                                       optional boolean lowerOpen = false,
                                       optional boolean upperOpen = false);
  boolean includes(any key);
};

[Exposed=(Window,Worker)]
interface IDBRecord {
  readonly attribute any key;
  readonly attribute any primaryKey;
  readonly attribute any value;
};

[Exposed=(Window,Worker)]
interface IDBCursor {
  readonly attribute (IDBObjectStore or IDBIndex) source;
  readonly attribute IDBCursorDirection direction;
  readonly attribute any key;
  readonly attribute any primaryKey;
  [SameObject] readonly attribute IDBRequest request;
  undefined advance([EnforceRange] unsigned long count);
  undefined continue(optional any key);
  undefined continuePrimaryKey(any key, any primaryKey);
  [NewObject] IDBRequest update(any value);
  [NewObject] IDBRequest delete();
};
enum IDBCursorDirection { "next", "nextunique", "prev", "prevunique" };

[Exposed=(Window,Worker)]
interface IDBCursorWithValue : IDBCursor { readonly attribute any value; };

[Exposed=(Window,Worker)]
interface IDBTransaction : EventTarget {
  readonly attribute DOMStringList objectStoreNames;
  readonly attribute IDBTransactionMode mode;
  readonly attribute IDBTransactionDurability durability;
  [SameObject] readonly attribute IDBDatabase db;
  readonly attribute DOMException? error;
  IDBObjectStore objectStore(DOMString name);
  undefined commit();
  undefined abort();
  attribute EventHandler onabort;
  attribute EventHandler oncomplete;
  attribute EventHandler onerror;
};
enum IDBTransactionMode { "readonly", "readwrite", "versionchange" };
enum IDBTransactionDurability { "default", "strict", "relaxed" };

partial interface mixin WindowOrWorkerGlobalScope {
  [SameObject] readonly attribute IDBFactory indexedDB;
};
```

### Приложение B. Пример байтового кодирования ключей (`KEY-v1`)

| Ключ | Байты (hex) |
|---|---|
| `1` | `10 BF F0 00 00 00 00 00 00` |
| `0` / `-0` | `10 80 00 00 00 00 00 00 00` |
| `-1` | `10 40 0F FF FF FF FF FF FF` |
| `-Infinity` | `10 00 0F FF FF FF FF FF FF` |
| `+Infinity` | `10 FF F0 00 00 00 00 00 00` |
| `new Date(0)` | `20 80 00 00 00 00 00 00 00` |
| `""` | `30 00 00` |
| `"a"` | `30 61 00 00` |
| `"\u0000"` | `30 00 FF 00 00` |
| `"\uD800"` (непарный суррогат) | `30 ED A0 80 00 00` |
| `new Uint8Array([0,1])` | `40 00 FF 01 00 00` |
| `[]` | `50 00` |
| `[1]` | `50 10 BF F0 00 00 00 00 00 00 00` |
| `[[]]` | `50 50 00 00` |

Проверка порядка на примере: `""` (`30 00 00`) < `"\u0000"` (`30 00 FF 00 00`) < `"a"` (`30 61 00 00`) < `"\uD800"` (`30 ED …`) < `[]` (`50 00`) < `[1]`; `1` (`10 BF …`) < `""`; `-1` < `0` < `1`. Все эти пары обязаны присутствовать в unit-тестах.

### Приложение C. Дерево исходников (ожидаемое)

```
crates/boa_idb_core/src/
  lib.rs
  key/{mod.rs, value.rs, encode.rs, compare.rs, range.rs, path.rs}
  clone/{mod.rs, scvalue.rs, encode.rs, decode.rs, limits.rs}
  engine/{mod.rs, registry.rs, connection.rs, open_queue.rs,
          transaction.rs, scheduler.rs, request.rs, cursor.rs,
          ops_store.rs, ops_index.rs, keygen.rs}
  backend/{mod.rs, traits.rs, capabilities.rs, error.rs, fs_abstraction.rs}
  error.rs
  proto.rs
  limits.rs

crates/boa_idb/src/
  lib.rs
  extension.rs            # IndexedDbExtension, builder, register
  runtime.rs              # IdbRuntime (host-defined в Context), end_of_task
  io.rs                   # IoRuntime, потоки, каналы
  driver.rs               # NativeAsyncJob-драйвер транзакции
  convert/{key.rs, value.rs, webidl.rs}   # JsValue ⇄ Key / ScValue, конвертации IDL
  dom/{mod.rs, event_target.rs, event.rs, exception.rs, string_list.rs, dispatch.rs}
  api/{factory.rs, database.rs, object_store.rs, index.rs, key_range.rs,
       record.rs, cursor.rs, transaction.rs, request.rs, version_change_event.rs}
  executor.rs             # IdbJobExecutor, SimpleIdbExecutor
```

### Приложение D. Приёмочный JS-сценарий (сокращённо)

```js
// 1. Создание и миграция схемы
let req = indexedDB.open("library", 3);
req.onupgradeneeded = (e) => {
  const db = req.result;
  if (e.oldVersion < 1) {
    const s = db.createObjectStore("books", { keyPath: "isbn" });
    s.createIndex("by_title", "title", { unique: true });
    s.createIndex("by_author", "author");
  }
  if (e.oldVersion < 2) {
    req.transaction.objectStore("books").createIndex("by_year", "year");
  }
  if (e.oldVersion < 3) {
    const m = db.createObjectStore("magazines", { autoIncrement: true });
    m.createIndex("by_tags", "tags", { multiEntry: true });
  }
};
req.onsuccess = () => runTests(req.result);

function runTests(db) {
  // 2. Запись, уникальный индекс, обработка ConstraintError
  const tx = db.transaction(["books", "magazines"], "readwrite", { durability: "strict" });
  const books = tx.objectStore("books");
  books.put({ isbn: 123456, title: "Quarry Memories", author: "Fred", year: 2011 });
  books.put({ isbn: 234567, title: "Water Buffaloes", author: "Fred", year: 2012 });
  const bad = books.add({ isbn: 345678, title: "Quarry Memories", author: "X" });
  bad.onerror = (e) => { console.assert(bad.error.name === "ConstraintError"); e.preventDefault(); };

  // 3. multiEntry
  tx.objectStore("magazines").put({ tags: ["rust", "db", "rust"] });

  tx.oncomplete = () => phase2(db);
}

function phase2(db) {
  // 4. Курсор по индексу, ключи-массивы, ArrayBuffer-ключи, cmp
  const tx = db.transaction("books", "readonly");
  const idx = tx.objectStore("books").index("by_author");
  const out = [];
  idx.openCursor(IDBKeyRange.only("Fred"), "next").onsuccess = (e) => {
    const c = e.target.result;
    if (c) { out.push(c.primaryKey); c.continue(); }
    else {
      console.assert(out.length === 2 && indexedDB.cmp(out[0], out[1]) === -1);
      phase3(db);
    }
  };
}

function phase3(db) {
  // 5. getAllRecords, abort и откат key generator
  const tx = db.transaction("magazines", "readwrite");
  const st = tx.objectStore("magazines");
  st.getAllRecords({ direction: "prev", count: 10 }).onsuccess = (e) => {
    console.assert(Array.isArray(e.target.result));
  };
  st.put({ tags: ["temp"] });
  tx.abort();
  tx.onabort = () => { /* после перезапуска процесса генератор обязан начать с 2 */ };
}
```

Сценарий выполняется хостом дважды: на чистом каталоге и повторно (проверка персистентности, версии 3, содержимого и состояния key generators).

### Приложение E. Чек-лист соответствия алгоритмам Спеки

Реализованы и покрыты тестами (каждый пункт — отдельная запись в `docs/traceability.md`):

- [ ] §5.1 Opening a database connection (включая `blocked`, `versionchange`, ожидание закрытия)
- [ ] §5.2 Closing a database connection (в т. ч. `forced flag` → событие `close`)
- [ ] §5.3 Deleting a database
- [ ] §5.4 Committing a transaction (в т. ч. `durability`)
- [ ] §5.5 Aborting a transaction (откат данных, схемы, key generators, `AbortError` для ожидающих запросов)
- [ ] §5.6 Asynchronously executing a request
- [ ] §5.7 Upgrading a database
- [ ] §5.8 Aborting an upgrade transaction (возврат версии соединения)
- [ ] §5.9 / §5.10 Firing success/error event (активация транзакции вокруг dispatch, автоабрт при необработанной ошибке)
- [ ] §5.11 Clone a value (`DataCloneError`, отсутствие мутации исходного объекта)
- [ ] §5.12 Creating a request to retrieve multiple items (`getAll*`, лимиты, направление)
- [ ] §6.1 Object store storage operation (keygen, inject, индексы, `no_overwrite`)
- [ ] §6.2 Object store retrieval operations
- [ ] §6.3 Index retrieval operations
- [ ] §6.4 Object store deletion operation (в т. ч. удаление по диапазону + синхронизация индексов)
- [ ] §6.5 Record counting operation
- [ ] §6.6 Object store clear operation (без сброса keygen)
- [ ] §6.7 Cursor iteration operation (все направления, `count`, `key`, `primaryKey`)
- [ ] §7.1 Extract a key from a value using a key path (включая `length` для строк/массивов)
- [ ] §7.2 Inject a key into a value using a key path
- [ ] §7.3 Convert a key to a value
- [ ] §7.4 Convert a value to a key (различение invalid type / invalid value)
- [ ] §2.11 Key generators (generate, possibly update, лимит 2⁵³, откат)
- [ ] §2.12 Record snapshot (`IDBRecord`)

---

*Конец документа.*
