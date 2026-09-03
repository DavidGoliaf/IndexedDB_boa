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
