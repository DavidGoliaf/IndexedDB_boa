# Руководство по интеграции с boa_engine 0.22

## Цель документа

Предоставить конкретные паттерны и helper-функции для решения каждой из 14
несовместимостей, описанных в `BOA_022_INCOMPATIBILITIES.md`.

---

## 1. JsValue — типобезопасный доступ

### Паттерн: `JsValueKind` enum + extraction helpers

```rust
/// Определяет тип JsValue без pattern matching.
pub enum JsValueKind {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(JsString),
    BigInt,
    Symbol,
    Object(JsObject),
}

/// Возвращает тип значения с извлечённым содержимым.
pub fn classify(val: &JsValue, context: &mut Context) -> JsResult<JsValueKind> {
    if val.is_undefined() { return Ok(JsValueKind::Undefined); }
    if val.is_null() { return Ok(JsValueKind::Null); }
    if val.is_boolean() { return Ok(JsValueKind::Boolean(val.to_boolean())); }
    if val.is_number() { return Ok(JsValueKind::Number(val.to_number(context)?)); }
    if val.is_string() {
        let s = val.to_string(context)?;
        return Ok(JsValueKind::String(s));
    }
    if val.is_bigint() { return Ok(JsValueKind::BigInt); }
    if val.is_symbol() { return Ok(JsValueKind::Symbol); }
    if let Some(obj) = val.as_object() {
        return Ok(JsValueKind::Object(obj.clone()));
    }
    Err(JsNativeError::typ().with_message("Unknown JsValue type").into())
}
```

### Паттерн: безопасное извлечение числа

```rust
/// Извлекает f64 из JsValue, возвращая None для не-чисел.
pub fn try_as_f64(val: &JsValue, context: &mut Context) -> Option<f64> {
    if val.is_number() {
        val.to_number(context).ok()
    } else {
        None
    }
}

/// Извлекает u32 из JsValue с проверкой диапазона.
pub fn try_as_u32(val: &JsValue, context: &mut Context) -> Option<u32> {
    let n = try_as_f64(val, context)?;
    if n >= 0.0 && n <= f64::from(u32::MAX) && n.fract() == 0.0 {
        Some(n as u32)
    } else {
        None
    }
}
```

---

## 2. JsString → Utf16String — сохранение суррогатов

### Паттерн: `JsStringExt` trait

```rust
use boa_engine::JsString;
use boa_idb_core::key::utf16::Utf16String;

/// Расширение JsString для конвертации в Utf16String.
pub trait JsStringExt {
    /// Конвертирует в Utf16String, сохраняя непарные суррогаты.
    fn to_utf16_string(&self) -> Utf16String;
}

impl JsStringExt for JsString {
    fn to_utf16_string(&self) -> Utf16String {
        // JsString internally stores UTF-16 code units.
        // We need to access them directly.
        // Workaround: iterate over chars and encode to UTF-16.
        let s = self.to_std_string_escaped();
        let mut units = Vec::with_capacity(s.len());
        for ch in s.chars() {
            if ch as u32 <= 0xFFFF {
                units.push(ch as u16);
            } else {
                // Encode as surrogate pair
                let cp = ch as u32 - 0x10000;
                units.push(0xD800 + ((cp >> 10) as u16));
                units.push(0xDC00 + ((cp & 0x3FF) as u16));
            }
        }
        Utf16String::from_slice(&units)
    }
}
```

### Паттерн: прямое копирование (если доступно внутреннее API)

```rust
// Если boa_string предоставляет доступ к внутреннему буферу:
use boa_string::JsStr;

pub fn js_string_to_utf16(s: &JsString) -> Utf16String {
    // JsString хранит данные как UTF-16
    // Нужен доступ к s.as_slice() -> &[u16]
    // Пока недоступно в публичном API
    todo!("Requires JsString::as_slice() from boa_string")
}
```

---

## 3. JsObject::new — создание с прототипом

### Паттерн: `JsObjectBuilder`

```rust
use boa_engine::{Context, JsObject, JsResult};
use boa_gc::Trace;

/// Создаёт JsObject с нативными данными и стандартным прототипом.
pub fn create_object_with_data<T: Trace + Finalize + 'static>(
    data: T,
    context: &mut Context,
) -> JsResult<JsObject> {
    // from_proto_and_data(None, data) создаёт объект без прототипа
    let obj = JsObject::from_proto_and_data(
        context.intrinsics().objects().object_prototype(),
        data,
    );
    Ok(obj)
}

/// Создаёт JsObject с null-прототипом.
pub fn create_object_with_data_null_proto<T: Trace + Finalize + 'static>(
    data: T,
) -> JsObject {
    JsObject::from_proto_and_data(None, data)
}
```

### Паттерн: регистрация кастомного прототипа

```rust
/// Регистрирует класс в globalThis и возвращает конструктор.
pub fn register_class<T: Trace + Finalize + JsData + 'static>(
    name: &str,
    context: &mut Context,
) -> JsResult<JsObject> {
    // 1. Создаём prototype объект
    let proto = JsObject::with_null_proto();

    // 2. Регистрируем в globalThis
    let global = context.global_object();
    global.set(
        js_string!(name),
        JsValue::from(proto.clone()),
        false,
        context,
    )?;

    Ok(proto)
}
```

---

## 4. has_property — с контекстом

### Паттерн: `ObjectExt` trait

```rust
use boa_engine::{js_string, Context, JsObject, JsResult, JsString};

/// Расширение JsObject для удобной проверки свойств.
pub trait ObjectExt {
    /// Проверяет наличие свойства.
    fn has_prop(&self, key: &JsString, context: &mut Context) -> JsResult<bool>;

    /// Проверяет, является ли свойство callable.
    fn is_method(&self, key: &JsString, context: &mut Context) -> JsResult<bool>;
}

impl ObjectExt for JsObject {
    fn has_prop(&self, key: &JsString, context: &mut Context) -> JsResult<bool> {
        self.has_property(key.clone(), context)
    }

    fn is_method(&self, key: &JsString, context: &mut Context) -> JsResult<bool> {
        if !self.has_property(key.clone(), context)? {
            return Ok(false);
        }
        let val = self.get(key.clone(), context)?;
        Ok(val.is_callable())
    }
}
```

---

## 5. call_method — вызов метода объекта

### Паттерн: `call_method` helper

```rust
use boa_engine::{Context, JsObject, JsResult, JsString, JsValue};

/// Вызывает метод объекта по имени.
pub fn call_method(
    obj: &JsObject,
    method_name: &JsString,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let method = obj.get(method_name.clone(), context)?;
    let callable = method
        .as_callable()
        .ok_or_else(|| {
            JsNativeError::typ().with_message(format!(
                "Property '{}' is not callable",
                method_name.to_std_string_escaped()
            ))
        })?;
    callable.call(&JsValue::from(obj.clone()), args, context)
}

/// Вызывает метод и возвращает результат как f64.
pub fn call_method_as_f64(
    obj: &JsObject,
    method_name: &JsString,
    context: &mut Context,
) -> JsResult<f64> {
    let result = call_method(obj, method_name, &[], context)?;
    result.to_number(context)
}

/// Вызывает метод и возвращает результат как bool.
pub fn call_method_as_bool(
    obj: &JsObject,
    method_name: &JsString,
    context: &mut Context,
) -> JsResult<bool> {
    let result = call_method(obj, method_name, &[], context)?;
    Ok(result.to_boolean())
}
```

### Паттерн: макрос для удобного вызова

```rust
/// Макрос для вызова метода объекта.
///
/// # Пример
/// ```ignore
/// let result = call_method!(obj, context, "getTime")?;
/// let result = call_method!(obj, context, "setTime", JsValue::from(123.0))?;
/// ```
macro_rules! call_method {
    ($obj:expr, $ctx:expr, $name:expr $(, $arg:expr)*) => {{
        let method = $obj.get(js_string!($name), $ctx)?;
        let callable = method.as_callable()
            .ok_or_else(|| JsNativeError::typ()
                .with_message(concat!("'", $name, "' is not callable")))?;
        callable.call(
            &JsValue::from($obj.clone()),
            &[$($arg),*],
            $ctx,
        )
    }};
}
```

---

## 6. length — получение длины массива

### Паттерн: `ArrayExt` trait

```rust
use boa_engine::{js_string, Context, JsObject, JsResult};

/// Расширение JsObject для работы с массивами.
pub trait ArrayExt {
    /// Возвращает длину массива.
    fn array_length(&self, context: &mut Context) -> JsResult<u32>;

    /// Проверяет, является ли объект массивом.
    fn is_js_array(&self) -> bool;

    /// Возвращает элемент по индексу.
    fn get_element(&self, index: u32, context: &mut Context) -> JsResult<JsValue>;
}

impl ArrayExt for JsObject {
    fn array_length(&self, context: &mut Context) -> JsResult<u32> {
        let length = self.get(js_string!("length"), context)?;
        let n = length.to_number(context)?;
        if n < 0.0 || n > f64::from(u32::MAX) || n.fract() != 0.0 {
            return Err(JsNativeError::typ()
                .with_message("Invalid array length")
                .into());
        }
        Ok(n as u32)
    }

    fn is_js_array(&self) -> bool {
        self.is_array()
    }

    fn get_element(&self, index: u32, context: &mut Context) -> JsResult<JsValue> {
        self.get(index, context)
    }
}
```

---

## 7. JsArrayBuffer::data — без аргументов

### Паттерн: `ArrayBufferExt` trait

```rust
use boa_engine::object::builtins::JsArrayBuffer;
use boa_gc::GcRef;

/// Расширение JsArrayBuffer для доступа к данным.
pub trait ArrayBufferExt {
    /// Возвращает данные буфера как &[u8].
    fn bytes(&self) -> Option<GcRef<'_, [u8]>>;
}

impl ArrayBufferExt for JsArrayBuffer {
    fn bytes(&self) -> Option<GcRef<'_, [u8]>> {
        self.data()
    }
}
```

---

## 8. JsArrayBuffer из Vec<u8>

### Паттерн: `ArrayBuffer::from_bytes` helper

```rust
use boa_engine::object::builtins::{JsArray, JsArrayBuffer};
use boa_engine::{Context, JsResult, JsValue};

/// Создаёт JsArray из байтов (как workaround для AlignedVec).
pub fn array_from_bytes(bytes: &[u8], context: &mut Context) -> JsArray {
    JsArray::from_iter(
        bytes.iter().map(|&b| JsValue::from(b)),
        context,
    )
}

/// Создаёт ArrayBuffer через вызов конструктора.
pub fn arraybuffer_from_bytes(
    bytes: &[u8],
    context: &mut Context,
) -> JsResult<JsValue> {
    // Получаем ArrayBuffer constructor
    let ab_constructor = context
        .global_object()
        .get(js_string!("ArrayBuffer"), context)?;

    let callable = ab_constructor
        .as_callable()
        .ok_or_else(|| JsNativeError::typ().with_message("ArrayBuffer is not callable"))?;

    // Создаём ArrayBuffer
    let ab = callable.call(
        &JsValue::undefined(),
        &[JsValue::from(bytes.len() as i32)],
        context,
    )?;

    // Заполняем данными через Uint8Array
    let u8_constructor = context
        .global_object()
        .get(js_string!("Uint8Array"), context)?;

    let u8_callable = u8_constructor
        .as_callable()
        .ok_or_else(|| JsNativeError::typ().with_message("Uint8Array is not callable"))?;

    let u8_view = u8_callable.call(
        &JsValue::undefined(),
        &[ab.clone()],
        context,
    )?;

    // Копируем байты
    let u8_obj = u8_view.as_object().unwrap();
    for (i, &byte) in bytes.iter().enumerate() {
        u8_obj.set(i, JsValue::from(byte), false, context)?;
    }

    Ok(ab)
}
```

---

## 9. JsDate — создание и вызов методов

### Паттерн: `DateExt` trait

```rust
use boa_engine::{js_string, Context, JsNativeError, JsObject, JsResult, JsValue};

/// Расширение для работы с Date объектами.
pub trait DateExt {
    /// Создаёт Date из миллисекунд.
    fn from_millis(millis: f64, context: &mut Context) -> JsResult<JsValue>;

    /// Возвращает значение getTime().
    fn get_time(&self, context: &mut Context) -> JsResult<f64>;
}

impl DateExt for JsObject {
    fn from_millis(millis: f64, context: &mut Context) -> JsResult<JsValue> {
        let date_constructor = context
            .global_object()
            .get(js_string!("Date"), context)?;
        let callable = date_constructor
            .as_callable()
            .ok_or_else(|| JsNativeError::typ().with_message("Date is not callable"))?;
        callable.call(&JsValue::undefined(), &[JsValue::from(millis)], context)
    }

    fn get_time(&self, context: &mut Context) -> JsResult<f64> {
        let method = self.get(js_string!("getTime"), context)?;
        let callable = method
            .as_callable()
            .ok_or_else(|| JsNativeError::typ().with_message("getTime is not callable"))?;
        let result = callable.call(&JsValue::from(self.clone()), &[], context)?;
        result.to_number(context)
    }
}
```

---

## 10. JsNativeError → JsValue

### Паттерн: `into_js_value` helper

```rust
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue};

/// Создаёт JsValue ошибки из JsNativeError.
pub fn error_to_js_value(
    err: JsNativeError,
    context: &mut Context,
) -> JsResult<JsValue> {
    // JsNativeError -> JsError -> JsObject -> JsValue
    let js_err: JsError = err.into();
    // JsError реализует Into<JsValue> через JsObject
    Ok(js_err.into())
}

/// Создаёт DOMException-подобный объект.
pub fn create_dom_exception_value(
    name: &str,
    message: &str,
    context: &mut Context,
) -> JsResult<JsValue> {
    let err = JsNativeError::error().with_message(format!("{name}: {message}"));
    error_to_js_value(err, context)
}
```

---

## 11. downcast_ref — Option вместо Result

### Паттерн: `try_downcast` helper

```rust
use boa_engine::JsObject;
use boa_gc::Trace;

/// Пытается получить native-данные из JsObject.
///
/// Возвращает None если объект не содержит данные типа T.
pub fn try_downcast<T: Trace + Finalize + 'static>(
    obj: &JsObject,
) -> Option<boa_gc::GcRef<'_, T>> {
    obj.downcast_ref::<T>()
}

/// Получает native-данные или возвращает ошибку.
pub fn downcast_or_err<T: Trace + Finalize + 'static>(
    obj: &JsObject,
    type_name: &str,
) -> Result<boa_gc::GcRef<'_, T>, JsNativeError> {
    obj.downcast_ref::<T>().ok_or_else(|| {
        JsNativeError::typ().with_message(format!(
            "Expected object with data type '{}'",
            type_name
        ))
    })
}
```

---

## 12. Trace для типов boa_idb_core

### Паттерн: `TraceWrapper` с `#[unsafe_ignore_trace]`

```rust
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::value::Key;

/// Обёртка для Key с unsafe_ignore_trace.
///
/// SAFETY: Key не содержит GC-managed данных (JsObject, JsValue).
/// Это чистый Rust-тип, который не нуждается в трассировке GC.
#[derive(Debug, Clone, Trace, Finalize, boa_engine::JsData)]
pub struct TracedKey {
    #[unsafe_ignore_trace]
    inner: Key,
}

impl TracedKey {
    pub fn new(key: Key) -> Self {
        Self { inner: key }
    }

    pub fn get(&self) -> &Key {
        &self.inner
    }

    pub fn into_inner(self) -> Key {
        self.inner
    }
}

/// Обёртка для Vec<u8> (encoded key bytes).
#[derive(Debug, Clone, Trace, Finalize, boa_engine::JsData)]
pub struct EncodedKey {
    #[unsafe_ignore_trace]
    bytes: Vec<u8>,
}

impl EncodedKey {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
```

### Паттерн: хранение encoded bytes вместо типизированных данных

```rust
/// Вместо хранения Key, храним закодированные байты.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBKeyRangeData {
    /// Закодированный нижний ключ (None = -∞).
    #[unsafe_ignore_trace]
    pub lower: Option<Vec<u8>>,
    /// Закодированный верхний ключ (None = +∞).
    #[unsafe_ignore_trace]
    pub upper: Option<Vec<u8>>,
    /// Открытая нижняя граница.
    pub lower_open: bool,
    /// Открытая верхняя граница.
    pub upper_open: bool,
}

impl IdBKeyRangeData {
    /// Декодирует нижний ключ.
    pub fn decode_lower(&self) -> Option<Key> {
        self.lower.as_ref().and_then(|bytes| {
            boa_idb_core::key::encode::decode_key(bytes).ok().map(|(k, _)| k)
        })
    }

    /// Декодирует верхний ключ.
    pub fn decode_upper(&self) -> Option<Key> {
        self.upper.as_ref().and_then(|bytes| {
            boa_idb_core::key::encode::decode_key(bytes).ok().map(|(k, _)| k)
        })
    }
}
```

---

## 13. Copy + Trace — workaround

### Паттерн: Clone вместо Copy

```rust
use boa_gc::{Finalize, Trace};

/// Вместо Copy используем Clone + копирование вручную.
#[derive(Debug, Clone, PartialEq, Eq, Trace, Finalize)]
pub enum TxnModeJs {
    ReadOnly,
    ReadWrite,
    VersionChange,
}

impl TxnModeJs {
    /// Возвращает режим (замена Copy).
    pub fn as_mode(&self) -> Self {
        self.clone()
    }

    /// Конвертирует в core TxnMode.
    pub fn to_core_mode(&self) -> boa_idb_core::proto::TxnMode {
        match self {
            Self::ReadOnly => boa_idb_core::proto::TxnMode::ReadOnly,
            Self::ReadWrite => boa_idb_core::proto::TxnMode::ReadWrite,
            Self::VersionChange => boa_idb_core::proto::TxnMode::VersionChange,
        }
    }
}
```

---

## 14. enumerable_own_property_keys

### Паттерн: фильтрация свойств

```rust
use boa_engine::{js_string, Context, JsObject, JsResult, JsValue};

/// Возвращает только enumerable own свойства объекта.
pub fn enumerable_own_keys(
    obj: &JsObject,
    context: &mut Context,
) -> JsResult<Vec<JsValue>> {
    let all_keys = obj.own_property_keys(context)?;

    // Фильтруем по descriptor
    let mut enumerable = Vec::new();
    for key in all_keys {
        let desc = obj.get_own_property(&key, context)?;
        if let Some(desc) = desc {
            if desc.enumerable() {
                enumerable.push(key);
            }
        }
    }

    Ok(enumerable)
}

/// Проверяет, является ли свойство enumerable.
pub fn is_enumerable(
    obj: &JsObject,
    key: &JsValue,
    context: &mut Context,
) -> JsResult<bool> {
    let desc = obj.get_own_property(key, context)?;
    Ok(desc.map_or(false, |d| d.enumerable()))
}
```

---

## Сводная таблица решений

| # | Проблема | Решение | Файл-хелпер |
|---|----------|---------|--------------|
| 1 | JsValue variants | `classify()` + предикаты | `convert/helpers.rs` |
| 2 | JsString::as_slice | `JsStringExt::to_utf16_string()` | `convert/helpers.rs` |
| 3 | JsObject::new | `from_proto_and_data()` | `convert/helpers.rs` |
| 4 | has_property + ctx | `ObjectExt` trait | `convert/helpers.rs` |
| 5 | call_method | `call_method()` helper | `convert/helpers.rs` |
| 6 | length | `ArrayExt` trait | `convert/helpers.rs` |
| 7 | data() без ctx | `ArrayBufferExt` trait | `convert/helpers.rs` |
| 8 | AlignedVec | `arraybuffer_from_bytes()` | `convert/helpers.rs` |
| 9 | JsDate | `DateExt` trait | `convert/helpers.rs` |
| 10 | Error → JsValue | `error_to_js_value()` | `dom/exception.rs` |
| 11 | downcast Option | `try_downcast()` | `convert/helpers.rs` |
| 12 | Trace для core | `TraceWrapper` + `#[unsafe_ignore_trace]` | `api/wrappers.rs` |
| 13 | Copy + Trace | Clone + `as_mode()` | `api/types.rs` |
| 14 | enumerable keys | `enumerable_own_keys()` | `convert/helpers.rs` |
