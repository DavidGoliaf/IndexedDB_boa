# Handoff: M6-B — filesystem completion (B1+B2+B3)

Work order: `tasks/08_TASK_M6B_FILESYSTEM_COMPLETION.md`  
Branch: `task/m6-fs-completion`  
Scope gate: `QUESTIONS.md` (B1→B2→B3)

## Commits (sub-order tips)

| Sub-order | Tip | Scope |
|---|---|---|
| M6-B1 | `488c837` / handoff `10f227c` | Segments, compaction, `rpds` MVCC |
| M6-B2 | `d3a6c48` / handoff `b556a6d` | `FileSystem` + fault matrix |
| M6-B3 | `2ea992a` | Crash worker, WPT `--backend fs`, differential |

## Design summary

- **ADR-010:** `rpds` maps + `ISEG`/`IMAN` segments + compaction thresholds.
- **ADR-011:** `FileSystem` / `FaultInjectingFs` IO seam.
- **Crash worker:** `boa-idb-fs-crash-worker` parks at marker; parent
  force-kills (`Child::kill` → SIGKILL / `TerminateProcess`), reopens, checks
  committed prefix + index/keygen consistency.
- **WPT:** `--backend fs` uses isolated temp root per file via existing runner.

## Crash suite

| Mode | Env | Command |
|---|---|---|
| CI subset (default 8) | unset | `cargo test -p boa_idb_fs --test m6b3_crash_tests` |
| Nightly 200 | `BOA_IDB_FS_CRASH_ITERS=200` | same; optional `BOA_IDB_FS_CRASH_SEED` |
| Workflow | `.github/workflows/nightly-fs-crash.yml` | ubuntu + windows |

Failure panics include seed + replay hints.

## WPT results (this delivery)

```text
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend fs --summary
SUMMARY 37 files: 482 PASS (100.0%), 0 FAIL
```

Memory / SQLite baselines unchanged (no expectation mass-edit).

## Differential

`crates/boa_idb_fs/tests/differential_fs_tests.rs` — memory vs FS CRUD /
schema / index / cursors / abort / savepoint; FS Strict reopen durability.

## Traceability

| Req | Status |
|---|---|
| R8.3.3, R8.3.4 | PASS (B1) |
| R8.5.2, R8.5.3 | PASS (B2) |
| R8.3.6, R8.5.1 | PASS (B3 crash worker + LOCK after kill) |
| R13.3.* FS | PASS (≥92 %; actual 100 %) |

## Verification

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny check
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend fs --summary
```

**Do not treat normative M6 as accepted until independent review.**
