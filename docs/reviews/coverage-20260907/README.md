# Coverage + deny evidence, 2026-09-07 (P2)

Independent local rerun of the exact nightly `coverage` job configuration.

## Coverage command (chunked, same accumulation as nightly)

Nightly runs `cargo llvm-cov --workspace --lib --bins --tests --examples
--no-report` then three WPT backend runs with `--no-clean`, then reports.
The single command exceeds one tool-call budget on this host, so it ran as
verified chunks with the correct flags (`cargo llvm-cov test --no-clean
...` per package, `cargo llvm-cov run --no-clean` for the WPT binaries,
examples and WPT lib included). A previous attempt used the illegal
`--no-report + --no-clean` combination, which errored without executing —
all chunks below were re-run with output-verified PASS lines.

Chunks (all PASS, zero failures):

- `boa_idb_core --lib --tests` (incl. new `engine_units_tests`,
  `clone_units_tests`, `key_units_tests`)
- `boa_idb_memory / boa_idb_fs / boa_idb_sqlite --lib --tests`
- `boa_idb --lib --bins` + 8 fast suites (incl. new `dom_api_tests`
  26/26 and in-crate observer units)
- `boa_idb --test memory_gates_tests` (rest 6/6; 1M store walk 1/1;
  1M index walk 1/1 — split because one chunk exceeds the timeout)
- `boa_idb --examples`, `boa_idb_wpt --lib --tests`
- WPT binaries: memory 482/482, sqlite 482/482, fs 482/482

## Gate (the exact workflow script, fail-closed)

`package-totals.txt` was produced by the same script body as
`nightly-m7.yml` ("Per-package threshold gate"), run against `cov.json`:

```
<see package-totals.txt>
```

## Artifacts

- `cov.json` (6.4 MB) — llvm-cov JSON report
- `lcov.info` (1.5 MB) — LCOV report (same as the nightly artifact)
- `package-totals.txt` — threshold gate output
- `deny-check.log` — `cargo deny check` output

## cargo-deny

`deny-check.log`: advisories/bans/licenses/sources ok against advisory DB
`5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5` dated 2026-09-02 (the CI-cached
copy at `target/cargo-deny-advisories`; `cargo deny fetch db` is a no-op
offline on this host, so freshness is the cached DB date — stated, not
claimed as a fresh fetch).

Command: `$env:CARGO_DENY_DB_PATH='target/cargo-deny-advisories';
cargo deny check` (same as CI).
