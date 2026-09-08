# boa_idb_fs

Filesystem backend for IndexedDB (M6-A foundation): WAL + recovery, advisory
`LOCK`, atomic `CURRENT`/`MANIFEST`/`meta.scf` updates, and an in-memory
ordered key index rebuilt on open.

## Memory limit (R8.3.5)

The entire key set of one database must fit in memory. Rough estimate:

```text
bytes ≈ keys × (key_len + 48)
```

Default `max_keys_in_memory` is **5_000_000**. Exceeding it returns
`QuotaExceededError` / `BackendError::QuotaExceeded` without partially
committing the offending transaction.

For large databases prefer the **SQLite** backend (`boa_idb_sqlite`).

## Topology

```text
<root>/<storage_key_hash>/<db_name_hash>/
  LOCK
  CURRENT
  MANIFEST-<seq>
  meta.scf
  wal/<seq>.log
```

`seg/` and external `blob/` layouts are reserved for M6-B (compaction /
externalized values) and are not required for M6-A correctness.

## Durability

- `Durability::Relaxed` / `Default`: WAL append without mandatory sync.
- `Durability::Strict`: sync WAL before commit returns success.

## What M6-A does **not** claim

- Immutable segments / compaction (R8.3.3)
- O(1)/O(log n) MVCC readonly snapshots (R8.3.4) — readonly uses map clone
- Full multi-process kill matrix / 200 crash iterations (R8.3.6 / R8.5.1) —
  seams exist; complete matrix is M6-B
- WPT runner wiring for the FS backend

## Tests

```powershell
cargo test -p boa_idb_fs
```
