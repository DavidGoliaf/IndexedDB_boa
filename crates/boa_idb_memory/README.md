# boa_idb_memory

The in-memory `BackendFactory` and transaction implementation used for fast
IndexedDB execution and deterministic tests.

It is selected by the WPT runner with `--backend memory`. Run its tests with
`cargo test -p boa_idb_memory`.
