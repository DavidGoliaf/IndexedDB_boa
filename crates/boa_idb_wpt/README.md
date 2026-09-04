# boa_idb_wpt

`boa_idb_wpt` is the conformance-test pipeline for the IndexedDB bindings. It
runs the checked-in `*.any.js` WPT subset in isolated Boa contexts, using
the memory, SQLite, or filesystem backend, and emits deterministic JSON
expectations for known implementation gaps.

```powershell
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend fs --summary
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --check-expectations
```

The default is one worker so expectation checks are reproducible. Use
`--threads N` for an explicitly parallel run.

Each run uses an isolated temp storage root; filesystem/`SQLite` handles and
`LOCK` files are released when the per-file context is dropped.
