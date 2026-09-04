# Матрица трассируемости требований IndexedDB 3.0

Матрица связывает требования ТЗ с реализацией и проверками. Статус `PASS`
означает, что соответствующая проверка существует и проходит; отдельные
известные расхождения WPT перечислены в
`crates/boa_idb_wpt/expectations.json`.

| Требования | Реализация | Проверка | Статус |
|---|---|---|---|
| R2.1–R2.4 | Workspace, safe Rust, toolchain | `cargo check --workspace`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| R5.0.1–R5.0.5, R5.11.1 | `boa_idb::convert`, DOM shim | `crates/boa_idb/tests/integration_tests.rs`, `basic_idb_flow_tests.rs` | PASS |
| R6.1.1–R6.6.3 | `boa_idb_core::key`, `clone` | `crates/boa_idb_core/tests/key_tests.rs`, `key_proptests.rs`, `keypath_tests.rs`, `scf_tests.rs`, `scf_proptests.rs` | PASS |
| R7.1.1–R7.4.4 | Registry, open queue, scheduler, cursor engine | `open_queue_tests.rs`, `scheduler_tests.rs`, `engine_ops_tests.rs` | PASS |
| R8.1.1–R8.5.3 | Memory and SQLite backends | `crates/boa_idb_memory/tests/*.rs`, `crates/boa_idb_sqlite/tests/*.rs` | PASS |
| R9.1.1–R9.4.2 | Runtime pump, transaction lifecycle, request dispatch | `crates/boa_idb/tests/basic_idb_flow_tests.rs`, `appendix_d_acceptance_tests.rs`, `transaction-lifetime-empty.any.js` | PASS |
| R10.2.1–R10.2.3 | Error mapping and DOMException | `integration_tests.rs`, `key-conversion-exceptions.any.js` | PASS |
| R11.1–R11.7 | Security, privacy, quota and resource limits | `limits.rs` tests, backend integration tests | PASS |
| R13.3.1 | Autonomous WPT runner and Boa environment | `cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary` | PASS |
| R13.3.2 | WPT conformance threshold | Memory: 93.2% (449/482); SQLite: 93.4% (450/482) | PASS |
| R13.3.3 | Priority WPT subset and deterministic expectations | `crates/boa_idb_wpt/wpt/IndexedDB/*.any.js`, `expectations.json` | PASS |
| R13.4.1–R13.4.2 | Backend differential behavior | `differential_model_tests.rs`, backend test suites | PASS |
| R13.5.1–R13.5.2 | Ordering and concurrency | `scheduler_tests.rs`, `sqlite_concurrency_tests.rs` | PASS |
| R13.6.1–R13.6.2 | GC/resource lifecycle | WPT isolated-context runner and backend cleanup tests | PASS |
| R14.1–R14.6 | Documentation, build and delivery | `cargo fmt`, `cargo doc`, this matrix, crate READMEs | PASS |

## Requirement ID coverage

The following index records every requirement identifier present in the
normative TZ and points it to the grouped row above:

* R5: `R5.0.1 R5.0.2 R5.0.3 R5.0.4 R5.0.5 R5.11.1`.
* R6: `R6.1.1 R6.1.2 R6.1.3 R6.2.1 R6.2.2 R6.2.3 R6.3.2 R6.3.3 R6.3.4 R6.3.5 R6.4.1 R6.4.2 R6.4.3 R6.4.4 R6.4.5 R6.4.6 R6.4.7 R6.4.8 R6.5.1 R6.5.2 R6.5.3 R6.5.4 R6.6.1 R6.6.2 R6.6.3`.
* R7: `R7.1.1 R7.1.2 R7.1.3 R7.1.4 R7.1.5 R7.2.1 R7.2.2 R7.2.3 R7.2.4 R7.2.5 R7.2.6 R7.2.7 R7.3.1 R7.3.2 R7.3.3 R7.4.1 R7.4.2 R7.4.3 R7.4.4`.
* R8: `R8.1.1 R8.1.2 R8.1.3 R8.1.4 R8.1.5 R8.1.6 R8.2.1 R8.2.2 R8.2.3 R8.3.1 R8.3.2 R8.3.3 R8.3.4 R8.3.5 R8.3.6 R8.5.1 R8.5.2 R8.5.3`.
* R9–R11: `R9.1.1 R9.1.2 R9.2.1 R9.3.1 R9.3.2 R9.3.3 R9.4.1 R9.4.2 R10.2.1 R10.2.2 R10.2.3 R11.1 R11.2 R11.3 R11.4 R11.5 R11.6 R11.7`.
* R13–R14: `R13.3.1 R13.3.2 R13.3.3 R13.4.1 R13.4.2 R13.5.1 R13.5.2 R13.6.1 R13.6.2 R14.1 R14.2 R14.3 R14.4 R14.5 R14.6`.
