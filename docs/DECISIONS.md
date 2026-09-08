# Architecture Decision Records

## ADR-001: Synchronous pump driver instead of `NativeAsyncJob` per transaction (fix-03)

**Context.** TASK-03 prescribed one `NativeAsyncJob` driver task per IDB
transaction (AD-5). The `boa_idb` execution core built during M5 instead uses
a single synchronous pump (`driver::drive_turn`): API methods enqueue typed
operations and schedule exactly one Boa `GenericJob`; the job drains the pump
to quiescence, preserving per-transaction FIFO order.

**Decision.** Keep the sync pump as the M3 execution model.

**Rationale.**
- Ordering (§2.7.1 "results return in request order") falls out of a single
  FIFO drain without cross-task synchronization.
- Backends are synchronous by design (`BackendTxn` executes on the calling
  thread), so there is nothing to `await`; an async driver would only add
  state-machine surface.
- Boa jobs still provide true asynchrony: handlers run in a later task than
  the issuing script, so `request.onsuccess` assignment patterns work.

**Consequences.** Hosts without a Boa job loop must pump explicitly
(`runtime::pump_all`); auto-commit happens on queue drain rather than on
exact task boundaries (documented in the fix-03 handoff).

## ADR-002: Scoped lock discipline instead of reentrant locks (fix-03)

**Context.** The first fix-03 cut used `parking_lot::ReentrantMutex` so event
handlers (running inside the pump) could re-lock the engine/driver state.
`ReentrantMutexGuard` only offers shared (`Deref`) access, so the design
could not mutate through it — and reentrant locking would only have masked
lock-ordering bugs.

**Decision.** Plain `std::sync::Mutex` with a strict discipline: guards are
never held across JS event dispatch. Pump phases split into lock-scoped pure
steps (queue/bookkeeping/backend IO) and lock-free dispatch steps.
Poisoning is recovered via `runtime::lock_mutex` (no `unwrap`/`expect` on
JS-reachable paths).

## ADR-003: `Database::begin` returns `'static` transactions (fix-03)

**Context.** The fix-02 trait tied `Box<dyn BackendTxn>` to the `&mut`
borrow of the `Database`, forcing self-referential ownership in the L1
driver (which must own the backend transaction for the whole IDB
transaction lifetime, AD-6).

**Decision.** Widen the bound to `Box<dyn BackendTxn + 'static>`: backends
own their transaction state (`Arc` state, pooled connections) instead of
borrowing the `Database` handle. The in-memory backend needed no logic
changes (it already owned everything); the SQLite backend (fix-04) checks
out a pooled connection per transaction.

## ADR-004: Versionchange transactions skip scope checks (fix-03)

**Context.** Upgrade backends are opened with an empty scope (stores are
created mid-upgrade, so ids are unknowable upfront), which tripped the
memory backend's `check_scope` during index backfill scans.

**Decision.** `MemoryTxn::check_scope` is a no-op in `VersionChange` mode:
upgrade transactions are exclusive and span the whole database by definition.
Schema-mutating methods keep their explicit mode gates.

## ADR-005: Statement cache via rusqlite's per-connection LRU instead of the `lru` crate (fix-04)

**Context.** TASK-04's manifest listed `lru = "0.12"` for the prepared
statement cache (R8.2.1: "statement cache — per connection, LRU for 64
queries"). rusqlite already ships exactly this mechanism:
`Connection::prepare_cached` consults a per-connection statement cache backed
by `hashlink::LruCache`, sized via
`Connection::set_prepared_statement_cache_capacity` (default 16).

**Decision.** Do not add the `lru` crate. Every pooled connection is opened
with `set_prepared_statement_cache_capacity(64)` and all hot statements
(`get`, `count`, cursor paging) go through `prepare_cached`, giving each
connection an LRU cache of 64 prepared statements as R8.2.1 requires.

**Rationale.** A second LRU instance wrapped around rusqlite statements would
duplicate state, fight rusqlite's own reset/finalize semantics and add a
dependency for zero behavioral gain. The spec (§2.3) itself allows deviation
with justification ("lru — or your own"); the cache built into rusqlite is
the same data structure, maintained upstream.

## ADR-006: Hex/Base32 via `data-encoding` plus a local `hex_encode` instead of the `hex` crate (fix-04)

**Context.** TASK-04's `naming.rs` sketch used `hex::encode` for blob file
names. The crate needs exactly two encodings: Base32 (path hashing, R8.1.4)
and lowercase hex (content-addressed blob names).

**Decision.** Use `data-encoding` (`BASE32_NOPAD`, already required for the
Base32 half) and a 5-line local `hex_encode` for the hex half. The `hex`
crate is not added.

**Rationale.** A one-trivial-function dependency (unmaintained status is not
a concern here, but dependency count is) buys nothing over a loop of
`write!`s that the crate owns and tests. `data-encoding` remains the single
encoding dependency.

## ADR-007: SHA-256 content addressing instead of CRC32 for external blobs (fix-04)

**Context.** The TZ (§8.2.5) sketched external blob files as "SCF + CRC"
content, which implied a `crc32fast` dependency and a sidecar checksum.
TASK-04 initially declared `crc32fast` but never used it.

**Decision.** Blob files are content-addressed: the file name is
`sha256(content)`, the content is stored verbatim, and every read re-hashes
the bytes and requires the digest to equal the file name
(`BackendError::Corrupted` otherwise). The `crc32fast` declaration was
removed from `Cargo.toml`.

**Rationale.** SHA-256 was already a dependency (`naming.rs` path hashing)
and the name-is-the-hash scheme makes the checksum tamper-evident and
self-verifying without a container format: a truncated, replaced or
bit-flipped file is detected on read, which is the entire purpose of the
CRC here. CRC32 would add a dependency *and* a weaker (32-bit,
non-cryptographic) check. GC and dedup are content-addressed too, so
collision risk is bounded by the same SHA-256 assumptions as the rest of
the design.

## ADR-009: Filesystem backend foundation — locking, WAL, in-memory index (M6-A)

**Context.** Normative M6 (`boa_idb_fs`) needs advisory multi-process locking,
WAL frames with CRC32C recovery, atomic `CURRENT`/`MANIFEST` updates, and an
ordered in-memory key index. Full segment/compaction/MVCC work is deferred to
M6-B. TZ suggests `fs4`/`fd-lock` for locks.

**Decision.**
1. **Advisory lock:** use the stable `std::fs::File::try_lock` /
   `File::unlock` API (Rust ≥ 1.89; workspace MSRV 1.91). TZ suggested
   `fs4`/`fd-lock`, but std now provides the same portable advisory lock
   without an extra dependency. A contended lock maps to
   `BackendError::Locked`. No `unsafe`, no FFI.
2. **Persistent map (M6-A):** keep a per-database `BTreeMap` index in memory,
   rebuilt from `meta.scf` + valid WAL prefix on open. Do **not** add `im` /
   `rpds` yet; readonly transactions use copy-on-write of the maps (O(n)
   clone). Document R8.3.4 as `PARTIAL` until M6-B.
3. **Encoding:** reuse `boa_idb_core::clone::crc32c` for WAL trailers; custom
   little-endian codecs for WAL frames and `meta.scf` (no new serde format
   crate). Path hashing mirrors SQLite (`sha2` + `data-encoding` Base32).
4. **Atomic replace:** write temp file → optional file sync → `rename` over
   target → optional directory sync via injectable `SyncHooks` (real OS sync
   by default; counting observer in tests). Never overwrite `MANIFEST` or
   `meta.scf` in place.
5. **Dev/test:** `tempfile` and `proptest` as already used by sibling crates.
6. **WAL flags / multi-frame protocol:** a committed transaction is either a
   single frame with flags exactly `FLAG_COMMIT` (`0x02`), or a chain of one or
   more frames with flags exactly `FLAG_CONTINUES` (`0x01`) followed by a final
   frame with flags exactly `FLAG_COMMIT`, all sharing the same `txn_seq`.
   `CONTINUES|COMMIT`, zero flags, and other bits are illegal and stop recovery
   at the prior committed prefix. Ops that do not fit in one payload are split
   across CONTINUES/COMMIT frames without splitting a single op; an op larger
   than the payload limit fails with `QuotaExceeded` before any WAL write.

**Consequences.** M6-A delivers durable single-writer recovery and lock
safety without claiming compaction or O(1)/O(log n) snapshots. New crates
are permissive-licensed and covered by `cargo deny`.

## ADR-010: M6-B1 — `im` OrdMap snapshots and immutable segments

**Context.** TASK-08 / M6-B1 must close R8.3.3 (immutable `seg/*.seg` +
manifest compaction) and R8.3.4 (readonly snapshot start in O(1)/O(log n)
by record count). M6-A used `BTreeMap` with a full clone on readonly begin.

**Decision.**
1. **Persistent maps:** depend on crates.io `rpds` 1.x
   (`RedBlackTreeMap`) under MIT. Structural `clone` is O(1); updates are
   path-copying. Chosen over `im`/`imbl` because those pull unmaintained
   `bitmaps`/`sized-chunks` crates rejected by `cargo deny`
   (`RUSTSEC-2026-0247` / `RUSTSEC-2026-0251`). TZ names `im`/`rpds`-like
   trees; `rpds` matches the requirement with a clean dependency graph.
2. **Readonly begin:** `DbState::clone` copies `im::OrdMap` structurally (no
   per-record walk). An injectable `SnapshotMeter` counts structural clones
   vs forbidden deep record walks so tests prove the complexity class without
   timing flakes.
3. **Segments:** versioned `seg/<seq>.seg` files encode a full durable
   snapshot (meta, records, indexes, key generators, `next_txn_seq`) with
   magic `ISEG`, version, and CRC32C. Manifest (`IMAN`) lists live segment
   sequence(s) and the active WAL file id. `CURRENT` is published only after
   the manifest file is fully synced (temp → sync → rename → dir sync).
4. **Compaction:** when WAL bytes ≥ 64 MiB or committed frame groups ≥
   10_000 (both configurable downward for tests), between write transactions
   write a new segment, publish a new manifest/`CURRENT`, then rotate to an
   empty WAL. Failure mid-publish leaves either the old or the new consistent
   generation — never a hybrid. Live state holds `Arc<SegmentGuard>`; old
   segment files are deleted only when the last `Arc` drops (readonly
   snapshots keep them alive).
5. **Out of scope for B1:** `FileSystem` fault injection (B2), kill-worker /
   WPT FS (B3).

**Consequences.** R8.3.3/R8.3.4 can be marked PASS for B1 once tests cover
thresholds, publication order, snapshot retention, and the meter proof.

## ADR-011: M6-B2 — `FileSystem` trait and fault injection

**Context.** TASK-08 / M6-B2 must close R8.5.2 / R8.5.3 for the filesystem
backend: every production IO path must be injectable so ENOSPC, EIO, short
writes, sync/rename failures, and interrupts can be tested without changing
commit/recovery control flow. Legacy `SyncHooks` only covered sync.

**Decision.**
1. Introduce internal trait `FileSystem` (`crates/boa_idb_fs/src/vfs.rs`)
   covering create/read/write/append/truncate/rename/remove/dir listing/
   sync/lock. Default implementation is `OsFileSystem`.
2. Factory holds `Arc<dyn FileSystem>`; `with_filesystem` injects test
   doubles. `with_sync_hooks` remains as a compatibility adapter
   (`SyncHooksFs`) so existing durability counting tests keep working.
3. `FaultInjectingFs` wraps any `FileSystem` and fires queued
   `(FaultSite, FaultKind)` rules classified by path (WAL / segment /
   manifest / `CURRENT` rename / meta / dir sync / cleanup / lock /
   truncate). `ENOSPC` maps to `BackendError::QuotaExceeded`; other faults
   to `BackendError::Io`. No production `unwrap`/`expect`/`panic!` on these
   paths.
4. Segment reclaim (`SegmentGuard` drop) unlinks through the same
   `FileSystem` so cleanup faults are observable.
5. Out of scope for B2: kill-worker process crash suite and WPT
   `--backend fs` (M6-B3).

**Consequences.** Table-driven `m6b2_tests` prove committed-prefix survival
after each publication-stage fault and safe handling of corrupt WAL /
segment / manifest tails. R8.5.2 / R8.5.3 → PASS for in-process injection;
R8.5.1 / R8.3.6 process-kill closed in M6-B3.

## ADR-012: M6-B3 — crash worker and WPT `--backend fs`

**Context.** TASK-08 / M6-B3 must close R8.3.6 / R8.5.1 with a real
inter-process kill (not only torn-WAL simulation) and expose the FS backend
to the WPT / differential runners at ≥92 % PASS.

**Decision.**
1. Ship `boa-idb-fs-crash-worker`: deterministic commits + index/keygen,
   marker file `READY <kind> <durable>`, then park. Parent waits for the
   marker and calls `Child::kill` (SIGKILL / TerminateProcess).
2. CI runs 8 seeded iterations by default; nightly sets
   `BOA_IDB_FS_CRASH_ITERS=200` (workflow + README). Failures print seed and
   replay command.
3. WPT gains `--backend fs` via `FsBackendFactory` on the existing per-file
   temp root (same isolation model as SQLite).
4. Differential FS scenarios live in `differential_fs_tests.rs` (memory↔FS
   parity for CRUD/schema/index/cursors/abort/savepoint; FS reopen for
   Strict durability).

**Consequences.** R8.3.6 / R8.5.1 and FS WPT can be marked PASS with measured
commands; no mass expectation updates were required (FS 482/482).

