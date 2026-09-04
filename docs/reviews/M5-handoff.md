# Handoff: M5 — WPT harness and stabilization

Work order: `tasks/05_TASK_WPT_HARNESS_AND_STABILIZATION.md`  
Branch: `task/m5-wpt-runner`

## Delivered

- Completed the `boa_idb_wpt` isolated Boa-context runner, browser polyfills,
  timer/event-loop driving, native completion bridge, CLI, report model and
  expectations support.
- Added the checked-in IndexedDB WPT subset and runner unit tests.
- Added deterministic `crates/boa_idb_wpt/expectations.json`; known failures
  are recorded with their observed status and reason, with no TIMEOUT/CRASH
  status.
- Added `docs/traceability.md` and the `boa_idb_wpt` crate README.
- Fixed empty-key-path key determination and ignored invalid optional index
  keys during index synchronization.
- Made the CLI default to one worker for reproducible expectation checks;
  explicit parallelism remains available with `--threads N`.

## Verification

All commands below passed locally:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --quiet --check-expectations
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --quiet --check-expectations
```

WPT summary after key-conversion, multiEntry, reverse-cursor, and upgrade connection lifecycle stabilization: Memory 449/482 PASS
(93.2%); SQLite 450/482 PASS (93.4%). The shared key conversion suite now
passes 27/27 on both backends.
Both exceed the required 80% threshold. The remaining 35 failures are
deterministically captured in `expectations.json` and are concentrated in
upgrade rollback/close behavior, deleted object-store/index state, getAll
invalid-query handling, and unsupported exotic structured-clone platform
objects.

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
- key-range handles use an explicit runtime identity registry because Boa's
  native prototype/type checks are not reliable for these objects.

Verification remains green for `cargo fmt --all -- --check`, workspace clippy,
and `cargo test -p boa_idb --tests`. The following WPT cases still reproduce on
the branch and must not be treated as resolved: one cursor-open assertion,
deleted-store/recreate-store lifecycle assertions, the deleteDatabase
open-queue close race, upgrade transaction abort event ordering, and invalid
getAll keys. Unsupported exotic structured-clone failures remain outside this
stabilization pass.
