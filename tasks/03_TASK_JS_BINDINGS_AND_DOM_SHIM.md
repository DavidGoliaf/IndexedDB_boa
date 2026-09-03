# ТЕХНИЧЕСКОЕ ЗАДАНИЕ НА ИСПОЛНЕНИЕ: TASK-03
## Реализация JS-биндингов `boa_idb`, DOM-шима и интеграции с Event Loop Boa

| Метаданные | Значение |
|---|---|
| **Идентификатор задачи** | `TASK-03-JS-BINDINGS-DOM-SHIM` |
| **Этап ТЗ** | **M3 (JS-биндинги, DOM-шим и интеграция с Event Loop Boa)** |
| **Целевой крейт** | `crates/boa_idb` (с интеграцией `boa_idb_core` и `boa_idb_memory`) |
| **Нормативное ТЗ** | `TZ_boa_idb_IndexedDB.md` (разделы 2.3, 2.4, 3, 4.1, 5, 9, 10, 13.3, 14, Приложения A, C, D, E) |
| **Пререквизиты** | Завершенные и принятые `TASK-01` (ключи, SCF-v1 кодек) и `TASK-02` (ядро движка, планировщик, memory-бэкенд) |
| **Роль архитектора** | Спроектированы 12 классов IDB, 4 класса DOM-шима, WebIDL-конвертеры, алгоритм DOM dispatch, модель доставки результатов через `NativeAsyncJob` и хук `end_of_task`. |
| **Роль исполнителя** | Строгая механическая реализация кода по приведенным сигнатурам, структуре файлов, алгоритмам и интеграционным тестам. **Никаких собственных архитектурных домыслов.** |

---

## 1. Архитектурный контекст и жесткие правила (Guardrails)

1. **Разделение L1 (JS-биндинги) и L2 (Ядро) (`AD-1`):**
   - Крейт `boa_idb` является единственным мостом между движком `boa_engine` и ядром `boa_idb_core`.
   - Объекты JS (`JsValue`, `JsObject`, `Context`) живут **только** в потоке JS. В ядро через каналы передаются исключительно чистые типы (`Key`, `ScValue`, `StoreId`, `TxnId` и т.д.).
2. **Синхронное структурное клонирование (`AD-2`):**
   - Сериализация `JsValue -> ScValue` выполняется **синхронно в потоке JS** внутри вызова API (`put`, `add`, `update`) до возврата объекта `IDBRequest`. Исключение `DataCloneError` бросается синхронно!
3. **Объектная модель и сборка мусора (`GC`, §3.3):**
   - Все native-структуры классов реализуют `#[derive(Trace, Finalize, JsData)]`.
   - В native-структурах хранятся идентификаторы (`ConnectionId`, `TransactionId`, `StoreId`, `RequestId`) и GC-трассируемые ссылки на связанные JS-объекты (`JsObject`, `JsValue`).
   - Использование `#[unsafe_ignore_trace]` разрешено **только** для примитивных не-JS типов (`u64`, `bool`, `TxnMode` и т.д.).
4. **Строгость WebIDL и брендирование (R5.0.1–R5.0.5):**
   - Проверка brand (`this` должен быть экземпляром целевого класса) обязательна для каждого метода и геттера -> `TypeError`.
   - Аргументы конвертируются строго по WebIDL (`EnforceRange` для `version` и `count`, `ToString` для `DOMString`).
   - Свойства без аннотации `[SameObject]` обязаны возвращать **новые** экземпляры JS-объектов (например, `IDBObjectStore.keyPath` возвращает новый `Array` при каждом чтении, если путь состоит из массива строк).
   - Запрещен вызов конструктора для интерфейсов без `constructor` в WebIDL (`new IDBRequest()` -> `TypeError`).
5. **Жизненный цикл транзакций и задачи (AD-5, AD-8, §9.3):**
   - Транзакция активна:
     1. Синхронно после `db.transaction()` до конца текущей микрозадачи/задачи;
     2. Во время диспатча событий `onsuccess`/`onerror` ее запросов;
     3. Во время диспатча события `onupgradeneeded` (для upgrade-транзакции).
   - Вне этих окон любая попытка поставить запрос обязана бросать `TransactionInactiveError`.
   - Хук `IdbRuntime::end_of_task(&mut Context)` вызывается после каждой задачи, деактивирует транзакции и инициирует автокоммит (§7.2.4).
6. **Доставка результатов запросов (AD-5):**
   - Для каждой транзакции запускается единственный драйвер (`NativeAsyncJob`), гарантирующий, что события `onsuccess`/`onerror` запросов диспатчатся строго в порядке их постановки (`FIFO seq`).

---

## 2. Полное дерево файлов задачи

```
crates/boa_idb/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── extension.rs            # IndexedDbExtension, builder, register
│   ├── runtime.rs              # IdbRuntime (HostDefined в Context), end_of_task
│   ├── io.rs                   # IoRuntime, каналы и синхронизация
│   ├── driver.rs               # TransactionDriver (NativeAsyncJob loop per transaction)
│   ├── executor.rs             # IdbJobExecutor, SimpleIdbExecutor
│   ├── convert/
│   │   ├── mod.rs
│   │   ├── webidl.rs           # WebIDL конвертеры аргументов, EnforceRange, ToString
│   │   ├── key.rs              # JsValue ⇄ Key (§7.3, §7.4)
│   │   └── value.rs            # JsValue ⇄ ScValue (структурное клонирование)
│   ├── dom/
│   │   ├── mod.rs
│   │   ├── event_target.rs     # EventTarget (addEventListener, removeEventListener, dispatchEvent)
│   │   ├── event.rs            # Event (type, target, bubbles, cancelable, preventDefault)
│   │   ├── exception.rs        # DOMException (name, message, code)
│   │   ├── string_list.rs      # DOMStringList (length, item, contains, indexing, iterator)
│   │   └── dispatch.rs         # DOM Event Dispatch algorithm (§5.9, §5.10, §2.1.1)
│   └── api/
│       ├── mod.rs
│       ├── factory.rs          # IDBFactory (open, deleteDatabase, databases, cmp)
│       ├── database.rs         # IDBDatabase (transaction, createObjectStore, deleteObjectStore, close)
│       ├── object_store.rs     # IDBObjectStore (put, add, get, getAll, delete, clear, count, openCursor...)
│       ├── index.rs            # IDBIndex (get, getAll, count, openCursor...)
│       ├── key_range.rs        # IDBKeyRange (only, lowerBound, upperBound, bound, includes)
│       ├── record.rs           # IDBRecord (key, primaryKey, value)
│       ├── cursor.rs           # IDBCursor / IDBCursorWithValue (advance, continue, continuePrimaryKey, update, delete)
│       ├── transaction.rs      # IDBTransaction (objectStore, commit, abort, mode, durability)
│       ├── request.rs          # IDBRequest / IDBOpenDBRequest (result, error, readyState, source, txn)
│       └── version_change_event.rs # IDBVersionChangeEvent (oldVersion, newVersion)
└── tests/
    ├── extension_registration_tests.rs
    ├── dom_shim_tests.rs
    ├── key_conversion_tests.rs
    ├── structured_clone_tests.rs
    ├── basic_idb_flow_tests.rs
    ├── transaction_lifetime_tests.rs
    ├── cursor_iteration_tests.rs
    ├── error_bubbling_tests.rs
    └── appendix_d_acceptance_tests.rs
```

---

## 3. Манифест `crates/boa_idb/Cargo.toml`

```toml
[package]
name = "boa_idb"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "IndexedDB API 3.0 implementation for the Boa JavaScript engine"

[dependencies]
boa_idb_core = { path = "../boa_idb_core" }
boa_idb_memory = { path = "../boa_idb_memory", optional = true }
boa_engine = "~0.22.0"
boa_gc = "~0.22.0"
boa_runtime = { version = "~0.22.0", optional = true }
thiserror = { workspace = true }
smallvec = { workspace = true }
indexmap = { workspace = true }
hashbrown = { workspace = true }
parking_lot = "0.12"
crossbeam-channel = "0.5"
futures-channel = "0.3"
futures-lite = "2"
tracing = { workspace = true, optional = true }

[dev-dependencies]
boa_idb_memory = { path = "../boa_idb_memory" }
proptest = { workspace = true }
rstest = { workspace = true }

[features]
default = ["dom-shim", "memory"]
dom-shim = []
memory = ["dep:boa_idb_memory"]
runtime-interop = ["dep:boa_runtime"]
tracing = ["dep:tracing", "boa_idb_core/tracing"]

[lints]
workspace = true
```

---

## 4. Спецификация конвертеров `convert/`

### 4.1. WebIDL конвертер `convert/webidl.rs`
```rust
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;

/// Конвертирует JsValue в u64 с правилом [EnforceRange].
/// Бросает TypeError, если значение NaN, Infinity или выходит за диапазон [0, 2^64-1].
pub fn to_unsigned_long_long_enforce_range(val: &JsValue, context: &mut Context) -> JsResult<u64> {
    let num = val.to_number(context)?;
    if num.is_nan() || num.is_infinite() {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is not a finite number")
            .into());
    }
    let integer = num.trunc();
    if integer < 0.0 || integer > u64::MAX as f64 {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is out of range for unsigned long long")
            .into());
    }
    Ok(integer as u64)
}

/// Конвертирует JsValue в u32 с правилом [EnforceRange].
pub fn to_unsigned_long_enforce_range(val: &JsValue, context: &mut Context) -> JsResult<u32> {
    let num = val.to_number(context)?;
    if num.is_nan() || num.is_infinite() {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is not a finite number")
            .into());
    }
    let integer = num.trunc();
    if integer < 0.0 || integer > u32::MAX as f64 {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is out of range for unsigned long")
            .into());
    }
    Ok(integer as u32)
}

/// Конвертирует JsValue в Utf16String с сохранением суррогатов.
pub fn to_dom_string(val: &JsValue, context: &mut Context) -> JsResult<Utf16String> {
    let js_str = val.to_string(context)?;
    Ok(Utf16String::from_slice(js_str.as_slice()))
}

/// Конвертирует (DOMString or sequence<DOMString>) в список строк.
pub fn to_sequence_of_dom_strings(val: &JsValue, context: &mut Context) -> JsResult<Vec<Utf16String>> {
    if let Some(obj) = val.as_object() {
        if obj.is_array() {
            let length = obj.length(context)?;
            let mut result = Vec::with_capacity(length as usize);
            for i in 0..length {
                let elem = obj.get(i, context)?;
                result.push(to_dom_string(&elem, context)?);
            }
            return Ok(result);
        }
    }
    Ok(vec![to_dom_string(val, context)?])
}

/// Конвертирует аргумент keyPath (null, string или array of strings) в KeyPath.
pub fn to_key_path_argument(val: &JsValue, context: &mut Context) -> JsResult<Option<KeyPath>> {
    if val.is_null_or_undefined() {
        return Ok(None);
    }
    if let Some(obj) = val.as_object() {
        if obj.is_array() {
            let length = obj.length(context)?;
            if length == 0 {
                return Err(JsNativeError::syntax()
                    .with_message("KeyPath array cannot be empty")
                    .into());
            }
            let mut paths = Vec::with_capacity(length as usize);
            for i in 0..length {
                let elem = obj.get(i, context)?;
                let s = to_dom_string(&elem, context)?;
                KeyPath::validate_identifier_name(s.as_slice())
                    .map_err(|e| JsNativeError::syntax().with_message(e.to_string()))?;
                paths.push(s);
            }
            return Ok(Some(KeyPath::Array(paths)));
        }
    }
    let s = to_dom_string(val, context)?;
    if s.is_empty() {
        return Ok(Some(KeyPath::Empty));
    }
    KeyPath::validate_identifier_name(s.as_slice())
        .map_err(|e| JsNativeError::syntax().with_message(e.to_string()))?;
    Ok(Some(KeyPath::Single(s)))
}
```

### 4.2. Конвертер `JsValue ⇄ Key` (§7.3, §7.4) `convert/key.rs`
- `value_to_key(val: &JsValue, context: &mut Context) -> Result<Key, KeyError>`:
  - `Number`: если `!isNaN` -> `Key::Number(n)`. Если `NaN` -> `KeyError::InvalidValue`.
  - `Date` (через `[[DateValue]]`): если валидна -> `Key::Date(d)`. Если `NaN` -> `KeyError::InvalidValue`.
  - `String`: -> `Key::String(utf16)`.
  - `ArrayBuffer`: -> `Key::Binary(bytes)`.
  - `TypedArray` / `DataView`: -> `Key::Binary(view_bytes)` (срез по `byteOffset..byteOffset+byteLength`).
  - `Array`: рекурсивный обход с защитой от циклов (`HashSet<*const ()>`) и лимитом глубины 32. Если элемент `undefined`/hole/invalid -> `KeyError::InvalidValue`.
  - Прочие типы (`null`, `undefined`, `Object`, `Function`, `Symbol`) -> `KeyError::InvalidType`.
- `key_to_value(key: &Key, context: &mut Context) -> JsResult<JsValue>`:
  - `Key::Number(n)` -> `JsValue::from(*n)`
  - `Key::Date(d)` -> `JsDate::from_milliseconds(*d, context)`
  - `Key::String(s)` -> `JsString::from(s.as_slice()).into()`
  - `Key::Binary(b)` -> `JsArrayBuffer::from_byte_block(b.clone(), context).into()`
  - `Key::Array(arr)` -> `JsArray::from_iter(children, context).into()`

### 4.3. Конвертер `JsValue ⇄ ScValue` (Структурное клонирование) `convert/value.rs`
Реализует `StructuredSerializeForStorage` (AD-2, §5.11):
- Поддерживает: примитивы, объекты-обертки, даты, регулярные выражения (`RegExp`), ошибки (`Error`..`AggregateError`), `Array` (включая дыры), `Object` (enumerable собственные строковые свойства), `Map`, `Set`, `ArrayBuffer`, `TypedArray`, `DataView`.
- Memo-таблица циклических графов: отображает `JsObject` на `memo_index`.
- Запрещенные типы (`Function`, `Symbol`, `WeakMap`, `WeakSet`, `Promise`, `Proxy`, `SharedArrayBuffer`, detached `ArrayBuffer`) -> бросает `DataCloneError`.

---

## 5. Спецификация DOM-шима `dom/`

### 5.1. Класс `DOMException` `dom/exception.rs`
```rust
use boa_engine::object::builtins::JsError;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsString, JsValue};
use boa_gc::{Finalize, Trace};

#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct DomExceptionData {
    pub name: JsString,
    pub message: JsString,
    pub code: u16,
}
```
Таблица кодов DOMException по WebIDL:
- `IndexSizeError` = 1, `HierarchyRequestError` = 3, `WrongDocumentError` = 4, `InvalidCharacterError` = 5, `NoModificationAllowedError` = 7, `NotFoundError` = 8, `NotSupportedError` = 9, `InUseAttributeError` = 10, `InvalidStateError` = 11, `SyntaxError` = 12, `InvalidModificationError` = 13, `NamespaceError` = 14, `InvalidAccessError` = 15, `TypeMismatchError` = 17, `SecurityError` = 18, `NetworkError` = 19, `AbortError` = 20, `URLMismatchError` = 21, `QuotaExceededError` = 22, `TimeoutError` = 23, `InvalidNodeTypeError` = 24, `DataCloneError` = 25.
- Остальные (включая `ReadOnlyError`, `VersionError`, `ConstraintError`, `DataError`, `TransactionInactiveError`) имеют `code = 0`.

### 5.2. Класс `Event` `dom/event.rs`
Свойства: `type`, `target`, `currentTarget`, `eventPhase`, `bubbles`, `cancelable`, `defaultPrevented`, `composed`, `isTrusted`, `timeStamp`.
Методы: `preventDefault()`, `stopPropagation()`, `stopImmediatePropagation()`.

### 5.3. Класс `EventTarget` `dom/event_target.rs`
Методы:
- `addEventListener(type, callback, options)`: поддержка флагов `capture`, `once`, `passive`. Поддержка объектов-слушателей с методом `handleEvent`.
- `removeEventListener(type, callback, options)`: снятие слушателя по совпадению `(type, callback, capture)`.
- `dispatchEvent(event)`: запуск алгоритма dispatch.

### 5.4. Алгоритм DOM Dispatch `dom/dispatch.rs`
Реализует полный цикл распространения события:
1. **Построение цепочки предков (Event Path):**
   - Для `IDBRequest` -> `request.transaction` (если есть) -> `transaction.db` -> `null`.
   - Для `IDBTransaction` -> `transaction.db` -> `null`.
   - Для `IDBDatabase` -> `null`.
2. **Фазы:**
   - Фаза Capturing (`eventPhase = 1`): обход от корня к родителю целевого объекта; вызов слушателей с `capture = true`.
   - Фаза At Target (`eventPhase = 2`): вызов всех слушателей целевого объекта (`capture = true`, затем `capture = false`).
   - Фаза Bubbling (`eventPhase = 3`): если `event.bubbles == true`, обход от родителя к корню; вызов слушателей с `capture = false`.
3. **Обработка исключений и авто-откат (§5.10):**
   - Если слушатель события выбросил JS-исключение -> транзакция немедленно прерывается с `AbortError`.
   - Если событие `error` на `IDBRequest` завершило всплытие и `!event.defaultPrevented` -> транзакция автоматически прерывается с ошибкой запроса.

---

## 6. Спецификация 12 классов IDB `api/`

### 6.1. Сводная таблица классов и native-данных

| Класс | Native-данные (`JsData`) | Наследует | Ключевые методы и геттеры |
|---|---|---|---|
| `IDBFactory` | `()` (singleton) | Object | `open`, `deleteDatabase`, `databases`, `cmp` |
| `IDBDatabase` | `ConnectionId`, `name`, `version`, `closed` | `EventTarget` | `name`, `version`, `objectStoreNames`, `transaction`, `close`, `createObjectStore`, `deleteObjectStore` |
| `IDBTransaction` | `TxnId`, `mode`, `durability`, `db: JsObject` | `EventTarget` | `objectStoreNames`, `mode`, `durability`, `db`, `error`, `objectStore`, `commit`, `abort` |
| `IDBObjectStore` | `StoreId`, `name`, `keyPath`, `autoIncrement`, `txn: JsObject` | Object | `name`, `keyPath`, `indexNames`, `transaction`, `autoIncrement`, `put`, `add`, `get`, `getKey`, `getAll`, `getAllKeys`, `getAllRecords`, `delete`, `clear`, `count`, `openCursor`, `openKeyCursor`, `index`, `createIndex`, `deleteIndex` |
| `IDBIndex` | `IndexId`, `name`, `keyPath`, `multiEntry`, `unique`, `store: JsObject` | Object | `name`, `objectStore`, `keyPath`, `multiEntry`, `unique`, `get`, `getKey`, `getAll`, `getAllKeys`, `getAllRecords`, `count`, `openCursor`, `openKeyCursor` |
| `IDBKeyRange` | `lower: Option<Key>`, `upper: Option<Key>`, `lower_open: bool`, `upper_open: bool` | Object | `only`, `lowerBound`, `upperBound`, `bound`, `includes`, `lower`, `upper`, `lowerOpen`, `upperOpen` |
| `IDBRecord` | `key: Key`, `primary_key: Key`, `value: ScValue` | Object | `key`, `primaryKey`, `value` |
| `IDBCursor` | `CursorId`, `direction`, `key: Key`, `primary_key: Key`, `req: JsObject` | Object | `source`, `direction`, `key`, `primaryKey`, `request`, `advance`, `continue`, `continuePrimaryKey`, `update`, `delete` |
| `IDBCursorWithValue` | (наследует `IDBCursor`) + `value: ScValue` | `IDBCursor` | `value` |
| `IDBRequest` | `RequestId`, `state: ReadyState`, `result: Option<JsValue>`, `error: Option<JsObject>`, `source`, `txn` | `EventTarget` | `result`, `error`, `source`, `transaction`, `readyState`, `onsuccess`, `onerror` |
| `IDBOpenDBRequest` | (наследует `IDBRequest`) | `IDBRequest` | `onblocked`, `onupgradeneeded` |
| `IDBVersionChangeEvent` | `old_version: u64`, `new_version: Option<u64>` | `Event` | `oldVersion`, `newVersion` |

---

## 7. Модуль интеграции `extension.rs`, `runtime.rs`, `driver.rs`, `executor.rs`

### 7.1. Регистрация `IndexedDbExtension` `extension.rs`
```rust
use boa_engine::{Context, JsNativeError, JsResult, JsValue};
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::proto::StorageKey;
use std::sync::Arc;

pub struct IndexedDbExtension {
    storage_key: StorageKey,
    backend_factory: Arc<dyn BackendFactory>,
    max_key_len: usize,
    max_value_len: usize,
}

impl IndexedDbExtension {
    pub fn builder() -> IndexedDbExtensionBuilder {
        IndexedDbExtensionBuilder::default()
    }

    pub fn register(&self, context: &mut Context) -> JsResult<()> {
        // 1. Идемпотентность: проверить, не зарегистрирован ли IDB ранее
        if context.get_data::<IdbRuntime>().is_some() {
            return Err(JsNativeError::error()
                .with_message("IndexedDbExtension is already registered in this Context")
                .into());
        }

        // 2. Инициализация IdbRuntime и помещение в HostDefined
        let runtime = IdbRuntime::new(self.storage_key.clone(), self.backend_factory.clone());
        context.insert_data(runtime);

        // 3. Регистрация классов DOM-шима (DOMException, Event, EventTarget, DOMStringList)
        // 4. Регистрация конструкторов 12 классов IDB на globalThis
        // 5. Установка свойства [SameObject] readonly attribute 'indexedDB' на globalThis
        Ok(())
    }
}
```

### 7.2. Драйвер транзакции `driver.rs`
Для каждой транзакции L1 создает драйвер на базе `NativeAsyncJob`:
```rust
pub fn spawn_transaction_driver(
    txn_id: TxnId,
    txn_obj: JsObject,
    context: &mut Context,
) {
    let job = NativeAsyncJob::new(async move |context| {
        // Цикл обработки очереди запросов транзакции:
        // 1. Извлечь следующий незавершенный Request
        // 2. Await ответа от IO/памяти
        // 3. Установить request.result / request.error и readyState = "done"
        // 4. Активировать транзакцию
        // 5. Диспатчить success / error через DOM dispatch
        // 6. Деактивировать транзакцию
        // 7. По завершении всех запросов — выполнить автокоммит и диспатчить "complete"
        Ok(JsValue::undefined())
    });
    context.enqueue_job(job);
}
```

### 7.3. Хук конца задачи `executor.rs`
```rust
pub fn end_of_task(context: &mut Context) {
    if let Some(mut runtime) = context.get_data_mut::<IdbRuntime>() {
        runtime.cleanup_active_transactions();
    }
}
```

---

## 8. Эталонный приемочный JS-сценарий (Приложение D ТЗ)

Исполнитель **ОБЯЗАН** обеспечить прохождение следующего интеграционного теста (`tests/appendix_d_acceptance_tests.rs`):

```js
// 1. Открытие базы и миграция схемы через versionchange
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

req.onsuccess = () => {
  const db = req.result;
  // 2. Запись, уникальный индекс, перехват ConstraintError
  const tx = db.transaction(["books", "magazines"], "readwrite");
  const books = tx.objectStore("books");
  books.put({ isbn: 123456, title: "Quarry Memories", author: "Fred", year: 2011 });
  books.put({ isbn: 234567, title: "Water Buffaloes", author: "Fred", year: 2012 });
  
  const bad = books.add({ isbn: 345678, title: "Quarry Memories", author: "X" });
  bad.onerror = (e) => {
    console.assert(bad.error.name === "ConstraintError");
    e.preventDefault(); // Предотвратить авто-откат транзакции
  };

  // 3. multiEntry индекс
  tx.objectStore("magazines").put({ tags: ["rust", "db", "rust"] });

  tx.oncomplete = () => {
    // 4. Чтение курсором по индексу
    const tx2 = db.transaction("books", "readonly");
    const idx = tx2.objectStore("books").index("by_author");
    const out = [];
    idx.openCursor(IDBKeyRange.only("Fred"), "next").onsuccess = (e) => {
      const c = e.target.result;
      if (c) {
        out.push(c.primaryKey);
        c.continue();
      } else {
        console.assert(out.length === 2 && indexedDB.cmp(out[0], out[1]) === -1);
      }
    };
  };
};
```

---

## 9. Пошаговый план работ для исполнителя (Execution Steps)

1. **Шаг 1. Конфигурация крейта `boa_idb`**
   - Наполнить `crates/boa_idb/Cargo.toml` всеми зависимостями.
   - Проверить сборку связки `boa_idb` -> `boa_idb_core` -> `boa_idb_memory`.
2. **Шаг 2. Реализация конвертеров `convert/`**
   - Реализовать `webidl.rs`, `key.rs`, `value.rs`.
   - Покрыть unit-тестами все конверсии типов (`key_conversion_tests.rs`, `structured_clone_tests.rs`).
3. **Шаг 3. Реализация DOM-шима `dom/`**
   - Реализовать `DOMException`, `Event`, `EventTarget`, `DOMStringList`.
   - Реализовать алгоритм `dispatch.rs` (capturing, bubbling, preventDefault, unhandled error auto-abort).
   - Написать тесты `dom_shim_tests.rs`, `error_bubbling_tests.rs`.
4. **Шаг 4. Реализация 12 классов IDB `api/`**
   - Реализовать классы `IDBFactory`, `IDBDatabase`, `IDBTransaction`, `IDBObjectStore`, `IDBIndex`, `IDBKeyRange`, `IDBRecord`, `IDBCursor`, `IDBCursorWithValue`, `IDBRequest`, `IDBOpenDBRequest`, `IDBVersionChangeEvent`.
5. **Шаг 5. Реализация рантайма, драйвера и экзекутора**
   - Реализовать `runtime.rs`, `driver.rs`, `executor.rs`, `extension.rs`.
   - Связать постановку запросов в `IDBObjectStore`/`IDBIndex` с задачами `NativeAsyncJob`.
6. **Шаг 6. Написание интеграционных тестов**
   - Написать тесты жизненного цикла транзакций `transaction_lifetime_tests.rs`.
   - Написать тесты итерации курсоров `cursor_iteration_tests.rs`.
   - Написать приемочный тест Приложения D `appendix_d_acceptance_tests.rs`.
7. **Шаг 7. Финальная верификация**
   - Запустить все команды из Раздела 10.

---

## 10. Команды валидации и критерии приёмки

Исполнитель сдает работу только тогда, когда **ВСЕ** команды выполняются со статусом SUCCESS (EXIT CODE 0):

```powershell
# 1. Проверка форматирования
cargo fmt --all -- --check

# 2. Строгий линтинг clippy
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 3. Полный прогон всех unit и интеграционных тестов
cargo test --workspace

# 4. Прогон приемочного сценария Приложения D
cargo test --package boa_idb --test appendix_d_acceptance_tests -- --nocapture

# 5. Проверка генерации документации
cargo doc --workspace --no-deps

# 6. Проверка покрытия кода (требование: ≥ 80% для boa_idb)
cargo llvm-cov --package boa_idb --summary-only
```

### Чек-лист соответствия ТЗ:
- [ ] `R5.0.1`–`R5.0.5`: Строгость WebIDL, порядок проверок исключений, брендирование, `[SameObject]` правила.
- [ ] `R5.1`–`R5.10`: Полная реализация всех 12 интерфейсов IDB и их методов.
- [ ] `R5.11`: Полная реализация DOM-шима (`EventTarget`, `Event`, `DOMException`, `DOMStringList`), bubbling, capturing, авто-откат при необработанной ошибке.
- [ ] `R9.1`–`R9.3`: Интеграция с `JobExecutor`, драйвер транзакций `NativeAsyncJob`, хук `end_of_task`, деактивация на границах задач.
- [ ] 100% Safe Rust (`#![deny(unsafe_code)]`), 0 предупреждений компилятора.
- [ ] Сценарий Приложения D успешно исполняется в среде Boa на memory-бэкенде.
