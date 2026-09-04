//! Path hashing for the filesystem backend (R8.1.4 / §8.3 topology).

use data_encoding::BASE32_NOPAD;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Directory name for a storage key: `sk-<base32(sha256(utf16le))[..26]>`.
pub fn storage_dir_name(key: &str) -> String {
    let units: Vec<u16> = key.encode_utf16().collect();
    format!("sk-{}", &base32_sha256(&units)[..26])
}

/// Directory name for a database: `db-<base32(sha256(utf16le(name)))[..26]>`.
pub fn database_dir_name(name: &str) -> String {
    let units: Vec<u16> = name.encode_utf16().collect();
    format!("db-{}", &base32_sha256(&units)[..26])
}

/// `<root>/<storage_key_hash>/`
pub fn storage_root(root: &Path, storage_key: &str) -> PathBuf {
    root.join(storage_dir_name(storage_key))
}

/// `<root>/<storage_key_hash>/<db_name_hash>/`
pub fn database_root(root: &Path, storage_key: &str, db_name: &str) -> PathBuf {
    storage_root(root, storage_key).join(database_dir_name(db_name))
}

fn base32_sha256(name_utf16: &[u16]) -> String {
    let mut bytes = Vec::with_capacity(name_utf16.len() * 2);
    for &cu in name_utf16 {
        bytes.extend_from_slice(&cu.to_le_bytes());
    }
    let hash = Sha256::digest(&bytes);
    BASE32_NOPAD.encode(&hash).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_path_safe() {
        let a = storage_dir_name("https://example.com");
        assert_eq!(a, storage_dir_name("https://example.com"));
        assert!(a.starts_with("sk-"));
        assert!(!a.contains("example"));
        assert_ne!(storage_dir_name("a"), storage_dir_name("b"));
        assert_ne!(database_dir_name("db1"), database_dir_name("db2"));
        assert!(!database_dir_name("../x").contains(".."));
    }
}
