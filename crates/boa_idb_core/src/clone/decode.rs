//! SCF-v1 binary decoder for `ScValue`.

use crate::clone::scvalue::{RegExpFlags, ScErrorKind, ScErrorObject, ScTypedArrayKind, ScValue};
use crate::clone::varint::{decode_ivarint, decode_uvarint};
use crate::error::ScError;
use crate::key::utf16::Utf16String;
use crate::limits::LimitConfig;
use indexmap::IndexMap;

// SCF-v1 tag constants
const TAG_UNDEFINED: u8 = 0x00;
const TAG_NULL: u8 = 0x01;
const TAG_FALSE: u8 = 0x02;
const TAG_TRUE: u8 = 0x03;
const TAG_INT32: u8 = 0x04;
const TAG_DOUBLE: u8 = 0x05;
const TAG_STRING: u8 = 0x06;
const TAG_BIGINT: u8 = 0x07;
const TAG_DATE: u8 = 0x08;
const TAG_REGEXP: u8 = 0x09;
const TAG_ARRAY: u8 = 0x0A;
const TAG_OBJECT: u8 = 0x0B;
const TAG_MAP: u8 = 0x0C;
const TAG_SET: u8 = 0x0D;
const TAG_ERROR: u8 = 0x0E;
const TAG_ARRAYBUFFER: u8 = 0x0F;
const TAG_TYPEDARRAY: u8 = 0x10;
const TAG_DATAVIEW: u8 = 0x11;
const TAG_BOXED: u8 = 0x12;
const TAG_MEMO_REF: u8 = 0x13;
const TAG_HOLE: u8 = 0x14;

// Boxed subtags
const BOXED_BOOL: u8 = 0;
const BOXED_NUM: u8 = 1;
const BOXED_STR: u8 = 2;
const BOXED_BIGINT: u8 = 3;

/// SCF-v1 magic bytes.
const SCF_MAGIC: &[u8; 4] = b"IDB1";

/// Decodes an `ScValue` from the SCF-v1 binary format.
///
/// Validates the header, CRC32C trailer, and format version.
pub fn decode_scf(data: &[u8], limits: &LimitConfig) -> Result<ScValue, ScError> {
    // Minimum size: 8 (header) + 1 (at least one tag) + 4 (crc) = 13
    if data.len() < 12 {
        return Err(ScError::CorruptedPayload("SCF data too short".into()));
    }

    // Validate magic
    if &data[0..4] != SCF_MAGIC {
        return Err(ScError::InvalidMagic);
    }

    // Validate version
    let format_ver = data[4];
    if format_ver != 1 {
        return Err(ScError::UnsupportedVersion(format_ver));
    }

    // Validate CRC32C
    let payload_end = data.len() - 4;
    let expected_crc = u32::from_le_bytes([
        data[payload_end],
        data[payload_end + 1],
        data[payload_end + 2],
        data[payload_end + 3],
    ]);
    let calculated_crc = crc32fast::hash(&data[0..payload_end]);
    if expected_crc != calculated_crc {
        return Err(ScError::ChecksumMismatch {
            expected: expected_crc,
            calculated: calculated_crc,
        });
    }

    // Validate payload size against limit (DoS protection)
    let payload_len = payload_end - 8;
    if payload_len > limits.max_value_len {
        return Err(ScError::ValueTooLarge {
            limit: limits.max_value_len,
            actual: payload_len,
        });
    }

    // Decode body
    let mut decoder = ScfDecoder::new(&data[8..payload_end], limits);
    decoder.decode_value()
}

struct ScfDecoder<'a> {
    data: &'a [u8],
    offset: usize,
    limits: &'a LimitConfig,
    memo_vec: Vec<ScValue>,
}

impl<'a> ScfDecoder<'a> {
    fn new(data: &'a [u8], limits: &'a LimitConfig) -> Self {
        Self {
            data,
            offset: 0,
            limits,
            memo_vec: Vec::new(),
        }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    fn read_byte(&mut self) -> Result<u8, ScError> {
        if self.offset >= self.data.len() {
            return Err(ScError::UnexpectedEof);
        }
        let b = self.data[self.offset];
        self.offset += 1;
        Ok(b)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ScError> {
        if self.offset + n > self.data.len() {
            return Err(ScError::UnexpectedEof);
        }
        let slice = &self.data[self.offset..self.offset + n];
        self.offset += n;
        Ok(slice)
    }

    fn read_f64_le(&mut self) -> Result<f64, ScError> {
        let bytes = self.read_bytes(8)?;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(bytes);
        Ok(f64::from_le_bytes(buf))
    }

    fn decode_value(&mut self) -> Result<ScValue, ScError> {
        self.decode_value_depth(0)
    }

    fn decode_value_depth(&mut self, depth: usize) -> Result<ScValue, ScError> {
        if depth > self.limits.max_clone_depth {
            return Err(ScError::MaxDepthExceeded(self.limits.max_clone_depth));
        }
        if self.remaining() == 0 {
            return Err(ScError::UnexpectedEof);
        }

        let tag = self.read_byte()?;
        match tag {
            TAG_UNDEFINED => Ok(ScValue::Undefined),
            TAG_NULL => Ok(ScValue::Null),
            TAG_FALSE => Ok(ScValue::Boolean(false)),
            TAG_TRUE => Ok(ScValue::Boolean(true)),
            TAG_INT32 => {
                let val = decode_ivarint(self.data, &mut self.offset)?;
                Ok(ScValue::Number(val as f64))
            }
            TAG_DOUBLE => {
                let n = self.read_f64_le()?;
                Ok(ScValue::Number(n))
            }
            TAG_STRING => self.decode_string_value(),
            TAG_BIGINT => self.decode_bigint_value(),
            TAG_DATE => {
                let d = self.read_f64_le()?;
                Ok(ScValue::Date(d))
            }
            TAG_REGEXP => self.decode_regexp_value(),
            TAG_ARRAY => self.decode_array_value(depth),
            TAG_OBJECT => self.decode_object_value(depth),
            TAG_MAP => self.decode_map_value(depth),
            TAG_SET => self.decode_set_value(depth),
            TAG_ERROR => self.decode_error_value(depth),
            TAG_ARRAYBUFFER => self.decode_arraybuffer_value(),
            TAG_TYPEDARRAY => self.decode_typedarray_value(),
            TAG_DATAVIEW => self.decode_dataview_value(),
            TAG_BOXED => self.decode_boxed_value(depth),
            TAG_MEMO_REF => {
                let idx = decode_uvarint(self.data, &mut self.offset)? as usize;
                if idx >= self.memo_vec.len() {
                    return Err(ScError::InvalidMemoRef(idx));
                }
                Ok(self.memo_vec[idx].clone())
            }
            TAG_HOLE => Err(ScError::CorruptedPayload(
                "Hole tag outside of array context".into(),
            )),
            _ => Err(ScError::UnknownTag(tag)),
        }
    }

    fn decode_string_value(&mut self) -> Result<ScValue, ScError> {
        let len = decode_uvarint(self.data, &mut self.offset)? as usize;
        if self.remaining() < len * 2 {
            return Err(ScError::UnexpectedEof);
        }
        let mut units = Vec::with_capacity(len);
        for _ in 0..len {
            let bytes = self.read_bytes(2)?;
            let unit = u16::from_le_bytes([bytes[0], bytes[1]]);
            units.push(unit);
        }
        Ok(ScValue::String(units.into()))
    }

    fn decode_bigint_value(&mut self) -> Result<ScValue, ScError> {
        let sign_byte = self.read_byte()?;
        let len = decode_uvarint(self.data, &mut self.offset)? as usize;
        let bytes = self.read_bytes(len)?;
        let sign = match sign_byte {
            0 => num_bigint::Sign::Plus,
            1 => num_bigint::Sign::Minus,
            _ => num_bigint::Sign::NoSign,
        };
        let bi = num_bigint::BigInt::from_bytes_le(sign, bytes);
        Ok(ScValue::BigInt(bi))
    }

    fn decode_regexp_value(&mut self) -> Result<ScValue, ScError> {
        // Pattern (string without tag)
        let pat_len = decode_uvarint(self.data, &mut self.offset)? as usize;
        if self.remaining() < pat_len * 2 {
            return Err(ScError::UnexpectedEof);
        }
        let mut units = Vec::with_capacity(pat_len);
        for _ in 0..pat_len {
            let bytes = self.read_bytes(2)?;
            units.push(u16::from_le_bytes([bytes[0], bytes[1]]));
        }
        let pattern: Utf16String = units.into();

        // Flags
        let flags_bits = decode_uvarint(self.data, &mut self.offset)? as u8;
        let flags = RegExpFlags::from_bitfield(flags_bits);

        Ok(ScValue::RegExp { pattern, flags })
    }

    fn decode_array_value(&mut self, depth: usize) -> Result<ScValue, ScError> {
        let memo_idx = self.memo_vec.len();
        // Reserve a placeholder — will be replaced after decoding
        self.memo_vec.push(ScValue::Undefined);

        let length = decode_uvarint(self.data, &mut self.offset)? as usize;
        let items_count = decode_uvarint(self.data, &mut self.offset)? as usize;

        let mut elements: Vec<Option<ScValue>> = vec![None; length];
        for _ in 0..items_count {
            let index = decode_uvarint(self.data, &mut self.offset)? as usize;
            let val = self.decode_value_depth(depth + 1)?;
            if index < length {
                elements[index] = Some(val);
            }
        }

        let extra_count = decode_uvarint(self.data, &mut self.offset)? as usize;
        let mut extra_props = Vec::with_capacity(extra_count);
        for _ in 0..extra_count {
            let ScValue::String(key) = self.decode_string_value()? else {
                return Err(ScError::CorruptedPayload("Expected string key".into()));
            };
            let val = self.decode_value_depth(depth + 1)?;
            extra_props.push((key, val));
        }

        let arr = ScValue::Array {
            elements,
            extra_props,
        };
        self.memo_vec[memo_idx] = arr.clone();
        Ok(arr)
    }

    fn decode_object_value(&mut self, depth: usize) -> Result<ScValue, ScError> {
        let memo_idx = self.memo_vec.len();
        self.memo_vec.push(ScValue::Undefined);

        let count = decode_uvarint(self.data, &mut self.offset)? as usize;
        let mut map = IndexMap::with_capacity(count);
        for _ in 0..count {
            let ScValue::String(key) = self.decode_string_value()? else {
                return Err(ScError::CorruptedPayload("Expected string key".into()));
            };
            let val = self.decode_value_depth(depth + 1)?;
            map.insert(key, val);
        }

        let obj = ScValue::Object(map);
        self.memo_vec[memo_idx] = obj.clone();
        Ok(obj)
    }

    fn decode_map_value(&mut self, depth: usize) -> Result<ScValue, ScError> {
        let memo_idx = self.memo_vec.len();
        self.memo_vec.push(ScValue::Undefined);

        let count = decode_uvarint(self.data, &mut self.offset)? as usize;
        let mut pairs = Vec::with_capacity(count);
        for _ in 0..count {
            let k = self.decode_value_depth(depth + 1)?;
            let v = self.decode_value_depth(depth + 1)?;
            pairs.push((k, v));
        }

        let map = ScValue::Map(pairs);
        self.memo_vec[memo_idx] = map.clone();
        Ok(map)
    }

    fn decode_set_value(&mut self, depth: usize) -> Result<ScValue, ScError> {
        let memo_idx = self.memo_vec.len();
        self.memo_vec.push(ScValue::Undefined);

        let count = decode_uvarint(self.data, &mut self.offset)? as usize;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            let v = self.decode_value_depth(depth + 1)?;
            values.push(v);
        }

        let set = ScValue::Set(values);
        self.memo_vec[memo_idx] = set.clone();
        Ok(set)
    }

    fn decode_error_value(&mut self, depth: usize) -> Result<ScValue, ScError> {
        let memo_idx = self.memo_vec.len();
        self.memo_vec.push(ScValue::Undefined);

        let kind_byte = self.read_byte()?;
        let kind = ScErrorKind::from_u8(kind_byte).ok_or_else(|| {
            ScError::CorruptedPayload(format!("Unknown error kind: {kind_byte:#04x}"))
        })?;

        let flags = self.read_byte()?;
        let has_message = flags & (1 << 0) != 0;
        let has_cause = flags & (1 << 1) != 0;
        let has_errors = flags & (1 << 2) != 0;

        let message = if has_message {
            let ScValue::String(s) = self.decode_string_value()? else {
                return Err(ScError::CorruptedPayload("Expected string message".into()));
            };
            Some(s)
        } else {
            None
        };

        let cause = if has_cause {
            Some(Box::new(self.decode_value_depth(depth + 1)?))
        } else {
            None
        };

        let errors = if has_errors {
            let count = decode_uvarint(self.data, &mut self.offset)? as usize;
            let mut errs = Vec::with_capacity(count);
            for _ in 0..count {
                errs.push(self.decode_value_depth(depth + 1)?);
            }
            Some(errs)
        } else {
            None
        };

        let err = ScValue::Error(ScErrorObject {
            kind,
            message,
            cause,
            errors,
        });
        self.memo_vec[memo_idx] = err.clone();
        Ok(err)
    }

    fn decode_arraybuffer_value(&mut self) -> Result<ScValue, ScError> {
        let memo_idx = self.memo_vec.len();
        self.memo_vec.push(ScValue::Undefined);

        let max_byte_length_raw = decode_uvarint(self.data, &mut self.offset)? as usize;
        let max_byte_length = if max_byte_length_raw == 0 {
            None
        } else {
            Some(max_byte_length_raw - 1)
        };

        let byte_len = decode_uvarint(self.data, &mut self.offset)? as usize;
        let data = self.read_bytes(byte_len)?.to_vec();

        let buf = ScValue::ArrayBuffer {
            data,
            max_byte_length,
        };
        self.memo_vec[memo_idx] = buf.clone();
        Ok(buf)
    }

    fn decode_typedarray_value(&mut self) -> Result<ScValue, ScError> {
        let kind_byte = self.read_byte()?;
        let kind = ScTypedArrayKind::from_u8(kind_byte).ok_or_else(|| {
            ScError::CorruptedPayload(format!("Unknown TypedArray kind: {kind_byte:#04x}"))
        })?;
        let byte_offset = decode_uvarint(self.data, &mut self.offset)? as usize;
        let length = decode_uvarint(self.data, &mut self.offset)? as usize;
        let buffer_memo_index = decode_uvarint(self.data, &mut self.offset)? as usize;

        Ok(ScValue::TypedArray {
            kind,
            byte_offset,
            length,
            buffer_memo_index,
        })
    }

    fn decode_dataview_value(&mut self) -> Result<ScValue, ScError> {
        let byte_offset = decode_uvarint(self.data, &mut self.offset)? as usize;
        let byte_length = decode_uvarint(self.data, &mut self.offset)? as usize;
        let buffer_memo_index = decode_uvarint(self.data, &mut self.offset)? as usize;

        Ok(ScValue::DataView {
            byte_offset,
            byte_length,
            buffer_memo_index,
        })
    }

    fn decode_boxed_value(&mut self, depth: usize) -> Result<ScValue, ScError> {
        let subtag = self.read_byte()?;
        match subtag {
            BOXED_BOOL => {
                let inner = self.decode_value_depth(depth + 1)?;
                match inner {
                    ScValue::Boolean(b) => Ok(ScValue::BoxedBoolean(b)),
                    _ => Err(ScError::CorruptedPayload(
                        "Expected boolean in boxed boolean".into(),
                    )),
                }
            }
            BOXED_NUM => {
                let inner = self.decode_value_depth(depth + 1)?;
                match inner {
                    ScValue::Number(n) => Ok(ScValue::BoxedNumber(n)),
                    _ => Err(ScError::CorruptedPayload(
                        "Expected number in boxed number".into(),
                    )),
                }
            }
            BOXED_STR => {
                let inner = self.decode_value_depth(depth + 1)?;
                match inner {
                    ScValue::String(s) => Ok(ScValue::BoxedString(s)),
                    _ => Err(ScError::CorruptedPayload(
                        "Expected string in boxed string".into(),
                    )),
                }
            }
            BOXED_BIGINT => {
                let inner = self.decode_value_depth(depth + 1)?;
                match inner {
                    ScValue::BigInt(bi) => Ok(ScValue::BoxedBigInt(bi)),
                    _ => Err(ScError::CorruptedPayload(
                        "Expected bigint in boxed bigint".into(),
                    )),
                }
            }
            _ => Err(ScError::CorruptedPayload(format!(
                "Unknown boxed subtag: {subtag:#04x}"
            ))),
        }
    }
}
