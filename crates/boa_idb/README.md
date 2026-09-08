# boa_idb

The public IndexedDB binding crate. It exposes the JavaScript API and connects
Boa values and events to the core storage engine.

Limitations and backend behavior are defined by the workspace specification;
backend-specific persistence lives in the sibling crates. Run its tests with
`cargo test -p boa_idb`.

## Memory model

- Cursors never materialize the iterated range: full scans stream with O(1)
  memory in the range size (gated by `memory_gates_tests`, all backends).
- `getAll`/`getAllKeys`/`getAllRecords` *without* an explicit `count` is the
  only operation allowed to hold a large in-memory result: it materializes
  every matching record. Prefer cursors or a bounded `count` for large
  stores; the default `max_get_all` limit (1 000 000 entries) guards the host
  against unbounded growth.
- Per-connection overhead stays below 64 KiB and per-live-transaction
  overhead below 8 KiB (backend caches excluded), likewise gated.
  Benchmarks live in `benches/` with receipts under `benches/receipts/`.
