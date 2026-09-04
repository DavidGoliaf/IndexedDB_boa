# Open questions / work-order gates

## TASK-08 / M6-B — scope gate (mandatory before code)

**Branch:** `task/m6-fs-completion` (from `94b4bff`)  
**Work order:** `tasks/08_TASK_M6B_FILESYSTEM_COMPLETION.md`  
**Date:** 2026-09-04  
**Current `boa_idb_fs` size:** ~3760 lines (already above a single soft work-order budget)

### Diff estimate by package

| Package | Scope | Est. new/changed LOC (src+tests+docs) | Notes |
|---|---|---|---|
| **P1 — Segments + compaction** | `seg/*.seg` codec, versioned manifest, CURRENT publication order, configurable WAL thresholds (64 MiB / 10k frames), interruptible compaction, generation lifecycle | **1100–1600** | Touches open/commit/recovery paths; must preserve M6-A WAL semantics |
| **P2 — MVCC readonly snapshots** | Replace O(n) `BTreeMap` clone; ADR for `im` (or equivalent); generation/refcount so compaction cannot delete live snapshots; instrumented O(1)/O(log n) proof seam | **700–1100** | New permissive dependency requires ADR before use |
| **P3 — `FileSystem` + fault matrix** | Production IO behind `FileSystem` trait; `FaultInjectingFs`; table-driven ENOSPC/EIO/short-write/sync/rename faults on WAL/segment/manifest/cleanup | **900–1400** | Large test surface; rewrite of atomic/sync paths |
| **P4 — Crash worker + WPT FS** | Worker binary + forced kill (Unix/Windows), compact CI subset + 200-iter nightly docs; `--backend fs` in `boa_idb_wpt`; differential FS parity; ≥92 % WPT | **800–1300** | CI/platform `cfg` risk; WPT expectations must not be mass-edited |

**Sum (mid):** ~**4500** lines · **Range:** ~3500–5400  

### Gate decision

Суммарный прогноз **выше ~3000 строк** и смешивает независимые инварианты
(storage format / MVCC ownership / fault injection / process crash+WPT).
По §1 work order: **код полного M6-B не начинать одной поставкой.**

### Статус

- **2026-09-04:** заказчик принял разрез B1→B2→B3; M6-B1 сдан на
  `task/m6-fs-completion`.
- **2026-09-04:** **M6-B2** (`FileSystem` + fault matrix) выполнен; далее
  M6-B3 (crash worker + WPT `--backend fs`).

### Предлагаемые поднаряды

1. **`M6-B1` — storage format + MVCC**  
   Segments, manifest, CURRENT publication, compaction thresholds, persistent
   snapshot map + refcount/generation, instrumented readonly-start proof,
   ADR(s). Acceptance: R8.3.3 + R8.3.4 → PASS (or honest PARTIAL only if a
   documented platform limit remains). Est. **1800–2500** LOC.

2. **`M6-B2` — `FileSystem` + fault/reclamation matrix**  
   Route all FS IO through `FileSystem`; fault injection; table-driven faults
   at every publication stage; segment retention until snapshot drop. Est.
   **900–1400** LOC.

3. **`M6-B3` — crash worker + WPT/differential**  
   Kill-worker committed-prefix suite (CI subset + nightly 200), `--backend fs`,
   full WPT ≥92 %, differential parity, `M6B-handoff.md` + traceability. Est.
   **800–1300** LOC.

Каждый поднаряд: своя ветка/handoff, quality gates, без ложного PASS по ещё
не покрытым R8.* .

### Вопрос заказчику

Подтвердите старт с **`M6-B1`** (segments + MVCC), либо укажите другой порядок /
другой разрез пакетов. До ответа реализация M6-B **не начинается**.
