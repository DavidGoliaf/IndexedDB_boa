# ТЕХНИЧЕСКОЕ ЗАДАНИЕ НА ИСПОЛНЕНИЕ: TASK-02
## Реализация ядра движка `boa_idb_core::engine`, абстракций бэкенда и `boa_idb_memory`

| Метаданные | Значение |
|---|---|
| **Идентификатор задачи** | `TASK-02-CORE-ENGINE-AND-MEMORY` |
| **Этап ТЗ** | **M2 (Ядро: движок, транзакции, планировщик, алгоритмы §6 и Memory-бэкенд)** |
| **Целевые крейты** | `crates/boa_idb_core` (модули `engine`, `backend`, `proto`), `crates/boa_idb_memory` |
| **Нормативное ТЗ** | `TZ_boa_idb_IndexedDB.md` (разделы 3, 4.2, 6.1, 6.6, 7, 8.1, 8.4, 10, 11, 13.4, 13.5, Приложения C, E) |
| **Пререквизиты** | Завершенный и прошедший верификацию `TASK-01` (типы `Key`, `ScValue`, `KeyPath`, кодеки `KEY-v1` и `SCF-v1`, ошибки `IdbError`, лимиты `LimitConfig`) |
| **Роль архитектора** | Спроектированы все конечные автоматы (FSM), трейты хранилища, структуры реестра, планировщика, транзакций, генератора ключей, алгоритмы §6 и эталонный memory-бэкенд. |
| **Роль исполнителя** | Строгая механическая реализация кода по приведенным сигнатурам, файловой структуре, алгоритмам и тестовым спецификациям. **Никаких собственных архитектурных домыслов.** |

---

## 1. Архитектурный контекст и жесткие правила (Guardrails)

1. **Изоляция от JS (`AD-1`):** Вся логика движка, планировщика и бэкендов в `boa_idb_core` и `boa_idb_memory` работает **исключительно** с чистыми типами Rust (`Key`, `ScValue`, `EncodedRange`, байты `Vec<u8>`). Никаких `JsValue`, `JsObject` или `Context`!
2. **Безопасность кода (`# Safety`):** Везде действует `#![deny(unsafe_code)]`. Запрещены любые блоки `unsafe`.
3. **Запрет паник:** Использование `unwrap()`, `expect()`, `panic!` в коде библиотек **КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНО**. Некорректный порядок вызовов или сбои обязаны возвращать `Result<T, BackendError>` или `Result<T, IdbError>`.
4. **Синхронность трейтов бэкенда:** Трейты `StorageBackend`, `Database`, `BackendTxn`, `BackendCursor` являются **синхронными** и `dyn`-совместимыми (выполняются в IO-потоке/базовом рантайме).
5. **Обслуживание индексов в ядре (`R8.1.2`):** Обновление индексов при `put`/`delete`/`clear` рассчитывается ядром (`boa_idb_core::engine::ops_index`), бэкенд лишь сохраняет пары `(index_key, primary_key)`.
6. **Гранулярность Savepoint на каждый запрос (`AD-7`):** Каждый запрос транзакции обрамляется `begin_request()`, `commit_request()`, `rollback_request()`. При ошибке запроса изменения и key generator откатываются до точки сохранения.
7. **Snapshot-изоляция для `readonly` (`AD-6`, `R8.1.3`):** `readonly`-транзакция обязана видеть снимок данных на момент старта и оставаться изолированной от параллельных коммитов `readwrite`.
8. **FIFO-справедливость планировщика (`R7.2.2`):** Транзакции стартуют строго по правилам пересечения scope без перестановок (голодание `readwrite` запрещено).

---

## 2. Полное дерево файлов задачи

```
crates/
├── boa_idb_core/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── error.rs             # из Task-01
│   │   ├── limits.rs            # из Task-01
│   │   ├── key/                 # из Task-01
│   │   ├── clone/               # из Task-01
│   │   ├── proto.rs             # команды, операции, идентификаторы
│   │   ├── backend/
│   │   │   ├── mod.rs
│   │   │   ├── traits.rs        # BackendFactory, Storage, Database, BackendTxn, BackendCursor
│   │   │   ├── capabilities.rs  # BackendCapabilities
│   │   │   ├── error.rs         # BackendError
│   │   │   └── types.rs         # DatabaseMeta, StoreSpec, IndexSpec, EncodedRange, SourceRef
│   │   └── engine/
│   │       ├── mod.rs
│   │       ├── registry.rs      # Реестр баз данных и хранилищ
│   │       ├── connection.rs    # Соединение (IDBDatabase host-side)
│   │       ├── open_queue.rs    # FSM очереди открытия/удаления баз (§5.1, §5.3)
│   │       ├── transaction.rs   # Состояние IDB-транзакции и FSM автокоммита
│   │       ├── scheduler.rs     # Планировщик транзакций (FIFO, проверка пересечения scope)
│   │       ├── request.rs       # Очередь запросов транзакции
│   │       ├── keygen.rs        # Генератор ключей с откатом (§2.11)
│   │       ├── cursor.rs        # Курсор ядра и алгоритм итерации §6.7
│   │       ├── ops_store.rs     # Алгоритмы операций над хранилищем (§6.1, §6.2, §6.4, §6.5, §6.6)
│   │       └── ops_index.rs     # Алгоритмы операций над индексами (§6.3, multiEntry, unique)
│   └── tests/
│       ├── open_queue_tests.rs
│       ├── scheduler_tests.rs
│       ├── ops_store_tests.rs
│       ├── ops_index_tests.rs
│       ├── cursor_tests.rs
│       └── keygen_tests.rs
└── boa_idb_memory/
    ├── Cargo.toml
    ├── src/
    │   ├── lib.rs               # MemoryBackendFactory, MemoryStorage
    │   ├── storage.rs
    │   ├── database.rs
    │   ├── txn.rs               # BTreeMap транзакция со стеком undo-логов
    │   └── cursor.rs            # In-memory курсор с поддержкой всех 4 направлений
    └── tests/
        ├── memory_backend_tests.rs
        └── differential_model_tests.rs
```

---

## 3. Манифест `crates/boa_idb_memory/Cargo.toml`

```toml
[package]
name = "boa_idb_memory"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "In-memory backend for IndexedDB (BTreeMap-based with snapshot isolation and undo logs)"

[dependencies]
boa_idb_core = { path = "../boa_idb_core" }
thiserror = { workspace = true }
indexmap = { workspace = true }
hashbrown = { workspace = true }
parking_lot = "0.12"

[dev-dependencies]
proptest = { workspace = true }
rstest = { workspace = true }

[lints]
workspace = true
```

---

## 4. Спецификация типов `proto.rs` и `backend`

### 4.1. Модуль `proto.rs`
Файл: `crates/boa_idb_core/src/proto.rs`

```rust
use crate::error::IdbError;
use crate::key::range::EncodedRange;
use crate::key::utf16::Utf16String;
use crate::key::value::Key;
use crate::clone::scvalue::ScValue;

/// Идентификатор хранилища ключей (аналог origin/tenant).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey(pub String);

impl StorageKey {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

pub type StoreId = u64;
pub type IndexId = u64;
pub type TxnId = u64;
pub type RequestId = u64;
pub type CursorId = u64;
pub type ConnectionId = u64;

/// Режим IDB-транзакции (§2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxnMode {
    ReadOnly,
    ReadWrite,
    VersionChange,
}

/// Гарантия сброса на диск (§2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Durability {
    #[default]
    Default,
    Strict,
    Relaxed,
}

/// Направление курсора (§2.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Next,
    NextUnique,
    Prev,
    PrevUnique,
}

impl Direction {
    pub fn is_unique(self) -> bool {
        matches!(self, Direction::NextUnique | Direction::PrevUnique)
    }

    pub fn is_prev(self) -> bool {
        matches!(self, Direction::Prev | Direction::PrevUnique)
    }
}

/// Источник операции (хранилище или индекс).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRef {
    Store(StoreId),
    Index { store: StoreId, index: IndexId },
}

/// Снимок отдельной записи IDB (§2.12 IDBRecord).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordSnapshot {
    pub key: Key,
    pub primary_key: Key,
    pub value: ScValue,
}

/// Операции внутри IDB-транзакции (§5, §6).
#[derive(Debug, Clone, PartialEq)]
pub enum Operation {
    Put {
        store: StoreId,
        key: Option<Key>,
        value: ScValue,
        no_overwrite: bool, // true для add(), false для put()
    },
    Get {
        source: SourceRef,
        range: EncodedRange,
    },
    GetKey {
        source: SourceRef,
        range: EncodedRange,
    },
    GetAll {
        source: SourceRef,
        range: EncodedRange,
        limit: Option<u32>,
        direction: Direction,
    },
    GetAllKeys {
        source: SourceRef,
        range: EncodedRange,
        limit: Option<u32>,
        direction: Direction,
    },
    GetAllRecords {
        source: SourceRef,
        range: EncodedRange,
        limit: Option<u32>,
        direction: Direction,
    },
    Delete {
        store: StoreId,
        range: EncodedRange,
    },
    Clear {
        store: StoreId,
    },
    Count {
        source: SourceRef,
        range: EncodedRange,
    },
    OpenCursor {
        source: SourceRef,
        range: EncodedRange,
        direction: Direction,
        key_only: bool,
    },
    CursorAdvance {
        cursor: CursorId,
        count: u32,
    },
    CursorContinue {
        cursor: CursorId,
        target_key: Option<Key>,
    },
    CursorContinuePrimaryKey {
        cursor: CursorId,
        target_key: Key,
        target_primary_key: Key,
    },
    CursorUpdate {
        cursor: CursorId,
        value: ScValue,
    },
    CursorDelete {
        cursor: CursorId,
    },
}

/// Результат выполнения операции.
#[derive(Debug, Clone, PartialEq)]
pub enum OpOutcome {
    Empty,
    Key(Key),
    Value(Option<ScValue>),
    Keys(Vec<Key>),
    Values(Vec<ScValue>),
    Records(Vec<RecordSnapshot>),
    Count(u64),
    CursorOpened {
        cursor_id: CursorId,
        key: Key,
        primary_key: Key,
        value: Option<ScValue>,
    },
    CursorAdvanced {
        has_value: bool,
        key: Option<Key>,
        primary_key: Option<Key>,
        value: Option<ScValue>,
    },
}
```

### 4.2. Модуль `backend/error.rs`
Файл: `crates/boa_idb_core/src/backend/error.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("Constraint violation: {0}")]
    Constraint(String),

    #[error("Entity not found: {0}")]
    NotFound(String),

    #[error("Invalid key range")]
    InvalidRange,

    #[error("Database is locked by another process or transaction")]
    Locked,

    #[error("Data corruption: {0}")]
    Corrupted(String),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("Storage quota exceeded: required {needed} bytes, available {available} bytes")]
    QuotaExceeded { needed: u64, available: u64 },

    #[error("Internal backend error: {0}")]
    Internal(String),
}
```

### 4.3. Модуль `backend/types.rs`
Файл: `crates/boa_idb_core/src/backend/types.rs`

```rust
use crate::key::path::KeyPath;
use crate::key::utf16::Utf16String;
use crate::proto::{IndexId, StoreId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSpec {
    pub name: Utf16String,
    pub key_path: KeyPath,
    pub auto_increment: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSpec {
    pub name: Utf16String,
    pub key_path: KeyPath,
    pub unique: bool,
    pub multi_entry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMeta {
    pub id: IndexId,
    pub store_id: StoreId,
    pub name: Utf16String,
    pub key_path: KeyPath,
    pub unique: bool,
    pub multi_entry: bool,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreMeta {
    pub id: StoreId,
    pub name: Utf16String,
    pub key_path: KeyPath,
    pub auto_increment: bool,
    pub key_gen: f64,
    pub indexes: Vec<IndexMeta>,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseMeta {
    pub name: Utf16String,
    pub version: u64,
    pub stores: Vec<StoreMeta>,
    pub next_store_id: StoreId,
    pub next_index_id: IndexId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CursorSeek {
    First,
    Key(Vec<u8>),
    KeyAndPrimaryKey { key: Vec<u8>, pkey: Vec<u8> },
}
```

### 4.4. Модуль `backend/traits.rs`
Файл: `crates/boa_idb_core/src/backend/traits.rs`

```rust
use crate::backend::capabilities::BackendCapabilities;
use crate::backend::error::BackendError;
use crate::backend::types::{CursorSeek, DatabaseMeta, IndexSpec, StoreSpec};
use crate::key::range::EncodedRange;
use crate::proto::{Direction, Durability, IndexId, SourceRef, StorageKey, StoreId, TxnMode};

pub trait BackendFactory: Send + Sync + 'static {
    fn open_storage(&self, key: &StorageKey) -> Result<Box<dyn Storage>, BackendError>;
}

pub trait Storage: Send + 'static {
    fn list_databases(&self) -> Result<Vec<(String, u64)>, BackendError>;
    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError>;
    fn delete_database(&self, name: &str) -> Result<(), BackendError>;
    fn usage_bytes(&self) -> Result<u64, BackendError>;
}

pub trait Database: Send + 'static {
    fn metadata(&self) -> &DatabaseMeta;
    fn capabilities(&self) -> BackendCapabilities;
    fn begin(
        &mut self,
        mode: TxnMode,
        scope: &[StoreId],
        durability: Durability,
    ) -> Result<Box<dyn BackendTxn + '_>, BackendError>;
    fn flush(&mut self) -> Result<(), BackendError>;
    fn close(self: Box<Self>) -> Result<(), BackendError>;
}

pub trait BackendTxn {
    // --- Savepoints (один savepoint на каждый IDBRequest) ---
    fn begin_request(&mut self) -> Result<(), BackendError>;
    fn commit_request(&mut self) -> Result<(), BackendError>;
    fn rollback_request(&mut self) -> Result<(), BackendError>;

    // --- Схема (только в режиме VersionChange) ---
    fn set_version(&mut self, version: u64) -> Result<(), BackendError>;
    fn create_store(&mut self, spec: &StoreSpec) -> Result<StoreId, BackendError>;
    fn delete_store(&mut self, id: StoreId) -> Result<(), BackendError>;
    fn rename_store(&mut self, id: StoreId, new_name: &str) -> Result<(), BackendError>;
    fn create_index(&mut self, store: StoreId, spec: &IndexSpec) -> Result<IndexId, BackendError>;
    fn delete_index(&mut self, store: StoreId, id: IndexId) -> Result<(), BackendError>;
    fn rename_index(&mut self, store: StoreId, id: IndexId, new_name: &str) -> Result<(), BackendError>;

    // --- Операции над данными хранилищ ---
    fn put(
        &mut self,
        store: StoreId,
        key: &[u8],
        value: &[u8],
        no_overwrite: bool,
    ) -> Result<(), BackendError>;

    fn get(
        &mut self,
        store: StoreId,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, BackendError>;

    fn delete_record(&mut self, store: StoreId, key: &[u8]) -> Result<bool, BackendError>;
    fn delete_range(&mut self, store: StoreId, range: &EncodedRange) -> Result<u64, BackendError>;
    fn clear(&mut self, store: StoreId) -> Result<(), BackendError>;
    fn count(&mut self, src: SourceRef, range: &EncodedRange) -> Result<u64, BackendError>;

    fn scan(
        &mut self,
        src: SourceRef,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
    ) -> Result<Box<dyn BackendCursor + '_>, BackendError>;

    // --- Генератор ключей ---
    fn key_gen_current(&self, store: StoreId) -> Result<f64, BackendError>;
    fn key_gen_set(&mut self, store: StoreId, value: f64) -> Result<(), BackendError>;

    // --- Индексные записи ---
    fn index_put(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
        unique: bool,
    ) -> Result<(), BackendError>;

    fn index_delete(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
    ) -> Result<(), BackendError>;

    fn index_delete_by_primary(
        &mut self,
        index: IndexId,
        primary_key: &[u8],
    ) -> Result<(), BackendError>;

    // --- Фиксация транзакции ---
    fn commit(self: Box<Self>) -> Result<(), BackendError>;
    fn abort(self: Box<Self>) -> Result<(), BackendError>;
}

pub trait BackendCursor {
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError>;
    fn step(&mut self, count: u32) -> Result<bool, BackendError>;
    fn current_key(&self) -> &[u8];
    fn current_primary_key(&self) -> &[u8];
    fn current_value(&self) -> Option<&[u8]>;
}
```

---

## 5. Спецификация модулей `engine` (`boa_idb_core::engine`)

### 5.1. Планировщик транзакций `scheduler.rs`
Файл: `crates/boa_idb_core/src/engine/scheduler.rs`

Планировщик обязан реализовывать правила §2.7.2 Спеки:
```rust
use crate::proto::{StoreId, TxnId, TxnMode};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct TxnQueueItem {
    pub id: TxnId,
    pub mode: TxnMode,
    pub scope: Vec<StoreId>,
}

#[derive(Default)]
pub struct TransactionScheduler {
    pending_queue: VecDeque<TxnQueueItem>,
    running_txns: Vec<TxnQueueItem>,
}

impl TransactionScheduler {
    pub fn enqueue(&mut self, item: TxnQueueItem) {
        self.pending_queue.push_back(item);
    }

    /// Проверяет готовность следующих транзакций к запуску.
    /// Возвращает список TxnId, которые могут стартовать немедленно.
    pub fn poll_ready(&mut self) -> Vec<TxnId> {
        let mut ready = Vec::new();
        let mut i = 0;

        while i < self.pending_queue.len() {
            let candidate = &self.pending_queue[i];

            if self.can_start(candidate, i) {
                let item = self.pending_queue.remove(i).unwrap();
                self.running_txns.push(item.clone());
                ready.push(item.id);
            } else {
                // Если readwrite транзакция заблокирована, мы НЕ пропускаем последующие
                // конфликтующие транзакции, чтобы избежать голодания (FIFO fairness).
                i += 1;
            }
        }

        ready
    }

    fn can_start(&self, candidate: &TxnQueueItem, index_in_pending: usize) -> bool {
        // 1. Проверка конфликтов с уже выполняющимися транзакциями
        for running in &self.running_txns {
            if self.conflicts(candidate, running) {
                return false;
            }
        }

        // 2. Проверка конфликтов с более ранними ожидающими в очереди
        for earlier in self.pending_queue.iter().take(index_in_pending) {
            if self.conflicts(candidate, earlier) {
                return false;
            }
        }

        true
    }

    fn conflicts(&self, a: &TxnQueueItem, b: &TxnQueueItem) -> bool {
        // VersionChange эксклюзивна ко всем транзакциям
        if a.mode == TxnMode::VersionChange || b.mode == TxnMode::VersionChange {
            return true;
        }
        // Две ReadOnly транзакции никогда не конфликтуют
        if a.mode == TxnMode::ReadOnly && b.mode == TxnMode::ReadOnly {
            return false;
        }
        // Если хотя бы одна ReadWrite — проверяем пересечение scope
        for s_a in &a.scope {
            if b.scope.contains(s_a) {
                return true;
            }
        }
        false
    }

    pub fn on_txn_finished(&mut self, id: TxnId) {
        self.running_txns.retain(|t| t.id != id);
    }
}
```

### 5.2. Конечный автомат открытия базы `open_queue.rs`
Файл: `crates/boa_idb_core/src/engine/open_queue.rs`

Реализует шаги алгоритма §5.1 «Opening a database connection» и §5.3 «Deleting a database»:
1. Запросы на `open` и `delete` ставятся в строгую FIFO-очередь на уровне базы.
2. Сравнение версий: если `requested_version < current_version` -> немедленная ошибка `VersionError`.
3. Если `requested_version > current_version` (или delete):
   - Разослать событие `versionchange` всем другим активным соединениям.
   - Если после этого остаются незакрытые соединения -> отправить событие `blocked`.
   - Ждать закрытия всех соединений.
   - Запустить эксклюзивную `VersionChange` транзакцию.
   - При успехе: обновить версию, перевести соединение в открытое состояние, вызвать `success`.
   - При ошибке / abort upgrade: откатить версию, вызвать `error` с `AbortError` / причиной сбоя.

### 5.3. Генератор ключей `keygen.rs`
Файл: `crates/boa_idb_core/src/engine/keygen.rs`

Спецификация §2.11:
- Диапазон: целые числа от $1$ до $2^{53}-1$ (`9_007_199_254_740_991.0`).
- Метод `generate()`: возвращает текущее значение, увеличивает на 1. Если превышает $2^{53}-1$ -> `IdbError::Constraint`.
- Метод `possibly_update(key: f64)`: если `key >= current`, устанавливает `current = floor(key) + 1.0`.
- Поддержка отката: хранит стек предыдущих значений для каждого вложенного savepoint запроса. При `rollback_request()` значение восстанавливается.

### 5.4. Операции над хранилищем `ops_store.rs` и индексами `ops_index.rs`

1. **Алгоритм `put / add` (§6.1):**
   - Если у хранилища есть `keyPath`: извлечь ключ из `ScValue` с помощью `key_path.extract(&value)`.
   - Если ключ не извлечен и включен `autoIncrement`: сгенерировать `keygen.generate()`, выполнить `key_path.inject(&mut value, &gen_key)`.
   - Если явный ключ передан, но у хранилища есть `keyPath` -> `IdbError::Data`.
   - Если ключа нет и нет генератора -> `IdbError::Data`.
   - Если `no_overwrite == true` (`add`): проверить существование ключа в бэкенде. Если существует -> `IdbError::Constraint`.
   - **Синхронизация индексов:**
     - Для каждого индекса хранилища: извлечь индексный ключ `idx_key = index.key_path.extract(&value)`.
     - Обработка `multiEntry`: если `idx_key` — `Key::Array`, извлечь уникальные валидные элементы.
     - Для каждого индексного ключа: если `index.unique == true`, проверить отсутствие записей с таким ключом в индексе. Нарушение -> `IdbError::Constraint`.
     - Если старая запись обновляется: удалить старые индексные записи через `index_delete`.
     - Вставить новые индексные записи через `index_put`.
   - Записать значение в бэкенд: `backend_txn.put(store_id, &encode_key(&key), &encode_scf(&value), no_overwrite)`.
   - Вернуть `OpOutcome::Key(key)`.

2. **Алгоритм `delete` (§6.4):**
   - Удалить записи из хранилища по диапазону.
   - Для всех удаленных записей удалить соответствующие записи из всех индексов через `index_delete_by_primary`.

---

## 6. Реализация In-Memory бэкенда `boa_idb_memory`

### 6.1. Архитектура `boa_idb_memory`
Хранилище полностью строится на структурах `BTreeMap` и стандартной памяти:
- `records: BTreeMap<(StoreId, Vec<u8>), Vec<u8>>` (первичные записи).
- `index_records: BTreeMap<(IndexId, Vec<u8>, Vec<u8>), ()>` (индексные записи: `(index_id, index_key_bytes, primary_key_bytes)`).
- `key_generators: HashMap<StoreId, f64>`.

### 6.2. Механизм Undo-логов для Savepoints (`txn.rs`)
Каждая мутация (`put`, `delete`, `index_put`, `index_delete`, `key_gen_set`) регистрирует обратную операцию в текущем undo-стеке:
```rust
enum UndoOp {
    RestoreRecord { store: StoreId, key: Vec<u8>, old_value: Option<Vec<u8>> },
    RestoreIndex { index: IndexId, idx_key: Vec<u8>, pkey: Vec<u8>, existed: bool },
    RestoreKeyGen { store: StoreId, old_val: f64 },
}
```
- `begin_request()`: создает новый уровень в стеке undo-логов: `undo_stack.push(Vec::new())`.
- `commit_request()`: схлопывает верхний уровень в предыдущий: `let ops = undo_stack.pop().unwrap(); if let Some(top) = undo_stack.last_mut() { top.extend(ops); }`.
- `rollback_request()`: проигрывает операции из верхнего уровня в обратном порядке и откатывает изменения в `BTreeMap`.

### 6.3. Реализация итерации курсора `cursor.rs`
In-memory курсор берет итератор по диапазону ключей `BTreeMap::range(...)`:
- Для `Direction::Next`: прямой итератор.
- Для `Direction::Prev`: реверсивный итератор (`.rev()`).
- Для `Direction::NextUnique`: пропускает записи с одинаковым `idx_key`, возвращая только первую.
- Для `Direction::PrevUnique`: при обратном сканировании возвращает первую запись (с наименьшим `primary_key`) для каждого уникального `idx_key`.

---

## 7. Эталонные тесты и дифференциальное тестирование

### 7.1. Тесты планировщика (`scheduler_tests.rs`)
1. Запуск 10 независимых `ReadOnly` транзакций к одному store -> все стартуют параллельно.
2. `ReadWrite` к store 1 блокирует последующую `ReadOnly` к store 1, но не блокирует `ReadOnly` к store 2.
3. Проверка отсутствия голодания: поток `ReadOnly` транзакций не должен вечно откладывать стоящую в очереди `ReadWrite`.

### 7.2. Дифференциальное тестирование (`differential_model_tests.rs`)
Создать легковесную эталонную модель `ReferenceModel` (простой синхронный `HashMap<Key, ScValue>`).
Сгенерировать 10 000 случайных последовательностей операций (`Put`, `Get`, `Delete`, `CursorAdvance`, `SavepointRollback`, `Abort`) и прогнать их параллельно на `ReferenceModel` и `MemoryBackend`.
**Инвариант:** Результаты каждой операции и итоговые дампы данных обязаны побайтово совпадать.

---

## 8. Пошаговый план работ для исполнителя (Execution Steps)

1. **Шаг 1. Типы протокола и трейты бэкенда (`boa_idb_core`)**
   - Наполнить `crates/boa_idb_core/src/proto.rs`.
   - Реализовать `crates/boa_idb_core/src/backend/` (`traits.rs`, `error.rs`, `types.rs`, `capabilities.rs`).
2. **Шаг 2. Движок транзакций и планировщик (`boa_idb_core::engine`)**
   - Реализовать `keygen.rs`, `transaction.rs`, `scheduler.rs`.
   - Покрыть unit-тестами планировщик (`scheduler_tests.rs`) и генератор ключей (`keygen_tests.rs`).
3. **Шаг 3. FSM очереди открытия соединений**
   - Реализовать `registry.rs`, `connection.rs`, `open_queue.rs`.
   - Покрыть тестами сценарии версионирования и блокировок (`open_queue_tests.rs`).
4. **Шаг 4. Алгоритмы хранилищ, индексов и курсоров**
   - Реализовать `ops_store.rs`, `ops_index.rs`, `cursor.rs`.
   - Написать тесты на `multiEntry`, `unique`, курсоры 4 направлений.
5. **Шаг 5. Реализация `boa_idb_memory`**
   - Реализовать `storage.rs`, `database.rs`, `txn.rs`, `cursor.rs`.
   - Реализовать Undo-логи для точных savepoints.
6. **Шаг 6. Дифференциальные и стресс-тесты**
   - Написать `memory_backend_tests.rs` и `differential_model_tests.rs`.
   - Прогнать 10 000 дифференциальных итераций.
7. **Шаг 7. Финальная проверка**
   - Запустить все команды из Раздела 9.

---

## 9. Команды валидации и критерии приёмки

Исполнитель сдает работу только тогда, когда **ВСЕ** команды выполняются со статусом SUCCESS (EXIT CODE 0):

```powershell
# 1. Форматирование
cargo fmt --all -- --check

# 2. Строгий линтинг
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 3. Полный прогон тестов ядра и memory-бэкенда
cargo test --package boa_idb_core
cargo test --package boa_idb_memory

# 4. Дифференциальный тест против эталонной модели
cargo test --package boa_idb_memory --test differential_model_tests -- --nocapture

# 5. Проверка компиляции ядра под wasm32
cargo check --package boa_idb_core --target wasm32-unknown-unknown

# 6. Проверка покрытия кода (требование: ≥ 90% для boa_idb_core)
cargo llvm-cov --package boa_idb_core --summary-only
```

### Чек-лист соответствия ТЗ:
- [ ] `R7.1.1`–`R7.1.5`: Реестр, connection queue, FSM открытия, `versionchange`, `blocked`, upgrade/abort upgrade.
- [ ] `R7.2.1`–`R7.2.6`: Планировщик транзакций, FIFO справедливость, проверка пересечений scope, автокоммит.
- [ ] `R7.3.1`–`R7.3.3`: Очередь запросов, savepoint на каждый запрос.
- [ ] `R7.4.1`–`R7.4.4`: Курсоры, живая позиция, 4 направления (`next`, `nextunique`, `prev`, `prevunique`).
- [ ] `R8.1.2`: Расчет изменений индексов в ядре, поддержка `multiEntry` и `unique`.
- [ ] `R8.4`: Полная реализация `boa_idb_memory` с поддержкой snapshot-изоляции и undo-логов.
- [ ] 100% Safe Rust (`#![deny(unsafe_code)]`), 0 предупреждений компилятора.
