//! `meta.scf` and `CURRENT` / `MANIFEST` helpers.

use crate::atomic::atomic_write;
use crate::vfs::{FileSystem, io_to_backend};
use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::types::{DatabaseMeta, IndexMeta, StoreMeta};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const META_MAGIC: u32 = u32::from_le_bytes(*b"IMET");
const META_VERSION: u32 = 1;
const MANIFEST_MAGIC: u32 = u32::from_le_bytes(*b"IMAN");
const MANIFEST_VERSION: u32 = 1;

/// Durable manifest pointing at live segments and the active WAL generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestData {
    /// Monotonic manifest sequence (also used in `MANIFEST-<seq>` filename).
    pub manifest_seq: u64,
    /// Active WAL file id (`wal/<wal_seq>.log`).
    pub wal_seq: u64,
    /// Live immutable segment sequences (usually 0 or 1 after compaction).
    pub segments: Vec<u64>,
}

/// Reads `CURRENT` and returns the absolute manifest path.
pub fn read_current_manifest(
    db_dir: &Path,
    fs: &Arc<dyn FileSystem>,
) -> Result<PathBuf, BackendError> {
    let current = db_dir.join("CURRENT");
    let name = fs
        .read_to_string(&current)
        .map_err(|e| io_to_backend(e, "read CURRENT"))?;
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(BackendError::Corrupted(format!(
            "invalid CURRENT content: {name:?}"
        )));
    }
    Ok(db_dir.join(name))
}

/// Encodes manifest bytes with CRC32C trailer.
pub fn encode_manifest(data: &ManifestData) -> Vec<u8> {
    use boa_idb_core::clone::crc32c::crc32c;
    let mut body = Vec::new();
    body.extend_from_slice(&MANIFEST_MAGIC.to_le_bytes());
    body.extend_from_slice(&MANIFEST_VERSION.to_le_bytes());
    body.extend_from_slice(&data.manifest_seq.to_le_bytes());
    body.extend_from_slice(&data.wal_seq.to_le_bytes());
    body.extend_from_slice(&(data.segments.len() as u32).to_le_bytes());
    for seq in &data.segments {
        body.extend_from_slice(&seq.to_le_bytes());
    }
    let crc = crc32c(&body);
    body.extend_from_slice(&crc.to_le_bytes());
    body
}

/// Decodes a manifest body; corrupt input returns `Corrupted` (no panic).
pub fn decode_manifest(bytes: &[u8]) -> Result<ManifestData, BackendError> {
    use boa_idb_core::clone::crc32c::crc32c;
    if bytes.len() < 4 + 4 + 8 + 8 + 4 + 4 {
        return Err(BackendError::Corrupted("manifest too short".into()));
    }
    let body = &bytes[..bytes.len() - 4];
    let expected = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
    let actual = crc32c(body);
    if expected != actual {
        return Err(BackendError::Corrupted(format!(
            "manifest crc mismatch expected={expected:#x} actual={actual:#x}"
        )));
    }
    let magic = u32::from_le_bytes(body[0..4].try_into().unwrap());
    if magic != MANIFEST_MAGIC {
        return Err(BackendError::Corrupted(format!(
            "bad manifest magic {magic:#x}"
        )));
    }
    let ver = u32::from_le_bytes(body[4..8].try_into().unwrap());
    if ver != MANIFEST_VERSION {
        return Err(BackendError::Corrupted(format!(
            "unsupported manifest version {ver}"
        )));
    }
    let manifest_seq = u64::from_le_bytes(body[8..16].try_into().unwrap());
    let wal_seq = u64::from_le_bytes(body[16..24].try_into().unwrap());
    let n = u32::from_le_bytes(body[24..28].try_into().unwrap()) as usize;
    let mut pos = 28;
    let mut segments = Vec::with_capacity(n);
    for _ in 0..n {
        if pos + 8 > body.len() {
            return Err(BackendError::Corrupted(
                "truncated manifest segments".into(),
            ));
        }
        segments.push(u64::from_le_bytes(body[pos..pos + 8].try_into().unwrap()));
        pos += 8;
    }
    if pos != body.len() {
        return Err(BackendError::Corrupted("trailing manifest bytes".into()));
    }
    Ok(ManifestData {
        manifest_seq,
        wal_seq,
        segments,
    })
}

/// Reads and decodes the manifest referenced by `CURRENT`.
pub fn load_manifest(
    db_dir: &Path,
    fs: &Arc<dyn FileSystem>,
) -> Result<ManifestData, BackendError> {
    let path = read_current_manifest(db_dir, fs)?;
    let bytes = fs
        .read(&path)
        .map_err(|e| io_to_backend(e, "read MANIFEST"))?;
    decode_manifest(&bytes)
}

/// Writes a new `MANIFEST-<seq>` and updates `CURRENT` atomically.
///
/// Publication order: manifest file fully synced, then `CURRENT` rename.
pub fn write_manifest(
    db_dir: &Path,
    data: &ManifestData,
    sync: bool,
    fs: &Arc<dyn FileSystem>,
) -> Result<(), BackendError> {
    let name = format!("MANIFEST-{:06}", data.manifest_seq);
    let path = db_dir.join(&name);
    let body = encode_manifest(data);
    atomic_write(&path, &body, sync, fs)?;
    atomic_write(
        &db_dir.join("CURRENT"),
        format!("{name}\n").as_bytes(),
        sync,
        fs,
    )?;
    Ok(())
}

/// Encodes [`DatabaseMeta`] to `meta.scf` bytes.
pub fn encode_meta(meta: &DatabaseMeta) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&META_MAGIC.to_le_bytes());
    out.extend_from_slice(&META_VERSION.to_le_bytes());
    write_utf16(&mut out, &meta.name);
    out.extend_from_slice(&meta.version.to_le_bytes());
    out.extend_from_slice(&meta.next_store_id.to_le_bytes());
    out.extend_from_slice(&meta.next_index_id.to_le_bytes());
    out.extend_from_slice(&(meta.stores.len() as u32).to_le_bytes());
    for store in &meta.stores {
        encode_store(&mut out, store);
    }
    out
}

/// Decodes `meta.scf` bytes.
pub fn decode_meta(bytes: &[u8]) -> Result<DatabaseMeta, BackendError> {
    let mut r = Reader::new(bytes);
    let magic = r.u32()?;
    if magic != META_MAGIC {
        return Err(BackendError::Corrupted(format!(
            "bad meta magic {magic:#x}"
        )));
    }
    let ver = r.u32()?;
    if ver != META_VERSION {
        return Err(BackendError::Corrupted(format!(
            "unsupported meta version {ver}"
        )));
    }
    let name = r.utf16()?;
    let version = r.u64()?;
    let next_store_id = r.u64()?;
    let next_index_id = r.u64()?;
    let store_count = r.u32()? as usize;
    let mut stores = Vec::with_capacity(store_count);
    for _ in 0..store_count {
        stores.push(decode_store(&mut r)?);
    }
    if !r.rest().is_empty() {
        return Err(BackendError::Corrupted("trailing meta bytes".into()));
    }
    Ok(DatabaseMeta {
        name,
        version,
        stores,
        next_store_id,
        next_index_id,
    })
}

/// Writes `meta.scf` atomically.
pub fn write_meta_file(
    db_dir: &Path,
    meta: &DatabaseMeta,
    sync: bool,
    fs: &Arc<dyn FileSystem>,
) -> Result<(), BackendError> {
    atomic_write(&db_dir.join("meta.scf"), &encode_meta(meta), sync, fs)
}

/// Reads `meta.scf` if present.
pub fn read_meta_file(
    db_dir: &Path,
    fs: &Arc<dyn FileSystem>,
) -> Result<Option<DatabaseMeta>, BackendError> {
    let path = db_dir.join("meta.scf");
    if !fs.exists(&path) {
        return Ok(None);
    }
    let bytes = fs
        .read(&path)
        .map_err(|e| io_to_backend(e, "read meta.scf"))?;
    Ok(Some(decode_meta(&bytes)?))
}

/// Resolves database metadata from `meta.scf` plus any committed WAL frames.
///
/// Used by `list_databases` so a crash window where WAL advanced ahead of
/// `meta.scf` still reports the recovered name/version.
pub fn load_meta_with_wal(
    db_dir: &Path,
    fs: &Arc<dyn FileSystem>,
) -> Result<Option<DatabaseMeta>, BackendError> {
    let has_current = fs.exists(&db_dir.join("CURRENT"));
    let has_meta = fs.exists(&db_dir.join("meta.scf"));
    if !has_current && !has_meta {
        return Ok(None);
    }

    let mut state = crate::state::DbState {
        meta: read_meta_file(db_dir, fs)?,
        ..crate::state::DbState::default()
    };
    let wal_seq = load_manifest(db_dir, fs).map(|m| m.wal_seq).unwrap_or(1);
    let wal_path = crate::compact::wal_path(db_dir, wal_seq);
    if fs.exists(&wal_path) {
        let bytes = fs
            .read(&wal_path)
            .map_err(|e| io_to_backend(e, "read wal for list"))?;
        let recovered = crate::wal::recover_committed_frames(&bytes);
        crate::apply::apply_frames(&mut state, &recovered.frames)?;
    }
    Ok(state.meta)
}

fn encode_store(out: &mut Vec<u8>, store: &StoreMeta) {
    out.extend_from_slice(&store.id.to_le_bytes());
    write_utf16(out, &store.name);
    encode_key_path(out, &store.key_path);
    out.push(u8::from(store.auto_increment));
    out.extend_from_slice(&store.key_gen.to_bits().to_le_bytes());
    out.push(u8::from(store.deleted));
    out.extend_from_slice(&(store.indexes.len() as u32).to_le_bytes());
    for index in &store.indexes {
        encode_index(out, index);
    }
}

fn decode_store(r: &mut Reader<'_>) -> Result<StoreMeta, BackendError> {
    let id = r.u64()?;
    let name = r.utf16()?;
    let key_path = decode_key_path(r)?;
    let auto_increment = r.u8()? != 0;
    let key_gen = f64::from_bits(r.u64()?);
    let deleted = r.u8()? != 0;
    let n = r.u32()? as usize;
    let mut indexes = Vec::with_capacity(n);
    for _ in 0..n {
        indexes.push(decode_index(r)?);
    }
    Ok(StoreMeta {
        id,
        name,
        key_path,
        auto_increment,
        key_gen,
        indexes,
        deleted,
    })
}

fn encode_index(out: &mut Vec<u8>, index: &IndexMeta) {
    out.extend_from_slice(&index.id.to_le_bytes());
    out.extend_from_slice(&index.store_id.to_le_bytes());
    write_utf16(out, &index.name);
    encode_key_path(out, &index.key_path);
    out.push(u8::from(index.unique));
    out.push(u8::from(index.multi_entry));
    out.push(u8::from(index.deleted));
}

fn decode_index(r: &mut Reader<'_>) -> Result<IndexMeta, BackendError> {
    Ok(IndexMeta {
        id: r.u64()?,
        store_id: r.u64()?,
        name: r.utf16()?,
        key_path: decode_key_path(r)?,
        unique: r.u8()? != 0,
        multi_entry: r.u8()? != 0,
        deleted: r.u8()? != 0,
    })
}

fn encode_key_path(out: &mut Vec<u8>, path: &KeyPath) {
    match path {
        KeyPath::Empty => out.push(0),
        KeyPath::Single(s) => {
            out.push(1);
            write_utf16(out, s);
        }
        KeyPath::Array(items) => {
            out.push(2);
            out.extend_from_slice(&(items.len() as u32).to_le_bytes());
            for item in items {
                write_utf16(out, item);
            }
        }
    }
}

fn decode_key_path(r: &mut Reader<'_>) -> Result<KeyPath, BackendError> {
    match r.u8()? {
        0 => Ok(KeyPath::Empty),
        1 => Ok(KeyPath::Single(r.utf16()?)),
        2 => {
            let n = r.u32()? as usize;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                items.push(r.utf16()?);
            }
            Ok(KeyPath::Array(items))
        }
        other => Err(BackendError::Corrupted(format!("bad keypath tag {other}"))),
    }
}

fn write_utf16(out: &mut Vec<u8>, s: &Utf16String) {
    let units = s.as_slice();
    out.extend_from_slice(&(units.len() as u32).to_le_bytes());
    for &u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
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
            return Err(BackendError::Corrupted("truncated meta".into()));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, BackendError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, BackendError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, BackendError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn utf16(&mut self) -> Result<Utf16String, BackendError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len * 2)?;
        let mut units = Vec::with_capacity(len);
        for chunk in bytes.chunks_exact(2) {
            units.push(u16::from_le_bytes(chunk.try_into().unwrap()));
        }
        Ok(Utf16String::from_slice(&units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_idb_core::key::path::KeyPath;
    use boa_idb_core::key::utf16::Utf16String;

    #[test]
    fn meta_roundtrip() {
        let meta = DatabaseMeta {
            name: Utf16String::from_str("demo"),
            version: 3,
            stores: vec![StoreMeta {
                id: 1,
                name: Utf16String::from_str("s"),
                key_path: KeyPath::Single(Utf16String::from_str("id")),
                auto_increment: true,
                key_gen: 2.0,
                indexes: vec![],
                deleted: false,
            }],
            next_store_id: 2,
            next_index_id: 1,
        };
        let bytes = encode_meta(&meta);
        assert_eq!(decode_meta(&bytes).unwrap(), meta);
    }
}
