//! Blob manager for large values (>256 KiB).
//!
//! Values exceeding `INLINE_VALUE_THRESHOLD` are stored as external files
//! under `blobs/<db_hash>/<xx>/<sha256>.bin` with atomic rename semantics.

use boa_idb_core::backend::error::BackendError;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::naming::hex_encode;

/// Threshold for inline vs external storage (256 KiB).
pub const INLINE_VALUE_THRESHOLD: usize = 256 * 1024;

/// Manages external blob files for a database.
pub struct BlobManager {
    /// Root directory for blobs (e.g., `<storage_root>/blobs/<db_hash>/`).
    blob_dir: PathBuf,
}

impl BlobManager {
    /// Creates a new blob manager for the given database hash.
    pub fn new(storage_root: &Path, db_hash: &str) -> Self {
        Self {
            blob_dir: storage_root.join("blobs").join(db_hash),
        }
    }

    /// Returns `true` if the value should be stored externally.
    pub fn should_externalize(value: &[u8]) -> bool {
        value.len() > INLINE_VALUE_THRESHOLD
    }

    /// Stores a large value as an external blob file.
    ///
    /// Returns the relative path (e.g., `xx/<sha256>.bin`) to store in `records.ext`.
    pub fn store(&self, value: &[u8]) -> Result<String, BackendError> {
        let hash = Sha256::digest(value);
        let hex_hash = hex_encode(&hash);
        let prefix = &hex_hash[..2];
        let rel_path = format!("{prefix}/{hex_hash}.bin");
        let final_path = self.blob_dir.join(&rel_path);

        // If file already exists (content-addressed), skip write
        if final_path.exists() {
            return Ok(rel_path);
        }

        // Ensure parent directory exists
        let parent = final_path
            .parent()
            .ok_or_else(|| BackendError::Internal("Invalid blob path (no parent)".into()))?;
        fs::create_dir_all(parent)
            .map_err(|e| BackendError::Io(format!("Failed to create blob dir: {e}")))?;

        // Write to a temporary file in the same directory, then atomic rename
        let tmp_dir = self.blob_dir.join("tmp");
        fs::create_dir_all(&tmp_dir)
            .map_err(|e| BackendError::Io(format!("Failed to create tmp dir: {e}")))?;

        let mut tmp_file = tempfile::NamedTempFile::new_in(&tmp_dir)
            .map_err(|e| BackendError::Io(format!("Failed to create temp file: {e}")))?;

        tmp_file
            .write_all(value)
            .map_err(|e| BackendError::Io(format!("Failed to write blob: {e}")))?;

        tmp_file
            .as_file()
            .sync_all()
            .map_err(|e| BackendError::Io(format!("Failed to fsync blob: {e}")))?;

        // Atomic rename
        tmp_file
            .persist(&final_path)
            .map_err(|e| BackendError::Io(format!("Failed to rename blob: {e}")))?;

        Ok(rel_path)
    }

    /// Reads an external blob file.
    ///
    /// Validates content integrity: the SHA-256 of the bytes must match the
    /// filename (content addressing doubles as the integrity check).
    pub fn read(&self, rel_path: &str) -> Result<Vec<u8>, BackendError> {
        let full_path = self.blob_dir.join(rel_path);
        let data = fs::read(&full_path)
            .map_err(|e| BackendError::Io(format!("Failed to read blob {rel_path}: {e}")))?;

        // Verify SHA-256 matches the filename
        let hash = Sha256::digest(&data);
        let hex_hash = hex_encode(&hash);
        let expected_name = format!("{hex_hash}.bin");
        let actual_name = Path::new(rel_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if actual_name != expected_name {
            return Err(BackendError::Corrupted(format!(
                "Blob hash mismatch: expected {expected_name}, got {actual_name}"
            )));
        }

        Ok(data)
    }

    /// Removes a blob file by relative path.
    pub fn remove(&self, rel_path: &str) -> Result<(), BackendError> {
        let full_path = self.blob_dir.join(rel_path);
        if full_path.exists() {
            fs::remove_file(&full_path)
                .map_err(|e| BackendError::Io(format!("Failed to remove blob: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_externalize() {
        assert!(!BlobManager::should_externalize(&[0u8; 100]));
        assert!(!BlobManager::should_externalize(
            &[0u8; INLINE_VALUE_THRESHOLD]
        ));
        assert!(BlobManager::should_externalize(
            &[0u8; INLINE_VALUE_THRESHOLD + 1]
        ));
    }

    #[test]
    fn test_store_and_read_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BlobManager::new(tmp.path(), "testhash");
        let value = vec![42u8; 512 * 1024]; // 512 KiB

        let rel_path = mgr.store(&value).unwrap();
        assert!(rel_path.ends_with(".bin"));

        let read_back = mgr.read(&rel_path).unwrap();
        assert_eq!(read_back, value);
    }

    #[test]
    fn test_content_addressed_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BlobManager::new(tmp.path(), "testhash");
        let value = vec![7u8; 300 * 1024];

        let p1 = mgr.store(&value).unwrap();
        let p2 = mgr.store(&value).unwrap();
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_read_detects_hash_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BlobManager::new(tmp.path(), "testhash");
        let value = vec![3u8; 300 * 1024];
        let rel = mgr.store(&value).unwrap();

        // Tamper with the stored content so SHA-256 no longer matches the name.
        let full = mgr.blob_dir.join(&rel);
        std::fs::write(&full, vec![4u8; 300 * 1024]).unwrap();

        let err = mgr.read(&rel).unwrap_err();
        assert!(matches!(
            err,
            boa_idb_core::backend::error::BackendError::Corrupted(_)
        ));
    }

    #[test]
    fn test_store_parent_dir_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BlobManager::new(tmp.path(), "hash123");
        let rel = mgr.store(&[1u8; 300 * 1024]).unwrap();

        // Relative path is `<xx>/<sha256>.bin` and resolves under blobs/<hash>/.
        let full = mgr.blob_dir.join(&rel);
        assert!(full.starts_with(tmp.path().join("blobs").join("hash123")));
        assert!(full.exists());
        assert!(full.extension().is_some_and(|e| e == "bin"));
    }

    #[test]
    fn test_store_large_value_with_odd_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BlobManager::new(tmp.path(), "aabb");
        // Two distinct large values with different hashes must not collide.
        let r1 = mgr.store(&[0x11u8; 300 * 1024]).unwrap();
        let r2 = mgr.store(&[0x22u8; 300 * 1024]).unwrap();
        assert_ne!(r1, r2);
    }

    #[test]
    fn test_remove_deletes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BlobManager::new(tmp.path(), "rm");
        let rel = mgr.store(&[9u8; 300 * 1024]).unwrap();
        let full = mgr.blob_dir.join(&rel);
        assert!(full.exists());

        mgr.remove(&rel).unwrap();
        assert!(!full.exists());

        // Removing a non-existent path is a no-op.
        mgr.remove(&rel).unwrap();
    }

    #[test]
    fn test_read_missing_file_is_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = BlobManager::new(tmp.path(), "missing");
        let err = mgr.read("aa/does_not_exist.bin").unwrap_err();
        assert!(matches!(
            err,
            boa_idb_core::backend::error::BackendError::Io(_)
        ));
    }
}
