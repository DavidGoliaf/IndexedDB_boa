//! SCF-v1 binary encoder for `ScValue`.

use crate::clone::crc32c::crc32c;
use crate::clone::scvalue::{RegExpFlags, ScErrorObject, ScTypedArrayKind, ScValue};
use crate::clone::varint::{encode_ivarint, encode_uvarint};
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
/// `0x14` (`Hole`) is never emitted: sparse-array holes are encoded implicitly
/// by omitting their indices from the `items` list of `TAG_ARRAY`.
#[allow(dead_code)]
const TAG_HOLE: u8 = 0x14;

// Boxed subtags
const BOXED_BOOL: u8 = 0;
const BOXED_NUM: u8 = 1;
const BOXED_STR: u8 = 2;
const BOXED_BIGINT: u8 = 3;

/// SCF-v1 magic bytes.
pub const SCF_MAGIC: &[u8; 4] = b"IDB1";
/// SCF-v1 format version.
pub const SCF_VERSION: u8 = 1;

/// Encodes an `ScValue` into the SCF-v1 binary format.
///
/// Returns the complete binary payload including header and CRC32C trailer.
pub fn encode_scf(value: &ScValue, limits: &LimitConfig) -> Result<Vec<u8>, ScError> {
    let mut encoder = ScfEncoder::new(limits);
    encoder.encode(value)?;
    Ok(encoder.finish())
}

struct ScfEncoder<'a> {
    limits: &'a LimitConfig,
    buf: Vec<u8>,
    next_memo: usize,
}

impl<'a> ScfEncoder<'a> {
    fn new(limits: &'a LimitConfig) -> Self {
        let mut buf = Vec::with_capacity(256);
        // Write header
        buf.extend_from_slice(SCF_MAGIC);
        buf.push(SCF_VERSION);
        buf.push(0); // flags
        buf.extend_from_slice(&[0, 0]); // reserved u16 LE
        Self {
            limits,
            buf,
            next_memo: 0,
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let checksum = crc32c(&self.buf);
        self.buf.extend_from_slice(&checksum.to_le_bytes());
        self.buf
    }

    fn allocate_memo(&mut self) -> usize {
        let idx = self.next_memo;
        self.next_memo += 1;
        idx
    }

    fn check_size(&self) -> Result<(), ScError> {
        if self.buf.len() > self.limits.max_value_len {
            return Err(ScError::ValueTooLarge {
                limit: self.limits.max_value_len,
                actual: self.buf.len(),
            });
        }
        Ok(())
    }

    fn encode(&mut self, value: &ScValue) -> Result<(), ScError> {
        self.encode_with_depth(value, 0)
    }

    fn encode_with_depth(&mut self, value: &ScValue, depth: usize) -> Result<(), ScError> {
        if depth > self.limits.max_clone_depth {
            return Err(ScError::MaxDepthExceeded(self.limits.max_clone_depth));
        }
        self.check_size()?;

        match value {
            ScValue::Undefined => self.buf.push(TAG_UNDEFINED),
            ScValue::Null => self.buf.push(TAG_NULL),
            ScValue::Boolean(false) => self.buf.push(TAG_FALSE),
            ScValue::Boolean(true) => self.buf.push(TAG_TRUE),
            ScValue::Number(n) => self.encode_number(*n),
            ScValue::BigInt(bi) => self.encode_bigint(bi),
            ScValue::String(s) => self.encode_string(s),
            ScValue::Date(d) => self.encode_date(*d),
            ScValue::RegExp { pattern, flags } => self.encode_regexp(pattern, *flags),
            ScValue::Array {
                elements,
                extra_props,
            } => self.encode_array(elements, extra_props, depth)?,
            ScValue::Object(map) => self.encode_object(map, depth)?,
            ScValue::Map(pairs) => self.encode_map(pairs, depth)?,
            ScValue::Set(values) => self.encode_set(values, depth)?,
            ScValue::Error(err) => self.encode_error(err, depth)?,
            ScValue::ArrayBuffer {
                data,
                max_byte_length,
            } => self.encode_arraybuffer(data, *max_byte_length),
            ScValue::TypedArray {
                kind,
                byte_offset,
                length,
                buffer_memo_index,
            } => self.encode_typedarray(*kind, *byte_offset, *length, *buffer_memo_index),
            ScValue::DataView {
                byte_offset,
                byte_length,
                buffer_memo_index,
            } => self.encode_dataview(*byte_offset, *byte_length, *buffer_memo_index),
            ScValue::BoxedBoolean(b) => self.encode_boxed_bool(*b),
            ScValue::BoxedNumber(n) => self.encode_boxed_num(*n),
            ScValue::BoxedString(s) => self.encode_boxed_str(s),
            ScValue::BoxedBigInt(bi) => self.encode_boxed_bigint(bi),
            ScValue::MemoRef(idx) => {
                self.buf.push(TAG_MEMO_REF);
                encode_uvarint(*idx as u64, &mut self.buf);
            }
        }
        Ok(())
    }

    #[allow(clippy::float_cmp, clippy::cast_possible_truncation)]
    fn encode_number(&mut self, n: f64) {
        // Try i32 optimization
        if n.fract() == 0.0 && n >= f64::from(i32::MIN) && n <= f64::from(i32::MAX) {
            let i = n as i32;
            if f64::from(i) == n {
                self.buf.push(TAG_INT32);
                encode_ivarint(i64::from(i), &mut self.buf);
                return;
            }
        }
        self.buf.push(TAG_DOUBLE);
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    fn encode_bigint(&mut self, bi: &num_bigint::BigInt) {
        self.buf.push(TAG_BIGINT);
        let (sign, bytes) = bi.to_bytes_le();
        let sign_byte: u8 = match sign {
            num_bigint::Sign::Minus => 1,
            _ => 0,
        };
        self.buf.push(sign_byte);
        encode_uvarint(bytes.len() as u64, &mut self.buf);
        self.buf.extend_from_slice(&bytes);
    }

    fn encode_string(&mut self, s: &Utf16String) {
        self.buf.push(TAG_STRING);
        self.encode_string_value(s);
    }

    fn encode_string_value(&mut self, s: &Utf16String) {
        let units = s.as_slice();
        encode_uvarint(units.len() as u64, &mut self.buf);
        for &unit in units {
            self.buf.extend_from_slice(&unit.to_le_bytes());
        }
    }

    fn encode_date(&mut self, d: f64) {
        self.buf.push(TAG_DATE);
        self.buf.extend_from_slice(&d.to_le_bytes());
    }

    fn encode_regexp(&mut self, pattern: &Utf16String, flags: RegExpFlags) {
        self.buf.push(TAG_REGEXP);
        // Encode pattern as string
        let units = pattern.as_slice();
        encode_uvarint(units.len() as u64, &mut self.buf);
        for &unit in units {
            self.buf.extend_from_slice(&unit.to_le_bytes());
        }
        // Encode flags as varint bitfield
        encode_uvarint(u64::from(flags.to_bitfield()), &mut self.buf);
    }

    fn encode_array(
        &mut self,
        elements: &[Option<ScValue>],
        extra_props: &[(Utf16String, ScValue)],
        depth: usize,
    ) -> Result<(), ScError> {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_ARRAY);
        // length = number of elements (including holes)
        encode_uvarint(elements.len() as u64, &mut self.buf);
        // items_count = number of non-hole elements
        let non_hole_count = elements.iter().filter(|e| e.is_some()).count();
        encode_uvarint(non_hole_count as u64, &mut self.buf);
        for (i, elem) in elements.iter().enumerate() {
            if let Some(v) = elem {
                encode_uvarint(i as u64, &mut self.buf);
                self.encode_with_depth(v, depth + 1)?;
            }
        }
        // extra props
        encode_uvarint(extra_props.len() as u64, &mut self.buf);
        for (key, val) in extra_props {
            self.encode_string_value(key);
            self.encode_with_depth(val, depth + 1)?;
        }
        Ok(())
    }

    fn encode_object(
        &mut self,
        map: &IndexMap<Utf16String, ScValue>,
        depth: usize,
    ) -> Result<(), ScError> {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_OBJECT);
        encode_uvarint(map.len() as u64, &mut self.buf);
        for (key, val) in map {
            self.encode_string_value(key);
            self.encode_with_depth(val, depth + 1)?;
        }
        Ok(())
    }

    fn encode_map(&mut self, pairs: &[(ScValue, ScValue)], depth: usize) -> Result<(), ScError> {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_MAP);
        encode_uvarint(pairs.len() as u64, &mut self.buf);
        for (k, v) in pairs {
            self.encode_with_depth(k, depth + 1)?;
            self.encode_with_depth(v, depth + 1)?;
        }
        Ok(())
    }

    fn encode_set(&mut self, values: &[ScValue], depth: usize) -> Result<(), ScError> {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_SET);
        encode_uvarint(values.len() as u64, &mut self.buf);
        for v in values {
            self.encode_with_depth(v, depth + 1)?;
        }
        Ok(())
    }

    fn encode_error(&mut self, err: &ScErrorObject, depth: usize) -> Result<(), ScError> {
        // Allocate memo for the error object
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_ERROR);
        self.buf.push(err.kind as u8);

        // flags: bit0 = has_message, bit1 = has_cause, bit2 = has_errors
        let mut flags = 0u8;
        if err.message.is_some() {
            flags |= 1 << 0;
        }
        if err.cause.is_some() {
            flags |= 1 << 1;
        }
        if err.errors.is_some() {
            flags |= 1 << 2;
        }
        self.buf.push(flags);

        if let Some(msg) = &err.message {
            self.encode_string_value(msg);
        }
        if let Some(cause) = &err.cause {
            self.encode_with_depth(cause, depth + 1)?;
        }
        if let Some(errors) = &err.errors {
            encode_uvarint(errors.len() as u64, &mut self.buf);
            for e in errors {
                self.encode_with_depth(e, depth + 1)?;
            }
        }
        Ok(())
    }

    fn encode_arraybuffer(&mut self, data: &[u8], max_byte_length: Option<usize>) {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_ARRAYBUFFER);
        // max_byte_length + 1, or 0 if not resizable
        match max_byte_length {
            Some(max) => encode_uvarint((max + 1) as u64, &mut self.buf),
            None => encode_uvarint(0, &mut self.buf),
        }
        encode_uvarint(data.len() as u64, &mut self.buf);
        self.buf.extend_from_slice(data);
    }

    fn encode_typedarray(
        &mut self,
        kind: ScTypedArrayKind,
        byte_offset: usize,
        length: usize,
        buffer_memo_index: usize,
    ) {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_TYPEDARRAY);
        self.buf.push(kind as u8);
        encode_uvarint(byte_offset as u64, &mut self.buf);
        encode_uvarint(length as u64, &mut self.buf);
        encode_uvarint(buffer_memo_index as u64, &mut self.buf);
    }

    fn encode_dataview(
        &mut self,
        byte_offset: usize,
        byte_length: usize,
        buffer_memo_index: usize,
    ) {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_DATAVIEW);
        encode_uvarint(byte_offset as u64, &mut self.buf);
        encode_uvarint(byte_length as u64, &mut self.buf);
        encode_uvarint(buffer_memo_index as u64, &mut self.buf);
    }

    /// Boxed values are encoded as `TAG_BOXED`, a one-byte subtag
    /// (`BOXED_BOOL`/`BOXED_NUM`/`BOXED_STR`/`BOXED_BIGINT`), followed by the
    /// complete tagged encoding of the wrapped primitive value (e.g.
    /// `TAG_BOXED BOXED_NUM TAG_INT32 <varint>`). The decoder mirrors this layout.
    fn encode_boxed_bool(&mut self, b: bool) {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_BOXED);
        self.buf.push(BOXED_BOOL);
        self.buf.push(if b { TAG_TRUE } else { TAG_FALSE });
    }

    fn encode_boxed_num(&mut self, n: f64) {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_BOXED);
        self.buf.push(BOXED_NUM);
        self.encode_number(n);
    }

    fn encode_boxed_str(&mut self, s: &Utf16String) {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_BOXED);
        self.buf.push(BOXED_STR);
        self.encode_string(s);
    }

    fn encode_boxed_bigint(&mut self, bi: &num_bigint::BigInt) {
        let _memo_idx = self.allocate_memo();

        self.buf.push(TAG_BOXED);
        self.buf.push(BOXED_BIGINT);
        self.encode_bigint(bi);
    }
}
