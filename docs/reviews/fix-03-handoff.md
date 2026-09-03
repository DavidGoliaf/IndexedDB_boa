# Handoff: fix-03-bindings

Branch: `task/fix-03-bindings`, base: `master@9c7e47c` (fix-02 merged).
Scope: findings of `tasks/03_REVIEW_JS_BINDINGS_AND_DOM_SHIM.md` (4 red blocks).

## Execution model (see `docs/DECISIONS.md` ADR-001…004)

- Sync pump (`driver::drive_turn`) + one deduped Boa `GenericJob` per burst
  (`runtime::schedule_pump`); `runtime::pump_all` for embedding/tests.
- Lock discipline: `std::sync::Mutex` + `lock_mutex` (poison-recovering, no
  `unwrap`); guards never held across event dispatch (phases split into
  lock-scoped pure steps + lock-free dispatch) — reentrant handlers
  (`createObjectStore` from `upgradeneeded`) cannot deadlock.
- `Database::begin` returns `'static` transactions (ADR-003); the L1
  `TxnHandle` owns the backend transaction for the whole IDB transaction.
- The old per-operation engine flow (`IdbEngine::{put,get,…}` opening and
  committing a fresh backend txn per call — no transactionality) is deleted.
- Scheduler gating: transaction backends start only when `poll_ready`
  releases them (FIFO/VersionChange-exclusivity honored); `forget()` added
  for handles that never went through the poll (upgrade txns).

## What was implemented

**Driver (`driver.rs`, rewritten phases):**
- Opens/deletes through the core `OpenQueue` (version compare, `blocked`
  event, upgrades, deletes with `IDBVersionChangeEvent` results).
- Upgrade lifecycle: backend VC txn + `upgradeneeded` (real
  `IDBVersionChangeEvent` with old/new versions) → schema sync ops →
  drain → atomic commit → `success`. Handler throw → upgrade abort +
  `AbortError` (no longer swallowed).
- Request FIFO per txn with backend+keygen savepoints per op (AD-7);
  lazy keygen seeding from `key_gen_current` (cross-txn sequences work);
  savepoint-balance fix for lazily created generators.
- `fail_op` returns default-prevented; handled request errors do NOT abort
  (§2.8). Throwing `success` listeners abort (§5.10). Pending requests of an
  aborted txn fail with `AbortError`. `error` events are cancelable.
- Cursors materialize at open; iteration reuses the opening request
  (done-flag dance); `continue` direction validation; `update` (in-line key
  rules) routes through `ops_store::put`, `delete` through
  `ops_store::delete` (index-aware, closing a driver-level D1 gap).
- Schema sync ops (`create/delete/rename` store/index) with duplicate/
  `InvalidAccess` validation and unique-backfill.

**API (`api/*`, `dom/*`):** all 12 IDB classes wired to the driver
(open/delete/databases-as-Promise/cmp; transaction/create/deleteObjectStore/
close; full objectStore/index/cursor/read surface incl. `getAllRecords`,
`IDBGetAllOptions`, `KeyRange` query args); same-handle caches (§2.6.1);
`on*` setters everywhere; `result`/`error` throw `InvalidStateError` while
pending; `DOMStringList` for `*Names`; class/data merges for
index/record/cursor/versionchange (constructible objects);
`addEventListener` fixed for IDB natives (was a silent no-op) + `once`
removal on dispatch; `IDBVersionChangeEvent(type, init)` constructor.

**Runtime:** `IdbEngine::new`/`with_engine` fallible; `end_of_task`
deactivates unfinished txns; `pump_scheduled` dedupe flag.

**Tests (35):** 26 integration (updated to async+pump), 8 flow
(upgrade/put/get, unique+preventDefault, multiEntry+cursors, handles,
pending-result, addEventListener+KeyRange, abort-lifetime, delete/count),
1 Appendix-D acceptance (migration chain, indexes, cmp ordering, records).

## Verification

- `cargo fmt --all -- --check` — clean
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — 0
- `cargo test --workspace --no-fail-fast` — all suites ok
- `cargo doc -p boa_idb --no-deps` — ok
- grep `unwrap/expect/panic/todo/unsafe` in `boa_idb/src` — clean
  (`unsafe_ignore_trace` attributes only; `debug_assert_eq!` ×3 in
  open-queue plumbing, debug-only)

## Known limitations / follow-ups

- Auto-commit on queue drain (no exact task-boundary tracking); cross-txn
  execution interleave is pump order (starts are scheduler-gated).
- `databases()` names via lossy strings; DB/store/index names with lone
  surrogates travel as `String` through the L1 (backend `naming.rs` in
  fix-04 hashes UTF-16 — the L1 side needs a follow-up).
- `createIndex` unique-backfill violation throws synchronously (async
  request-error modeling deferred).
- `IDBTransaction.error`, `durability: "strict"` enforcement, `close` event
  (abnormal close only), `getAll` default-limit config knob — open.
- `lib.rs` keeps broad `allow(dead_code, …)` from the skeleton phase;
  tightening is a separate cleanup order.
- WPT conformance ≥80% is task-05 (runner lives in `task/m5-wip`).

## Demo

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p boa_idb
cargo test -p boa_idb --test appendix_d_acceptance_tests
```
