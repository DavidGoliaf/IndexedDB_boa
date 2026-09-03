# Руководство по интеграции с `boa_engine` 0.22: Разбор особенностей API, исправление ошибок и готовые инструкции

| Метаданные | Значение |
|---|---|
| **Документ** | Руководство по интеграции с JS-движком Boa (версия 0.22.0) |
| **Целевой продукт** | `boa-idb` (L1 биндинги, DOM-шим, конвертеры типов) |
| **Целевой тулчейн** | Rust 1.91.0 (edition 2024), `boa_engine` 0.22.0, `boa_gc` 0.22.0 |
| **Статус** | Нормативное руководство для разработчиков и AI-агентов |

---

## Обзор

При разработке JS-биндингов для IndexedDB (M3) был выявлен ряд архитектурных и синтаксических особенностей `boa_engine 0.22`. Непонимание некоторых внутренних механизмов движка привело к временным «воркараундам», нарушающим спецификацию W3C (например, потеря UTF-16 суррогатов при вызове `to_std_string_escaped()` или подмена `ArrayBuffer` обычным массивом `Array`).

Данный документ содержит **исчерпывающий разбор всех 14 пунктов**, детальное объяснение работы API Boa 0.22, устранение дефектов и готовые к копированию фрагменты кода.

---

## 1. `JsValue` — безопасное извлечение типов без аллокаций и без `Context`

### Описание ситуации
В Boa 0.22 тип `JsValue` использует внутреннюю оптимизацию **NaN-boxing** (значения упакованы в 64-битное число), поэтому публичные варианты `enum` отсутствуют. Попытка писать `match val { JsValue::Number(n) => ... }` не компилируется. Попытка вызывать `val.to_number(context)?` или `val.to_string(context)?` выполняет JS-коэрсию типов (type coercion) и требует передачи `&mut Context`, что неприемлемо при проверке исходного типа значения.

### Реальность API Boa 0.22
Тип `JsValue` предоставляет легковесные, не аллоцирующие и не требующие контекста методы-аксессоры `as_*()`:
- `val.as_number() -> Option<f64>`
- `val.as_string() -> Option<JsString>`
- `val.as_boolean() -> Option<bool>`
- `val.as_bigint() -> Option<JsBigInt>`
- `val.as_object() -> Option<JsObject>`
- `val.as_symbol() -> Option<JsSymbol>`
- `val.as_i32() -> Option<i32>`
- `val.is_null() -> bool`
- `val.is_undefined() -> bool`

### Готовое решение для агента
```rust
use boa_engine::JsValue;
use boa_idb_core::error::KeyError;
use boa_idb_core::key::value::Key;
use boa_idb_core::key::utf16::Utf16String;

pub fn extract_primitive_key(val: &JsValue) -> Result<Option<Key>, KeyError> {
    if let Some(n) = val.as_number() {
        if n.is_nan() {
            return Err(KeyError::InvalidValue("Key cannot be NaN".into()));
        }
        return Ok(Some(Key::Number(n)));
    }
    
    if let Some(s) = val.as_string() {
        return Ok(Some(Key::String(Utf16String::from_slice(&s.to_vec()))));
    }
    
    if val.is_null() || val.is_undefined() {
        return Err(KeyError::InvalidType("null or undefined is not a valid key".into()));
    }
    
    Ok(None) // Переход к объектным типам (Date, Array, ArrayBuffer)
}
```

---

## 2. `JsString` — 100% сохранение UTF-16 суррогатов (без `U+FFFD`)

### Описание ситуации
JS-строки в IndexedDB могут содержать непарные суррогаты (unpaired surrogates `0xD800..=0xDFFF`). Использование стандартных методов конвертации в UTF-8 (`to_std_string_escaped()`, `to_std_string_lossy()`) заменяет суррогаты символом замещения `U+FFFD`, что приводит к безвозвратной порче ключей и падению тестов W3C.

### Реальность API Boa 0.22
`JsString` внутри хранит кодовые единицы UTF-16 и предоставляет прямой доступ к ним:
- `js_str.to_vec() -> Vec<u16>` — возвращает точный вектор `u16` кодовых единиц без потерь;
- `js_str.iter() -> Iter<'a>` — итератор по `u16`;
- `js_str.code_unit_at(index) -> Option<u16>`;
- `JsString::from(slice: &[u16])` — обратное создание `JsString` из слайса UTF-16.

### Готовое решение для агента
```rust
use boa_engine::JsString;
use boa_idb_core::key::utf16::Utf16String;

/// Конвертация JsString -> Utf16String без потерь суррогатов.
#[inline]
pub fn js_string_to_utf16(js_str: &JsString) -> Utf16String {
    Utf16String::from_slice(&js_str.to_vec())
}

/// Конвертация Utf16String -> JsString.
#[inline]
pub fn utf16_to_js_string(s: &Utf16String) -> JsString {
    JsString::from(s.as_slice())
}
```

---

## 3. `JsObject::new` — создание объектов с нативными данными и прототипами

### Описание ситуации
В Boa 0.22 изменилась сигнатура низкоуровневого конструктора `JsObject::new`: теперь он требует `&RootShape` для оптимизации свойств.

### Реальность API Boa 0.22
Для создания экземпляров пользовательских классов с native-данными (`JsData`) и правильной цепочкой прототипов (`IDBRequest.prototype`, `IDBDatabase.prototype`) используется метод:
```rust
JsObject::from_proto_and_data_with_shared_shape(
    context.root_shape(),
    Some(prototype),
    native_data,
)
```

### Готовое решение для агента
```rust
use boa_engine::{Context, JsObject};

pub fn create_native_object<T: boa_engine::JsData>(
    prototype: JsObject,
    data: T,
    context: &mut Context,
) -> JsObject {
    JsObject::from_proto_and_data_with_shared_shape(
        context.root_shape(),
        Some(prototype),
        data,
    )
}
```

---

## 4. `JsObject::has_property` и передача `Context`

### Описание ситуации
`has_property` требует передачи `&mut Context`, так как в JS проверка наличия свойства может вызывать Proxy-ловушки `has` или геттеры в цепочке прототипов.

### Готовое решение для агента
Передавать `context` в местах взаимодействия со средой JS. Для проверки только собственных свойств без вызова прототипов использовать `obj.has_own_property(key, context)?`.

---

## 5. Вызов методов JS-объектов (`call_method`)

### Описание ситуации
Часто требуется вызвать JS-метод объекта по строковому имени (например, `"dispatchEvent"` или `"handleEvent"`).

### Готовое решение для агента
Использовать трейт-расширение `JsObjectExt`:
```rust
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue};

pub trait JsObjectExt {
    fn call_method(&self, name: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue>;
}

impl JsObjectExt for JsObject {
    fn call_method(&self, name: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let property = self.get(name, context)?;
        let callable = property.as_callable().ok_or_else(|| {
            JsNativeError::typ().with_message(format!("Property '{name}' is not callable"))
        })?;
        callable.call(&JsValue::from(self.clone()), args, context)
    }
}
```

---

## 6. Получение длины массива `JsArray::length`

### Описание ситуации
`JsObject::length()` отсутствует, так как свойство `length` специфично для массивов и функций.

### Готовое решение для агента
Для массивов использовать типизированную обертку `JsArray`:
```rust
use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsObject, JsResult};

pub fn get_array_length(obj: &JsObject, context: &mut Context) -> JsResult<u64> {
    let arr = JsArray::from_object(obj.clone())?;
    arr.length(context)
}
```

---

## 7. `JsArrayBuffer::data` — получение данных буфера

### Описание ситуации
В Boa 0.22 метод `JsArrayBuffer::data(&self)` не принимает контекст и возвращает `Option<GcRef<'_, [u8]>>`.

### Готовое решение для агента
```rust
use boa_engine::object::builtins::JsArrayBuffer;

pub fn get_array_buffer_bytes(buf: &JsArrayBuffer) -> Option<Vec<u8>> {
    buf.data().map(|d| d.to_vec())
}
```

---

## 8. `JsArrayBuffer::from_byte_block` — создание настоящего `ArrayBuffer`

### Описание ситуации
`from_byte_block` требует `AlignedVec<u8>`. Из-за этого ранее ошибочно создавался `JsArray` вместо `JsArrayBuffer`, что нарушало спецификацию WebIDL.

### Готовое решение для агента
`AlignedVec` создается из любого итератора или среза за $O(N)$:
```rust
use boa_engine::object::builtins::{AlignedVec, JsArrayBuffer};
use boa_engine::{Context, JsResult};

pub fn create_js_array_buffer(bytes: &[u8], context: &mut Context) -> JsResult<JsArrayBuffer> {
    let block = AlignedVec::from_iter(0, bytes.iter().copied());
    JsArrayBuffer::from_byte_block(block, context)
}
```

---

## 9. `JsDate` — типизированная работа со временем

### Описание ситуации
Создание Date через вызов конструктора `globalThis.Date` избыточно и неэффективно.

### Готовое решение для агента
В `boa_engine` встроен типизированный `JsDate`:
```rust
use boa_engine::object::builtins::JsDate;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue};

/// Создание даты с заданным timestamp.
pub fn create_js_date(timestamp_ms: f64, context: &mut Context) -> JsResult<JsDate> {
    let date = JsDate::new(context);
    date.set_time(JsValue::from(timestamp_ms), context)?;
    Ok(date)
}

/// Извлечение миллисекунд из объекта даты.
pub fn extract_date_timestamp(obj: &JsObject, context: &mut Context) -> JsResult<f64> {
    let date = JsDate::from_object(obj.clone())?;
    let time_val = date.get_time(context)?;
    time_val.as_number().ok_or_else(|| {
        JsNativeError::typ()
            .with_message("Date getTime did not return a valid number")
            .into()
    })
}
```

---

## 10. `JsNativeError` → `JsValue` — генерация объектов ошибок

### Описание ситуации
Для передачи объекта ошибки в JS (например, в событие `request.onerror`) требуется получить экземпляр `JsValue`, а не Rust `Result::Err`.

### Готовое решение для агента
Использовать метод `to_opaque()` структуры `JsError`:
```rust
use boa_engine::{Context, JsError, JsNativeError, JsValue};

#[inline]
pub fn native_error_to_js_value(err: JsNativeError, context: &mut Context) -> JsValue {
    JsError::from(err).to_opaque(context)
}
```

---

## 11. `JsObject::downcast_ref` — брендирование и проверка классов

### Описание ситуации
`downcast_ref()` возвращает `Option<GcRef<T>>`.

### Готовое решение для агента
Использовать идиоматичный `match` или `if let`:
```rust
use boa_engine::{Context, JsNativeError, JsObject, JsResult};
use boa_gc::GcRef;

pub fn with_native_data<T: boa_engine::JsData, R, F>(
    obj: &JsObject,
    class_name: &str,
    f: F,
) -> JsResult<R>
where
    F: FnOnce(&T) -> JsResult<R>,
{
    match obj.downcast_ref::<T>() {
        Some(data) => f(&data),
        None => Err(JsNativeError::typ()
            .with_message(format!("'this' is not an instance of {class_name}"))
            .into()),
    }
}
```

---

## 12. Хранение типов ядра в `JsData` (`#[unsafe_ignore_trace]`)

### Описание ситуации
Типы из `boa_idb_core` (`Key`, `IdbError`, `StoreId`, `LimitConfig`) не содержат GC-указателей. Ранее ошибочно предполагалось, что из-за отсутствия `Trace` их нельзя хранить в `JsData`, и данные сериализовались в `Vec<u8>`.

### Реальность API `boa_gc`
Макрос `boa_gc::Trace` поддерживает атрибут **`#[unsafe_ignore_trace]`** для любых полей, не содержащих внутри себя GC-управляемых указателей (`JsObject`, `JsValue`).

### Готовое решение для агента
```rust
use boa_engine::JsData;
use boa_gc::{Finalize, Trace};
use boa_engine::JsObject;
use boa_idb_core::key::value::Key;
use boa_idb_core::proto::{StoreId, TxnId};

#[derive(Debug, Trace, Finalize, JsData)]
pub struct IDBKeyRangeData {
    #[unsafe_ignore_trace]
    pub lower: Option<Key>,
    #[unsafe_ignore_trace]
    pub upper: Option<Key>,
    #[unsafe_ignore_trace]
    pub lower_open: bool,
    #[unsafe_ignore_trace]
    pub upper_open: bool,
}

#[derive(Debug, Trace, Finalize, JsData)]
pub struct IDBObjectStoreData {
    #[unsafe_ignore_trace]
    pub store_id: StoreId,
    // Ссылки на связанные JS-объекты трассируются GC:
    pub transaction: JsObject,
}
```

---

## 13. `Copy` для типов с `Trace` и `Finalize`

### Описание ситуации
Типы с derive-макросом `Trace` не могут реализовывать маркерный трейт `Copy`, так как `Trace` подразумевает потенциальное наличие деструктора.

### Готовое решение для агента
Для легковесных enum использовать `Clone` вместо `Copy`.

---

## 14. Получение только `enumerable` свойств для Structured Clone

### Описание ситуации
`own_property_keys()` возвращает все ключи, включая неперечислимые (`enumerable: false`). Спецификация HTML Structured Clone требует клонировать только собственные `enumerable` свойства.

### Готовое решение для агента
```rust
use boa_engine::{Context, JsObject, JsResult, JsString};

pub fn get_enumerable_own_property_keys(
    obj: &JsObject,
    context: &mut Context,
) -> JsResult<Vec<JsString>> {
    let keys = obj.own_property_keys(context)?;
    let mut result = Vec::new();
    
    for key in keys {
        if let Some(desc) = obj.get_own_property_descriptor(&key, context)? {
            if desc.enumerable().unwrap_or(false) {
                if let Some(s) = key.as_string() {
                    result.push(s.clone());
                }
            }
        }
    }
    
    Ok(result)
}
```

---

## 15. Готовый файл-утилита `crates/boa_idb/src/convert/boa_compat.rs`

Скопируйте данный код в файл `crates/boa_idb/src/convert/boa_compat.rs`:

```rust
//! Вспомогательные функции и расширения для работы с API boa_engine 0.22.

use boa_engine::object::builtins::{AlignedVec, JsArray, JsArrayBuffer, JsDate};
use boa_engine::{Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue};
use boa_idb_core::key::utf16::Utf16String;

/// Расширения методов для работы с JsObject.
pub trait JsObjectExt {
    /// Вызывает метод объекта по строковому имени.
    fn call_method(&self, name: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue>;

    /// Возвращает список только enumerable собственных строковых ключей.
    fn get_enumerable_keys(&self, context: &mut Context) -> JsResult<Vec<JsString>>;

    /// Получает длину массива (если объект является массивом).
    fn array_length(&self, context: &mut Context) -> JsResult<u64>;
}

impl JsObjectExt for JsObject {
    fn call_method(&self, name: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        let property = self.get(name, context)?;
        let callable = property.as_callable().ok_or_else(|| {
            JsNativeError::typ().with_message(format!("Property '{name}' is not callable"))
        })?;
        callable.call(&JsValue::from(self.clone()), args, context)
    }

    fn get_enumerable_keys(&self, context: &mut Context) -> JsResult<Vec<JsString>> {
        let keys = self.own_property_keys(context)?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(desc) = self.get_own_property_descriptor(&key, context)? {
                if desc.enumerable().unwrap_or(false) {
                    if let Some(s) = key.as_string() {
                        result.push(s.clone());
                    }
                }
            }
        }
        Ok(result)
    }

    fn array_length(&self, context: &mut Context) -> JsResult<u64> {
        let arr = JsArray::from_object(self.clone())?;
        arr.length(context)
    }
}

/// Конвертирует JsString в Utf16String без потерь суррогатов.
#[inline]
pub fn js_string_to_utf16(js_str: &JsString) -> Utf16String {
    Utf16String::from_slice(&js_str.to_vec())
}

/// Конвертирует Utf16String в JsString.
#[inline]
pub fn utf16_to_js_string(s: &Utf16String) -> JsString {
    JsString::from(s.as_slice())
}

/// Создает настоящий JsArrayBuffer из байтов.
#[inline]
pub fn create_array_buffer(bytes: &[u8], context: &mut Context) -> JsResult<JsArrayBuffer> {
    let block = AlignedVec::from_iter(0, bytes.iter().copied());
    JsArrayBuffer::from_byte_block(block, context)
}

/// Создает JsDate из миллисекунд timestamp.
#[inline]
pub fn create_date(timestamp_ms: f64, context: &mut Context) -> JsResult<JsDate> {
    let date = JsDate::new(context);
    date.set_time(JsValue::from(timestamp_ms), context)?;
    Ok(date)
}

/// Преобразует JsNativeError в непрозрачный JS Error объект (JsValue).
#[inline]
pub fn native_error_to_js_value(err: JsNativeError, context: &mut Context) -> JsValue {
    JsError::from(err).to_opaque(context)
}
```
