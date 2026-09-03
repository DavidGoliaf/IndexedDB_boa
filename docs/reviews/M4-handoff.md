# Handoff: M4 — SQLite backend (`boa_idb_sqlite`)

Branch: `task/fix-04-sqlite`, base: `master@4a0649b` (fix-03 merged).
Work order: `tasks/04_TASK_SQLITE_BACKEND.md`. This handoff covers the full
M4 delivery and the rework after the first acceptance review (all review
P0s resolved — see the table below).

## What was built

Production SQLite backend per §8.2 of the TZ, one crate, nine modules:

- **`naming.rs`** — path hashing per R8.1.4: names never reach the
  filesystem verbatim; `db_name_hash = base32(sha256(utf16le(name)))[..26]`
  → `db-<hash>.sqlite`, storage keys → `sk-<hash>`, blob files →
  `blobs/<db_hash>/<xx>/<sha256>.bin`.
- **`schema.rs`** — DDL exactly as §8.2.2 (`WITHOUT ROWID`, clustered
  composite PKs, FK cascade, `idx_index_records_pkey`), pragmas
  (`WAL`, `busy_timeout = 5000`, `cache_size = -8000`, …),
  `registry.sqlite` schema, `meta.schema_version` + `migrate_if_needed`
  (R8.2.4; future migrations hook into the chain).
- **`pool.rs`** — 1 writer + N readers (default 4) with an *owned checkout*
  model (AD-3/AD-6): a transaction owns its connection, drop returns it.
  Writer slot fails fast with `BackendError::Locked` (never blocks the
  single-threaded pump). Every connection carries a 64-entry LRU prepared
  statement cache (ADR-005) and the standard pragmas.
- **`txn.rs`** — `SqliteTxn` (`BackendTxn`): `BEGIN IMMEDIATE`/`DEFERRED`
  per §8.2.3, `SAVEPOINT r<seq>` per request, `INSERT .. ON CONFLICT`
  upserts, unique-index enforcement via existence check (single writer),
  key_gen persistence, stale-entry sync on overwrite, and full blob
  lifecycle tracking (below).
- **`cursor.rs`** — `SqliteCursor` for all 4 directions over stores and
  indexes. Fetches in pages (`LIMIT ?/OFFSET ?`, 128 rows/page) from the
  clustered PK index — no full materialization, OOM-safe on large stores.
  `NextUnique`/`PrevUnique` collapse groups in SQL (`GROUP BY ir.key` +
  `MIN(ir.pkey)`, §5.4/§8.2.3). Seeks are direction-aware (Prev seek
  `k` lands at the greatest key ≤ `k`, tuple seeks flip both comparators).
  Externalized values are read lazily — only the row under the cursor is
  materialized, and the previous row's bytes are dropped (≤1 blob in
  memory regardless of store size). Range predicates are compiled once
  (open/closed fixed at build time); the same compiler serves `count`
  (R8.2.3).
- **`blob.rs`** — values > 256 KiB externalized: temp file → `fsync` →
  atomic rename *before* the SQLite commit (§8.2.5); integrity check =
  SHA-256 of content must equal the file name (ADR-007).
- **`database.rs` / `storage.rs` / `factory.rs`** — `Database` (metadata
  snapshot semantics, `flush` = WAL truncate-checkpoint), `Storage` with
  `registry.sqlite` (races handled idempotently), `delete_database`
  removes db + WAL/SHM + blob dir (R11.6), `usage_bytes`.

### Blob lifecycle (review P0: leaks)

Every path that can orphan a created file is covered:

- `put` overwrite, `delete_record`, `delete_range`, `clear`, and
  `delete_store` (pre-CASCADE ext collection) record the old external
  reference as *orphaned*;
- files created in a request are tracked per savepoint level;
- `rollback_request` deletes files created in the rolled-back scope,
  `abort`/`Drop` delete everything the txn created — all via a
  *reference check* against `records.ext`, so a rolled-back or re-inserted
  reference keeps its file (leak-safe, never data-loss);
- `commit` collects the final orphan set after `COMMIT`.

## Review findings → resolution

| Review finding | Resolution |
|---|---|
| No `lru` from spec; `crc32fast` declared, unused | rusqlite per-connection LRU cache, capacity 64, all hot statements via `prepare_cached` (ADR-005); `crc32fast` removed from manifest |
| Cursor materializes whole range + eager blobs, no LIMIT/OFFSET | Paged fetch (`LIMIT/OFFSET`, 128/page), buffer pruning; blobs resolved lazily, ≤1 cached row value at a time |
| `*Unique` = Rust postfilter | `GROUP BY ir.key` + `MIN(ir.pkey)` in SQL (§5.4) |
| `seek` ignores Direction (Prev seek(b) returned c) | Direction-aware predicates incl. tuple seeks; regression tests `test_store_cursor_prev_seek_is_direction_aware`, `test_index_cursor_seek_key_and_pkey_both_directions`, `test_index_cursor_unique_seek` |
| Blob leaks: rename-immediately with no cleanup on rollback/abort/Drop/put/delete/clear/delete_store; `blob::remove` dead code | Savepoint-level created/orphan tracking, reference-checked GC at rollback/abort/Drop/commit; `delete_store` collects exts before cascade; `blob::remove` live; suite `sqlite_blob_gc_tests.rs` (7 tests) |
| "CRC32" in docs was a lie (SHA of name checked) | Docs corrected; scheme formalized as SHA-256 content addressing (ADR-007) |
| `rename_store`/`rename_index` didn't map UNIQUE to `Constraint` | `ConstraintViolation` → `BackendError::Constraint` in both |
| EXPLAIN tested handwritten SQL | `debug_sql_with_literals` / `debug_count_sql_with_literals` share the real builders (`build_query`/`build_count_sql`); 5 EXPLAIN tests over store/index/unique/seek/count shapes |
| Concurrency test never crossed writer and readers | `test_concurrent_readers_during_write` holds an uncommitted writer txn while 10 reader threads assert snapshot isolation |
| 16× `allow(dead_code, …)` masking "0 warnings" | No `dead_code`/quality allows remain; lib.rs keeps 12 scoped pedantic-style allows (same policy as `boa_idb_core`), 2 dropped by fixing the code (`collapsible_if` → `and_then`, `manual_is_multiple_of` → `is_multiple_of`) |
| Everything uncommitted, no handoff/ADRs | Committed on `task/fix-04-sqlite`; this handoff; ADR-005…007; task file restored |
| Workspace `fmt --all --check` FAIL | Reformatted whole workspace; clean |
| llvm-cov ≥85% never run | Run: lines **89.63%**, regions **85.68%** (`boa_idb_sqlite`) |

Related L1 fix in `boa_idb/src/driver.rs`: a `BackendError::Locked` when
starting a backend txn is now retried on a later pump turn instead of
failing the transaction's request queue — required by the fail-fast writer
slot model.

## Verification (all SUCCESS, task §8)

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --no-fail-fast        # all suites ok
cargo test --package boa_idb_sqlite          # 101 tests: 24 lib + 48 backend
                                             # + 7 blob-gc + 9 overflow + 2 concurrency
                                             # + 5 explain + 6 migration
cargo test --package boa_idb_sqlite --test sqlite_explain_plan_tests
cargo doc --package boa_idb_sqlite --no-deps
.cargo-tools/bin/cargo-llvm-cov.exe llvm-cov --package boa_idb_sqlite --summary-only
```

Coverage (llvm-cov, `boa_idb_sqlite`): lines 2217/230 missed = **89.63%**,
regions **85.68%** — above the 85% gate. No unsafe, no `unwrap`/`expect`
outside tests.

## Deviations (all ADR-tracked)

- **ADR-005** — no `lru` crate: rusqlite's built-in per-connection LRU
  statement cache (capacity 64) implements R8.2.1.
- **ADR-006** — no `hex` crate: `data-encoding` Base32 + local
  `hex_encode`.
- **ADR-007** — SHA-256 content addressing instead of "SCF + CRC" blob
  container (§8.2.5); self-verifying files, no sidecar checksum,
  `crc32fast` dropped.

## Known limitations / follow-ups

- Crash-orphan sweep at DB open ("GC при открытии базы", §8.2.5) is
  deferred: crash windows leave at most a few content-addressed files that
  the next overwrite/delete cycle reclaims; a safe sweep needs the
  pending-delete journal called out in the TZ and is scheduled with M5
  hardening.
- `Durability::Strict` does not yet flip `synchronous=FULL` +
  `wal_checkpoint(TRUNCATE)` per commit (§8.2.3 row "commit") — plumbing
  the durability flag into `SqliteTxn` is M5 work.
- Background GC "после каждых N коммитов" same as the sweep above.
