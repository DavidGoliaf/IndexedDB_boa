# Receipt: full 1M cursor matrix (P1-2/B2), 2026-09-06

Local inaugural run of the full normative matrix (store+index x Next/Prev x
memory/SQLite/FS at 10^6 records, dhat peak < 1 MiB each). The nightly
`cursor-matrix-1m` job runs the same matrix on Linux; this run proves the
harness and the cursors at full scale.

| Field | Value |
|---|---|
| date (UTC) | 2026-09-06 |
| git | branch `task/m7b-optimizations-reliability`, HEAD `62f32db` + uncommitted (coverage tests, dispatch fix) |
| rustc | 1.91.0 |
| OS / CPU / arch | Windows 11 / Intel i7-14700K / x86_64 |
| profile | `release` (allocator gate is profile-independent; debug PR subset also green) |
| commands | `BOA_IDB_FULL_MATRIX=1 cargo test --release -p boa_idb --test memory_gates_tests cursor_store_walk_bounded_1m -- --nocapture --test-threads=1` and `... cursor_index_directions_bounded ...` |
| bound | 1048576 bytes peak heap per walk (dhat, allocations inside the window only; DB open happens outside — see H-FS-OPEN) |

## Store walks (1M each)

```
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=memory dir=Next scale=1000000 bound=1048576 peak=936
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=memory dir=Prev scale=1000000 bound=1048576 peak=936
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=sqlite dir=Next scale=1000000 bound=1048576 peak=236944
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=sqlite dir=Prev scale=1000000 bound=1048576 peak=236884
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=fs dir=Next scale=1000000 bound=1048576 peak=1088
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=fs dir=Prev scale=1000000 bound=1048576 peak=1088
test result: ok. 1 passed; finished in 59.37s
```

## Index walks (1M each)

```
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=memory dir=Next scale=1000000 bound=1048576 peak=952
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=memory dir=Prev scale=1000000 bound=1048576 peak=952
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=sqlite dir=Next scale=1000000 bound=1048576 peak=237396
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=sqlite dir=Prev scale=1000000 bound=1048576 peak=237560
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=fs dir=Next scale=1000000 bound=1048576 peak=1112
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=fs dir=Prev scale=1000000 bound=1048576 peak=1112
test result: ok. 1 passed; finished in 79.93s
```

Verdict: 12/12 PASS. Peaks: memory ~1 KiB, FS ~1 KiB, SQLite ~237 KiB
(page-cache warmup inside the window; no per-range allocation).
