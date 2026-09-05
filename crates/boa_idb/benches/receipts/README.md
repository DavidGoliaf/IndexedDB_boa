# Benchmark receipts and baseline workflow (M7-A)

Suite: `crates/boa_idb/benches/backends.rs` (`cargo bench -p boa_idb --bench backends`).
Reference configuration (§12.1): 4 vCPU, NVMe SSD, Linux, release profile with
`lto = "thin"` (workspace `Cargo.toml`), ~200 B values, numeric keys,
`relaxed` durability unless the scenario states otherwise.

## Scale: normative vs diagnostic

Normative (default) scale matches §12.1: `put` batch 10 000, open fixture
1 000 000 records. **Exception — cursor scan (P1-2):** the `criterion`
`cursor_scan_<N>` benches run at the **diagnostic** 20 000-record scale
(the id carries `N`); the **normative 1M full scan** is the `scan-1m`
example runner below. Diagnostic numbers must never be quoted as release
evidence or compared against §12.1 targets.
`BOA_IDB_BENCH_SMOKE=1` shrinks every fixture further for CI speed.

## Commands

```sh
# Full normative run (authoritative host only for evidence)
cargo bench -p boa_idb --bench backends -- --save-baseline m7a-<yyyymmdd>

# Normative 1M cursor scan, one pass with a controlled timeout (P1-2).
# Defaults ARE the normative case: --records 1000000 --timeout-secs 1800.
cargo run -p boa_idb --example scan-1m -- --backend sqlite --root ./scan-1m-data
cargo run -p boa_idb --example scan-1m -- --backend fs --root ./scan-1m-data
# Split long runs: build once, then scan against the reused fixture.
cargo run -p boa_idb --example scan-1m -- --backend sqlite --root ./scan-1m-data --build-only
cargo run -p boa_idb --example scan-1m -- --backend sqlite --root ./scan-1m-data --reuse

# Fast smoke run (any runner, CI)
BOA_IDB_BENCH_SMOKE=1 cargo bench -p boa_idb --bench backends

# Compare the current tree against a stored baseline (>10 % regress blocks)
cargo bench -p boa_idb --bench backends -- --baseline m7a-<yyyymmdd>
```

Exit codes of `scan-1m`: `0` result (the verdict is in the output —
`RESULT` or `MISS`), `1` usage, `2` backend error, `3` controlled scan
timeout. A `TIMEOUT` line records rows scanned, elapsed and timeout value
and counts as a `MISS`, never a pass.

Criterion stores raw output under `target/criterion/` (per-benchmark
`estimates.json` plus plots); `--save-baseline` snapshots it for later
`--baseline` comparison.

## Receipt template

Every quoted result (`RECEIPT-<yyyymmdd>-<host>.md` next to this file) must
contain:

| Field | Example |
|---|---|
| date (UTC) | 2026-09-05 |
| git commit | full SHA of the measured tree |
| rustc / cargo | `1.91.0` |
| OS / CPU / RAM / storage | `Windows 11 / <cpu> / <ram> / <disk>` |
| profile | `bench` (inherits `release`, `lto = "thin"`) |
| command | exact `cargo bench …` invocation |
| scale | `normative` or `smoke` (+ env) |
| params | key/value sizes, durability per scenario |
| results | per-scenario table: bench id, mean, throughput |
| raw output | `target/criterion/…` paths or attached archive |
| host role | `authoritative` or `diagnostic` |

A metric without platform/commit/configuration/provenance is diagnostic, not
release evidence.

## Policies (§3.4–§3.5 of the work order)

* Baseline comparison counts only on the pinned benchmark host or with a
  provably identical configuration. Heterogeneous runners (default CI) run
  smoke/format only; the authoritative comparison lives in a labelled /
  self-hosted job (M7-B wires the blocking gate).
* A missed target gets a profile (flamegraph or equivalent reproducible
  profiler output), the deviation magnitude, an explanation, and a bounded
  follow-up. Targets are never weakened, benchmarks never deleted, and local
  numbers never presented as the reference.
