# Handoff: M5 — WPT harness and stabilization

Work order: `tasks/05_TASK_WPT_HARNESS_AND_STABILIZATION.md`  
Branch: `task/m5-wpt-runner`

## Delivered

- Completed the `boa_idb_wpt` isolated Boa-context runner, browser polyfills,
  timer/event-loop driving, native completion bridge, CLI, report model and
  expectations support.
- Added the checked-in IndexedDB WPT subset and runner unit tests.
- Added deterministic `crates/boa_idb_wpt/expectations.json`; the runner now
  reports TIMEOUT separately from FAIL and treats timeout/not-run as CLI
  failures.
- Added `docs/traceability.md` and the `boa_idb_wpt` crate README.
- Fixed empty-key-path key determination and ignored invalid optional index
  keys during index synchronization.
- Made the CLI default to one worker for reproducible expectation checks;
  explicit parallelism remains available with `--threads N`.
- Made expectation checking strict about file/subtest shape and exact status,
  and added regression tests for status and shape changes.
- Added role READMEs for `boa_idb`, `boa_idb_core`, `boa_idb_fs`,
  `boa_idb_memory`, and `boa_idb_sqlite`.

## Verification

The following local checks passed:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p boa_idb --tests
cargo test -p boa_idb_wpt --tests
```

Final WPT totals for both backends are intentionally pending remediation. The
historical totals previously recorded in this file are not acceptance evidence
and must be replaced only by a fresh full run.

## Decisions and deviations

No new dependency or ADR was needed. The runner keeps the existing synchronous
Boa pump design documented by the previous handoff; the only M5 execution
deviation is the deterministic single-worker default.

## Follow-up stabilization

The M5 branch also contains a bounded stabilization pass for the remaining
IndexedDB lifecycle cases:

- cursor navigation now tracks pending requests and preserves cursor state for
  post-transaction error classification;
- deleted object-store handles are rejected immediately, while queued writes
  are allowed to complete before physical deletion at upgrade commit;
- cursor update argument validation and getAll query parsing were tightened;
- `IDBCursor.update(null)` now raises `DataError` synchronously when the store
  has a key path;
- key-range handles use an explicit runtime identity registry because Boa's
  native prototype/type checks are not reliable for these objects.

Verification remains green for `cargo fmt --all -- --check`, workspace clippy,
and `cargo test -p boa_idb --tests`. The following WPT cases still reproduce on
the branch and must not be treated as resolved: one cursor-open assertion,
deleted-store/recreate-store lifecycle assertions, the deleteDatabase
open-queue close race, upgrade transaction abort event ordering, and invalid
getAll keys. Unsupported exotic structured-clone failures remain outside this
stabilization pass.
