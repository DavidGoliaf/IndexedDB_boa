//! Software CRC-32/Castagnoli (CRC32C) implementation.
//!
//! SCF-v1 integrity trailers are CRC32C checksums (Castagnoli polynomial
//! `0x1EDC6F41`, reflected form `0x82F63B78`), as required by the format
//! specification. This module provides a small dependency-free, table-driven
//! implementation so the core crate does not need an extra checksum
//! dependency for a single call site.
//!
//! Test vector: `crc32c(b"123456789") == 0xE3069283`.

/// Reflected Castagnoli polynomial.
const POLY_REFLECTED: u32 = 0x82F6_3B78;

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ POLY_REFLECTED;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const TABLE: [u32; 256] = build_table();

/// Computes the CRC32C (Castagnoli) checksum of `data`.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        let idx = ((crc ^ u32::from(b)) & 0xFF) as usize;
        crc = (crc >> 8) ^ TABLE[idx];
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn crc32c_known_vector() {
        // Standard CRC32C check value.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn crc32c_empty() {
        assert_eq!(crc32c(b""), 0x0000_0000);
    }
}
