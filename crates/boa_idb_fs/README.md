# boa_idb_fs

Filesystem backend for IndexedDB. M6-A foundation (WAL/LOCK), M6-B1
immutable segments / MVCC snapshots, and M6-B2 `FileSystem` + fault injection.

## Memory limit (R8.3.5)

```text
bytes ≈ keys × (key_len + 48)
```

Default `max_keys_in_memory` is **5_000_000**. Prefer SQLite for large DBs.

## Topology

```text
<root>/<storage_key_hash>/<db_name_hash>/
  LOCK
  CURRENT                 # points at a fully written MANIFEST-*
  MANIFEST-<seq>          # IMAN + CRC32C: wal_seq + live segment list
  meta.scf
  wal/<seq>.log           # active WAL generation
  seg/<seq>.seg           # immutable ISEG snapshots + CRC32C
```

## Generation lifecycle

1. Writes append WAL frames (CONTINUES*/COMMIT).
2. When WAL bytes ≥ 64 MiB **or** ≥ 10_000 committed frame groups (configurable),
   compaction writes a new `seg/*.seg`, publishes a new manifest/`CURRENT`,
   then rotates to an empty WAL generation.
3. Live tip holds `Arc<SegmentGuard>` with `reclaim=false`. Superseded segments
   are marked reclaimable and unlinked only after the last readonly snapshot
   drops its `Arc`.

## Readonly snapshots (R8.3.4)

Record/index maps use `rpds::RedBlackTreeMap`. Readonly begin structurally
clones maps (O(1) in record count). `SnapshotMeter` counts structural clones
vs deep walks for proof tests.

## Durability

- `Relaxed` / `Default`: WAL append without mandatory sync (schema still syncs
  WAL before `meta.scf`).
- `Strict`: sync WAL before commit returns.

## FileSystem seam (R8.5.2 / R8.5.3)

All production IO goes through `FileSystem` (`OsFileSystem` by default).
Tests inject `FaultInjectingFs` via `FsBackendFactory::with_filesystem`:

| Site | Typical faults |
|---|---|
| WAL append/sync | ENOSPC, EIO, short write, interrupt, sync fail |
| Segment / manifest / `CURRENT` | write/rename/sync fail during compaction |
| Cleanup | remove fail on reclaimable segment drop |

`ENOSPC` → `BackendError::QuotaExceeded`. After any fault + reopen, only a
committed prefix is readable; torn/corrupt WAL/segment/manifest never panic.

## Known platform notes

- Crash kill uses `std::process::Child::kill` (Unix SIGKILL / Windows
  `TerminateProcess`). Advisory `LOCK` is released by the OS on process death;
  reopen retries briefly on `Locked` for slow unlock.
- Nightly: `BOA_IDB_FS_CRASH_ITERS=200 cargo test -p boa_idb_fs --test m6b3_crash_tests`
  (workflow `.github/workflows/nightly-fs-crash.yml`).

## Tests

```powershell
cargo test -p boa_idb_fs
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend fs --summary
```
