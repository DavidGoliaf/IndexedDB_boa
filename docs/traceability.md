# Матрица трассируемости требований IndexedDB 3.0

Матрица связывает требования ТЗ с реализацией и проверками. Статус `PASS`
означает, что соответствующая проверка существует и проходит; отдельные
известные расхождения WPT перечислены в
`crates/boa_idb_wpt/expectations.json`.

| Требования | Реализация | Проверка | Статус |
|---|---|---|---|
| R2.1–R2.4 | Workspace, safe Rust, toolchain | `cargo check --workspace`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| R5.0.1–R5.0.5, R5.11.1 | `boa_idb::convert`, DOM shim; Rust-side platform-clone identity registry | `integration_tests.rs`, `basic_idb_flow_tests.rs`, `platform_clone_hardening_tests.rs`, `structured-clone.any.js` | PASS |
| R6.1.1–R6.6.3 | `boa_idb_core::key`, `clone` | `crates/boa_idb_core/tests/key_tests.rs`, `key_proptests.rs`, `keypath_tests.rs`, `scf_tests.rs`, `scf_proptests.rs` | PASS |
| R7.1.1–R7.4.4 | Registry, open queue, scheduler, cursor engine | `open_queue_tests.rs`, `scheduler_tests.rs`, `engine_ops_tests.rs` | PASS |
| R8.1.1–R8.2.3 | Memory and SQLite backends | `crates/boa_idb_memory/tests/*.rs`, `crates/boa_idb_sqlite/tests/*.rs` | PASS |
| R8.3.1, R8.3.2, R8.3.5 | FS WAL codec/recovery (txn_seq isolation + CONTINUES/COMMIT chains), strict/relaxed sync, `max_keys_in_memory` | `boa_idb_fs` `wal` unit/proptests (incl. sequence mismatch / multi-frame encode), `fs_backend_tests.rs` multi-frame + reopen cases | PASS |
| R8.3.3, R8.3.4 | Immutable `seg/*.seg` + manifest compaction; `rpds` persistent maps + segment refcount | `segment`/`compact` unit tests, `m6b1_tests.rs` (threshold compact, retain-until-drop, SnapshotMeter) | PASS |
| R8.3.6 | Exclusive `LOCK` on open; lock released after forced worker kill; reopen recovers | `second_open_fails_while_lock_held`, `m6b3_crash_tests` (`Child::kill` / SIGKILL·TerminateProcess) | PASS |
| R8.5.1 | Crash consistency: kill-worker committed-prefix + index/keygen | `boa-idb-fs-crash-worker` + `m6b3_crash_tests` (CI 8 / nightly 200 via `BOA_IDB_FS_CRASH_ITERS`) | PASS |
| R8.5.2, R8.5.3 | `FileSystem` trait + deterministic fault matrix at WAL/segment/manifest/cleanup | `vfs.rs` (`OsFileSystem`, `FaultInjectingFs`); `m6b2_tests.rs` table-driven ENOSPC/EIO/short-write/sync/rename + corrupt reopen | PASS |
| R9.1.1–R9.4.2 | Runtime pump, transaction lifecycle, request dispatch | `crates/boa_idb/tests/basic_idb_flow_tests.rs`, `appendix_d_acceptance_tests.rs`, `transaction-lifetime-empty.any.js` | PASS |
| R10.2.1–R10.2.3 | Error mapping and DOMException | `integration_tests.rs`, `key-conversion-exceptions.any.js` | PASS |
| R11.1–R11.7 | Security, privacy, quota and resource limits; sealed platform-clone brand | `limits.rs` tests, backend integration tests, `platform_clone_hardening_tests.rs` | PASS |
| R13.3.1 | Autonomous WPT runner and Boa environment | `cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary` | PASS |
| R13.3.2 | WPT conformance threshold | Full memory, SQLite, and FS runs: 482/482 PASS (100.0%), 0 FAIL, 0 TIMEOUT, 0 NOTRUN | PASS |
| R13.3.3 | Priority WPT subset and deterministic expectations | Full memory and SQLite runs pass; strict snapshot checks report 482 matched and 0 unexpected | PASS |
| R13.4.1–R13.4.2 | Backend differential behavior | `differential_model_tests.rs`, `differential_fs_tests.rs` (memory↔FS), backend suites | PASS |
| R13.5.1–R13.5.2 | Ordering and concurrency | `scheduler_tests.rs`, `sqlite_concurrency_tests.rs` | PASS |
| R13.6.1–R13.6.2 | GC/resource lifecycle | WPT isolated-context runner and backend cleanup tests | PASS |
| R12.1 | Benchmark harness, 7 §12.1 scenarios (SQLite/FS + memory control), receipts, CI smoke | `crates/boa_idb/benches/backends.rs` (scan ids carry `N`, diagnostic), `examples/scan-1m.rs` (normative 1M one-pass runner, defaults = 1M), `benches/receipts/` (`README.md`, `RECEIPT-2026-09-05-*`, raw logs), `.github/workflows/ci.yml` (`bench-smoke`) | PARTIAL (harness + smoke + real 1M evidence with honest MISS verdicts; authoritative baseline comparison and >10 % blocking gate need the pinned host → M7-B; misses carry magnitude + bounded follow-ups F1–F5, profiles → M7-B) |
| R12.2 | Memory: no range materialization, connection/txn overhead probes, lifecycle RSS gates | Observer byte counters + bench suite as measurement base; full gates (cursor O(1) memory, ≤64 KiB/≤8 KiB probes, `dhat`/`valgrind`) | PARTIAL (measurement base only → M7-B) |
| R12.3 | Observability: `tracing` spans, `IdbObserver`, dev CLI | `boa_idb::observer` (`idb.open/txn/request`, counters, bounded histograms), `IndexedDbExtensionBuilder::observer`, `examples/boa-idb-cli.rs` (`ls/dump/verify/compact/stats`, `--values` gate); `observer_tests.rs`, `cli_tests.rs`, CI tracing job | PASS |
| R13.1 | Test levels: perf benchmarks + memory gates | `criterion` suite (perf level present); `dhat`/`valgrind` lifecycle gates | PARTIAL (memory level → M7-B) |
| R13.2 | Coverage thresholds (`boa_idb_core` ≥90 %, `boa_idb` ≥80 %, `llvm-cov` gate) | No enforced gate yet | PARTIAL (gate + evidence → M7-B) |
| R13.4.1 | Backend differential behavior (10 000 nightly scenarios) | `differential_model_tests.rs` (10 000 proptest cases), `differential_fs_tests.rs`, backend suites | PASS (existing evidence; 10k seeded nightly expansion → M7-B) |
| R13.6.1–R13.6.2 | GC/resource lifecycle: RSS growth, handle release | WPT isolated-context runner and backend cleanup tests; long-lifecycle RSS gate | PARTIAL (existing cleanup PASS; RSS/long-lifecycle gate → M7-B) |
| R14.1–R14.6 | Documentation, build and delivery; license allowlist hygiene | `cargo fmt`, `cargo doc`, `cargo deny check`, this matrix, crate READMEs, `M6-handoff.md`, `M6A-handoff.md` | PASS |

## Requirement ID coverage

The following index records every requirement identifier present in the
normative TZ and points it to the grouped row above:

* R5: `R5.0.1 R5.0.2 R5.0.3 R5.0.4 R5.0.5 R5.11.1`.
* R6: `R6.1.1 R6.1.2 R6.1.3 R6.2.1 R6.2.2 R6.2.3 R6.3.2 R6.3.3 R6.3.4 R6.3.5 R6.4.1 R6.4.2 R6.4.3 R6.4.4 R6.4.5 R6.4.6 R6.4.7 R6.4.8 R6.5.1 R6.5.2 R6.5.3 R6.5.4 R6.6.1 R6.6.2 R6.6.3`.
* R7: `R7.1.1 R7.1.2 R7.1.3 R7.1.4 R7.1.5 R7.2.1 R7.2.2 R7.2.3 R7.2.4 R7.2.5 R7.2.6 R7.2.7 R7.3.1 R7.3.2 R7.3.3 R7.4.1 R7.4.2 R7.4.3 R7.4.4`.
* R8: `R8.1.1 R8.1.2 R8.1.3 R8.1.4 R8.1.5 R8.1.6 R8.2.1 R8.2.2 R8.2.3 R8.3.1 R8.3.2 R8.3.3 R8.3.4 R8.3.5 R8.3.6 R8.5.1 R8.5.2 R8.5.3`.
* R9–R11: `R9.1.1 R9.1.2 R9.2.1 R9.3.1 R9.3.2 R9.3.3 R9.4.1 R9.4.2 R10.2.1 R10.2.2 R10.2.3 R11.1 R11.2 R11.3 R11.4 R11.5 R11.6 R11.7`.
* R13–R14: `R13.3.1 R13.3.2 R13.3.3 R13.4.1 R13.4.2 R13.5.1 R13.5.2 R13.6.1 R13.6.2 R14.1 R14.2 R14.3 R14.4 R14.5 R14.6`.
* R12–R13 (M7-A): `R12.1 R12.2 R12.3 R13.1 R13.2 R13.4.1 R13.6.1 R13.6.2` (grouped rows above).
