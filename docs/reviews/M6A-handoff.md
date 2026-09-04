# Handoff: M6-A — Filesystem backend foundation

Work order: `tasks/07_TASK_M6A_FILESYSTEM_BACKEND_FOUNDATION.md`  
Branch: `task/m6-fs-foundation`

## Commits

- `ba4af08` — Record M6-A work order and ADR-009
- `1c91a7a` — Implement `boa_idb_fs` WAL, lock, and backend traits
- `34c2f19` — Document M6-A in handoff and traceability

## Delivered

- Implemented `boa_idb_fs` as a real `BackendFactory` / `Storage` / `Database` /
  `BackendTxn` / `BackendCursor` backend (no longer a placeholder).
- Disk topology: `<root>/<sk-hash>/<db-hash>/{LOCK,CURRENT,MANIFEST-*,meta.scf,wal/}`.
- WAL frames (`IWAL` + length + txn_seq + flags + payload + CRC32C) with
  encode/decode module and recovery that keeps only the committed prefix and
  truncates a torn/corrupt tail.
- `Durability::Strict` syncs the WAL before commit success; `Relaxed` does not.
  Injectable `SyncHooks` / `CountingSyncHooks` observe sync without timing.
- Exclusive advisory `LOCK` via `std::fs::File::try_lock` (MSRV 1.91); second
  open returns `BackendError::Locked` until close.
- `max_keys_in_memory` (default 5_000_000) with `QuotaExceeded` and README
  estimate `keys × (key_len + 48)`; recommend SQLite for large DBs.
- ADR-009 records locking, in-memory index, codecs, and atomic replace.
- Integration coverage: CRUD reopen, abort/savepoint, torn WAL, strict/relaxed
  sync counts, exclusive lock, max-keys boundary, index/cursor/keygen parity,
  crash-seam (torn append; full kill-worker deferred to M6-B).

## Verification

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny check
```

All of the above passed on the M6-A tip.

## Size note

The implementation commit is ~3.1k insertions (crate sources + tests + README).
That sits at the work-order soft limit; M6-B must stay a separate on-disk
segments/compaction/MVCC/WPT delivery and must not grow this foundation further
without a new work order.

## Known gaps → M6-B

- R8.3.3 compaction / immutable `seg/*.seg` not implemented.
- R8.3.4 O(1)/O(log n) snapshots not met: readonly uses full map clone (O(n)).
- R8.3.6 / R8.5.1 full kill-worker matrix (200 iterations) not run; Windows
  uses a torn-append reopen seam instead of `TerminateProcess` worker.
- No FS backend wiring in `boa_idb_wpt` / differential runner.
- Externalized blobs not implemented.

**Do not treat normative M6 as complete.**
