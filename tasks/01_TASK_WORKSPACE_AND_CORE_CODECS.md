# ТЕХНИЧЕСКОЕ ЗАДАНИЕ НА ИСПОЛНЕНИЕ: TASK-01
## Инициализация Workspace и реализация `boa_idb_core` (Ключи, Key Path и сериализатор SCF-v1)

| Метаданные | Значение |
|---|---|
| **Идентификатор задачи** | `TASK-01-CORE-CODECS` |
| **Этапы ТЗ** | **M0 (Проектирование / Каркас)** + **M1 (Ядро: ключи и кодеки)** |
| **Целевой крейт** | `crates/boa_idb_core` (плюс корневой workspace) |
| **Нормативное ТЗ** | `d:\projects\BoaX\boa-idb\TZ_boa_idb_IndexedDB.md` (разделы 2, 3, 6, 10, 11, 13, 14, 15, Приложения B, C) |
| **Роль архитектора** | Спроектированы все структуры данных, бинарные форматы, алгоритмы кодирования/декодирования, ограничения, коды ошибок и тесты. |
| **Роль исполнителя** | Строгая механическая реализация кода по приведенным сигнатурам, файловой структуре, алгоритмам и тестовым спецификациям. **Никаких собственных архитектурных домыслов.** |

---

## 1. Архитектурный контекст и жесткие правила (Guardrails)

Исполнитель **ОБЯЗАН** строго соблюдать следующие требования:

1. **Изоляция от движка JS (`AD-1`):** Крейт `boa_idb_core` **НЕ ДОЛЖЕН** зависеть от `boa_engine`, `boa_gc`, `boa_runtime` или любых JS-типов. В ядре используются только чистые Rust-типы (`ScValue`, `Key`, `Utf16String`, `LimitConfig`, `IdbError`). Крейт обязан компилироваться под `wasm32-unknown-unknown` с `alloc`.
2. **Безопасность кода (`# Safety`):** Везде действует `#![deny(unsafe_code)]`. Запрещены любые блоки `unsafe`.
3. **Запрет паник:** Использование `unwrap()`, `expect()`, `panic!` в коде библиотеки **КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНО**. Все ошибочные состояния транслируются в `Result<T, KeyError>`, `Result<T, ScError>` или `Result<T, KeyPathError>`.
4. **Работа со строками:** JS-строки в IndexedDB могут содержать непарные суррогаты (unpaired surrogates: `U+D800..=U+DFFF`). Использование `std::string::String` для ключей и key path запрещено, так как Rust `String` форсирует UTF-8 и повреждает суррогаты. Вся работа ведется через собственный тип `Utf16String` (`Vec<u16>`).
5. **Нормализация чисел (`-0.0` vs `+0.0`):** Перед любым кодированием или сравнением ключей выполняется нормализация: `if v == 0.0 { v = 0.0 }`. Это устраняет битовое различие `-0.0` и `+0.0`. `NaN` в ключах недопустим и возвращает ошибку `KeyError::InvalidValue`.
6. **Защита от переполнения стека и DoS:**
   - Максимальная глубина вложенности массивов ключей: **32**.
   - Максимальная глубина вложенности при клонировании `ScValue`: **512**.
   - Максимальный размер закодированного ключа: `max_key_len` (по умолчанию **1024 байта**).
   - Максимальный размер сериализованного значения: `max_value_len` (по умолчанию **64 МБ**).

---

## 2. Полное дерево файлов задачи

Исполнитель должен создать и наполнить следующую структуру файлов в директории `d:\projects\BoaX\boa-idb`:

```
d:\projects\BoaX\boa-idb/
├── Cargo.toml
├── rust-toolchain.toml
├── clippy.toml
├── deny.toml
├── .gitignore
├── crates/
│   ├── boa_idb_core/
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── error.rs
│   │   │   ├── limits.rs
│   │   │   ├── key/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── utf16.rs
│   │   │   │   ├── value.rs
│   │   │   │   ├── compare.rs
│   │   │   │   ├── encode.rs
│   │   │   │   ├── range.rs
│   │   │   │   └── path.rs
│   │   │   └── clone/
│   │   │       ├── mod.rs
│   │   │       ├── scvalue.rs
│   │   │       ├── varint.rs
│   │   │       ├── encode.rs
│   │   │       └── decode.rs
│   │   └── tests/
│   │       ├── key_tests.rs
│   │       ├── key_proptests.rs
│   │       ├── scf_tests.rs
│   │       ├── scf_proptests.rs
│   │       └── keypath_tests.rs
│   ├── boa_idb/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── boa_idb_sqlite/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── boa_idb_fs/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── boa_idb_memory/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── boa_idb_wpt/
│       ├── Cargo.toml
│       └── src/lib.rs
└── fuzz/
    ├── Cargo.toml
    └── fuzz_targets/
        ├── fuzz_key_decode.rs
        ├── fuzz_scf_decode.rs
        └── fuzz_keypath_parse.rs
```

---

## 3. Конфигурационные файлы проекта

### 3.1. `rust-toolchain.toml`
```toml
[toolchain]
channel = "1.91.0"
components = ["rustfmt", "clippy"]
targets = ["x86_64-pc-windows-msvc", "wasm32-unknown-unknown"]
```

### 3.2. Корневой `Cargo.toml`
```toml
[workspace]
resolver = "3"
members = [
    "crates/boa_idb_core",
    "crates/boa_idb",
    "crates/boa_idb_sqlite",
    "crates/boa_idb_fs",
    "crates/boa_idb_memory",
    "crates/boa_idb_wpt",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.91.0"
license = "MIT OR Apache-2.0"
authors = ["BoaX Developers"]
repository = "https://github.com/boa-dev/boa"

[workspace.dependencies]
thiserror = "2.0"
smallvec = { version = "1.13", features = ["union", "const_generics"] }
indexmap = "2.7"
hashbrown = "0.15"
crc32fast = "1.4"
num-bigint = "0.4"
bytemuck = { version = "1.20", features = ["derive"] }
tracing = "0.1"
proptest = "1.6"
rstest = "0.24"

[workspace.lints.rust]
unsafe_code = "deny"
missing_docs = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
must_use_candidate = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
module_name_repetitions = "allow"
```

### 3.3. `clippy.toml`
```toml
avoid-breaking-exported-api = false
```

### 3.4. `deny.toml`
```toml
[licenses]
unlicensed = "reject"
allow = [
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unlicense",
    "Zlib",
    "CC0-1.0",
]
confidence-threshold = 0.8

[bans]
multiple-versions = "warn"
deny = []

[sources]
unknown-registry = "warn"
unknown-git = "warn"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

### 3.5. `crates/boa_idb_core/Cargo.toml`
```toml
[package]
name = "boa_idb_core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Core engine and codecs for IndexedDB implementation without JS dependencies"

[dependencies]
thiserror = { workspace = true }
smallvec = { workspace = true }
indexmap = { workspace = true }
hashbrown = { workspace = true }
crc32fast = { workspace = true }
num-bigint = { workspace = true }
bytemuck = { workspace = true }
tracing = { workspace = true, optional = true }

[dev-dependencies]
proptest = { workspace = true }
rstest = { workspace = true }

[features]
default = []
tracing = ["dep:tracing"]

[lints]
workspace = true
```

### 3.6. Крейты-заглушки для сборки workspace
Создать минимальные `Cargo.toml` и `src/lib.rs` для `boa_idb`, `boa_idb_sqlite`, `boa_idb_fs`, `boa_idb_memory`, `boa_idb_wpt`:
```toml
# crates/boa_idb/Cargo.toml
[package]
name = "boa_idb"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
boa_idb_core = { path = "../boa_idb_core" }

[lints]
workspace = true
```
(В `src/lib.rs` каждого крейта-заглушки поместить `//! Placeholder` и `#![deny(unsafe_code)]`).

---

## 4. Спецификация типов данных и сигнатур `boa_idb_core`

### 4.1. Модуль `error.rs`
Файл: `crates/boa_idb_core/src/error.rs`

```rust
use thiserror::Error;

/// Общие типы ошибок спецификации IndexedDB.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum IdbError {
    #[error("Transaction was aborted")]
    Abort,

    #[error("Constraint violation: {0}")]
    Constraint(String),

    #[error("Data clone error: {0}")]
    DataClone(String),

    #[error("Data error: {0}")]
    Data(String),

    #[error("Invalid access: {0}")]
    InvalidAccess(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Not readable: {0}")]
    NotReadable(String),

    #[error("Syntax error: {0}")]
    Syntax(String),

    #[error("Transaction is read-only")]
    ReadOnly,

    #[error("Transaction is inactive")]
    TransactionInactive,

    #[error("Unknown error: {0}")]
    Unknown(String),

    #[error("Version error: {0}")]
    Version(String),

    #[error("Quota exceeded: needed {needed} bytes, available {available} bytes")]
    QuotaExceeded { needed: u64, available: u64 },
}

/// Ошибки при валидации и кодировании ключей.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyError {
    #[error("Invalid key type: {0}")]
    InvalidType(String),

    #[error("Invalid key value: {0}")]
    InvalidValue(String),

    #[error("Key exceeds maximum allowed length of {limit} bytes (got {actual})")]
    KeyTooLarge { limit: usize, actual: usize },

    #[error("Key array nesting depth exceeds maximum allowed of {0}")]
    MaxDepthExceeded(usize),

    #[error("Corrupted key encoding: {0}")]
    InvalidEncoding(String),

    #[error("Unexpected end of key buffer")]
    UnexpectedEof,
}

/// Ошибки синтаксиса и применения KeyPath.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyPathError {
    #[error("Invalid key path syntax: {0}")]
    InvalidSyntax(String),

    #[error("Key path array cannot be empty")]
    EmptyArray,

    #[error("Cannot inject key into primitive value")]
    CannotInjectIntoPrimitive,
}

/// Ошибки сериализации/десериализации значений SCF-v1.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScError {
    #[error("Data clone error: {0}")]
    DataClone(String),

    #[error("Format version {0} is not supported (expected v1)")]
    UnsupportedVersion(u8),

    #[error("Invalid SCF magic bytes: expected 'IDB1'")]
    InvalidMagic,

    #[error("CRC32C checksum mismatch: expected {expected:#010x}, calculated {calculated:#010x}")]
    ChecksumMismatch { expected: u32, calculated: u32 },

    #[error("Value nesting depth exceeds maximum of {0}")]
    MaxDepthExceeded(usize),

    #[error("Serialized value size exceeds limit of {limit} bytes (got {actual})")]
    ValueTooLarge { limit: usize, actual: usize },

    #[error("Invalid memo reference ID: {0}")]
    InvalidMemoRef(usize),

    #[error("Unknown or reserved SCF tag: {0:#04x}")]
    UnknownTag(u8),

    #[error("Corrupted SCF payload: {0}")]
    CorruptedPayload(String),

    #[error("Unexpected end of SCF stream")]
    UnexpectedEof,
}
```

### 4.2. Модуль `limits.rs`
Файл: `crates/boa_idb_core/src/limits.rs`

```rust
/// Лимиты безопасности и квоты ядра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitConfig {
    /// Максимальный размер закодированного ключа в байтах (по умолчанию 1024).
    pub max_key_len: usize,
    /// Максимальная глубина вложенности массивов ключей (по умолчанию 32).
    pub max_key_depth: usize,
    /// Максимальный размер сериализованного значения в байтах (по умолчанию 64 МБ).
    pub max_value_len: usize,
    /// Максимальная глубина вложенности при клонировании значений (по умолчанию 512).
    pub max_clone_depth: usize,
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            max_key_len: 1024,
            max_key_depth: 32,
            max_value_len: 64 * 1024 * 1024,
            max_clone_depth: 512,
        }
    }
}
```

### 4.3. Модуль `key/utf16.rs`
Файл: `crates/boa_idb_core/src/key/utf16.rs`

```rust
use smallvec::SmallVec;
use std::fmt;

/// Строка из UTF-16 code units, способная хранить непарные суррогаты.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf16String {
    units: SmallVec<[u16; 16]>,
}

impl Utf16String {
    pub fn new() -> Self {
        Self { units: SmallVec::new() }
    }

    pub fn from_slice(slice: &[u16]) -> Self {
        Self { units: SmallVec::from_slice(slice) }
    }

    pub fn from_str(s: &str) -> Self {
        Self { units: s.encode_utf16().collect() }
    }

    pub fn as_slice(&self) -> &[u16] {
        &self.units
    }

    pub fn len(&self) -> usize {
        self.units.len()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    pub fn push(&mut self, unit: u16) {
        self.units.push(unit);
    }

    pub fn extend_from_slice(&mut self, slice: &[u16]) {
        self.units.extend_from_slice(slice);
    }
}

impl fmt::Debug for Utf16String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Utf16String(\"{}\")", String::from_utf16_lossy(&self.units))
    }
}

impl fmt::Display for Utf16String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf16_lossy(&self.units))
    }
}

impl From<&str> for Utf16String {
    fn from(s: &str) -> Self {
        Self::from_str(s)
    }
}

impl From<String> for Utf16String {
    fn from(s: String) -> Self {
        Self::from_str(&s)
    }
}

impl From<&[u16]> for Utf16String {
    fn from(slice: &[u16]) -> Self {
        Self::from_slice(slice)
    }
}

impl From<Vec<u16>> for Utf16String {
    fn from(vec: Vec<u16>) -> Self {
        Self { units: SmallVec::from_vec(vec) }
    }
}
```

### 4.4. Модуль `key/value.rs`
Файл: `crates/boa_idb_core/src/key/value.rs`

```rust
use crate::error::KeyError;
use crate::key::utf16::Utf16String;
use crate::limits::LimitConfig;

/// Ключ IndexedDB согласно §2.4 спецификации.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Number(f64),
    Date(f64),
    String(Utf16String),
    Binary(Vec<u8>),
    Array(Vec<Key>),
}

impl Key {
    /// Валидация ключа перед использованием (проверка на NaN и глубину).
    pub fn validate(&self, limits: &LimitConfig) -> Result<(), KeyError> {
        Self::validate_recursive(self, 0, limits)
    }

    fn validate_recursive(key: &Key, depth: usize, limits: &LimitConfig) -> Result<(), KeyError> {
        if depth > limits.max_key_depth {
            return Err(KeyError::MaxDepthExceeded(limits.max_key_depth));
        }
        match key {
            Key::Number(n) => {
                if n.is_nan() {
                    return Err(KeyError::InvalidValue("Number key cannot be NaN".into()));
                }
            }
            Key::Date(d) => {
                if d.is_nan() {
                    return Err(KeyError::InvalidValue("Date key cannot be NaN".into()));
                }
            }
            Key::String(_) | Key::Binary(_) => {}
            Key::Array(arr) => {
                for item in arr {
                    Self::validate_recursive(item, depth + 1, limits)?;
                }
            }
        }
        Ok(())
    }
}
```

### 4.5. Модуль `key/compare.rs`
Файл: `crates/boa_idb_core/src/key/compare.rs`

Сравнение двух ключей строго по W3C IndexedDB §2.4:
Порядок типов: `Number < Date < String < Binary < Array`.

```rust
use crate::key::value::Key;
use std::cmp::Ordering;

fn type_order(key: &Key) -> u8 {
    match key {
        Key::Number(_) => 1,
        Key::Date(_) => 2,
        Key::String(_) => 3,
        Key::Binary(_) => 4,
        Key::Array(_) => 5,
    }
}

pub fn compare_keys(a: &Key, b: &Key) -> Ordering {
    let order_a = type_order(a);
    let order_b = type_order(b);
    if order_a != order_b {
        return order_a.cmp(&order_b);
    }

    match (a, b) {
        (Key::Number(x), Key::Number(y)) => {
            let x_norm = if *x == 0.0 { 0.0 } else { *x };
            let y_norm = if *y == 0.0 { 0.0 } else { *y };
            x_norm.partial_cmp(&y_norm).unwrap_or(Ordering::Equal)
        }
        (Key::Date(x), Key::Date(y)) => {
            let x_norm = if *x == 0.0 { 0.0 } else { *x };
            let y_norm = if *y == 0.0 { 0.0 } else { *y };
            x_norm.partial_cmp(&y_norm).unwrap_or(Ordering::Equal)
        }
        (Key::String(x), Key::String(y)) => x.as_slice().cmp(y.as_slice()),
        (Key::Binary(x), Key::Binary(y)) => x.as_slice().cmp(y.as_slice()),
        (Key::Array(x), Key::Array(y)) => {
            let min_len = x.len().min(y.len());
            for i in 0..min_len {
                let ord = compare_keys(&x[i], &y[i]);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            x.len().cmp(&y.len())
        }
        _ => unreachable!(),
    }
}

impl Eq for Key {}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_keys(self, other)
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
```

---

## 5. Бинарный кодек ключей `KEY-v1` (Order-Preserving)

Файл: `crates/boa_idb_core/src/key/encode.rs`

### 5.1. Спецификация формата `KEY-v1`

| Тип ключа | Тег байта | Тело кодирования | Терминатор |
|---|---|---|---|
| `Number` | `0x10` | 8 байт IEEE-754 Big-Endian с инверсией знака | нет |
| `Date` | `0x20` | 8 байт IEEE-754 Big-Endian с инверсией знака | нет |
| `String` | `0x30` | CESU-8 байты, экранирование `0x00` -> `0x00 0xFF` | `0x00 0x00` |
| `Binary` | `0x40` | байты, экранирование `0x00` -> `0x00 0xFF` | `0x00 0x00` |
| `Array` | `0x50` | последовательность закодированных элементов | `0x00` |

### 5.2. Точный алгоритм преобразования `f64` (IEEE-754 Order-Preserving)
```rust
pub fn encode_f64_orderable(val: f64) -> [u8; 8] {
    // 1. Нормализация -0.0 в +0.0
    let v = if val == 0.0 { 0.0 } else { val };
    let bits = v.to_bits();
    // 2. Если знаковый бит взведен (отрицательное число) — инвертировать все биты.
    // Иначе — взвести знаковый бит.
    let ord = if (bits & (1u64 << 63)) != 0 {
        !bits
    } else {
        bits | (1u64 << 63)
    };
    ord.to_be_bytes()
}

pub fn decode_f64_orderable(bytes: [u8; 8]) -> f64 {
    let ord = u64::from_be_bytes(bytes);
    let bits = if (ord & (1u64 << 63)) != 0 {
        ord & !(1u64 << 63)
    } else {
        !ord
    };
    f64::from_bits(bits)
}
```

### 5.3. Алгоритм CESU-8 кодирования и экранирования строк
Каждый UTF-16 code unit (`u16`) кодируется независимо:
1. `0x0000..=0x007F`: 1 байт `cu as u8`.
2. `0x0080..=0x07FF`: 2 байта: `0xC0 | ((cu >> 6) as u8)`, `0x80 | ((cu & 0x3F) as u8)`.
3. `0x0800..=0xFFFF` (включая непарные суррогаты `0xD800..=0xDFFF`): 3 байта:
   - `0xE0 | ((cu >> 12) as u8)`
   - `0x80 | (((cu >> 6) & 0x3F) as u8)`
   - `0x80 | ((cu & 0x3F) as u8)`

Экранирование байтов:
- Каждый полученный байт `0x00` записывается как пара байтов `[0x00, 0xFF]`.
- Все остальные байты записываются без изменений.
- В конце строки записывается терминатор: `[0x00, 0x00]`.

### 5.4. Алгоритм декодирования CESU-8 строк
1. Читать поток байтов до терминатора `[0x00, 0x00]`.
2. Если встречается `[0x00, 0xFF]` — превращать в один байт `0x00`.
3. Если встретился одиночный `0x00` без `0xFF` или `0x00` — вернуть ошибку `KeyError::InvalidEncoding`.
4. Декодировать CESU-8 поток в `Vec<u16>`:
   - `0x00..=0x7F` -> 1 байт = code unit.
   - `0xC0..=0xDF` -> требует 1 последующий байт `0x80..=0xBF`.
   - `0xE0..=0xEF` -> требует 2 последующих байта `0x80..=0xBF`.
   - Иные байты -> `KeyError::InvalidEncoding`.

### 5.5. Кодирование Binary
1. Записать тег `0x40`.
2. Для каждого байта: если `0x00` -> записать `[0x00, 0xFF]`, иначе `byte`.
3. Записать терминатор `[0x00, 0x00]`.

### 5.6. Кодирование Array
1. Записать тег `0x50`.
2. Для каждого элемента массива рекурсивно закодировать ключ (тег + тело).
3. Записать терминатор `0x00`.

### 5.7. Публичные функции кодека `KEY-v1`
```rust
pub const TAG_NUMBER: u8 = 0x10;
pub const TAG_DATE: u8 = 0x20;
pub const TAG_STRING: u8 = 0x30;
pub const TAG_BINARY: u8 = 0x40;
pub const TAG_ARRAY: u8 = 0x50;
pub const TAG_ARRAY_TERMINATOR: u8 = 0x00;

pub fn encode_key(key: &Key, out: &mut Vec<u8>, limits: &LimitConfig) -> Result<(), KeyError> {
    key.validate(limits)?;
    let start_len = out.len();
    encode_key_internal(key, out, 0, limits)?;
    let added = out.len() - start_len;
    if added > limits.max_key_len {
        out.truncate(start_len);
        return Err(KeyError::KeyTooLarge {
            limit: limits.max_key_len,
            actual: added,
        });
    }
    Ok(())
}

pub fn decode_key(bytes: &[u8]) -> Result<(Key, usize), KeyError> {
    if bytes.is_empty() {
        return Err(KeyError::UnexpectedEof);
    }
    decode_key_internal(bytes, 0)
}
```

---

## 6. Модуль `key/path.rs` (KeyPath, Валидация, Извлечение и Инжекция)

Файл: `crates/boa_idb_core/src/key/path.rs`

### 6.1. Структура `KeyPath`
```rust
use crate::clone::scvalue::ScValue;
use crate::error::{KeyError, KeyPathError};
use crate::key::utf16::Utf16String;
use crate::key::value::Key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPath {
    Empty,
    Single(Utf16String),
    Array(Vec<Utf16String>),
}
```

### 6.2. Валидация синтаксиса `IdentifierName` (ECMA-262 / W3C §2.5)
Правила:
- Пустая строка `""` — валидна (соответствует `KeyPath::Empty`).
- Строка `KeyPath` состоит из идентификаторов, разделенных точкой: `id1.id2.id3`.
- Каждый идентификатор должен быть валидным `IdentifierName`:
  - Первый символ: Unicode `ID_Start`, или `$`, или `_`.
  - Последующие символы: Unicode `ID_Continue`, или `$`, или `_`, или `U+200C` (ZWNJ), или `U+200D` (ZWJ).
  - Экранирования (escape sequences `\uXXXX`) **ЗАПРЕЩЕНЫ**.
- Массив строк: каждая строка валидируется по правилам выше. Массив не может быть пустым.

### 6.3. Алгоритм извлечения ключа (`extract`)
```rust
impl KeyPath {
    pub fn extract(&self, value: &ScValue) -> Result<Option<Key>, KeyError> {
        match self {
            KeyPath::Empty => {
                // Преобразование самого ScValue в Key
                value.to_key()
            }
            KeyPath::Single(path) => extract_single_path(path, value),
            KeyPath::Array(paths) => {
                let mut result = Vec::with_capacity(paths.len());
                for p in paths {
                    match extract_single_path(p, value)? {
                        Some(k) => result.push(k),
                        None => return Ok(None), // Любой сбой в массиве дает None
                    }
                }
                Ok(Some(Key::Array(result)))
            }
        }
    }
}
```
Пошаговое извлечение `extract_single_path`:
1. Разбить `path` по разделителю `.` (символ `0x002E`).
2. Текущее значение `curr = value`.
3. Для каждого шага `step`:
   - Если `curr` — `ScValue::Object(map)`: искать `step` среди ключей. Если нет -> `return Ok(None)`.
   - Если `curr` — `ScValue::Array`: если `step == "length"`, взять длину массива как `ScValue::Number(len as f64)`.
   - Если `curr` — `ScValue::String`: если `step == "length"`, взять длину строки как `ScValue::Number(len as f64)`.
   - Иначе -> `return Ok(None)`.
4. В конце цепочки вызвать `curr.to_key()`.

### 6.4. Алгоритм инжекции ключа (`inject` и `can_inject`)
1. `can_inject(&self, target: &ScValue) -> bool`:
   - Проверяет возможность установки ключа по пути без изменения объекта. Если промежуточный узел — примитив (число, строка, булево и т.д.), возвращает `false`.
2. `inject(&self, target: &mut ScValue, key: &Key) -> Result<(), KeyPathError>`:
   - Применяется только к `KeyPath::Single`.
   - Для каждого промежуточного шага пути: если свойства нет, создает `ScValue::Object(IndexMap::new())`. Если на пути встретился примитив -> вернуть `KeyPathError::CannotInjectIntoPrimitive`.
   - В конечный шаг пути записывает `key.to_scvalue()`.

---

## 7. Промежуточное представление `ScValue` и кодек `SCF-v1`

Файлы:
- `crates/boa_idb_core/src/clone/scvalue.rs`
- `crates/boa_idb_core/src/clone/varint.rs`
- `crates/boa_idb_core/src/clone/encode.rs`
- `crates/boa_idb_core/src/clone/decode.rs`

### 7.1. Типы данных `ScValue`
```rust
use crate::key::utf16::Utf16String;
use indexmap::IndexMap;
use num_bigint::BigInt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScErrorKind {
    Error = 0,
    EvalError = 1,
    RangeError = 2,
    ReferenceError = 3,
    SyntaxError = 4,
    TypeError = 5,
    URIError = 6,
    AggregateError = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScTypedArrayKind {
    Int8 = 0,
    Uint8 = 1,
    Uint8Clamped = 2,
    Int16 = 3,
    Uint16 = 4,
    Int32 = 5,
    Uint32 = 6,
    Float32 = 7,
    Float64 = 8,
    BigInt64 = 9,
    BigUint64 = 10,
    Float16 = 11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegExpFlags {
    pub has_indices: bool,  // d
    pub global: bool,       // g
    pub ignore_case: bool,  // i
    pub multiline: bool,    // m
    pub dot_all: bool,      // s
    pub unicode: bool,      // u
    pub unicode_sets: bool, // v
    pub sticky: bool,       // y
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScErrorObject {
    pub kind: ScErrorKind,
    pub message: Option<Utf16String>,
    pub cause: Option<Box<ScValue>>,
    pub errors: Option<Vec<ScValue>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    BigInt(BigInt),
    String(Utf16String),
    Date(f64),
    RegExp {
        pattern: Utf16String,
        flags: RegExpFlags,
    },
    Array {
        elements: Vec<Option<ScValue>>, // None представляет дыры (holes)
        extra_props: Vec<(Utf16String, ScValue)>,
    },
    Object(IndexMap<Utf16String, ScValue>),
    Map(Vec<(ScValue, ScValue)>),
    Set(Vec<ScValue>),
    Error(ScErrorObject),
    ArrayBuffer {
        data: Vec<u8>,
        max_byte_length: Option<usize>, // resizable
    },
    TypedArray {
        kind: ScTypedArrayKind,
        byte_offset: usize,
        length: usize,
        buffer_memo_index: usize,
    },
    DataView {
        byte_offset: usize,
        byte_length: usize,
        buffer_memo_index: usize,
    },
    BoxedBoolean(bool),
    BoxedNumber(f64),
    BoxedString(Utf16String),
    BoxedBigInt(BigInt),
}
```

### 7.2. Кодирование Varint
Файл: `crates/boa_idb_core/src/clone/varint.rs`
- `encode_uvarint(val: u64, out: &mut Vec<u8>)` (LEB128 unsigned).
- `decode_uvarint(bytes: &[u8], offset: &mut usize) -> Result<u64, ScError>`.
- `encode_ivarint(val: i64, out: &mut Vec<u8>)` (ZigZag varint: `(n << 1) ^ (n >> 63)`).
- `decode_ivarint(bytes: &[u8], offset: &mut usize) -> Result<i64, ScError>`.

### 7.3. Бинарный формат `SCF-v1`

```
Заголовок (8 байт):
  magic:       [u8; 4] = b"IDB1"
  format_ver:  u8      = 1
  flags:       u8      = 0 (зарезервировано)
  reserved:    u16     = 0 (LE)

Тело (Payload):
  Последовательность байт тегов и значений.

Трейлер (4 байта):
  crc32c:      u32     = CRC32C сумма по (Заголовок + Тело), Little-Endian.
```

### 7.4. Таблица тегов `SCF-v1`

| Тег | Тип | Структура полезной нагрузки |
|---|---|---|
| `0x00` | Undefined | нет |
| `0x01` | Null | нет |
| `0x02` | False | нет |
| `0x03` | True | нет |
| `0x04` | Int32 | `i32` ZigZag Varint |
| `0x05` | Double | `f64` 8 байт Little-Endian |
| `0x06` | String | `uvarint(len_utf16)` + `len_utf16 * 2` байт UTF-16LE |
| `0x07` | BigInt | `sign: u8` (0=положительный/0, 1=отрицательный) + `uvarint(len)` + `len` байт величины LE |
| `0x08` | Date | `f64` 8 байт Little-Endian (мс) |
| `0x09` | RegExp | `uvarint(pattern_len)` + UTF-16LE + `uvarint(flags_bitfield)` |
| `0x0A` | Array | `uvarint(length)` + `uvarint(items_count)` + `items_count * (uvarint(index), Value)` + `uvarint(extra_props_count)` + `extra_props * (String, Value)` |
| `0x0B` | Object | `uvarint(count)` + `count * (String, Value)` |
| `0x0C` | Map | `uvarint(count)` + `count * (Value, Value)` |
| `0x0D` | Set | `uvarint(count)` + `count * Value` |
| `0x0E` | Error | `kind: u8` + `flags: u8` + message? + cause? + errors? |
| `0x0F` | ArrayBuffer | `uvarint(max_byte_length + 1 или 0)` + `uvarint(byte_len)` + сырые байты |
| `0x10` | TypedArray | `kind: u8` + `uvarint(byte_offset)` + `uvarint(length)` + `uvarint(buffer_memo_index)` |
| `0x11` | DataView | `uvarint(byte_offset)` + `uvarint(byte_length)` + `uvarint(buffer_memo_index)` |
| `0x12` | Boxed | `subtag: u8` (0=bool, 1=num, 2=str, 3=bigint) + полезная нагрузка |
| `0x13` | Memo Ref | `uvarint(memo_index)` |
| `0x14` | Hole | (дыра в разреженном массиве) |

### 7.5. Механизм Memo-таблицы (Ссылки и циклические графы)
Объекты, получающие номер в Memo-таблице при сериализации/десериализации:
`Array`, `Object`, `Map`, `Set`, `Error`, `ArrayBuffer`, `TypedArray`, `DataView`, `Boxed`.

1. **Сериализация:**
   - Перед кодированием объекта проверяется, есть ли его идентификатор/ссылка в таблице `memo_map: HashMap<ObjectId, usize>`.
   - Если есть -> записывается `TAG_MEMO_REF (0x13)` + `uvarint(memo_index)`.
   - Если нет -> новому объекту присваивается следующий `memo_index`, он вносится в таблицу, затем записывается его тег и содержимое.
2. **Десериализация:**
   - При обнаружении тега составного объекта декодер **ДО** разбора дочерних полей резервирует индекс в `memo_vec: Vec<ScValue>`.
   - При встрече `TAG_MEMO_REF` декодер возвращает ссылку на уже существующий объект по индексу.

### 7.6. Проверка целостности CRC32C
- При кодировании: `let checksum = crc32fast::Hasher::new(); ... checksum.finalize()` -> записывается в 4 байта в хвост.
- При декодировании: сначала вычисляется контрольная сумма по `data[0..data.len()-4]`, сравнивается с трейлером. Несовпадение -> `ScError::ChecksumMismatch`.

---

## 8. Тестовые векторы (Test Vectors)

Исполнитель **ОБЯЗАН** включить следующие точные тестовые векторы в unit-тесты:

### 8.1. Векторы `KEY-v1`

| Ключ | Ожидаемые байты (Hex) |
|---|---|
| `Key::Number(1.0)` | `10 BF F0 00 00 00 00 00 00` |
| `Key::Number(0.0)` | `10 80 00 00 00 00 00 00 00` |
| `Key::Number(-0.0)` | `10 80 00 00 00 00 00 00 00` (нормализован) |
| `Key::Number(-1.0)` | `10 40 0F FF FF FF FF FF FF` |
| `Key::Number(f64::NEG_INFINITY)` | `10 00 0F FF FF FF FF FF FF` |
| `Key::Number(f64::INFINITY)` | `10 FF F0 00 00 00 00 00 00` |
| `Key::Date(0.0)` | `20 80 00 00 00 00 00 00 00` |
| `Key::String("")` | `30 00 00` |
| `Key::String("a")` | `30 61 00 00` |
| `Key::String("\u{0000}")` | `30 00 FF 00 00` |
| `Key::String(Utf16String([0xD800]))` | `30 ED A0 80 00 00` |
| `Key::Binary(vec![0x00, 0x01])` | `40 00 FF 01 00 00` |
| `Key::Array(vec![])` | `50 00` |
| `Key::Array(vec![Key::Number(1.0)])`| `50 10 BF F0 00 00 00 00 00 00 00` |
| `Key::Array(vec![Key::Array(vec![])])` | `50 50 00 00` |

### 8.2. Инвариант сортировки ключей (Проверка порядка)
Тест обязан проверять строгую монотонность:
```rust
assert!(encode(&Key::Number(-1.0)) < encode(&Key::Number(0.0)));
assert!(encode(&Key::Number(0.0)) < encode(&Key::Number(1.0)));
assert!(encode(&Key::Number(1.0)) < encode(&Key::Date(0.0)));
assert!(encode(&Key::Date(0.0)) < encode(&Key::String("".into())));
assert!(encode(&Key::String("".into())) < encode(&Key::String("\u{0000}".into())));
assert!(encode(&Key::String("\u{0000}".into())) < encode(&Key::String("a".into())));
assert!(encode(&Key::String("a".into())) < encode(&Key::Binary(vec![])));
assert!(encode(&Key::Binary(vec![])) < encode(&Key::Array(vec![])));
assert!(encode(&Key::Array(vec![])) < encode(&Key::Array(vec![Key::Number(1.0)])));
```

---

## 9. Property-Based Тесты (Proptest)

Файлы:
- `crates/boa_idb_core/tests/key_proptests.rs`
- `crates/boa_idb_core/tests/scf_proptests.rs`

### 9.1. Инварианты для `key_proptests.rs`
1. **Изоморфизм порядка:** Для любых двух валидных случайных ключей `a` и `b`:
   `compare_keys(&a, &b) == encode_key(&a).cmp(&encode_key(&b))`
2. **Round-trip обратимость:** Для любого валидного случайного ключа `k`:
   `decode_key(&encode_key(&k)) == k`
3. **Генератор ключей:** обязан генерировать:
   - Специальные `f64`: `±0.0`, `±Infinity`, денормализованные числа, случайные `f64`.
   - Граничные строки: пустые, строки с `\0`, непарные суррогаты `0xD800..=0xDFFF`, символы `≥ U+E000`, длинные строки.
   - Бинарные буферы: пустые, с нулями, случайные.
   - Вложенные массивы ключей до глубины 5.

### 9.2. Инварианты для `scf_proptests.rs`
1. **Round-trip сериализации:** Для любого `ScValue`:
   `decode_scf(&encode_scf(&v)) == v`
2. **Устойчивость к мусору:** Для произвольных случайных байтов декодер `decode_scf` никогда не должен паниковать, а возвращать `Err(ScError)`.

---

## 10. Fuzz-таргеты (Cargo Fuzz / libFuzzer)

Создать в `fuzz/`:
1. `fuzz_key_decode`:
   ```rust
   #![no_main]
   use libfuzzer_sys::fuzz_target;
   use boa_idb_core::key::encode::decode_key;

   fuzz_target!(|data: &[u8]| {
       let _ = decode_key(data);
   });
   ```
2. `fuzz_scf_decode`:
   ```rust
   #![no_main]
   use libfuzzer_sys::fuzz_target;
   use boa_idb_core::clone::decode::decode_scf;
   use boa_idb_core::limits::LimitConfig;

   fuzz_target!(|data: &[u8]| {
       let _ = decode_scf(data, &LimitConfig::default());
   });
   ```
3. `fuzz_keypath_parse`:
   ```rust
   #![no_main]
   use libfuzzer_sys::fuzz_target;
   use boa_idb_core::key::path::KeyPath;

   fuzz_target!(|data: &str| {
       let _ = KeyPath::parse(data);
   });
   ```

---

## 11. Пошаговый план работ для исполнителя (Execution Steps)

1. **Шаг 1. Развертывание Workspace и конфигураций**
   - Создать корневые файлы `Cargo.toml`, `rust-toolchain.toml`, `clippy.toml`, `deny.toml`, `.gitignore`.
   - Создать структуру каталогов всех 6 крейтов workspace.
   - Проверить сборку пустого workspace командой `cargo check --workspace`.

2. **Шаг 2. Реализация `boa_idb_core::error` и `limits`**
   - Создать `error.rs` со всеми типами `IdbError`, `KeyError`, `KeyPathError`, `ScError`.
   - Создать `limits.rs` с `LimitConfig`.

3. **Шаг 3. Реализация модуля `key`**
   - Реализовать `key/utf16.rs` (`Utf16String`).
   - Реализовать `key/value.rs` (`Key`).
   - Реализовать `key/compare.rs` (`compare_keys`, `Ord`, `Eq`).
   - Реализовать `key/encode.rs` (кодирование/декодирование `KEY-v1`, нормализация float, CESU-8, экранирование).
   - Реализовать `key/range.rs` (`KeyRange`, `EncodedRange`).

4. **Шаг 4. Реализация модуля `key/path`**
   - Реализовать парсер `KeyPath` и валидатор `IdentifierName`.
   - Реализовать `extract`, `inject`, `can_inject`.

5. **Шаг 5. Реализация модуля `clone` (SCF-v1)**
   - Реализовать `clone/scvalue.rs` (`ScValue` и вспомогательные типы).
   - Реализовать `clone/varint.rs` (unsigned и ZigZag varint).
   - Реализовать `clone/encode.rs` (сериализация, заголовок, memo-таблица, CRC32C).
   - Реализовать `clone/decode.rs` (десериализация, валидация CRC32C, восстановление memo-ссылок).

6. **Шаг 6. Написание тестов**
   - Написать unit-тесты по всем векторам раздела 8.
   - Написать proptests по разделу 9.
   - Написать тесты на `KeyPath` и edge cases (`-0.0`, `NaN`, суррогаты).

7. **Шаг 7. Настройка Fuzzing**
   - Создать файлы в `fuzz/` и проверить компиляцию таргетов через `cargo check --manifest-path fuzz/Cargo.toml`.

8. **Шаг 8. Финальная верификация**
   - Запустить все команды из Раздела 12.

---

## 12. Команды валидации и критерии приёмки

Исполнитель сдает работу только тогда, когда **ВСЕ** нижеперечисленные команды выполняются с нулевым кодом возврата (EXIT CODE 0):

```powershell
# 1. Проверка форматирования
cargo fmt --all -- --check

# 2. Проверка линтером со строгими правилами
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 3. Запуск всех unit и integration тестов
cargo test --workspace

# 4. Запуск property-based тестов
cargo test --test key_proptests -- --nocapture
cargo test --test scf_proptests -- --nocapture

# 5. Проверка компиляции под wasm32 (требование ТЗ R2.1)
cargo check --package boa_idb_core --target wasm32-unknown-unknown

# 6. Проверка лицензий зависимостей
cargo deny check licenses

# 7. Проверка генерации документации
cargo doc --workspace --no-deps

# 8. Проверка покрытия кода (требование ТЗ R13.2: ≥ 90% для boa_idb_core)
cargo llvm-cov --package boa_idb_core --summary-only
```

### Чек-лист соответствия ТЗ:
- [ ] `R2.1`: Компиляция под `wasm32-unknown-unknown` без бэкендов.
- [ ] `R6.1.1`–`R6.1.3`: Строки в UTF-16, корректный `Ord` для `Key`, лимит глубины 32.
- [ ] `R6.3.1`–`R6.3.5`: Формат `KEY-v1`, нормализация `-0.0`, экранирование `0x00`, property-тест совпадения порядка байт и `cmp`.
- [ ] `R6.4.1`–`R6.4.8`: Формат `SCF-v1`, Memo-таблица, CRC32C, лимиты глубины 512 и размера 64 МБ, обработка `format_ver > 1`.
- [ ] `R6.5.1`–`R6.5.4`: Валидация IdentifierName, extract и inject ключей в `ScValue`.
- [ ] `R13.2`: Покрытие строк `boa_idb_core` ≥ 90%.
- [ ] Отсутствие любых `unsafe`, `unwrap()`, `expect()`, `panic!` в основном коде.
