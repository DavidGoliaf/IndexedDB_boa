# Несовместимости с API boa_engine 0.22

## Обзор

При реализации TASK-03 (JS-биндинги для IndexedDB) выявлен ряд несовместимостей
с текущей версией `boa_engine` 0.22.0. Документ описывает каждую проблему,
ожидаемое поведение и текущий workaround.

---

## 1. JsValue — отсутствие enum variants

**Проблема:** `JsValue` в boa 0.22 не имеет публичных enum variants
(`Boolean`, `Null`, `Undefined`, `Integer`, `Rational`, `String`, `BigInt`,
`Symbol`, `Object`). Pattern matching через `match val { JsValue::Boolean(b) => ... }`
не компилируется.

**Ожидаемое поведение (WebIDL):** Конвертеры должны различать типы значений
для корректной маршрутизации (Number → Key::Number, String → Key::String, etc.).

**Текущее решение:** Использование методов-предикатов:
```rust
if val.is_boolean() { ... }
if val.is_number() { ... }
if val.is_string() { ... }
if val.is_null() { ... }
if val.is_undefined() { ... }
if val.is_bigint() { ... }
if val.is_symbol() { ... }
if let Some(obj) = val.as_object() { ... }
```

**Проблема с этим решением:** Невозможно извлечь значение напрямую через
деструктуризацию. Для чисел приходится вызывать `val.to_number(context)?`,
для строк `val.to_string(context)?`, что может вернуть ошибку конвертации
вместо прямого доступа к данным.

---

## 2. JsString — отсутствие `as_slice()`

**Проблема:** `JsString` в boa 0.22 не имеет метода `as_slice() -> &[u16]`.
Невозможно получить доступ к UTF-16 code units напрямую.

**Ожидаемое поведение:** Для конвертации в `Utf16String` (boa_idb_core) нужен
доступ к UTF-16 представлению строки без потерь (суррогаты).

**Текущее решение:** Использование `js_str.to_std_string_escaped()` с последующей
конвертацией через `Utf16String::from_rust_str(&s)`.

**Проблема с этим решением:** `to_std_string_escaped()` конвертирует в UTF-8,
при этом непарные суррогаты заменяются на `U+FFFD`. Это теряет информацию,
что нарушает требование спецификации IndexedDB о сохранении суррогатов.

**Требуется:** Либо добавить `as_slice() -> &[u16]` в `JsString`, либо
реализовать `From<&JsString> for Utf16String` в boa_idb_core с прямым
копированием code units.

---

## 3. JsObject::new — изменённая сигнатура

**Проблема:** `JsObject::new()` в boa 0.22 принимает 3 аргумента:
```rust
pub fn new<O: Into<Option<JsObject>>>(
    root_shape: &RootShape,
    prototype: O,
    data: T,
) -> Self
```

**Ожидаемое поведение (из ТЗ):** Создание объекта с нативными данными:
```rust
let obj = JsObject::new(context, data);  // Не компилируется
```

**Текущее решение:** Использование `JsObject::with_data(data)` (не существует)
или `JsObject::from_proto_and_data(None, data)`.

**Проблема:** `from_proto_and_data` не устанавливает прототип по умолчанию.
Для создания объектов с правильным прототипом (например, `IDBRequest.prototype`)
нужен доступ к `RootShape` из контекста.

---

## 4. JsObject::has_property — требует Context

**Проблема:** `has_property()` в boa 0.22 принимает 2 аргумента:
```rust
pub fn has_property<K>(&self, key: K, context: &mut Context) -> JsResult<bool>
```

**Ожидаемое поведение:** Проверка наличия свойства без контекста:
```rust
if obj.has_property(js_string!("getTime")) { ... }  // Не компилируется
```

**Текущее решение:** Передача контекста:
```rust
if obj.has_property(js_string!("getTime"), context)? { ... }
```

**Проблема:** Это требует `&mut Context` в местах, где контекст может быть
недоступен (например, в замыканиях или при рекурсивной обработке).

---

## 5. JsObject::call_method — не существует

**Проблема:** Метод `call_method()` не существует в boa 0.22.

**Ожидаемое поведение:** Вызов метода объекта:
```rust
let result = obj.call_method(js_string!("getTime"), &[], context)?;
```

**Текущее решение:** Ручной вызов через `get` + `as_callable` + `call`:
```rust
let method = obj.get(js_string!("getTime"), context)?;
let result = method.as_callable()
    .ok_or_else(|| JsNativeError::typ().with_message("Not callable"))?
    .call(&JsValue::from(obj.clone()), &[], context)?;
```

**Проблема:** Громоздкий код, дублирование логики проверки callable во многих местах.

---

## 6. JsObject::length — не существует

**Проблема:** Метод `length()` не существует в boa 0.22.

**Ожидаемое поведение:** Получение длины массива:
```rust
let length = obj.length(context)?;  // Не компилируется
```

**Текущее решение:** Ручное получение свойства `length`:
```rust
let length = obj.get(js_string!("length"), context)?
    .to_number(context)? as u32;
```

**Проблема:** Дублирование кода во всех местах, где нужна длина массива.

---

## 7. JsArrayBuffer::data — не принимает Context

**Проблема:** `data()` в boa 0.22 не принимает аргументов:
```rust
pub fn data(&self) -> Option<GcRef<'_, [u8]>>
```

**Ожидаемое поведение (из ТЗ):** Получение данных буфера:
```rust
let data = bytes.data(context);  // Не компилируется
```

**Текущее решение:** Вызов без аргументов:
```rust
let data = bytes.data();
```

**Проблема:** Несовместимость с предыдущими версиями API.

---

## 8. JsArrayBuffer::from_byte_block — принимает AlignedVec

**Проблема:** `from_byte_block()` принимает `AlignedVec<u8>`, а не `Vec<u8>`:
```rust
pub fn from_byte_block(byte_block: AlignedVec<u8>, context: &mut Context) -> JsResult<Self>
```

**Ожидаемое поведение:** Создание ArrayBuffer из Vec<u8>:
```rust
let buf = JsArrayBuffer::from_byte_block(bytes.clone(), context)?;
```

**Текущее решение:** Конвертация через итератор:
```rust
let arr = JsArray::from_iter(
    bytes.iter().map(|&byte| JsValue::from(byte)),
    context,
);
```

**Проблема:** Создаёт Array вместо ArrayBuffer. Для настоящего ArrayBuffer
нужна конвертация `Vec<u8>` → `AlignedVec<u8>`.

---

## 9. JsDate — отсутствие call_method

**Проблема:** `JsDate` не имеет `call_method()`.

**Ожидаемое поведение:** Создание Date и вызов setTime:
```rust
let date = JsDate::new(context)?;
date.call_method(js_string!("setTime"), &[JsValue::from(d)], context)?;
```

**Текущее решение:** Создание Date через конструктор globalThis:
```rust
let date_constructor = context.global_object().get(js_string!("Date"), context)?;
let date = date_constructor.as_callable()
    .ok_or_else(|| JsNativeError::typ())?
    .call(&JsValue::undefined(), &[JsValue::from(d)], context)?;
```

**Проблема:** Нетипизированный вызов, потеря информации о типе Date.

---

## 10. JsNativeError → JsValue — отсутствие From

**Проблема:** `JsNativeError` не реализует `From<JsNativeError> for JsValue`.

**Ожидаемое поведение:** Создание DOMException:
```rust
let err: JsValue = JsNativeError::error()
    .with_message("...")
    .into();  // Не компилируется
```

**Текущее решение:** Возврат через `Err(err.into())` (JsNativeError → JsError).

**Проблема:** Невозможно создать JS-объект ошибки и вернуть его как JsValue
(для dispatchEvent и обработчиков).

---

## 11. JsObject::downcast_ref — возвращает Option вместо Result

**Проблема:** `downcast_ref()` в boa 0.22 возвращает `Option<GcRef<T>>`,
а не `Result`.

**Ожидаемое поведение (из ТЗ):** Проверка типа с ошибкой:
```rust
if let Ok(data) = obj.downcast_ref::<EventTargetData>() { ... }
```

**Текущее решение:** Использование `Some`:
```rust
if let Some(data) = obj.downcast_ref::<EventTargetData>() { ... }
```

**Проблема:** Несовместимость с предыдущими версиями API.

---

## 12. Trace для типов boa_idb_core

**Проблема:** Типы из `boa_idb_core` (`Key`, `ScValue`, `Utf16String`,
`StorageKey`, `TxnMode`, `EncodedRange`) не реализуют `Trace` из `boa_gc`.

**Ожидаемое поведение:** Хранение этих типов в native-данных JS-объектов:
```rust
#[derive(Trace, Finalize, JsData)]
pub struct IdBKeyRangeData {
    pub lower: Option<Key>,  // Key не реализует Trace
}
```

**Текущее решение:** Использование `#[unsafe_ignore_trace]` для всех полей
с типами из boa_idb_core, хранение данных как `Vec<u8>` (encoded bytes).

**Проблема:** Потеря типизации. Вместо `Key` хранятся байты, что требует
дополнительного кодирования/декодирования при каждом обращении.

**Требуется:** Либо реализовать `Trace` для типов boa_idb_core (невозможно
без unsafe, так как типы не содержат GC-managed данных), либо использовать
обёртки с `#[unsafe_ignore_trace]`.

---

## 13. Copy для типов с Trace/Finalize

**Проблема:** Типы, реализующие `Trace` (через derive), не могут реализовывать
`Copy`, так как `Trace` подразумевает деструктор.

**Ожидаемое поведение:**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Trace, Finalize)]
pub enum TxnModeJs { ... }  // Не компилируется
```

**Текущее решение:** Удаление `Copy` из derive:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Trace, Finalize)]
pub enum TxnModeJs { ... }
```

**Проблема:** Необходимость клонирования вместо копирования для простых enum.

---

## 14. JsObject::own_property_keys — вместо enumerable_own_property_keys

**Проблема:** Метод `enumerable_own_property_keys()` не существует.

**Ожидаемое поведение:** Получение только enumerable свойств для structured clone.

**Текущее решение:** Использование `own_property_keys()`, который возвращает
все свойства (включая не-enumerable).

**Проблема:** Нарушение спецификации structured clone, которая требует
обрабатывать только enumerable own properties.

---

## Рекомендации

1. **Добавить `JsString::as_slice() -> &[u16]`** — критично для IndexedDB
   (сохранение суррогатов).

2. **Добавить `JsObject::call_method()`** — упростит大量 кода конвертеров.

3. **Добавить `JsObject::length()`** — часто используемая операция.

4. **Реализовать `From<JsNativeError> for JsValue`** — нужно для DOM shim.

5. **Добавить `JsArrayBuffer::from_bytes(Vec<u8>)`** — convenience метод.

6. **Рассмотреть возможность `#[derive(Trace)]` для типов без GC-данных**
   — позволит хранить `Key`, `ScValue` в native-данных без unsafe.
