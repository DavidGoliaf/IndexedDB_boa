# Handoff: M6-B2 — FileSystem + fault injection matrix

Work order: `tasks/08_TASK_M6B_FILESYSTEM_COMPLETION.md` (sub-order **B2**)  
Branch: `task/m6-fs-completion` (continues from M6-B1 tip `10f227c`)  
Tip: `d3a6c48`  
Scope gate: `QUESTIONS.md` — B1 done; B2 in this delivery; B3 next

## Design

- **ADR-011:** internal `FileSystem` trait; `OsFileSystem` default;
  `FaultInjectingFs` for deterministic ENOSPC/EIO/short-write/sync/rename/
  interrupt at WAL, segment, manifest, `CURRENT`, meta, dir-sync, cleanup,
  lock, truncate sites.
- Factory: `with_filesystem`; legacy `with_sync_hooks` → `SyncHooksFs`.
- All production IO (atomic write, WAL append, lock, open/list/delete,
  segment reclaim) routes through the trait; no control-flow fork for tests.

## Verification

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p boa_idb_fs
cargo test --workspace
cargo doc --workspace --no-deps
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny check
```

## Traceability

- R8.5.2, R8.5.3 → **PASS** (`m6b2_tests.rs`)
- R8.5.1, R8.3.6 process-kill → remain **PARTIAL** until B3

## Not in B2

Kill-worker 200-iter suite, WPT `--backend fs`, differential FS parity.

**Do not treat normative M6 as complete.**
