//! WAL frame codec (R8.3.1).

use boa_idb_core::clone::crc32c::crc32c;
use boa_idb_core::proto::{IndexId, StoreId};

/// Magic bytes `IWAL` as little-endian u32.
pub const WAL_MAGIC: u32 = u32::from_le_bytes(*b"IWAL");

/// Frame continues a multi-frame transaction (must not combine with [`FLAG_COMMIT`]).
pub const FLAG_CONTINUES: u8 = 0x01;
/// Frame commits a transaction — either alone or as the final frame of a chain
/// (must not combine with [`FLAG_CONTINUES`]).
pub const FLAG_COMMIT: u8 = 0x02;

/// Maximum accepted frame payload length (64 MiB) to reject length overflow.
pub const MAX_FRAME_PAYLOAD: u32 = 64 * 1024 * 1024;

/// WAL operation kinds inside a frame payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalOp {
    /// Insert or replace a store record.
    Put {
        /// Store id.
        store: StoreId,
        /// Encoded key.
        key: Vec<u8>,
        /// Value bytes.
        value: Vec<u8>,
    },
    /// Delete a store record.
    Delete {
        /// Store id.
        store: StoreId,
        /// Encoded key.
        key: Vec<u8>,
    },
    /// Clear all records of a store.
    Clear {
        /// Store id.
        store: StoreId,
    },
    /// Insert an index entry.
    IndexPut {
        /// Index id.
        index: IndexId,
        /// Index key bytes.
        idx_key: Vec<u8>,
        /// Primary key bytes.
        primary_key: Vec<u8>,
    },
    /// Delete an index entry.
    IndexDelete {
        /// Index id.
        index: IndexId,
        /// Index key bytes.
        idx_key: Vec<u8>,
        /// Primary key bytes.
        primary_key: Vec<u8>,
    },
    /// Set key generator.
    KeyGenSet {
        /// Store id.
        store: StoreId,
        /// Generator value bits.
        value_bits: u64,
    },
    /// Replace database metadata blob (`meta.scf` payload).
    MetaReplace {
        /// Encoded metadata.
        bytes: Vec<u8>,
    },
}

const OP_PUT: u8 = 1;
const OP_DELETE: u8 = 2;
const OP_CLEAR: u8 = 3;
const OP_INDEX_PUT: u8 = 4;
const OP_INDEX_DELETE: u8 = 5;
const OP_KEY_GEN: u8 = 6;
const OP_META: u8 = 7;

/// Decoded WAL frame header + payload ops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalFrame {
    /// Transaction sequence number.
    pub txn_seq: u64,
    /// Frame flags.
    pub flags: u8,
    /// Operations.
    pub ops: Vec<WalOp>,
}

impl WalFrame {
    /// Returns true when this frame finalizes a transaction.
    pub fn is_commit(&self) -> bool {
        self.flags & FLAG_COMMIT != 0
    }

    /// Returns true when more frames belong to the same transaction.
    pub fn continues(&self) -> bool {
        self.flags & FLAG_CONTINUES != 0
    }
}

/// Encodes a frame to bytes (magic..crc32c).
pub fn encode_frame(frame: &WalFrame) -> Result<Vec<u8>, CodecError> {
    encode_frame_limited(frame, MAX_FRAME_PAYLOAD)
}

/// Decodes the next frame from `input`, returning `(frame, bytes_consumed)`.
///
/// Returns `Ok(None)` on clean EOF. Returns `Err(Torn)` when the remaining
/// bytes are a partial/corrupt frame (recovery must stop and keep prefix).
pub fn decode_frame(input: &[u8]) -> Result<Option<(WalFrame, usize)>, CodecError> {
    if input.is_empty() {
        return Ok(None);
    }
    if input.len() < 4 + 4 + 8 + 1 + 4 {
        return Err(CodecError::Torn);
    }
    let magic = u32::from_le_bytes(input[0..4].try_into().unwrap());
    if magic != WAL_MAGIC {
        return Err(CodecError::BadMagic(magic));
    }
    let len = u32::from_le_bytes(input[4..8].try_into().unwrap());
    if len > MAX_FRAME_PAYLOAD {
        return Err(CodecError::PayloadTooLarge(len as usize));
    }
    let header_and_payload = 4 + 4 + 8 + 1 + len as usize;
    let total = header_and_payload + 4;
    if input.len() < total {
        return Err(CodecError::Torn);
    }
    let body = &input[..header_and_payload];
    let expected = u32::from_le_bytes(input[header_and_payload..total].try_into().unwrap());
    let actual = crc32c(body);
    if expected != actual {
        return Err(CodecError::BadCrc { expected, actual });
    }
    let txn_seq = u64::from_le_bytes(input[8..16].try_into().unwrap());
    let flags = input[16];
    let payload = &input[17..header_and_payload];
    let ops = decode_ops(payload)?;
    Ok(Some((
        WalFrame {
            txn_seq,
            flags,
            ops,
        },
        total,
    )))
}

/// Scans a WAL buffer and returns only complete committed transactions.
///
/// A transaction is either a single `COMMIT` frame or a chain
/// `CONTINUES*(same seq) → COMMIT(same seq)`. Sequence mismatches, illegal
/// flag combinations, torn/corrupt frames, and incomplete chains stop recovery
/// at the end of the last fully committed transaction. Defective bytes are
/// never skipped in search of a later `COMMIT`.
pub fn recover_committed_frames(buf: &[u8]) -> RecoveredWal {
    let mut offset = 0usize;
    let mut committed = Vec::new();
    let mut pending: Vec<WalFrame> = Vec::new();
    let mut pending_seq: Option<u64> = None;
    let mut valid_prefix = 0usize;

    while offset < buf.len() {
        match decode_frame(&buf[offset..]) {
            Ok(Some((frame, consumed))) => {
                let next = offset + consumed;
                let flags = frame.flags;
                let is_commit = flags == FLAG_COMMIT;
                let is_continues = flags == FLAG_CONTINUES;
                if !is_commit && !is_continues {
                    // Illegal flags (0, CONTINUES|COMMIT, unknown bits): stop.
                    pending.clear();
                    break;
                }

                match pending_seq {
                    None => {
                        if is_commit {
                            offset = next;
                            committed.push(frame);
                            valid_prefix = offset;
                        } else {
                            // Start a multi-frame transaction.
                            offset = next;
                            pending_seq = Some(frame.txn_seq);
                            pending.push(frame);
                        }
                    }
                    Some(seq) => {
                        if frame.txn_seq != seq {
                            pending.clear();
                            break;
                        }
                        if is_continues {
                            offset = next;
                            pending.push(frame);
                        } else {
                            // Matching COMMIT closes the chain.
                            offset = next;
                            pending.push(frame);
                            committed.append(&mut pending);
                            pending_seq = None;
                            valid_prefix = offset;
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => {
                pending.clear();
                break;
            }
        }
    }

    RecoveredWal {
        frames: committed,
        valid_prefix_len: valid_prefix,
    }
}

/// Encodes one logical transaction as one or more WAL frames under
/// [`MAX_FRAME_PAYLOAD`].
///
/// Intermediate frames use only [`FLAG_CONTINUES`]; the last uses only
/// [`FLAG_COMMIT`]. A single operation that cannot fit returns
/// [`CodecError::PayloadTooLarge`] without producing frames.
pub fn encode_txn_frames(txn_seq: u64, ops: &[WalOp]) -> Result<Vec<Vec<u8>>, CodecError> {
    encode_txn_frames_limited(txn_seq, ops, MAX_FRAME_PAYLOAD)
}

/// Like [`encode_txn_frames`] with an injectable payload limit (tests).
pub fn encode_txn_frames_limited(
    txn_seq: u64,
    ops: &[WalOp],
    max_payload: u32,
) -> Result<Vec<Vec<u8>>, CodecError> {
    if ops.is_empty() {
        return Ok(Vec::new());
    }

    let mut groups: Vec<Vec<WalOp>> = Vec::new();
    let mut current: Vec<WalOp> = Vec::new();
    for op in ops {
        let mut candidate = current.clone();
        candidate.push(op.clone());
        match payload_len(&candidate) {
            Ok(len) if len <= max_payload as usize => {
                current = candidate;
            }
            Ok(_) | Err(CodecError::PayloadTooLarge(_)) => {
                if current.is_empty() {
                    // Lone operation exceeds the limit.
                    let alone = payload_len(std::slice::from_ref(op))?;
                    return Err(CodecError::PayloadTooLarge(alone));
                }
                groups.push(std::mem::take(&mut current));
                current.push(op.clone());
                let alone = payload_len(&current)?;
                if alone > max_payload as usize {
                    return Err(CodecError::PayloadTooLarge(alone));
                }
            }
            Err(err) => return Err(err),
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let last = groups.len() - 1;
    let mut out = Vec::with_capacity(groups.len());
    for (i, group) in groups.into_iter().enumerate() {
        let flags = if i == last {
            FLAG_COMMIT
        } else {
            FLAG_CONTINUES
        };
        out.push(encode_frame_limited(
            &WalFrame {
                txn_seq,
                flags,
                ops: group,
            },
            max_payload,
        )?);
    }
    Ok(out)
}

fn payload_len(ops: &[WalOp]) -> Result<usize, CodecError> {
    let mut payload = Vec::new();
    write_varint(&mut payload, ops.len() as u64);
    for op in ops {
        encode_op(&mut payload, op)?;
    }
    Ok(payload.len())
}

fn encode_frame_limited(frame: &WalFrame, max_payload: u32) -> Result<Vec<u8>, CodecError> {
    let mut payload = Vec::new();
    write_varint(&mut payload, frame.ops.len() as u64);
    for op in &frame.ops {
        encode_op(&mut payload, op)?;
    }
    if payload.len() > max_payload as usize {
        return Err(CodecError::PayloadTooLarge(payload.len()));
    }

    let mut out = Vec::with_capacity(4 + 4 + 8 + 1 + payload.len() + 4);
    out.extend_from_slice(&WAL_MAGIC.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&frame.txn_seq.to_le_bytes());
    out.push(frame.flags);
    out.extend_from_slice(&payload);
    let crc = crc32c(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(out)
}

/// Result of WAL prefix recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredWal {
    /// Frames belonging to fully committed transactions, in order.
    pub frames: Vec<WalFrame>,
    /// Byte length of the valid committed prefix.
    pub valid_prefix_len: usize,
}

/// Codec errors (never panic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Incomplete trailing bytes.
    Torn,
    /// Unexpected magic.
    BadMagic(u32),
    /// CRC mismatch.
    BadCrc {
        /// Expected trailer.
        expected: u32,
        /// Computed checksum.
        actual: u32,
    },
    /// Payload exceeds limit.
    PayloadTooLarge(usize),
    /// Malformed payload varints/ops.
    Malformed(&'static str),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Torn => write!(f, "torn WAL frame"),
            Self::BadMagic(m) => write!(f, "bad WAL magic {m:#x}"),
            Self::BadCrc { expected, actual } => {
                write!(f, "bad WAL crc expected={expected:#x} actual={actual:#x}")
            }
            Self::PayloadTooLarge(n) => write!(f, "WAL payload too large ({n})"),
            Self::Malformed(msg) => write!(f, "malformed WAL payload: {msg}"),
        }
    }
}

impl std::error::Error for CodecError {}

fn encode_op(out: &mut Vec<u8>, op: &WalOp) -> Result<(), CodecError> {
    match op {
        WalOp::Put { store, key, value } => {
            out.push(OP_PUT);
            write_varint(out, *store);
            write_bytes(out, key);
            write_bytes(out, value);
        }
        WalOp::Delete { store, key } => {
            out.push(OP_DELETE);
            write_varint(out, *store);
            write_bytes(out, key);
        }
        WalOp::Clear { store } => {
            out.push(OP_CLEAR);
            write_varint(out, *store);
        }
        WalOp::IndexPut {
            index,
            idx_key,
            primary_key,
        } => {
            out.push(OP_INDEX_PUT);
            write_varint(out, *index);
            write_bytes(out, idx_key);
            write_bytes(out, primary_key);
        }
        WalOp::IndexDelete {
            index,
            idx_key,
            primary_key,
        } => {
            out.push(OP_INDEX_DELETE);
            write_varint(out, *index);
            write_bytes(out, idx_key);
            write_bytes(out, primary_key);
        }
        WalOp::KeyGenSet { store, value_bits } => {
            out.push(OP_KEY_GEN);
            write_varint(out, *store);
            out.extend_from_slice(&value_bits.to_le_bytes());
        }
        WalOp::MetaReplace { bytes } => {
            out.push(OP_META);
            write_bytes(out, bytes);
        }
    }
    Ok(())
}

fn decode_ops(mut payload: &[u8]) -> Result<Vec<WalOp>, CodecError> {
    let (count, rest) = read_varint(payload)?;
    payload = rest;
    let mut ops = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if payload.is_empty() {
            return Err(CodecError::Malformed("truncated ops"));
        }
        let kind = payload[0];
        payload = &payload[1..];
        let op = match kind {
            OP_PUT => {
                let (store, r) = read_varint(payload)?;
                let (key, r) = read_bytes(r)?;
                let (value, r) = read_bytes(r)?;
                payload = r;
                WalOp::Put { store, key, value }
            }
            OP_DELETE => {
                let (store, r) = read_varint(payload)?;
                let (key, r) = read_bytes(r)?;
                payload = r;
                WalOp::Delete { store, key }
            }
            OP_CLEAR => {
                let (store, r) = read_varint(payload)?;
                payload = r;
                WalOp::Clear { store }
            }
            OP_INDEX_PUT => {
                let (index, r) = read_varint(payload)?;
                let (idx_key, r) = read_bytes(r)?;
                let (primary_key, r) = read_bytes(r)?;
                payload = r;
                WalOp::IndexPut {
                    index,
                    idx_key,
                    primary_key,
                }
            }
            OP_INDEX_DELETE => {
                let (index, r) = read_varint(payload)?;
                let (idx_key, r) = read_bytes(r)?;
                let (primary_key, r) = read_bytes(r)?;
                payload = r;
                WalOp::IndexDelete {
                    index,
                    idx_key,
                    primary_key,
                }
            }
            OP_KEY_GEN => {
                let (store, r) = read_varint(payload)?;
                if r.len() < 8 {
                    return Err(CodecError::Malformed("keygen bits"));
                }
                let value_bits = u64::from_le_bytes(r[..8].try_into().unwrap());
                payload = &r[8..];
                WalOp::KeyGenSet { store, value_bits }
            }
            OP_META => {
                let (bytes, r) = read_bytes(payload)?;
                payload = r;
                WalOp::MetaReplace { bytes }
            }
            _ => return Err(CodecError::Malformed("unknown op kind")),
        };
        ops.push(op);
    }
    if !payload.is_empty() {
        return Err(CodecError::Malformed("trailing payload bytes"));
    }
    Ok(ops)
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
            out.push(b);
        } else {
            out.push(b);
            break;
        }
    }
}

fn read_varint(input: &[u8]) -> Result<(u64, &[u8]), CodecError> {
    let mut result = 0u64;
    let mut shift = 0u32;
    for (i, &b) in input.iter().enumerate() {
        if shift >= 64 {
            return Err(CodecError::Malformed("varint overflow"));
        }
        result |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok((result, &input[i + 1..]));
        }
        shift += 7;
    }
    Err(CodecError::Malformed("truncated varint"))
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    write_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn read_bytes(input: &[u8]) -> Result<(Vec<u8>, &[u8]), CodecError> {
    let (len, rest) = read_varint(input)?;
    let len = len as usize;
    if rest.len() < len {
        return Err(CodecError::Malformed("truncated bytes"));
    }
    Ok((rest[..len].to_vec(), &rest[len..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_idb_core::clone::crc32c::crc32c;
    use proptest::prelude::*;

    #[test]
    fn roundtrip_commit_frame() {
        let frame = WalFrame {
            txn_seq: 7,
            flags: FLAG_COMMIT,
            ops: vec![
                WalOp::Put {
                    store: 1,
                    key: b"k".to_vec(),
                    value: b"v".to_vec(),
                },
                WalOp::Delete {
                    store: 1,
                    key: b"x".to_vec(),
                },
            ],
        };
        let bytes = encode_frame(&frame).unwrap();
        let (decoded, n) = decode_frame(&bytes).unwrap().unwrap();
        assert_eq!(n, bytes.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn recovery_drops_torn_tail_and_incomplete_txn() {
        let commit = encode_frame(&WalFrame {
            txn_seq: 1,
            flags: FLAG_COMMIT,
            ops: vec![WalOp::Clear { store: 1 }],
        })
        .unwrap();
        let cont = encode_frame(&WalFrame {
            txn_seq: 2,
            flags: FLAG_CONTINUES,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"a".to_vec(),
                value: b"b".to_vec(),
            }],
        })
        .unwrap();
        let mut buf = commit.clone();
        buf.extend_from_slice(&cont);
        buf.extend_from_slice(&[0xff, 0x00, 0x01]); // torn junk
        let recovered = recover_committed_frames(&buf);
        assert_eq!(recovered.valid_prefix_len, commit.len());
        assert_eq!(recovered.frames.len(), 1);
        assert_eq!(recovered.frames[0].txn_seq, 1);
    }

    #[test]
    fn recovery_rejects_continues_then_commit_with_different_seq() {
        let cont = encode_frame(&WalFrame {
            txn_seq: 41,
            flags: FLAG_CONTINUES,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"a".to_vec(),
                value: b"1".to_vec(),
            }],
        })
        .unwrap();
        let commit = encode_frame(&WalFrame {
            txn_seq: 42,
            flags: FLAG_COMMIT,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"b".to_vec(),
                value: b"2".to_vec(),
            }],
        })
        .unwrap();
        let mut buf = cont;
        buf.extend_from_slice(&commit);
        let recovered = recover_committed_frames(&buf);
        assert_eq!(recovered.valid_prefix_len, 0);
        assert!(recovered.frames.is_empty());
    }

    #[test]
    fn recovery_keeps_prior_commit_when_later_chain_mismatches() {
        let first = encode_frame(&WalFrame {
            txn_seq: 41,
            flags: FLAG_COMMIT,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"ok".to_vec(),
                value: b"1".to_vec(),
            }],
        })
        .unwrap();
        let cont = encode_frame(&WalFrame {
            txn_seq: 42,
            flags: FLAG_CONTINUES,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"x".to_vec(),
                value: b"2".to_vec(),
            }],
        })
        .unwrap();
        let bad = encode_frame(&WalFrame {
            txn_seq: 43,
            flags: FLAG_COMMIT,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"y".to_vec(),
                value: b"3".to_vec(),
            }],
        })
        .unwrap();
        let mut buf = first.clone();
        buf.extend_from_slice(&cont);
        buf.extend_from_slice(&bad);
        let recovered = recover_committed_frames(&buf);
        assert_eq!(recovered.valid_prefix_len, first.len());
        assert_eq!(recovered.frames.len(), 1);
        assert_eq!(recovered.frames[0].txn_seq, 41);
    }

    #[test]
    fn recovery_applies_matching_continues_commit_chain() {
        let cont = encode_frame(&WalFrame {
            txn_seq: 41,
            flags: FLAG_CONTINUES,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"a".to_vec(),
                value: b"1".to_vec(),
            }],
        })
        .unwrap();
        let commit = encode_frame(&WalFrame {
            txn_seq: 41,
            flags: FLAG_COMMIT,
            ops: vec![WalOp::Put {
                store: 1,
                key: b"b".to_vec(),
                value: b"2".to_vec(),
            }],
        })
        .unwrap();
        let mut buf = cont;
        buf.extend_from_slice(&commit);
        let recovered = recover_committed_frames(&buf);
        assert_eq!(recovered.valid_prefix_len, buf.len());
        assert_eq!(recovered.frames.len(), 2);
        assert!(recovered.frames.iter().all(|f| f.txn_seq == 41));
    }

    #[test]
    fn recovery_stops_on_illegal_flag_combinations() {
        for flags in [0u8, FLAG_CONTINUES | FLAG_COMMIT, 0x04, 0xff] {
            let good = encode_frame(&WalFrame {
                txn_seq: 1,
                flags: FLAG_COMMIT,
                ops: vec![WalOp::Clear { store: 1 }],
            })
            .unwrap();
            let mut bad = encode_frame(&WalFrame {
                txn_seq: 2,
                flags: FLAG_COMMIT,
                ops: vec![WalOp::Clear { store: 2 }],
            })
            .unwrap();
            // Patch flags byte (offset 16) and recompute CRC.
            bad[16] = flags;
            let body_len = bad.len() - 4;
            let crc = crc32c(&bad[..body_len]);
            bad[body_len..].copy_from_slice(&crc.to_le_bytes());

            let mut buf = good.clone();
            buf.extend_from_slice(&bad);
            let recovered = recover_committed_frames(&buf);
            assert_eq!(recovered.valid_prefix_len, good.len(), "flags={flags:#x}");
            assert_eq!(recovered.frames.len(), 1);
        }
    }

    #[test]
    fn encode_txn_frames_splits_under_small_limit() {
        let ops: Vec<WalOp> = (0..8)
            .map(|i| WalOp::Put {
                store: 1,
                key: vec![i],
                value: vec![i],
            })
            .collect();
        // Tiny limit forces multiple frames without shrinking production constant.
        let frames = encode_txn_frames_limited(9, &ops, 40).unwrap();
        assert!(frames.len() >= 2);
        let mut decoded = Vec::new();
        for (i, bytes) in frames.iter().enumerate() {
            let (frame, n) = decode_frame(bytes).unwrap().unwrap();
            assert_eq!(n, bytes.len());
            assert_eq!(frame.txn_seq, 9);
            if i + 1 == frames.len() {
                assert_eq!(frame.flags, FLAG_COMMIT);
            } else {
                assert_eq!(frame.flags, FLAG_CONTINUES);
            }
            decoded.extend(frame.ops);
        }
        assert_eq!(decoded, ops);
        let recovered = recover_committed_frames(&frames.concat());
        assert_eq!(recovered.frames.len(), frames.len());
    }

    #[test]
    fn encode_txn_frames_rejects_single_oversized_op() {
        let op = WalOp::Put {
            store: 1,
            key: vec![0; 64],
            value: vec![0; 64],
        };
        let err = encode_txn_frames_limited(1, &[op], 8).unwrap_err();
        assert!(matches!(err, CodecError::PayloadTooLarge(_)));
    }

    proptest! {
        #[test]
        fn garbage_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
            let _ = recover_committed_frames(&bytes);
            let _ = decode_frame(&bytes);
        }
    }
}
