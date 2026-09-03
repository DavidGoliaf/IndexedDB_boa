//! Вспомогательные функции и расширения для работы с API boa_engine 0.22.

use boa_engine::object::builtins::{AlignedVec, JsArray, JsArrayBuffer, JsDate};
use boa_engine::{
    Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, js_string,
};
use boa_gc::Trace;
use boa_idb_core::key::utf16::Utf16String;

/// Расширения методов для работы с JsObject.
pub trait JsObjectExt {
    /// Вызывает метод объекта по строковому имени.
    fn call_method(&self, name: &str, args: &[JsValue], context: &mut Context)
    -> JsResult<JsValue>;

    /// Получает длину массива (если объект является массивом).
    fn array_length(&self, context: &mut Context) -> JsResult<u64>;
}

impl JsObjectExt for JsObject {
    fn call_method(
        &self,
        name: &str,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let property = self.get(js_string!(name), context)?;
        let callable = property.as_callable().ok_or_else(|| {
            JsNativeError::typ().with_message(format!("Property '{name}' is not callable"))
        })?;
        callable.call(&JsValue::from(self.clone()), args, context)
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
    JsError::from(err)
        .into_opaque(context)
        .unwrap_or_else(|_| JsValue::undefined())
}

/// Создает native объект с прототипом и данными.
pub fn create_native_object<T: Trace + boa_engine::JsData + 'static>(
    prototype: JsObject,
    data: T,
    _context: &mut Context,
) -> JsObject {
    JsObject::from_proto_and_data(Some(prototype), data)
}

/// Проверяет бренд объекта и возвращает native-данные.
pub fn with_native_data<T: Trace + boa_engine::JsData + 'static, R, F>(
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

/// Извлекает примитивный ключ из JsValue (Number, String).
pub fn extract_primitive_key(val: &JsValue) -> Option<boa_idb_core::key::value::Key> {
    if let Some(n) = val.as_number() {
        if !n.is_nan() {
            return Some(boa_idb_core::key::value::Key::Number(n));
        }
    }
    if let Some(s) = val.as_string() {
        return Some(boa_idb_core::key::value::Key::String(js_string_to_utf16(
            &s,
        )));
    }
    None
}
