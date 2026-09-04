//! Immutable segment codec (`seg/<seq>.seg`) — R8.3.3.

use crate::meta::{decode_meta, encode_meta};
use crate::state::{DbState, IndexKey, RecordKey, SegmentGuard};
use crate::sync_hooks::{SyncHooks, io_to_backend};
use boa_idb_core::backend::error::BackendError;
use boa_idb_core::clone::crc32c::crc32c;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Segment magic `ISEG`.
pub const SEG_MAGIC: u32 = u32::from_le_bytes(*b"ISEG");
/// Segment format version.
pub const SEG_VERSION: u32 = 1;

/// Absolute path for `seg/<seq>.seg`.
pub fn segment_path(db_dir: &Path, seq: u64) -> PathBuf {
    db_dir.join("seg").join(format!("{seq:06}.seg"))
}

/// Encodes a full durable snapshot into segment bytes (body + CRC32C).
pub fn encode_segment(seq: u64, state: &DbState) -> Result<Vec<u8>, BackendError> {
    let meta = state
        .meta
        .as_ref()
        .ok_or_else(|| BackendError::Internal("segment encode without meta".into()))?;
    let mut body = Vec::new();
    body.extend_from_slice(&SEG_MAGIC.to_le_bytes());
    body.extend_from_slice(&SEG_VERSION.to_le_bytes());
    body.extend_from_slice(&seq.to_le_bytes());
    body.extend_from_slice(&state.next_txn_seq.to_le_bytes());

    let meta_bytes = encode_meta(meta);
    body.extend_from_slice(&(meta_bytes.len() as u32).to_le_bytes());
    body.extend_from_slice(&meta_bytes);

    body.extend_from_slice(&(state.records.size() as u32).to_le_bytes());
    for (key, value) in state.records.iter() {
        write_record_key(&mut body, key);
        write_bytes(&mut body, value);
    }

    body.extend_from_slice(&(state.index_entries.size() as u32).to_le_bytes());
    for (key, ()) in state.index_entries.iter() {
        write_index_key(&mut body, key);
    }

    body.extend_from_slice(&(state.key_generators.len() as u32).to_le_bytes());
    for (store, value) in &state.key_generators {
        body.extend_from_slice(&store.to_le_bytes());
        body.extend_from_slice(&value.to_bits().to_le_bytes());
    }

    let crc = crc32c(&body);
    body.extend_from_slice(&crc.to_le_bytes());
    Ok(body)
}

/// Decodes a segment file into `state` fields (replaces maps).
pub fn decode_segment(bytes: &[u8], state: &mut DbState) -> Result<u64, BackendError> {
    if bytes.len() < 4 + 4 + 8 + 8 + 4 + 4 {
        return Err(BackendError::Corrupted("segment too short".into()));
    }
    let body = &bytes[..bytes.len() - 4];
    let expected = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
    let actual = crc32c(body);
    if expected != actual {
        return Err(BackendError::Corrupted(format!(
            "segment crc mismatch expected={expected:#x} actual={actual:#x}"
        )));
    }
    let mut r = Reader::new(body);
    let magic = r.u32()?;
    if magic != SEG_MAGIC {
        return Err(BackendError::Corrupted(format!(
            "bad segment magic {magic:#x}"
        )));
    }
    let ver = r.u32()?;
    if ver != SEG_VERSION {
        return Err(BackendError::Corrupted(format!(
            "unsupported segment version {ver}"
        )));
    }
    let seq = r.u64()?;
    state.next_txn_seq = r.u64()?;
    let meta_len = r.u32()? as usize;
    let meta_bytes = r.take(meta_len)?;
    state.meta = Some(decode_meta(meta_bytes)?);

    let n_records = r.u32()? as usize;
    let mut records = crate::state::PersistMap::new_with_ptr_kind();
    for _ in 0..n_records {
        let key = read_record_key(&mut r)?;
        let value = read_bytes(&mut r)?;
        records.insert_mut(key, value);
    }
    state.records = records;

    let n_index = r.u32()? as usize;
    let mut index_entries = crate::state::PersistMap::new_with_ptr_kind();
    for _ in 0..n_index {
        let key = read_index_key(&mut r)?;
        index_entries.insert_mut(key, ());
    }
    state.index_entries = index_entries;

    let n_kg = r.u32()? as usize;
    state.key_generators.clear();
    for _ in 0..n_kg {
        let store = r.u64()?;
        let bits = r.u64()?;
        state.key_generators.insert(store, f64::from_bits(bits));
    }
    if !r.rest().is_empty() {
        return Err(BackendError::Corrupted("trailing segment bytes".into()));
    }
    Ok(seq)
}

/// Writes `state` to a new segment file and returns a live guard.
pub fn write_segment_file(
    db_dir: &Path,
    seq: u64,
    state: &DbState,
    sync: bool,
    hooks: &Arc<dyn SyncHooks>,
) -> Result<Arc<SegmentGuard>, BackendError> {
    fs::create_dir_all(db_dir.join("seg")).map_err(|e| io_to_backend(e, "create seg dir"))?;
    let path = segment_path(db_dir, seq);
    let bytes = encode_segment(seq, state)?;
    crate::atomic::atomic_write(&path, &bytes, sync, hooks)?;
    Ok(SegmentGuard::adopt_live(seq, path))
}

/// Loads a segment file into `state` and adopts a guard.
pub fn load_segment_file(
    db_dir: &Path,
    seq: u64,
    state: &mut DbState,
) -> Result<Arc<SegmentGuard>, BackendError> {
    let path = segment_path(db_dir, seq);
    let bytes = fs::read(&path).map_err(|e| io_to_backend(e, "read segment"))?;
    let decoded = decode_segment(&bytes, state)?;
    if decoded != seq {
        return Err(BackendError::Corrupted(format!(
            "segment seq mismatch file={seq} body={decoded}"
        )));
    }
    Ok(SegmentGuard::adopt_live(seq, path))
}

fn write_record_key(out: &mut Vec<u8>, key: &RecordKey) {
    out.extend_from_slice(&key.0.to_le_bytes());
    write_bytes(out, &key.1);
}

fn read_record_key(r: &mut Reader<'_>) -> Result<RecordKey, BackendError> {
    let store = r.u64()?;
    let key = read_bytes(r)?;
    Ok((store, key))
}

fn write_index_key(out: &mut Vec<u8>, key: &IndexKey) {
    out.extend_from_slice(&key.0.to_le_bytes());
    write_bytes(out, &key.1);
    write_bytes(out, &key.2);
}

fn read_index_key(r: &mut Reader<'_>) -> Result<IndexKey, BackendError> {
    let index = r.u64()?;
    let idx_key = read_bytes(r)?;
    let primary = read_bytes(r)?;
    Ok((index, idx_key, primary))
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn read_bytes(r: &mut Reader<'_>) -> Result<Vec<u8>, BackendError> {
    let len = r.u32()? as usize;
    Ok(r.take(len)?.to_vec())
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], BackendError> {
        if self.pos + n > self.buf.len() {
            return Err(BackendError::Corrupted("truncated segment".into()));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, BackendError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, BackendError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_idb_core::backend::types::DatabaseMeta;
    use boa_idb_core::key::utf16::Utf16String;

    #[test]
    fn segment_roundtrip() {
        let mut state = DbState {
            meta: Some(DatabaseMeta {
                name: Utf16String::from_str("db"),
                version: 2,
                stores: vec![],
                next_store_id: 1,
                next_index_id: 1,
            }),
            next_txn_seq: 9,
            ..DbState::default()
        };
        state.records.insert_mut((1, b"k".to_vec()), b"v".to_vec());
        state
            .index_entries
            .insert_mut((1, b"ik".to_vec(), b"k".to_vec()), ());
        state.key_generators.insert(1, 3.0);
        let bytes = encode_segment(7, &state).unwrap();
        let mut out = DbState::default();
        assert_eq!(decode_segment(&bytes, &mut out).unwrap(), 7);
        assert_eq!(out.next_txn_seq, 9);
        assert_eq!(out.records.get(&(1, b"k".to_vec())).unwrap(), b"v");
        assert!(
            out.index_entries
                .contains_key(&(1, b"ik".to_vec(), b"k".to_vec()))
        );
        assert!((out.key_generators.get(&1).copied().unwrap() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn corrupt_segment_crc_is_error_not_panic() {
        let mut state = DbState {
            meta: Some(DatabaseMeta {
                name: Utf16String::from_str("db"),
                version: 1,
                stores: vec![],
                next_store_id: 1,
                next_index_id: 1,
            }),
            ..DbState::default()
        };
        let mut bytes = encode_segment(1, &state).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert!(decode_segment(&bytes, &mut state).is_err());
    }
}
