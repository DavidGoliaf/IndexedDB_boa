# ТЕХНИЧЕСКОЕ ЗАДАНИЕ НА ИСПОЛНЕНИЕ: TASK-04
## Реализация production-бэкенда SQLite (`boa_idb_sqlite`)

| Метаданные | Значение |
|---|---|
| **Идентификатор задачи** | `TASK-04-SQLITE-BACKEND` |
| **Этап ТЗ** | **M4 (SQLite-бэкенд `boa_idb_sqlite`)** |
| **Целевой крейт** | `crates/boa_idb_sqlite` (интеграция с `boa_idb_core` и `boa_idb`) |
| **Нормативное ТЗ** | `TZ_boa_idb_IndexedDB.md` (разделы 2.3, 4.2, 8.1, 8.2, 8.5, 12.1, 13.4, 15, Приложение C) |
| **Пререквизиты** | Принятые `TASK-01` (кодеки), `TASK-02` (ядро движка), `TASK-03` (JS-биндинги) |
| **Роль архитектора** | Спроектированы схема SQLite, DDL, SQL-шаблоны курсоров, пул соединений, механизм SAVEPOINT, хранение крупных значений (Blobs) и хеширование путей. |
| **Роль исполнителя** | Строгая механическая реализация кода по приведенным сигнатурам, структуре файлов, SQL-запросам и тестам. **Никаких собственных архитектурных домыслов.** |

---

## 1. Архитектурный контекст и жесткие правила (Guardrails)

1. **Безопасность кода (`# Safety`):** Везде действует `#![deny(unsafe_code)]`. Запрещены любые блоки `unsafe`.
2. **Запрет паник:** Использование `unwrap()`, `expect()`, `panic!` в коде библиотеки **КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНО**. Все ошибки SQLite транслируются в `BackendError` с сохранением исходной причины.
3. **Изоляция хранилищ и топология файлов (R8.1.4, R8.2.1):**
   - Имена баз и хранилищ никогда не используются в путях ФС напрямую!
   - Имя файла базы = `db-<db_name_hash>.sqlite`, где `db_name_hash = base32(sha256(utf16le(name)))[..26]`.
   - Реестр баз storage key хранится в отдельном файле `registry.sqlite`.
   - Крупные значения выносятся в `blobs/<db_name_hash>/xx/<sha256>.bin`.
4. **Конкурентность и WAL-режим (AD-6, R8.2.1):**
   - SQLite база всегда открывается в режиме `PRAGMA journal_mode = WAL`.
   - Пул соединений: **1 писатель** (для `ReadWrite` и `VersionChange`, `BEGIN IMMEDIATE`) + **N читателей** (для `ReadOnly`, `BEGIN DEFERRED`).
   - `ReadOnly` транзакция получает выделенное соединение из пула, что гарантирует **snapshot-изоляцию** от параллельных коммитов писателя.
5. **Вложенные точки сохранения на каждый запрос (AD-7, §8.2.3):**
   - `begin_request()` -> `SAVEPOINT r<seq>`
   - `commit_request()` -> `RELEASE SAVEPOINT r<seq>`
   - `rollback_request()` -> `ROLLBACK TO SAVEPOINT r<seq>; RELEASE SAVEPOINT r<seq>`
6. **Кластеризация и оптимизация запросов (R8.2.3):**
   - Таблицы `records` и `index_records` создаются с `WITHOUT ROWID` и составным `PRIMARY KEY`. Это обеспечивает кластеризацию по ключам в B-дереве и исключает `SCAN TABLE`.
   - Обязателен тест `sqlite_explain_plan_tests.rs`, проверяющий отсутствие полного сканирования таблицы при запросах диапазонов.
7. **Крупные значения (Blobs, §8.2.5):**
   - Значения размером > `inline_value_threshold` (по умолчанию 256 КиБ) сохраняются во внешний файл `blobs/<hash>.bin`.
   - В `records.value` пишется `NULL`, в `records.ext` — имя файла. Запись во временный файл + `fsync` + `rename` выполняется **до** коммита транзакции SQLite.

---

## 2. Полное дерево файлов `crates/boa_idb_sqlite`

```
crates/boa_idb_sqlite/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── naming.rs            # Хеширование путей: SHA-256 + Base32
│   ├── schema.rs            # DDL схемы, миграции версий метаданных
│   ├── pool.rs              # Пул соединений (1 writer + N readers, LRU statement cache)
│   ├── blob.rs              # Менеджер внешних файлов для крупных значений (>256KB)
│   ├── cursor.rs            # SqliteCursor (BackendCursor по 4 направлениям)
│   ├── txn.rs               # SqliteTxn (BackendTxn, SAVEPOINT, CRUD, индексы)
│   ├── database.rs          # SqliteDatabase (Database trait)
│   ├── storage.rs           # SqliteStorage (Storage trait, registry.sqlite)
│   └── factory.rs           # SqliteBackendFactory (BackendFactory trait)
└── tests/
    ├── sqlite_backend_tests.rs
    ├── sqlite_concurrency_tests.rs
    ├── sqlite_blob_overflow_tests.rs
    ├── sqlite_explain_plan_tests.rs
    └── sqlite_migration_tests.rs
```

---

## 3. Манифест `crates/boa_idb_sqlite/Cargo.toml`

```toml
[package]
name = "boa_idb_sqlite"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "SQLite backend for IndexedDB implementation (WAL mode, connection pool, blob overflow)"

[dependencies]
boa_idb_core = { path = "../boa_idb_core" }
rusqlite = { version = "0.32", features = ["bundled", "backup", "blob"] }
parking_lot = "0.12"
thiserror = { workspace = true }
lru = "0.12"
crc32fast = { workspace = true }
data-encoding = "2.6"
sha2 = "0.10"
tempfile = "3"
tracing = { workspace = true, optional = true }

[dev-dependencies]
tempfile = "3"
proptest = { workspace = true }
rstest = { workspace = true }

[features]
default = []
tracing = ["dep:tracing", "boa_idb_core/tracing"]

[lints]
workspace = true
```

---

## 4. Спецификация DDL схемы SQLite (`schema.rs`)

Файл: `crates/boa_idb_sqlite/src/schema.rs`

```sql
-- DDL инициализации базы IndexedDB
PRAGMA page_size = 4096;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -8000;
PRAGMA wal_autocheckpoint = 1000;

-- 1. Метаданные базы
CREATE TABLE IF NOT EXISTS meta (
    k TEXT PRIMARY KEY,
    v BLOB
) WITHOUT ROWID;

-- 2. Объектные хранилища
CREATE TABLE IF NOT EXISTS object_stores (
    id             INTEGER PRIMARY KEY,
    name           BLOB NOT NULL,          -- UTF-16LE байты
    key_path       BLOB,                   -- SCF-закодированный KeyPath
    auto_increment INTEGER NOT NULL DEFAULT 0,
    key_gen        REAL    NOT NULL DEFAULT 1,
    UNIQUE(name)
);

-- 3. Индексы
CREATE TABLE IF NOT EXISTS indexes (
    id          INTEGER PRIMARY KEY,
    store_id    INTEGER NOT NULL REFERENCES object_stores(id) ON DELETE CASCADE,
    name        BLOB NOT NULL,             -- UTF-16LE байты
    key_path    BLOB NOT NULL,             -- SCF-закодированный KeyPath
    is_unique   INTEGER NOT NULL DEFAULT 0,
    multi_entry INTEGER NOT NULL DEFAULT 0,
    UNIQUE(store_id, name)
);

-- 4. Записи объектных хранилищ (кластеризованные по store_id, key)
CREATE TABLE IF NOT EXISTS records (
    store_id INTEGER NOT NULL REFERENCES object_stores(id) ON DELETE CASCADE,
    key      BLOB NOT NULL,                -- KEY-v1 байты
    value    BLOB,                         -- SCF-v1 байты (NULL если вынесено в blobs)
    ext      TEXT,                         -- относительный путь к blob-файлу
    vlen     INTEGER NOT NULL,             -- логический размер значения в байтах
    PRIMARY KEY (store_id, key)
) WITHOUT ROWID;

-- 5. Записи индексов (кластеризованные по index_id, key, pkey)
CREATE TABLE IF NOT EXISTS index_records (
    index_id INTEGER NOT NULL REFERENCES indexes(id) ON DELETE CASCADE,
    key      BLOB NOT NULL,                -- Индексный ключ KEY-v1
    pkey     BLOB NOT NULL,                -- Первичный ключ KEY-v1
    PRIMARY KEY (index_id, key, pkey)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_index_records_pkey ON index_records(index_id, pkey);
```

### Схема `registry.sqlite` (хранится в корне storage key):
```sql
CREATE TABLE IF NOT EXISTS databases (
    name       BLOB PRIMARY KEY,           -- UTF-16LE байты имени базы
    version    INTEGER NOT NULL DEFAULT 0,
    db_file    TEXT NOT NULL,              -- имя файла "db-<hash>.sqlite"
    created_at INTEGER NOT NULL            -- timestamp в мс
);
```

---

## 5. Алгоритмы и компоненты

### 5.1. Хеширование путей (`naming.rs`)
Файл: `crates/boa_idb_sqlite/src/naming.rs`

```rust
use data_encoding::BASE32_NOPAD;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Преобразует UTF-16 имя базы в безопасное имя файла на диске (R8.1.4).
pub fn db_name_to_filename(name_utf16: &[u16]) -> String {
    let mut bytes = Vec::with_capacity(name_utf16.len() * 2);
    for &cu in name_utf16 {
        bytes.extend_from_slice(&cu.to_le_bytes());
    }
    let hash = Sha256::digest(&bytes);
    let encoded = BASE32_NOPAD.encode(&hash).to_ascii_lowercase();
    format!("db-{}.sqlite", &encoded[..26])
}

/// Вычисляет путь к blob-файлу по sha256 хешу значения.
pub fn blob_path(root: &Path, db_hash: &str, value_bytes: &[u8]) -> PathBuf {
    let hash = Sha256::digest(value_bytes);
    let hex_hash = hex::encode(hash);
    let prefix = &hex_hash[..2];
    root.join("blobs").join(db_hash).join(prefix).join(format!("{hex_hash}.bin"))
}
```

---

### 5.2. Пул соединений (`pool.rs`)
Файл: `crates/boa_idb_sqlite/src/pool.rs`

- **Writer соединение:** Защищено `parking_lot::Mutex<Connection>`. Выделяется монопольно для `ReadWrite` и `VersionChange` транзакций (`BEGIN IMMEDIATE`).
- **Reader пул:** Пул из `N` (по умолчанию 4) соединений, защищенный очередью/мьютексом. Выделяется для `ReadOnly` транзакций (`BEGIN DEFERRED`).
- **LRU Cache:** Каждое соединение содержит LRU-кэш на 64 подготовленных SQL-выражения (`Statement`).

```rust
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ConnectionPool {
    db_path: PathBuf,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    max_readers: usize,
}
```

---

### 5.3. Трансляция запросов и Savepoints (`txn.rs`)
Файл: `crates/boa_idb_sqlite/src/txn.rs`

#### Маппинг Savepoints:
```rust
impl BackendTxn for SqliteTxn<'_> {
    fn begin_request(&mut self) -> Result<(), BackendError> {
        let savepoint_name = format!("r{}", self.request_seq);
        self.request_seq += 1;
        self.conn.execute_batch(&format!("SAVEPOINT {savepoint_name}"))
            .map_err(|e| BackendError::Internal(format!("Savepoint begin failed: {e}")))?;
        self.savepoints.push(savepoint_name);
        Ok(())
    }

    fn commit_request(&mut self) -> Result<(), BackendError> {
        if let Some(sp) = self.savepoints.pop() {
            self.conn.execute_batch(&format!("RELEASE SAVEPOINT {sp}"))
                .map_err(|e| BackendError::Internal(format!("Savepoint release failed: {e}")))?;
        }
        Ok(())
    }

    fn rollback_request(&mut self) -> Result<(), BackendError> {
        if let Some(sp) = self.savepoints.pop() {
            self.conn.execute_batch(&format!("ROLLBACK TO SAVEPOINT {sp}; RELEASE SAVEPOINT {sp}"))
                .map_err(|e| BackendError::Internal(format!("Savepoint rollback failed: {e}")))?;
        }
        Ok(())
    }
}
```

#### SQL-шаблоны для операций хранилища:
1. **`put` (вставка / обновление записи):**
   ```sql
   INSERT INTO records (store_id, key, value, ext, vlen)
   VALUES (?1, ?2, ?3, ?4, ?5)
   ON CONFLICT(store_id, key) DO UPDATE SET
       value = excluded.value,
       ext = excluded.ext,
       vlen = excluded.vlen;
   ```
2. **`put` с `no_overwrite = true` (`add`):**
   ```sql
   INSERT INTO records (store_id, key, value, ext, vlen)
   VALUES (?1, ?2, ?3, ?4, ?5);
   -- При ошибке SQLITE_CONSTRAINT_PRIMARYKEY -> вернуть BackendError::Constraint
   ```
3. **`get` (чтение одной записи):**
   ```sql
   SELECT value, ext FROM records WHERE store_id = ?1 AND key = ?2;
   ```
4. **`delete_range` (удаление по диапазону):**
   ```sql
   DELETE FROM records WHERE store_id = ?1 AND key >= ?2 AND key <= ?3;
   ```
5. **`count` по диапазону хранилища:**
   ```sql
   SELECT COUNT(*) FROM records WHERE store_id = ?1 AND key >= ?2 AND key <= ?3;
   ```

---

### 5.4. SQL-шаблоны для курсоров 4 направлений (`cursor.rs`)
Файл: `crates/boa_idb_sqlite/src/cursor.rs`

Индексы и сканирование записей по B-дереву без материализации всего набора данных:

1. **Курсор `Direction::Next` по ObjectStore:**
   ```sql
   SELECT key, value, ext FROM records
    WHERE store_id = ?1 AND key >= ?2 AND key <= ?3
    ORDER BY key ASC
    LIMIT ?4 OFFSET ?5;
   ```
2. **Курсор `Direction::Prev` по ObjectStore:**
   ```sql
   SELECT key, value, ext FROM records
    WHERE store_id = ?1 AND key >= ?2 AND key <= ?3
    ORDER BY key DESC
    LIMIT ?4 OFFSET ?5;
   ```
3. **Курсор `Direction::Next` по Index (с первичным ключом):**
   ```sql
   SELECT ir.key, ir.pkey, r.value, r.ext
     FROM index_records ir
     JOIN records r ON r.store_id = ?1 AND r.key = ir.pkey
    WHERE ir.index_id = ?2
      AND (ir.key > ?3 OR (ir.key = ?3 AND ir.pkey >= ?4))
      AND ir.key <= ?5
    ORDER BY ir.key ASC, ir.pkey ASC
    LIMIT ?6;
   ```
4. **Курсор `Direction::NextUnique` по Index (только первая запись для каждого индексного ключа):**
   ```sql
   SELECT ir.key, ir.pkey, r.value, r.ext
     FROM index_records ir
     JOIN records r ON r.store_id = ?1 AND r.key = ir.pkey
    WHERE ir.index_id = ?2
      AND ir.key > ?3 AND ir.key <= ?4
    GROUP BY ir.key
    ORDER BY ir.key ASC, ir.pkey ASC
    LIMIT ?5;
   ```
5. **Курсор `Direction::PrevUnique` по Index (наименьший `primary_key` для каждого уникального `idx_key` при обратном порядке):**
   ```sql
   SELECT ir.key, MIN(ir.pkey) AS pkey, r.value, r.ext
     FROM index_records ir
     JOIN records r ON r.store_id = ?1 AND r.key = ir.pkey
    WHERE ir.index_id = ?2
      AND ir.key < ?3 AND ir.key >= ?4
    GROUP BY ir.key
    ORDER BY ir.key DESC
    LIMIT ?5;
   ```

---

### 5.5. Менеджер крупных значений (`blob.rs`)
Файл: `crates/boa_idb_sqlite/src/blob.rs`

- Порог выноса: `inline_value_threshold = 256 * 1024` (256 КиБ).
- Если `value.len() > inline_value_threshold`:
  1. Вычисляется SHA-256 хеш содержимого.
  2. Значение записывается во временный файл в каталоге `blobs/<db_hash>/tmp/`.
  3. Вызывается `file.sync_all()` (`fsync`).
  4. Файл атомарно перемещается (`rename`) в `blobs/<db_hash>/xx/<sha256>.bin`.
  5. В SQLite пишется `records.value = NULL`, `records.ext = "xx/<sha256>.bin"`.
- При чтении: если `records.value IS NULL`, файл читается из `blobs/`, проверяется CRC32C сумма. Несовпадение -> `BackendError::Corrupted`.

---

## 6. Тестовые сценарии

Исполнитель **ОБЯЗАН** разработать следующие тест-сьюты в `crates/boa_idb_sqlite/tests/`:

1. **`tests/sqlite_backend_tests.rs`:**
   - Полный цикл CRUD для хранилищ и индексов на дисковом SQLite;
   - Проверка персистентности данных после переоткрытия `SqliteBackendFactory`;
   - Проверка работы Savepoints и отката при `rollback_request()`.
2. **`tests/sqlite_concurrency_tests.rs`:**
   - 10 параллельных `ReadOnly` транзакций читают снимок базы во время активной записи писателя (`ReadWrite`);
   - Проверка отсутствия взаимных блокировок и взаимного влияния.
3. **`tests/sqlite_blob_overflow_tests.rs`:**
   - Вставка значений размером 512 КиБ, 2 МБ и 10 МБ;
   - Проверка создания blob-файлов на диске и корректного чтения данных;
   - Проверка отката: при `abort()` или `rollback_request()` ссылки в базе не появляются.
4. **`tests/sqlite_explain_plan_tests.rs`:**
   - Выполнение `EXPLAIN QUERY PLAN` для запросов get/count/cursor по диапазону;
   - Ассерт на отсутствие подстроки `SCAN TABLE` (запросы обязаны использовать составной индекс B-дерева).
5. **`tests/sqlite_migration_tests.rs`:**
   - Проверка чтения баз со старой версией `meta.schema_version` и выполнение миграций.

---

## 7. Пошаговый план работ для исполнителя (Execution Steps)

1. **Шаг 1. Манифест и зависимости `crates/boa_idb_sqlite`**
   - Наполнить `Cargo.toml` (`rusqlite`, `lru`, `data-encoding`, `sha2`, `tempfile`).
   - Проверить сборку крейта через `cargo check --package boa_idb_sqlite`.
2. **Шаг 2. Хеширование путей и DDL схемы**
   - Реализовать `naming.rs` (SHA-256 + Base32) и `schema.rs` (DDL скрипты, прагмы WAL).
3. **Шаг 3. Менеджер Blob-файлов**
   - Реализовать `blob.rs` (вынос значений > 256KB, атомарный `rename`, проверка CRC32).
4. **Шаг 4. Пул соединений и управление блокировками**
   - Реализовать `pool.rs` (1 writer + N readers, LRU кэш prepared statements, `busy_timeout = 5000ms`).
5. **Шаг 5. Транзакции и точки сохранения**
   - Реализовать `txn.rs` (`BackendTxn` имплементация, `SAVEPOINT r<seq>`, CRUD, синхронизация индексов).
6. **Шаг 6. Курсоры всех 4 направлений**
   - Реализовать `cursor.rs` (`SqliteCursor`, B-дерево поиск, `Next`, `NextUnique`, `Prev`, `PrevUnique`).
7. **Шаг 7. Реестр баз и фабрика хранилища**
   - Реализовать `database.rs`, `storage.rs`, `factory.rs`.
8. **Шаг 8. Написание тестов и верификация планов запросов**
   - Реализовать все 5 тест-сьютов из Раздела 6.
   - Запустить верификационные команды из Раздела 8.

---

## 8. Команды валидации и критерии приёмки

Исполнитель сдает работу только тогда, когда **ВСЕ** команды выполняются со статусом SUCCESS (EXIT CODE 0):

```powershell
# 1. Проверка форматирования
cargo fmt --all -- --check

# 2. Строгий линтинг clippy
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 3. Полный прогон всех тестов SQLite-бэкенда
cargo test --package boa_idb_sqlite -- --nocapture

# 4. Проверка планов выполнения запросов (отсутствие SCAN TABLE)
cargo test --package boa_idb_sqlite --test sqlite_explain_plan_tests -- --nocapture

# 5. Проверка генерации документации
cargo doc --package boa_idb_sqlite --no-deps

# 6. Проверка покрытия кода (требование: ≥ 85% для boa_idb_sqlite)
cargo llvm-cov --package boa_idb_sqlite --summary-only
```

### Чек-лист соответствия ТЗ:
- [ ] `R8.1.1`–`R8.1.6`: Атомарность коммитов, snapshot-изоляция `readonly`, хеширование путей (SHA256+Base32), верификация целостности.
- [ ] `R8.2.1`–`R8.2.5`: Схема `WITHOUT ROWID`, пул соединений (1 writer + N readers), Savepoints `r<seq>`, вынос блобов > 256KB, миграции схемы.
- [ ] Отсутствие `SCAN TABLE` в `EXPLAIN QUERY PLAN` для диапазонных запросов и курсоров.
- [ ] 100% Safe Rust (`#![deny(unsafe_code)]`), 0 предупреждений компилятора.
- [ ] Данные переживают перезапуск процесса и переоткрытие базы.
