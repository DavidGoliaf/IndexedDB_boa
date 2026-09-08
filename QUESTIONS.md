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

- **2026-09-04:** заказчик принял разрез B1→B2→B3; M6-B1 и M6-B2 сданы на
  `task/m6-fs-completion`.
- **2026-09-04:** **M6-B3** (crash worker + WPT `--backend fs` + differential)
  выполнен; сводный handoff `docs/reviews/M6B-handoff.md`.

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

---

## TASK-09 / M7 — scope gate (mandatory before code)

**Work order:** `tasks/09_TASK_M7_PERFORMANCE_RELIABILITY.md`
**Date:** 2026-09-05
**Base (planned):** новый `task/m7-performance-reliability` от tip M6-B
(`1cc9aa9`); незакоммиченный 1-строчный doc-твик `crates/boa_idb_fs/src/vfs.rs`
и untracked `docs/reviews/M6-review.md` в новый наряд не тянуть без отдельного
решения.
**Checked-in state:** `benches/` отсутствует, `criterion` нигде не задекларирован,
`[profile.release] lto` нет; спанов `idb.open/txn/request`, `IdbObserver`,
`boa-idb-cli`, `llvm-cov`-гейта нет; fuzz-целей 3 из 4 (нет WAL recovery /
stateful txn-segment); traceability без строк R12.\*, R13.1, R13.2, R13.4.1,
R13.6. Есть: crash worker + nightly 200 (`BOA_IDB_FS_CRASH_ITERS`), model
differential 10 000 proptest-кейсов, FS differential (меньший масштаб).

### Diff estimate by package (src+tests+docs+CI)

| Package | Scope | Est. new/changed LOC | Notes |
|---|---|---|---|
| **A — Benchmark harness + baseline (§3)** | `criterion` + release `lto="thin"`; 7 сценариев §12.1 × SQLite/FS (+ memory как контроль); изолированный temp-root, фиксированный seed; receipts JSON/Markdown с provenance; baseline artifact + CI (smoke везде, authoritative comparison на labelled host, блок регресса >10 %) | **1000–1500** | Нулевое изменение поведения; только инфраструктура + доки |
| **B — Направленные оптимизации (§4)** | Hypothesis-first дисциплина; cursor без материализации (1M records, счётчик allocation, O(1) памяти), `continue()` 100k без квадрата, сохранение O(1)/O(log n) snapshot, open/recovery без перечитывания superseded данных, compaction без просадки throughput; фиксы только по профилю | **800–1200** | Объём фиксов зависит от baseline; поведение менять нельзя |
| **C — Memory + lifecycle reliability (§5)** | Linux gate `dhat`/`valgrind` + долгий lifecycle test; пробы overhead ≤64 KiB (connection) / ≤8 KiB (txn), Linux authoritative + документированный platform limitation; доки `getAll`/cursor; nightly: расширенный differential 10 000 seeded + 200 crash + memory gates, seed/replay | **800–1200** | Часть задела есть (crash 200, model 10k); добить FS-масштаб и memory gates |
| **D — Observability + quality gates (§6+§7)** | Feature `tracing` (default-off, zero-cost), спаны `idb.open/txn/request` + privacy-тесты; `IdbObserver` (counters, bytes, bounded histograms); dev-only `boa-idb-cli` (`ls/dump/verify/compact/stats`, `--values`); `cargo-llvm-cov` gates (core ≥90 %, `boa_idb` ≥80 %); 4-я fuzz-цель (WAL recovery + stateful) + nightly 4h; traceability R12.\*/R13.1/R13.2/R13.4.1/R13.6 | **1200–1700** | CLI и observer — крупнейшие куски; без нового public API без ADR |

**Sum (mid):** ~**4700** lines · **Range:** ~**3800–5600**

### Gate decision

Прогноз **выше ~3000 строк** и смешивает независимые инварианты
(измерительная инфраструктура / оптимизация горячих путей / память и lifecycle /
наблюдаемость и coverage-gates). По §2 work order: **код M7 одним нарядом не
начинать.**

### Предлагаемые поднаряды

1. **`M7-A` — harness, baselines, observability (без изменения поведения)**
   Пакеты A + D-наблюдаемость (tracing, observer, CLI) + traceability-скелет
   R12/R13. Acceptance: criterion suite по 7 сценариям §12.1 для SQLite/FS
   формирует receipts; tracing/observer/CLI покрыты tests; ни одного изменения
   семантики/форматов. Est. **2000–2800** LOC.
2. **`M7-B` — оптимизации, memory/reliability gates**
   Пакеты B + C + coverage/fuzz-gates (llvm-cov thresholds, 4 fuzz-цели 4h,
   nightly 10k differential + 200 crash + блок регресса >10 %). Acceptance:
   miss target → profile + обоснование + follow-up; cursor/memory тесты;
   traceability R12.\*/R13.x → PASS только по реальным gates. Est. **2000–3000**
   LOC.

Каждый поднаряд: своя ветка/handoff, quality gates, без ослабления targets и
без выдачи локальных цифр за эталон.

### Вопрос заказчику

Подтвердите разрез **`M7-A` → `M7-B`** и старт с **`M7-A`**, либо укажите другой
порядок / разрез. До ответа реализация M7 **не начинается**. Отдельно
подтвердите создание ветки `task/m7-performance-reliability` от `1cc9aa9`
(грязный doc-твик `vfs.rs` остаётся на `task/m6-fs-completion`).

### Статус

- **2026-09-05:** заказчик подтвердил **`M7-A` first** и создание ветки
  `task/m7-performance-reliability` (doc-твик `vfs.rs` убран в stash
  `m6: vfs.rs doc wording tweak` на `task/m6-fs-completion`). M7-A сдан на
  новой ветке: handoff `docs/reviews/M7A-handoff.md`.

## TASK-09 / M7-B — `unsafe` note (исполнитель уведомляет по §2 AGENTS.md)

Портативные allocation gates (`crates/boa_idb/tests/memory_gates_tests.rs`)
требуют считающего глобального аллокатора — 15 строк textbook `GlobalAlloc`
шимма, форвардящего всё в `System` и считающего байты, БЕЗ разыменования
памяти. Только test-target (`#![allow(unsafe_code)]` scoped на файл);
production crates остаются под `deny(unsafe_code)` (проверено: grep по
`unsafe` в `crates/*/src` пуст — см. handoff M7-B). Альтернативы
(`tikv-jemalloc` — новая зависимость + ADR; `mallinfo` — только Linux)
хуже для переносимого гейта. При несогласии — заменить massif-only
гейтом и удалить файл; явного стопа не требовалось, т.к. риск
ограничен тестовым таргетом.
