//! Intermediate value representation for structured cloning.

use crate::error::KeyError;
use crate::key::utf16::Utf16String;
use crate::key::value::Key;
use indexmap::IndexMap;
use num_bigint::BigInt;

/// Error kind for `Error` objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ScErrorKind {
    /// Generic Error.
    Error = 0,
    /// EvalError.
    EvalError = 1,
    /// RangeError.
    RangeError = 2,
    /// ReferenceError.
    ReferenceError = 3,
    /// SyntaxError.
    SyntaxError = 4,
    /// TypeError.
    TypeError = 5,
    /// URIError.
    URIError = 6,
    /// AggregateError.
    AggregateError = 7,
}

impl ScErrorKind {
    /// Converts a `u8` tag to an `ScErrorKind`.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Error),
            1 => Some(Self::EvalError),
            2 => Some(Self::RangeError),
            3 => Some(Self::ReferenceError),
            4 => Some(Self::SyntaxError),
            5 => Some(Self::TypeError),
            6 => Some(Self::URIError),
            7 => Some(Self::AggregateError),
            _ => None,
        }
    }
}

/// TypedArray element kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ScTypedArrayKind {
    /// Int8Array.
    Int8 = 0,
    /// Uint8Array.
    Uint8 = 1,
    /// Uint8ClampedArray.
    Uint8Clamped = 2,
    /// Int16Array.
    Int16 = 3,
    /// Uint16Array.
    Uint16 = 4,
    /// Int32Array.
    Int32 = 5,
    /// Uint32Array.
    Uint32 = 6,
    /// Float32Array.
    Float32 = 7,
    /// Float64Array.
    Float64 = 8,
    /// BigInt64Array.
    BigInt64 = 9,
    /// BigUint64Array.
    BigUint64 = 10,
    /// Float16Array.
    Float16 = 11,
}

impl ScTypedArrayKind {
    /// Converts a `u8` tag to an `ScTypedArrayKind`.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Int8),
            1 => Some(Self::Uint8),
            2 => Some(Self::Uint8Clamped),
            3 => Some(Self::Int16),
            4 => Some(Self::Uint16),
            5 => Some(Self::Int32),
            6 => Some(Self::Uint32),
            7 => Some(Self::Float32),
            8 => Some(Self::Float64),
            9 => Some(Self::BigInt64),
            10 => Some(Self::BigUint64),
            11 => Some(Self::Float16),
            _ => None,
        }
    }
}

/// RegExp flags as individual boolean fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct RegExpFlags {
    /// `d` — has indices.
    pub has_indices: bool,
    /// `g` — global.
    pub global: bool,
    /// `i` — ignore case.
    pub ignore_case: bool,
    /// `m` — multiline.
    pub multiline: bool,
    /// `s` — dot all.
    pub dot_all: bool,
    /// `u` — unicode.
    pub unicode: bool,
    /// `v` — unicode sets.
    pub unicode_sets: bool,
    /// `y` — sticky.
    pub sticky: bool,
}

impl RegExpFlags {
    /// Converts flags to a bitfield for serialization.
    ///
    /// **Note:** Limited to 8 flags (fits in `u8`). If more flags are added,
    /// the encoding/decoding must be updated to use a wider type.
    #[allow(clippy::wrong_self_convention)]
    pub fn to_bitfield(self) -> u8 {
        let mut bits = 0u8;
        if self.has_indices {
            bits |= 1 << 0;
        }
        if self.global {
            bits |= 1 << 1;
        }
        if self.ignore_case {
            bits |= 1 << 2;
        }
        if self.multiline {
            bits |= 1 << 3;
        }
        if self.dot_all {
            bits |= 1 << 4;
        }
        if self.unicode {
            bits |= 1 << 5;
        }
        if self.unicode_sets {
            bits |= 1 << 6;
        }
        if self.sticky {
            bits |= 1 << 7;
        }
        bits
    }

    /// Converts a bitfield back to flags.
    pub fn from_bitfield(bits: u8) -> Self {
        Self {
            has_indices: bits & (1 << 0) != 0,
            global: bits & (1 << 1) != 0,
            ignore_case: bits & (1 << 2) != 0,
            multiline: bits & (1 << 3) != 0,
            dot_all: bits & (1 << 4) != 0,
            unicode: bits & (1 << 5) != 0,
            unicode_sets: bits & (1 << 6) != 0,
            sticky: bits & (1 << 7) != 0,
        }
    }
}

/// An Error object in the structured clone format.
#[derive(Debug, Clone, PartialEq)]
pub struct ScErrorObject {
    /// The error kind.
    pub kind: ScErrorKind,
    /// The error message (if any).
    pub message: Option<Utf16String>,
    /// The `cause` property (if any).
    pub cause: Option<Box<ScValue>>,
    /// The `errors` property (for `AggregateError`).
    pub errors: Option<Vec<ScValue>>,
}

/// Intermediate value representation for structured cloning.
///
/// This type is independent of any JavaScript engine and can be
/// serialized to/from the SCF-v1 binary format.
#[derive(Debug, Clone, PartialEq)]
pub enum ScValue {
    /// `undefined`.
    Undefined,
    /// `null`.
    Null,
    /// Boolean value.
    Boolean(bool),
    /// Numeric value (f64).
    Number(f64),
    /// BigInt value.
    BigInt(BigInt),
    /// String value (UTF-16 code units).
    String(Utf16String),
    /// Date value (milliseconds since epoch).
    Date(f64),
    /// Regular expression.
    RegExp {
        /// The pattern string.
        pattern: Utf16String,
        /// The flags.
        flags: RegExpFlags,
    },
    /// Array (possibly sparse with holes).
    Array {
        /// Array elements. `None` represents holes.
        elements: Vec<Option<ScValue>>,
        /// Extra named properties beyond the array indices.
        extra_props: Vec<(Utf16String, ScValue)>,
    },
    /// Plain object (ordered string-keyed properties).
    Object(IndexMap<Utf16String, ScValue>),
    /// Map (key-value pairs).
    Map(Vec<(ScValue, ScValue)>),
    /// Set (values).
    Set(Vec<ScValue>),
    /// Error object.
    Error(ScErrorObject),
    /// ArrayBuffer (raw bytes).
    ArrayBuffer {
        /// The raw data.
        data: Vec<u8>,
        /// Maximum byte length for resizable buffers (`None` = not resizable).
        max_byte_length: Option<usize>,
    },
    /// TypedArray (references a buffer via memo index).
    TypedArray {
        /// Element type.
        kind: ScTypedArrayKind,
        /// Byte offset into the buffer.
        byte_offset: usize,
        /// Number of elements.
        length: usize,
        /// Memo index of the backing `ArrayBuffer`.
        buffer_memo_index: usize,
    },
    /// DataView (references a buffer via memo index).
    DataView {
        /// Byte offset into the buffer.
        byte_offset: usize,
        /// Byte length of the view.
        byte_length: usize,
        /// Memo index of the backing `ArrayBuffer`.
        buffer_memo_index: usize,
    },
    /// Boxed boolean (`new Boolean(true)`).
    BoxedBoolean(bool),
    /// Boxed number (`new Number(42)`).
    BoxedNumber(f64),
    /// Boxed string (`new String("hello")`).
    BoxedString(Utf16String),
    /// Boxed bigint (`new BigInt(42n)`).
    BoxedBigInt(BigInt),
}

impl ScValue {
    /// Attempts to convert this `ScValue` to an IndexedDB `Key`.
    ///
    /// Returns `Ok(None)` if the value cannot be converted to a key.
    pub fn to_key(&self) -> Result<Option<Key>, KeyError> {
        match self {
            ScValue::Number(n) => {
                if n.is_nan() {
                    return Err(KeyError::InvalidValue("NaN cannot be a key".into()));
                }
                Ok(Some(Key::Number(*n)))
            }
            ScValue::Date(d) => {
                if d.is_nan() {
                    return Err(KeyError::InvalidValue(
                        "Invalid Date cannot be a key".into(),
                    ));
                }
                Ok(Some(Key::Date(*d)))
            }
            ScValue::String(s) => Ok(Some(Key::String(s.clone()))),
            ScValue::BigInt(_) => Err(KeyError::InvalidType("BigInt cannot be a key".into())),
            ScValue::Boolean(_)
            | ScValue::Undefined
            | ScValue::Null
            | ScValue::RegExp { .. }
            | ScValue::Error(_)
            | ScValue::ArrayBuffer { .. }
            | ScValue::TypedArray { .. }
            | ScValue::DataView { .. }
            | ScValue::BoxedBoolean(_)
            | ScValue::BoxedNumber(_)
            | ScValue::BoxedString(_)
            | ScValue::BoxedBigInt(_) => Err(KeyError::InvalidType(format!(
                "{:?} cannot be a key",
                std::mem::discriminant(self)
            ))),
            ScValue::Array { elements, .. } => {
                let mut keys = Vec::with_capacity(elements.len());
                for elem in elements {
                    match elem {
                        Some(v) => match v.to_key()? {
                            Some(k) => keys.push(k),
                            None => return Ok(None),
                        },
                        None => return Ok(None), // holes → failure
                    }
                }
                Ok(Some(Key::Array(keys)))
            }
            ScValue::Object(_) | ScValue::Map(_) | ScValue::Set(_) => Err(KeyError::InvalidType(
                "Object/Map/Set cannot be a key".into(),
            )),
        }
    }

    /// Converts an IndexedDB `Key` back to an `ScValue`.
    pub fn from_key(key: &Key) -> Self {
        match key {
            Key::Number(n) => ScValue::Number(*n),
            Key::Date(d) => ScValue::Date(*d),
            Key::String(s) => ScValue::String(s.clone()),
            Key::Binary(b) => ScValue::ArrayBuffer {
                data: b.clone(),
                max_byte_length: None,
            },
            Key::Array(arr) => {
                let elements: Vec<Option<ScValue>> =
                    arr.iter().map(|k| Some(ScValue::from_key(k))).collect();
                ScValue::Array {
                    elements,
                    extra_props: Vec::new(),
                }
            }
        }
    }
}
