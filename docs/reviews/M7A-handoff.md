# Handoff: M7-A — harness, baselines, observability (no behavior change)

Work order: `tasks/09_TASK_M7_PERFORMANCE_RELIABILITY.md` (M7-A half)
Branch: `task/m7-performance-reliability` (from M6-B tip `1cc9aa9`)
Scope gate: `QUESTIONS.md` (TASK-09 / M7 gate: 3800–5600 LOC → split M7-A → M7-B)
Review rework: `docs/reviews/M7A-review.md` (`REWORK REQUIRED` → addressed
below; P1-1, P1-2, P1-3, P2 all closed on this branch)

## What was built

| Track | Files | Notes |
|---|---|---|
| Benchmark harness (§3) | `crates/boa_idb/benches/backends.rs`, `benches/receipts/*`, workspace `[profile.release] lto = "thin"`, `criterion` dev-dep | 7 §12.1 scenarios × SQLite/FS (+ memory control, no `open`); isolated temp roots, fixed numeric keys, ~200 B values; `BOA_IDB_BENCH_SMOKE=1` CI mode (diagnostic only) |
| Baseline workflow | `benches/receipts/README.md`, `RECEIPT-2026-09-05-win11-i7-14700K.md`, raw logs, criterion `--save-baseline m7a-20260905` | Authoritative comparison stays on the pinned host (M7-B wires the blocking >10 % gate) |
| Tracing (§6.1) | `tracing` feature on `boa_idb` (default off) + `boa_idb_core/tracing`; spans `idb.open/txn/request` in `driver.rs`/`api/factory.rs` | Zero-cost when disabled (`#[cfg]`-gated fields); privacy-safe attributes only (db name, mode, scope, seq, durations, bytes) |
| `IdbObserver` (§4.3/§6.2) | `crates/boa_idb/src/observer.rs`, `runtime.rs` (`observer` state, `stats()`, `add_observer`), `extension.rs` (builder `.observer()`) | Events: opened/begun/committed/aborted/request/quota/corruption; built-in counters + bounded 9-bucket histograms; byte counters from exact SCF payload lengths (core `PutResult::value_bytes`, read-helper tuples) |
| Dev CLI (§6.3) | `crates/boa_idb/examples/boa-idb-cli.rs` (`ls/dump/verify/compact/stats`, `--values` gate) | Example target (dev-only, not production API); hand-rolled argv, no new deps |
| CI | `.github/workflows/ci.yml` (`bench-smoke` job + tracing observer-tests line) | Smoke is diagnostic; heterogeneous runners never authoritative |
| Traceability | `docs/traceability.md` rows R12.1–R12.3, R13.1, R13.2, R13.4.1, R13.6 | PASS only where gates are real; the rest honest PARTIAL → M7-B |
| ADR | `docs/DECISIONS.md` ADR-013 (`criterion` 0.5, dev-only, MIT/Apache-2.0) | One-paragraph, before use |

Behavior changes: **none**, with two mechanical exceptions, both
non-semantic and covered by tests (see rework §R1 and §R3):

* `Database::begin` now returns `Box<dyn BackendTxn + Send>` (was
  `Box<dyn BackendTxn>`). All in-tree transaction types already satisfy
  `Send`; the bound only documents it and lets test probes hold driver
  state across the `Send + Sync` observer boundary. No IO, ordering, or
  error path changed (WPT still 482/482 on all backends).
* `SqliteStorage::open_database` runs `migrate_if_needed` on every open
  (separate commit `cbe6c16`, review path 1) instead of pool-creation only.

WPT memory/SQLite/FS all 482/482 PASS after the driver hooks — the pump
paths are observation-only.

## How to run the demo commands

```sh
# Benchmarks (full) + baseline save/compare
cargo bench -p boa_idb --bench backends -- --save-baseline m7a-<date>
BOA_IDB_BENCH_SMOKE=1 cargo bench -p boa_idb --bench backends
cargo bench -p boa_idb --bench backends -- --baseline m7a-20260905
# Normative 1M cursor scan (defaults = 1M records, 1800 s timeout)
cargo run --release -p boa_idb --example scan-1m -- --backend sqlite --root ./scan-1m-data
cargo run --release -p boa_idb --example scan-1m -- --backend fs --root ./scan-1m-data
# Observability tests (default + tracing) + P1-1 upgrade regression
cargo test -p boa_idb --test observer_tests
cargo test -p boa_idb --features tracing --test observer_tests
cargo test -p boa_idb --test upgrade_observer_tests
# CLI (example; needs a seeded root — tests seed via backend traits)
cargo run -p boa_idb --example boa-idb-cli -- --root ./idb-data --backend sqlite ls
cargo test -p boa_idb --test cli_tests
# Gates
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
```

## First normative results (diagnostic host, not evidence)

Full table: `crates/boa_idb/benches/receipts/RECEIPT-2026-09-05-win11-i7-14700K.md`.
Criterion suite: 9 of 12 backend targets PASS (scan excluded — diagnostic).
Normative 1M scan via `scan-1m`: FS **PASS** (1.43M/s), SQLite **MISS ×177**
(847/s measured over 1180.9 s). Misses → M7-B follow-ups (profile first):
**F1** FS `put` batch collapse (958 ops/s vs 120 000; superlinear in batch
size; FS 1M fixture build took 1734 s, corroborating); **F2** SQLite 1M scan
cliff (measured 847/s vs 403k/s at 20k — 475× for 50× rows; `LIMIT`/`OFFSET`
suspect); **F3** JS↔core 80 µs vs 15 µs (upper bound incl. eval/readback);
**F4** SQLite 1M open 310 ms vs 200 ms; **F5** FS strict commits 212/s
(marginal, regression canary). Normative/diagnostic boundary is explicit
(`scan-1m` defaults = 1M; criterion `cursor_scan_<N>` carries its count and
is excluded from target counts). No target weakened, no benchmark deleted.

## Rework for `M7A-review.md` (all items closed, no M7-B work started)

### R1 (P1-1): observer callbacks never under engine/driver locks

* `start_upgrade_txn` no longer emits: the `StartUpgrade` arm carries
  `txn_id` out of the dual-guard scope and emits `TransactionBegun`
  lock-free, strictly before `upgradeneeded` dispatch (order preserved).
* `ObserverState::emit` split into `record` (counters under the observer
  mutex, returns cloned observer list) + lock-free fan-out in the
  `observer::emit` helper: callbacks now run with no engine/driver/observer
  lock held, so a re-entrant `stats()` cannot deadlock.
* Full audit of every M7-A emit path (open/success/fail, upgrade
  begin/commit, txn begin/commit/abort, request wrapper, drop cascades):
  all fire in lock-free regions; the upgrade path was the only violator.
* Regression test `crates/boa_idb/tests/upgrade_observer_tests.rs`:
  `try_lock` probe over driver/engine/observer mutexes (never blocks, so a
  violation fails instead of deadlocking) + re-entrant `stats()` read +
  manual `drive_turn` stepping proving versionchange Begun is the first
  observed event, commit is terminal, lifecycle completes. Negative control
  performed (emit-under-lock reintroduced → probe fails with
  `driver/engine mutex held during TransactionBegun`), then reverted.
* Enabling detail: `Database::begin` gained a `+ Send` bound (see above);
  5-site mechanical change, all impls already `Send`.
* Follow-up fix (review finding [P1], double-counted bytes): global
  `bytes_read`/`bytes_written` were summed in both `RequestCompleted` and
  `TransactionCommitted` (2× for committed txns, 1× for aborted —
  outcome-dependent). Summation now lives in `RequestCompleted` only, which
  covers aborted txns too since their executed requests complete first;
  `TransactionCommitted` keeps per-transaction totals as observer data.
  `observer_lifecycle_counters_and_bytes` asserts exactness: put `(0, W)`,
  get `(W, 0)` round-trip identity, aborted put counted exactly once
  (`stats == put_w + abort_w` / `== get_r`), plus the committed txn's
  per-transaction `(W, W)` event.

### R2 (P1-2): honest 1M cursor-scan evidence

* New `examples/scan-1m.rs` (dev-only, like the CLI): defaults ARE the
  normative case (`--records 1000000 --timeout-secs 1800`); one full pass
  with per-8k-row deadline checks; exit `0` result (`RESULT`/`MISS` in
  output), `1` usage, `2` backend, `3` controlled timeout; `--reuse` /
  `--build-only` split long runs; checksum forces every value to be read.
* Criterion `cursor_scan` renamed to `cursor_scan_<N>` (diagnostic id),
  module docs + `benches/receipts/README.md` state the normative/diagnostic
  boundary; default smoke command unchanged and small.
* Fixture-isolation bug found by the smoke run fixed in the same pass:
  per-scenario fixtures shared one temp root + database name, so the scan
  assert failed at smoke scale (and passed at full scale only by
  arithmetic luck). Each fixture now uses its own database
  (`bench_crud`/`bench_scan`/`bench_open`); smoke is fully green.
* Receipt rewritten: 1M section (SQLite MISS ×177 measured, FS PASS 1.43M/s,
  memory control) with raw logs; 20k table moved to a diagnostic section
  excluded from target counts. R12.1 stays `PARTIAL` (blocking gate → M7-B).
* No SQLite optimization in this rework — harness/receipt only.
* Controlled-timeout path proven live: `--reuse --timeout-secs 1` on the 1M
  SQLite fixture reports `TIMEOUT rows_scanned=49152 elapsed_s=1.1
  timeout_s=1` with exit code 3.

### R3 (P1-3): green `cargo test --workspace` (preferred path 1)

* Root cause: `migrate_if_needed` ran only at pool creation
  (`ConnectionPool::open_with_readers`); `open_database` reuses the cached
  pool, so a rewound/older file reopened through it never migrated —
  `test_old_schema_version_is_migrated` failed on the clean tip too.
* Fix in separate minimal commit `cbe6c16` (2 files, +83): every
  `open_database` runs `migrate_if_needed` on a non-blocking writer
  checkout; `Locked` keeps prior behavior (a later free open retries —
  self-healing, zero regression risk for concurrent-write opens).
* Additional regression test `test_cached_reopen_migrates_and_preserves_data`
  (same storage/cached pool, version rewind, data intact after migration).
  Migration suite: 7/7 green.

### R4 (P2): strict docs in CI

* `.github/workflows/ci.yml` checks job gained
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` (passes
  locally; the two pre-existing broken intra-doc links were fixed with
  one-liners as part of M7-A).

## Deviations: none (plus two carried notes, not M7-A scope)

1. ~~Migration test failing on the clean tip~~ — FIXED in this rework
   (commit `cbe6c16`, §R3 above); workspace suite is green.
2. `cargo doc` had two pre-existing broken intra-doc links
   (`scheduler.rs` `BackendError::Locked`, `vfs.rs` private `atomic_write`);
   fixed with one-liners as part of M7-A acceptance requires warning-free
   docs. The `vfs.rs` reword overlaps the `m6: vfs.rs doc wording tweak`
   stash on `task/m6-fs-completion` — that stash is now redundant and should
   be dropped at m6 cleanup.
3. Backend-level quota/corruption errors are flattened to `IdbError::Data`
   by the `backend_err` mappers, so `QuotaExceeded`/`Corruption` observer
   events fire only for the explicit `IdbError` variants today (documented in
   `observer.rs`). Preserving error kinds changes JS-visible error names —
   an M7-B semantic decision, not smuggled into M7-A.

## DECISIONS.md entries

ADR-013 (`criterion` 0.5 dev-only benchmark harness). No other new
dependencies (CLI parses argv by hand; span capture in tests uses only the
already-present `tracing` API; hex helpers are local per ADR-006 precedent).

## Known limitations → M7-B

F1–F5 profiling/optimization; cursor/memory O(1) + RSS/`dhat` gates;
≤64 KiB/≤8 KiB overhead probes; nightly 10k differential + 200 crash wiring;
`llvm-cov` thresholds; 4th fuzz target + 4 h corpus; labelled-host blocking
regression gate; backend error-kind preservation decision.
