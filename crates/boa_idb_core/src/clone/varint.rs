//! Varint encoding/decoding (LEB128 unsigned, ZigZag signed).

use crate::error::ScError;

/// Encodes a `u64` as an unsigned LEB128 varint.
pub fn encode_uvarint(mut val: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if val == 0 {
            break;
        }
    }
}

/// Decodes an unsigned LEB128 varint from the byte stream.
///
/// Advances `offset` past the varint bytes.
pub fn decode_uvarint(bytes: &[u8], offset: &mut usize) -> Result<u64, ScError> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        if *offset >= bytes.len() {
            return Err(ScError::UnexpectedEof);
        }
        let byte = bytes[*offset];
        *offset += 1;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err(ScError::CorruptedPayload(
                "Varint too long (>= 10 bytes)".into(),
            ));
        }
    }
}

/// Encodes an `i64` as a ZigZag-encoded varint.
///
/// ZigZag encoding: `(n << 1) ^ (n >> 63)`.
#[allow(clippy::cast_sign_loss)]
pub fn encode_ivarint(val: i64, out: &mut Vec<u8>) {
    let zigzag = ((val << 1) ^ (val >> 63)) as u64;
    encode_uvarint(zigzag, out);
}

/// Decodes a ZigZag-encoded varint from the byte stream.
///
/// Advances `offset` past the varint bytes.
#[allow(clippy::cast_possible_wrap)]
pub fn decode_ivarint(bytes: &[u8], offset: &mut usize) -> Result<i64, ScError> {
    let zigzag = decode_uvarint(bytes, offset)?;
    Ok(((zigzag >> 1) as i64) ^ -((zigzag & 1) as i64))
}
