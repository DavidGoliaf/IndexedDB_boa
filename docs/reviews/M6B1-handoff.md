# Handoff: M6-B1 — Segments, compaction, MVCC snapshots

Work order: `tasks/08_TASK_M6B_FILESYSTEM_COMPLETION.md` (sub-order **B1**)  
Branch: `task/m6-fs-completion` (from M6-A `94b4bff`)  
Tip: `488c837`  
Scope gate: `QUESTIONS.md` — full M6-B split into B1/B2/B3

## Design

- **ADR-010:** `rpds` `RedBlackTreeMap` (MIT) for structural-share maps;
  `ISEG` / `IMAN` codecs with CRC32C; compaction thresholds 64 MiB / 10k
  frames; `SegmentGuard` reclaim-on-drop only for superseded segments.
- Readonly begin uses `DbState::snapshot_clone` + `SnapshotMeter` (structural
  clones, zero deep walks).
- Compaction: write segment → empty next WAL → publish manifest/`CURRENT` →
  replace live guards (mark old reclaimable).

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

- R8.3.3, R8.3.4 → **PASS** (B1 tests)
- R8.3.6, R8.5.* → remain **PARTIAL** until B2/B3

## Not in B1

FileSystem fault injection, kill-worker 200-iter suite, WPT `--backend fs`.

**Do not treat normative M6 as complete.**
