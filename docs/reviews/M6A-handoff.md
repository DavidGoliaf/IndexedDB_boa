# Handoff: M6-A — Filesystem backend foundation

Work order: `tasks/07_TASK_M6A_FILESYSTEM_BACKEND_FOUNDATION.md`  
Branch: `task/m6-fs-foundation`  
Remediation: `docs/reviews/M6A-remediation-plan.md`

## Commits

- `ba4af08` — Record M6-A work order and ADR-009
- `1c91a7a` — Implement `boa_idb_fs` WAL, lock, and backend traits
- `34c2f19` / `6a5b18a` — Document M6-A in handoff and traceability
- `d5cd06d` — Harden FS commit durability (review bugs 1–6)
- `563065e` — P1 WAL sequence isolation + multi-frame commits

## Delivered

- Real `BackendFactory` / `Storage` / `Database` / `BackendTxn` / `BackendCursor`.
- Disk topology: `<root>/<sk-hash>/<db-hash>/{LOCK,CURRENT,MANIFEST-*,meta.scf,wal/}`.
- **WAL multi-frame protocol:** one `txn_seq` per transaction; frames are either
  a lone `FLAG_COMMIT`, or `FLAG_CONTINUES`* → `FLAG_COMMIT` (exact flag bytes;
  `CONTINUES|COMMIT` and other combinations abort recovery). Ops are packed
  into frames under `MAX_FRAME_PAYLOAD` without splitting a single op; an
  oversized lone op returns `QuotaExceeded` before any WAL write.
- **Recovery invariant:** after crash/reopen, state equals some prefix of fully
  committed transactions only. Sequence mismatch / illegal flags / torn CRC
  stop at the prior committed prefix (no skip-ahead to a later COMMIT).
- Strict / schema commits sync the final WAL write before `meta.scf`.
- Exclusive `LOCK`, `max_keys_in_memory`, ADR-009 (incl. flag rules).

## WAL regression coverage (would fail before P1 rework)

Unit (`wal` module):

- `recovery_rejects_continues_then_commit_with_different_seq`
- `recovery_keeps_prior_commit_when_later_chain_mismatches`
- `recovery_applies_matching_continues_commit_chain`
- `recovery_stops_on_illegal_flag_combinations`
- `encode_txn_frames_splits_under_small_limit`
- `encode_txn_frames_rejects_single_oversized_op`

Integration (`fs_backend_tests`):

- `sequence_mismatch_wal_does_not_apply_on_reopen`
- `multi_frame_commit_recovers_and_torn_continues_is_dropped`
- `multi_frame_commit_via_backend_persists`
- `multi_frame_sync_failure_rolls_back_entire_chain`
- `oversized_single_op_does_not_touch_wal_or_state`

## Verification

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p boa_idb_fs
cargo test --workspace
cargo doc --workspace --no-deps
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny check
```

## Known gaps → M6-B

- R8.3.3 compaction / immutable `seg/*.seg` not implemented.
- R8.3.4 O(1)/O(log n) snapshots not met: readonly uses full map clone (O(n)).
- R8.3.6 / R8.5.1 full kill-worker matrix (200 iterations) not run.
- No FS backend wiring in `boa_idb_wpt` / differential runner.
- Externalized blobs not implemented.

**Do not treat normative M6 as complete.**
