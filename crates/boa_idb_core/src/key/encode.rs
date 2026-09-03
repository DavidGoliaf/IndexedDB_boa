//! Binary codec for IndexedDB keys (`KEY-v1` format).
//!
//! The encoding is order-preserving: lexicographic comparison of encoded bytes
//! matches the `Key::cmp` ordering.

use crate::error::KeyError;
use crate::key::value::Key;
use crate::limits::LimitConfig;

/// Tag byte for `Number` keys.
pub const TAG_NUMBER: u8 = 0x10;
/// Tag byte for `Date` keys.
pub const TAG_DATE: u8 = 0x20;
/// Tag byte for `String` keys.
pub const TAG_STRING: u8 = 0x30;
/// Tag byte for `Binary` keys.
pub const TAG_BINARY: u8 = 0x40;
/// Tag byte for `Array` keys.
pub const TAG_ARRAY: u8 = 0x50;
/// Terminator byte for arrays.
pub const TAG_ARRAY_TERMINATOR: u8 = 0x00;

/// Encodes an `f64` value into 8 bytes that preserve ordering.
///
/// - Normalizes `-0.0` to `+0.0`.
/// - Negative numbers: invert all bits.
/// - Non-negative numbers: set the sign bit.
pub fn encode_f64_orderable(val: f64) -> [u8; 8] {
    let v = if val == 0.0 { 0.0 } else { val };
    let bits = v.to_bits();
    let ord = if (bits & (1u64 << 63)) != 0 {
        !bits
    } else {
        bits | (1u64 << 63)
    };
    ord.to_be_bytes()
}

/// Decodes an order-preserving 8-byte representation back to `f64`.
pub fn decode_f64_orderable(bytes: [u8; 8]) -> f64 {
    let ord = u64::from_be_bytes(bytes);
    let bits = if (ord & (1u64 << 63)) != 0 {
        ord & !(1u64 << 63)
    } else {
        !ord
    };
    f64::from_bits(bits)
}

/// Encodes a single UTF-16 code unit into CESU-8 bytes.
///
/// Returns the number of bytes written (1, 2, or 3).
#[allow(clippy::cast_possible_truncation)]
fn encode_cesu8_unit(cu: u16, out: &mut Vec<u8>) {
    match cu {
        0x0000 => {
            // U+0000 encoded as single byte 0x00 (not modified UTF-8 C0 80)
            // to preserve sort order. The byte 0x00 is then escaped.
            out.push(0x00);
        }
        0x0001..=0x007F => {
            out.push(cu as u8);
        }
        0x0080..=0x07FF => {
            out.push(0xC0 | ((cu >> 6) as u8));
            out.push(0x80 | ((cu & 0x3F) as u8));
        }
        _ => {
            // 0x0800..=0xFFFF (including unpaired surrogates 0xD800..=0xDFFF)
            out.push(0xE0 | ((cu >> 12) as u8));
            out.push(0x80 | (((cu >> 6) & 0x3F) as u8));
            out.push(0x80 | ((cu & 0x3F) as u8));
        }
    }
}

/// Encodes a string as CESU-8 with null-byte escaping and a terminator.
///
/// Each `0x00` byte in the CESU-8 output is escaped as `[0x00, 0xFF]`.
/// The string is terminated with `[0x00, 0x00]`.
fn encode_string_cesu8(units: &[u16], out: &mut Vec<u8>) {
    let mut cesu8_buf = Vec::new();
    for &unit in units {
        encode_cesu8_unit(unit, &mut cesu8_buf);
    }
    // Escape 0x00 bytes: 0x00 -> [0x00, 0xFF]
    for &b in &cesu8_buf {
        if b == 0x00 {
            out.push(0x00);
            out.push(0xFF);
        } else {
            out.push(b);
        }
    }
    // Terminator
    out.push(0x00);
    out.push(0x00);
}

/// Encodes binary data with null-byte escaping and a terminator.
fn encode_binary(data: &[u8], out: &mut Vec<u8>) {
    for &b in data {
        if b == 0x00 {
            out.push(0x00);
            out.push(0xFF);
        } else {
            out.push(b);
        }
    }
    // Terminator
    out.push(0x00);
    out.push(0x00);
}

/// Encodes a key into the `KEY-v1` binary format.
///
/// The encoded key is appended to `out`. Returns an error if the key is invalid
/// or exceeds `max_key_len`.
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

fn encode_key_internal(
    key: &Key,
    out: &mut Vec<u8>,
    depth: usize,
    limits: &LimitConfig,
) -> Result<(), KeyError> {
    if depth > limits.max_key_depth {
        return Err(KeyError::MaxDepthExceeded(limits.max_key_depth));
    }
    match key {
        Key::Number(n) => {
            out.push(TAG_NUMBER);
            out.extend_from_slice(&encode_f64_orderable(*n));
        }
        Key::Date(d) => {
            out.push(TAG_DATE);
            out.extend_from_slice(&encode_f64_orderable(*d));
        }
        Key::String(s) => {
            out.push(TAG_STRING);
            encode_string_cesu8(s.as_slice(), out);
        }
        Key::Binary(b) => {
            out.push(TAG_BINARY);
            encode_binary(b, out);
        }
        Key::Array(arr) => {
            out.push(TAG_ARRAY);
            for item in arr {
                encode_key_internal(item, out, depth + 1, limits)?;
            }
            out.push(TAG_ARRAY_TERMINATOR);
        }
    }
    Ok(())
}

/// Decodes a key from the `KEY-v1` binary format.
///
/// Returns the decoded key and the number of bytes consumed.
pub fn decode_key(bytes: &[u8]) -> Result<(Key, usize), KeyError> {
    if bytes.is_empty() {
        return Err(KeyError::UnexpectedEof);
    }
    decode_key_internal(bytes, 0)
}

fn decode_key_internal(bytes: &[u8], pos: usize) -> Result<(Key, usize), KeyError> {
    if pos >= bytes.len() {
        return Err(KeyError::UnexpectedEof);
    }
    let tag = bytes[pos];
    let start = pos;
    match tag {
        TAG_NUMBER => {
            if pos + 9 > bytes.len() {
                return Err(KeyError::UnexpectedEof);
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[pos + 1..pos + 9]);
            let val = decode_f64_orderable(buf);
            Ok((Key::Number(val), 9))
        }
        TAG_DATE => {
            if pos + 9 > bytes.len() {
                return Err(KeyError::UnexpectedEof);
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[pos + 1..pos + 9]);
            let val = decode_f64_orderable(buf);
            Ok((Key::Date(val), 9))
        }
        TAG_STRING => {
            let (units, consumed) = decode_string_cesu8(bytes, pos + 1)?;
            Ok((Key::String(units.into()), consumed + 1))
        }
        TAG_BINARY => {
            let (data, consumed) = decode_binary_escaped(bytes, pos + 1)?;
            Ok((Key::Binary(data), consumed + 1))
        }
        TAG_ARRAY => {
            let mut elements = Vec::new();
            let mut offset = pos + 1;
            loop {
                if offset >= bytes.len() {
                    return Err(KeyError::UnexpectedEof);
                }
                if bytes[offset] == TAG_ARRAY_TERMINATOR {
                    offset += 1;
                    break;
                }
                let (key, consumed) = decode_key_internal(bytes, offset)?;
                elements.push(key);
                offset += consumed;
            }
            Ok((Key::Array(elements), offset - start))
        }
        _ => Err(KeyError::InvalidEncoding(format!(
            "Unknown key tag: {tag:#04x}"
        ))),
    }
}

/// Decodes a CESU-8 encoded, null-escaped string from the byte stream.
///
/// Returns the UTF-16 code units and the number of bytes consumed (including terminator).
fn decode_string_cesu8(bytes: &[u8], pos: usize) -> Result<(Vec<u16>, usize), KeyError> {
    // First, un-escape: read until terminator [0x00, 0x00]
    let mut unescaped = Vec::new();
    let mut i = pos;
    while i < bytes.len() {
        if bytes[i] == 0x00 {
            if i + 1 >= bytes.len() {
                return Err(KeyError::InvalidEncoding(
                    "Unexpected end after 0x00 in string".into(),
                ));
            }
            if bytes[i + 1] == 0x00 {
                // Terminator found
                i += 2;
                break;
            }
            if bytes[i + 1] == 0xFF {
                // Escaped null
                unescaped.push(0x00);
                i += 2;
            } else {
                return Err(KeyError::InvalidEncoding(
                    "Lone 0x00 without 0xFF or 0x00 terminator".into(),
                ));
            }
        } else {
            unescaped.push(bytes[i]);
            i += 1;
        }
    }
    let consumed = i - pos;

    // Now decode CESU-8 bytes into UTF-16 code units
    let mut units = Vec::new();
    let mut j = 0;
    while j < unescaped.len() {
        let b = unescaped[j];
        match b {
            0x00..=0x7F => {
                units.push(u16::from(b));
                j += 1;
            }
            0xC0..=0xDF => {
                if j + 1 >= unescaped.len() {
                    return Err(KeyError::InvalidEncoding(
                        "Truncated CESU-8 2-byte sequence".into(),
                    ));
                }
                let b2 = unescaped[j + 1];
                if !(0x80..=0xBF).contains(&b2) {
                    return Err(KeyError::InvalidEncoding(
                        "Invalid CESU-8 continuation byte".into(),
                    ));
                }
                let cu = (u16::from(b & 0x1F) << 6) | u16::from(b2 & 0x3F);
                units.push(cu);
                j += 2;
            }
            0xE0..=0xEF => {
                if j + 2 >= unescaped.len() {
                    return Err(KeyError::InvalidEncoding(
                        "Truncated CESU-8 3-byte sequence".into(),
                    ));
                }
                let b2 = unescaped[j + 1];
                let b3 = unescaped[j + 2];
                if !(0x80..=0xBF).contains(&b2) || !(0x80..=0xBF).contains(&b3) {
                    return Err(KeyError::InvalidEncoding(
                        "Invalid CESU-8 continuation byte".into(),
                    ));
                }
                let cu = (u16::from(b & 0x0F) << 12)
                    | (u16::from(b2 & 0x3F) << 6)
                    | u16::from(b3 & 0x3F);
                units.push(cu);
                j += 3;
            }
            _ => {
                return Err(KeyError::InvalidEncoding(format!(
                    "Invalid CESU-8 leading byte: {b:#04x}"
                )));
            }
        }
    }

    Ok((units, consumed))
}

/// Decodes null-escaped binary data from the byte stream.
///
/// Returns the raw bytes and the number of bytes consumed (including terminator).
fn decode_binary_escaped(bytes: &[u8], pos: usize) -> Result<(Vec<u8>, usize), KeyError> {
    let mut data = Vec::new();
    let mut i = pos;
    while i < bytes.len() {
        if bytes[i] == 0x00 {
            if i + 1 >= bytes.len() {
                return Err(KeyError::InvalidEncoding(
                    "Unexpected end after 0x00 in binary".into(),
                ));
            }
            if bytes[i + 1] == 0x00 {
                // Terminator
                i += 2;
                break;
            }
            if bytes[i + 1] == 0xFF {
                data.push(0x00);
                i += 2;
            } else {
                return Err(KeyError::InvalidEncoding(
                    "Lone 0x00 without 0xFF or 0x00 terminator in binary".into(),
                ));
            }
        } else {
            data.push(bytes[i]);
            i += 1;
        }
    }
    Ok((data, i - pos))
}
