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
