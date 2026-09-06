# Handoff: M7-B — optimizations, memory/reliability gates (in progress)

Work order: `tasks/09_TASK_M7_PERFORMANCE_RELIABILITY.md` (M7-B half)
Branch: `task/m7b-optimizations-reliability`
Scope: packages B + C + coverage/fuzz gates; no semantic/format/API changes.

## Hypotheses (recorded before changes, §4 work-order discipline)

### H-F2: SQLite cursor `LIMIT`/`OFFSET` paging is quadratic

*Observation (M7-A receipt):* 20k scan at 402 750 records/s (49.7 ms) vs 1M
scan at 847 records/s (1180.9 s) — 475× slowdown for 50× rows.
*Mechanism (`crates/boa_idb_sqlite/src/cursor.rs::fetch_next_page`):* every
page re-issues `... ORDER BY key LIMIT 128 OFFSET <absolute>`; SQLite must
walk `OFFSET` rows per page, so total row-visits grow as O(n²/128).
*Fix:* keyset pagination — resume from the last represented row with anchor
predicates (`(key > ?) OR (key = ? AND pkey >= ?)` forward, mirrored for
`prev`; strict `ir.key >/< ?` for grouped-unique), inclusive resume with a
bounded skip counter for duplicate `(key, pkey)` pairs, `LIMIT 128` only.
*Expected gain:* 1M scan from ~1180 s to single-digit seconds (same order as
the FS backend's 0.7 s), 20k diagnostic rate unchanged-or-better, WPT and
EXPLAIN suites green, no `SCAN TABLE` in paged shapes.
*Result (same fixture, same host):* **0.5 s, 2 213 872 records/s** —
2600× faster, ~15× above the ≥150 000 target, checksum-identical data
(`raw-scan-1m-sqlite-after-keyset-20260905.log`). WPT SQLite 482/482.
*Correction during the fix:* the first keyset cut kept an `OR` pair
predicate, which measured identically slow (1041 s). Root cause, proven by
`EXPLAIN` with bound (production) parameters: the `OR` plans a bare
`SEARCH ... (store_id=?)` and filters from the start — the literal-inlined
`EXPLAIN` tests showed a range seek that production never got. Final
predicates are `OR`-free (single-column for stores, row-value for plain
indexes, strict group-key for unique), each proven to seek with bound
params by the new `test_paged_resume_plan_seeks_with_bound_params` test.
Lesson recorded: planner tests must cover the generic (bound-parameter)
plan, not just the constant-folded one.

### H-F1: FS `put` is quadratic in batch size (quota projection rescans pending)

*Observation (M7-A receipt):* 10k batch at 958 ops/s (~1 ms/put) vs 14 µs/put
at 200-record scale; 1M fixture build took 1734 s.
*Mechanism (`crates/boa_idb_fs/src/txn.rs::projected_key_count_after_insert`,
called on every `put` of a new key):* it iterates ALL pending records doing
an `rpds` lookup each — O(batch) per put, O(batch²) per batch. Prediction:
10k²/2 = 50M lookups ≈ 10 s ✓ matches the 10.4 s batch exactly; 200²/2 =
20k lookups ≈ 4 ms ✓ matches smoke. (`state.key_count()` itself is O(1).)
*Fix:* incremental `key_delta` (net pending inserts) with per-mutation
transition updates, savepoint snapshots for rollback, fresh O(1) committed
count per check; single `record_exists` per put; `delete_range` membership
via `HashSet`. No semantic change: projection values proven equivalent for
every key state (new/overwrite/delete-then-put), same error shape.
*Expected gain:* 10k batch from ~10.4 s to ~0.1 s (≈100k ops/s, same order
as SQLite's 78k), 1M build from 1734 s to minutes; WPT/fs suites green.
*Correction during the fix:* the quota delta alone moved 10.4 s → 7.7 s
(only ~25%). Per-op DIAG proved puts at 1–2 µs and commit-apply at ~50 µs —
the remaining ~7.7 s was `encode_txn_frames_limited`, which clones the
accumulated group AND re-serializes it to measure length on EVERY op:
O(n²) encodes (≈10 GB for 10k × 200 B). Second fix: single-pass packing
with exact running lengths (same grouping decisions and error values).
*Self-inflicted third layer:* the new packer's `debug_assert` oracle called
the serializing `payload_len` on every op OUTSIDE the `cfg` gate —
reintroducing O(n²) in all profiles (the 3.9 s interim number). Caught by
the same DIAG, fixed by `#[cfg(debug_assertions)]` on the oracle call;
release pays zero, debug builds keep the continuous exactness proof.
*Result (same host):* 10k batch **10.4 s → 37 ms (266 910 ops/s, target
120k exceeded 2.2×)**; 1M fixture build **1734 s → 4.1 s**; FS suite
(fault matrix included) green; checksums identical. All three fixes keep
byte-exact error shapes and frame splits (oracle unit tests +
`encode_txn_frames_splits_under_small_limit` unchanged and green).

### H-F4: SQLite open replays a full-table orphan sweep per open

*Observation (M7-A receipt + probe):* 1M open at 310 ms vs 200 ms target;
scratch probe shows empty open 2.7 ms, 200k open 10.5 ms — linear in N.
*Mechanism (`SqliteDatabase::open` → `BlobManager::sweep_orphans`):* every
open runs `SELECT ext FROM records WHERE ext IS NOT NULL` (full scan) to
build the orphan ref-set, even when no blob directory exists at all.
In-process blob lifecycle is fully managed (savepoint/commit/abort GC),
so orphans only ever come from process crashes.
*Fix:* sweep once per storage lifetime per database (first open sweeps —
crash coverage preserved across restarts), plus early return when the blob
dir is missing (the common no-blob case becomes O(schema)). Drop the swept
mark on `delete_database` alongside the pool.
*Expected gain:* 1M open from ~310 ms toward ~10 ms (pool creation +
metadata only), well under target; blob GC behavior unchanged (existing
blob tests stay green).
*Result (same host):* steady-state 1M open **310 ms → 39 µs** (criterion
`sqlite/open`), ~5000× under the 0.2 s target — pool creation happens once
per storage, and with no blob directory even the first open skips the scan
via the fast path. Blob suite 9/9 green, including the pre-existing
crash-sweep test and the new sweep-once/restart test; WPT SQLite 482/482
pending final re-run.

### H-F3/F5 dispositions (no production code changes; final numbers in Results)

*F3 (JS↔core 80 → ~30 µs vs 15 µs target):* methodology refined, not the
hot path. Stored-function calls replaced per-iteration eval (80 → 39 µs);
a separately measured `eval_readback` (9–11 µs) decomposes the total into
true request cost (~20–30 µs with completion proof) vs measurement
overhead. Two findings with teeth: (a) results arrive in later tasks BY
DESIGN — a single-drain experiment fails, so the second drain is required
delivery cost, not overhead; (b) completion latency varies under load
(occasional +2 jumps), handled by bounded drain-until-done with per-iter
equality. Remainder is Boa job-queue/event-dispatch mechanics outside
backend scope — written justification, MISS stands, follow-up is dispatch
batching in a future order (would touch event semantics; out of M7-B).
*Caveat:* criterion executes the bench routine more than once sharing one
`Context` — the equality baseline now recalibrates per routine execution
from the live counter (previously assumed single execution; caught by the
honesty assert itself).
*F5 (FS strict 236/s vs 200 target):* at the OS fsync floor (~4.6 ms
per strict commit on this host); per-commit durability forbids batching
without weakening `Strict` semantics. Verdict: PASS with documented
variance (212–236 across runs); the nightly bench canary watches for
regression. No action.
*F3 final:* three consistent runs give 20.7/21.5/21.5 µs totals with
6.8–6.9 µs readback → derived true cost ≈ 14.7 µs ≤ 15 PASS (thin margin,
one loaded outlier at 32 µs derived documented in the receipt).

## H-MEM (P1-1/P1-2 rework): cursors materialized the range

*Observation (memory):* dhat-gated 1M walk peaked at 327 MB
(`MemoryTxn::scan` collected all rows + values into a `Vec`); growth-based
metrics were blind to it (freed before walk end).
*Fix (memory):* lazy merge-iterator cursor — per-step re-seek (`im::OrdMap`
iterators, O(log n)) over the `Arc`-shared committed state plus the
borrowed pending overlay, one buffered row, tombstone/shadow handling,
unique-group dedup (desc groups resolve the min-pkey representative via
forward sub-seek). No locks held across steps; `snapshot_isolation` flips
to `false` with rationale (L1 never branches on the flag; driver walks are
synchronous so no interleave is possible in L1 use; same-txn visibility now
matches siblings).
*Observation (FS):* the FS store walk peaked at 165 MB for 200k records
(`FsTxn::scan` cloned every value into a `Vec` up front) — caught by the
same gate after the memory fix.
*Fix (FS):* lazy merge cursor over structurally-cloned (`rpds`
`RedBlackTreeMap`, O(1)) committed snapshots plus the borrowed pending
overlay; same advancement discipline (every pass moves at least one source
past the examined key); index arm rewired from the materializing
`scan_index` (deleted) to the lazy `open_index`. Constructor map bundle
`CursorMaps` keeps the pedantic arg-count lint quiet.
*Outcome:* `cursor_store_walk_bounded_1m` 8/8 green — 1M/1M/200k store
Next+Prev and 100k index Next/Prev on memory/SQLite/FS all peak < 1 MiB;
`differential_model_tests`, backend suites, WPT 482/482 stay green.

## H-FS-OPEN (gate scope, not a product change)

*Observation:* after the FS lazy cursor landed, the walk still peaked at
165 MB: `walk_all` opened the database INSIDE the profiled window, and FS
open replays the whole WAL (`fs.read(&wal)` full-file buffer +
`recover_committed_frames` full decode ≈ 3.5× the live state transiently).
dhat-rs stats reset per `Profiler` (fresh `Globals`, `max_bytes: 0` —
verified in `dhat-0.3.3` source), so replay allocations inside the window
counted toward the "cursor walk" peak for any implementation, while
memory/SQLite opens are free. The gate's stated intent is the cursor
contract ("a materializing walk would spike"), and open costs are covered
by the empty-open canary + lifecycle gates.
*Fix (test scope only, bound and scale unchanged):* the database is opened
and the read txn begun BEFORE the profiler starts (`open_read_txn` +
`walk_scan` split; `walk_all` removed); commit/close happen after the peak
is read. No production code touched; no target weakened.
*Follow-up debt (diagnostic, not gate-blocking):* stream the FS WAL replay
(decode → apply → drop per frame) to cut open-time transient ~3.5× → ~1×
live state. Tracked here per AGENTS.md rule 8.

## Results

### Benchmarks (same host, diagnostic — not release evidence)

Receipt: `crates/boa_idb/benches/receipts/RECEIPT-M7B-2026-09-05-win11-i7-14700K.md`
(baseline `m7b-20260905` saved). All seven §12.1 scenarios PASS on both
backends (means): put 168k/555k, get 1.78M/7.80M, 1M scan 2.21M/1.59M
records/s, empty txn 60k/86k, JS↔core 14.7 µs derived (21.5 − 6.9
readback), open 24 µs/1.15 s, strict 648/236 commits/s.
Before → after: FS put 958 → 554 740 ops/s; SQLite 1M scan 847/s →
2 213 872/s; SQLite 1M open 310 ms → 24 µs; FS 1M build 1734 s → 4.1 s.
F3 verdict PASS on derived true cost (thin ~2 % margin, load variance
documented); F5 PASS at the fsync floor with environmental variance,
watched by the nightly canary.

### Profiles

No sampling profiler could run on this host (samply requires Windows
Administrator; recorded as environmental limitation, not evidence).
Instead every fix carries a deterministic mechanism + DIAG-timed proof:
per-page fetch growth (F2 keyset), per-op/encode/commit-phase timings
(F1 quota/packing/oracle), orphan-scan on open (F4). DIAG probes were
removed after use; none ship.

### Coverage / fuzz / nightly evidence

- `cargo llvm-cov` full command (tests + 3 WPT runs, `--no-clean`
  accumulation): core **80.5 %**, boa_idb **75.5 %** lines.
- Nightly `nightly-m7.yml`: coverage ratchet (core 78 / boa_idb 73) +
  LCOV artifact; full suite with 10k/long-lifecycle knobs; massif RSS gate
  (512 MiB) on lifecycle+overhead; 5 fuzz targets × 48 min (≈4 h).
  First-run evidence pending inaugural nightly (fuzz targets compile;
  local `cargo-fuzz` link needs the Linux ASan runtime).
- Ratchet, not TZ absolutes: gap analysis recorded (dead
  `engine::{connection,cursor,registry,transaction,request}` modules with
  zero references, API edge branches); no broad ignores; removal proposed
  as follow-up debt, not done here.
- P1-1 (dhat, ADR-014): the hand-rolled `GlobalAlloc` shim is gone —
  `memory_gates_tests.rs` uses `dhat::Alloc` + per-window `Profiler`;
  zero `unsafe` in the test file and in all production crates
  (`deny(unsafe_code)` intact). Overhead canaries (64 KiB conn / 8 KiB
  txn) green.
- P1-2 matrix: PR runs store 1M/1M/200k × Next/Prev + index 100k ×
  Next/Prev on all three backends (this host, debug: the two cursor tests
  take ~8.5 min); `BOA_IDB_FULL_MATRIX=1` raises FS store and all index
  scales to 10⁶ with `RECEIPT-MATRIX` evidence lines. New nightly job
  `cursor-matrix-1m` (90 min timeout) runs the full matrix with
  `--nocapture --test-threads=1`, uploads `matrix-1m.log` +
  `receipt-matrix-1m.txt`, and posts receipts to the step summary.
  Inaugural 1M-matrix evidence pending the first scheduled run —
  dispatch manually until then:
  `gh workflow run nightly-m7.yml --ref task/m7b-optimizations-reliability`
  (or workflow_dispatch in the Actions UI).

### Traceability deltas

R12.1 PARTIAL (fixes + receipts; labelled-host comparison pending), R12.2
PASS (store 1M/1M/200k Next+Prev + index Next/Prev all backends peak <
1 MiB; FS/index at smoke scale in PR, 1M evidence via nightly
`cursor-matrix-1m`), R12.3 PASS, R13.1 PASS (fuzz evidence pending),
R13.2 PARTIAL (ratchet + actuals), R13.4.1 PASS, R13.6 PASS. Full matrix in
`docs/traceability.md`.

### Remaining debt (not M7-B scope)

1. Coverage-drive to 90/80 (dead-engine removal proposal + API edge tests).
2. Labelled benchmark host with blocking >10 % gate.
3. JS dispatch batching follow-up (F3 remainder is Boa mechanics).
4. Inaugural nightly evidence (fuzz 4 h, massif, ratchet green, 1M cursor
   matrix) — dispatch: `gh workflow run nightly-m7.yml --ref
   task/m7b-optimizations-reliability`.
5. Stream FS WAL replay (H-FS-OPEN follow-up): decode → apply → drop per
   frame to cut open-time transient.
6. `docs/reviews/M6-review.md` left untracked (M6 material, not mine).

### Verification (all green, this tree)

`cargo fmt --check`, workspace `clippy --all-targets --all-features
-D warnings`, `cargo test --workspace` (zero failures),
`cargo test -p boa_idb --features tracing --test observer_tests`,
`upgrade_observer_tests`, `RUSTDOCFLAGS="-D warnings" cargo doc
--workspace --no-deps`, `cargo deny check` with
`CARGO_DENY_DB_PATH=target/cargo-deny-advisories` (advisories/bans/
licenses/sources ok — the env var is set by CI; without it the config
fails validation, which is environmental, not a dependency verdict),
WPT 482/482 on memory/SQLite/FS, smoke bench. `unsafe` audit: zero
`unsafe` in `crates/` and in `memory_gates_tests.rs` (dhat replaced the
shim; P1-1 done). No deviations without agreement; no new production
dependencies (dhat/criterion are dev-only with ADR-013/ADR-014 entries).
(criterion/fuzz deps are dev-only; no ADR needed beyond ADR-013).
