//! Path hashing: SHA-256 + Base32 for safe filesystem names (R8.1.4).

use data_encoding::BASE32_NOPAD;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Converts a UTF-16 database name into a safe filename on disk.
///
/// `db_name_hash = base32(sha256(utf16le(name)))[..26]`
/// Result: `db-<hash>.sqlite`
pub fn db_name_to_filename(name_utf16: &[u16]) -> String {
    format!("db-{}.sqlite", &base32_sha256(name_utf16)[..26])
}

/// Derives a safe directory name for a storage key (R8.1.4).
///
/// The storage key is never used verbatim in filesystem paths; it is hashed
/// the same way as database names: `base32(sha256(utf16le(key)))[..26]`.
/// Result: `sk-<hash>`
pub fn storage_dir_name(key: &str) -> String {
    let name_utf16: Vec<u16> = key.encode_utf16().collect();
    format!("sk-{}", &base32_sha256(&name_utf16)[..26])
}

/// Computes `base32(sha256(utf16le(name)))[..26]` for a sequence of UTF-16 units.
fn base32_sha256(name_utf16: &[u16]) -> String {
    let mut bytes = Vec::with_capacity(name_utf16.len() * 2);
    for &cu in name_utf16 {
        bytes.extend_from_slice(&cu.to_le_bytes());
    }
    let hash = Sha256::digest(&bytes);
    BASE32_NOPAD.encode(&hash).to_ascii_lowercase()
}

/// Computes the blob file path by SHA-256 hash of the value content.
///
/// Layout: `blobs/<db_hash>/<xx>/<sha256>.bin`
pub fn blob_path(root: &Path, db_hash: &str, value_bytes: &[u8]) -> PathBuf {
    let hash = Sha256::digest(value_bytes);
    let hex_hash = hex_encode(hash.as_slice());
    let prefix = &hex_hash[..2];
    root.join("blobs")
        .join(db_hash)
        .join(prefix)
        .join(format!("{hex_hash}.bin"))
}

/// Extracts the db hash portion from a db filename like `db-<hash>.sqlite`.
pub fn db_hash_from_filename(filename: &str) -> Option<&str> {
    filename
        .strip_prefix("db-")
        .and_then(|s| s.strip_suffix(".sqlite"))
}

/// Hex-encodes a byte slice.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_name_to_filename_deterministic() {
        let name: Vec<u16> = "test_db".encode_utf16().collect();
        let f1 = db_name_to_filename(&name);
        let f2 = db_name_to_filename(&name);
        assert_eq!(f1, f2);
        assert!(f1.starts_with("db-"));
        assert!(f1.ends_with(".sqlite"));
    }

    #[test]
    fn test_db_name_to_filename_different_names() {
        let n1: Vec<u16> = "db1".encode_utf16().collect();
        let n2: Vec<u16> = "db2".encode_utf16().collect();
        assert_ne!(db_name_to_filename(&n1), db_name_to_filename(&n2));
    }

    #[test]
    fn test_blob_path_structure() {
        let root = Path::new("/tmp/storage");
        let path = blob_path(root, "abc123", b"hello world");
        assert!(path.starts_with(root.join("blobs").join("abc123")));
        assert!(path.to_string_lossy().ends_with(".bin"));
    }

    #[test]
    fn test_db_hash_from_filename() {
        assert_eq!(
            db_hash_from_filename("db-abcdefghijklmno.sqlite"),
            Some("abcdefghijklmno")
        );
        assert_eq!(db_hash_from_filename("other.sqlite"), None);
    }

    #[test]
    fn test_storage_dir_name_is_hashed_and_stable() {
        let d1 = storage_dir_name("https://example.com");
        let d2 = storage_dir_name("https://example.com");
        assert_eq!(d1, d2);
        assert!(d1.starts_with("sk-"));
        assert_eq!(d1.len(), 3 + 26);
        // Names never appear verbatim in the directory name.
        assert!(!d1.contains("example"));
        // Distinct keys hash differently.
        assert_ne!(storage_dir_name("a"), storage_dir_name("b"));
    }
}
