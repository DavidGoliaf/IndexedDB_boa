# Handoff: M7 — performance, reliability, final acceptance (EXTERNAL BLOCKER)

> **Status 2026-09-07 (M7-C):** `EXTERNAL BLOCKER` — code scope M7-A/M7-B
> closed and locally green on this tree. All CI-backed acceptance items
> (§7 work order) remain without run URLs/artifacts: no labelled runner,
> no labelled baseline, no inaugural nightly runs. No `PASS` is claimed
> for them. Owner: CI maintainer; recheck: after runner provisioning +
> first scheduled runs (see §6).

Work order: `tasks/10_TASK_M7C_CI_EVIDENCE_AND_FINAL_ACCEPTANCE.md`
(`TASK-10-M7C-CI-EVIDENCE-FINAL-ACCEPTANCE`)
Branch: `task/m7c-ci-evidence-final` (from `task/m7b-optimizations-reliability`
tip `40f6681`)
Predecessors: M7-A handoff `docs/reviews/M7A-handoff.md`, M7-B handoff
`docs/reviews/M7B-handoff.md` (rework №3 code closure `3d29f8b` + WAL-sidecar
test fix `f1912a6`)
Scope: M7-C opens no optimization and changes no IndexedDB semantics,
formats, or WPT expectations. Docs-only delivery: this file + pointer in
M7B-handoff. No production code touched.

## 1. What M7 delivered (A → B → C)

| Half | Content | Handoff |
|---|---|---|
| M7-A | Criterion harness (7 §12.1 scenarios × SQLite/FS + memory control), baseline workflow, `tracing` feature, `IdbObserver`, dev-only `boa-idb-cli`, traceability skeleton | `docs/reviews/M7A-handoff.md` |
| M7-B | Keyset SQLite cursor (F2), FS quota/packing fixes (F1), SQLite blob-sweep fast path (F4), F3/F5 dispositions, lazy cursors memory+FS, dhat gates, 1M matrix, absolute coverage gate, 5 fuzz targets, `bench_compare.py` fail-closed comparator + host-id binding, `bench-regression.yml` PR gate, nightly workflows | `docs/reviews/M7B-handoff.md` |
| M7-C | CI evidence collection + final acceptance (this file). No code changes required or made beyond docs | — |

## 2. Commits (this branch)

Base `40f6681 Clarify M7-C labelled baseline identity` (tip of
`task/m7b-optimizations-reliability`, includes `f1912a6` WAL-sidecar test
fix). M7-C adds:

- `M7-handoff.md` (this file, mandatory deliverable §8 of the M7 work order)
- pointer in `docs/reviews/M7B-handoff.md` → this file
- `docs/traceability.md`: audited, no drift found (PARTIALs stay PARTIAL,
  see §7); no status change needed

No workflow/comparator code change: §1 work order allows it only when a
real CI run demands it. No such run exists, so none was made.

## 3. Platform receipts (local, diagnostic — not release evidence)

Dev host (same as M7-A/M7-B receipts): Windows 11 / Intel i7-14700K /
x86_64, rustc 1.91.0 (`f8297e351 2025-10-28`), cargo 1.91.0.

| Artifact | Content |
|---|---|
| `crates/boa_idb/benches/receipts/RECEIPT-M7B-2026-09-05-win11-i7-14700K.md` | after-fix means: put 168k/555k, get 1.78M/7.80M, 1M scan 2.21M/1.59M rec/s, empty txn 60k/86k, JS↔core 14.7 µs derived, open 24 µs/1.15 s, strict 648/236 commits/s |
| `crates/boa_idb/benches/receipts/` raw logs | `raw-scan-1m-sqlite-after-keyset-20260905.log`, `raw-scan-1m-fs-after-f1-20260905.log`, `raw-bench-m7b-full-20260905.log` |
| `docs/reviews/RECEIPT-MATRIX-1M-20260906.md` | full 1M cursor matrix 12/12 (store/index × Next/Prev × memory/SQLite/FS), peaks memory ~1 KiB, FS ~1 KiB, SQLite ~237 KiB |
| `docs/reviews/coverage-20260907/` | `cov.json` + `lcov.info` + `package-totals.txt`: core 90.1 % (2329/2585), boa_idb 80.9 % (7232/8938), gate exit 0; `deny-check.log` (advisories/bans/licenses/sources ok, advisory DB `5a0ebed` 2026-09-02, cached) |

## 4. Labelled baseline and blocking benchmark evidence (§3)

**Not available — EXTERNAL BLOCKER.**

- `crates/boa_idb/benches/baselines/` contains only `m7b-label-ref.json`
  (`host_role: interim`, Windows dev box — diagnostic, must never gate
  releases) and `README.md` (capture procedure). `m7b-label-baseline.json`
  does not exist in the repo.
- Comparator (`scripts/bench_compare.py`) is fail-closed and unit-tested
  (15/15, incl. 4 host-id cases: matching PASS; missing-env /
  missing-baseline / mismatch rejected before any bench runs).
  `bench-regression.yml` has the `pull_request` trigger (branches: `main`),
  runs only on `[self-hosted, bench]`, forwards protected
  `vars.BOA_IDB_BENCH_HOST_ID`. Without a provisioned runner the job
  queues and no PR is actually gated.
- Negative proof without touching production code: comparator unit test
  `test_10_01_percent_regression_fails` (10.01 % synthetic regression →
  non-zero). Command/result: `python scripts/test_bench_compare.py` →
  15/15 OK (this tree, 2026-09-07). No baseline/bench source was faked
  for a red run, per §3.4.
- Required-check setup (`bench-regression` in required checks + URL/
  screenshot of the setting + PR run URL): pending runner provisioning.
  Owner: CI maintainer.

Replay for the owner (on the provisioned runner, clean tree):

```sh
BOA_IDB_BASELINE_ROLE=labelled \
python3 scripts/bench_compare.py write \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
python3 scripts/bench_compare.py compare \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
```

Commit the new file separately (never rename/hand-edit `m7b-label-ref.json`),
then add `bench-regression` to required checks. `BOA_IDB_BENCH_HOST_ID`
comes from the runner's protected configuration, never from workflow YAML.

## 5. Inaugural nightly evidence (§4)

**Partially available.** `nightly-fs-crash` inaugural run is green
(see table: run `34050249898`); `nightly-m7.yml` inaugural run still
pending. No `gh` CLI on the dev host; dispatch was done by the owner.
Workflows are wired and reviewed:

| Job (`nightly-m7.yml` / `nightly-fs-crash.yml`) | Required evidence | Status |
|---|---|---|
| `coverage` | lcov.info, cov.json, core ≥90 % / boa_idb ≥80 %, tests+bins+examples + WPT ×3 | BLOCKED (local equivalent: `docs/reviews/coverage-20260907/`, 90.1/80.9) |
| `differential-and-lifecycle` | 10 000 seeded scenarios + `BOA_IDB_LONG_LIFECYCLE=1` success log | BLOCKED (suites green locally via `cargo test --workspace`) |
| `cursor-matrix-1m` | `matrix-1m.log`, `receipt-matrix-1m.txt`, 12 combos at 1M | BLOCKED (local inaugural: `RECEIPT-MATRIX-1M-20260906.md`, 12/12) |
| `memory-massif` | `massif.out`, Valgrind version, peak + gate result | BLOCKED (needs Linux/valgrind) |
| `fuzz` (5 targets × 48 min, ≥4 h total) | final stats, corpus/cache identity, crash artifacts + replay | BLOCKED (targets compile: `cargo check --manifest-path fuzz/Cargo.toml` exit 0; ASan link needs Linux) |
| `nightly-fs-crash` (`BOA_IDB_FS_CRASH_ITERS=200`) | run URL/ID, seed/replay, green M6 fault matrix | **PASS (CI)** — run `34050249898` 2026-09-06, `workflow_dispatch` on `task/m7c-ci-evidence-final` @ `5f21831`: `crash-consistency (ubuntu-latest)` success (43 s step) + `crash-consistency (windows-latest)` success; seed `0xC0FFEE`, command `BOA_IDB_FS_CRASH_ITERS=200 BOA_IDB_FS_CRASH_SEED=0xC0FFEE cargo test -p boa_idb_fs --test m6b3_crash_tests -- --nocapture` per `.github/workflows/nightly-fs-crash.yml`. Run: https://github.com/DavidGoliaf/IndexedDB_boa/actions/runs/34050249898 |

Dispatch (owner with Actions access):

```sh
gh workflow run nightly-m7.yml --ref task/m7c-ci-evidence-final
gh workflow run nightly-fs-crash.yml --ref task/m7c-ci-evidence-final
gh workflow run bench-regression.yml --ref task/m7c-ci-evidence-final  # after §4 baseline
```

File the top-level run URL/ID, per-job URLs/IDs, exact commands, and
artifact names here before flipping any status to PASS. On failure:
seed + replay command + corpus/artifact. No workflow scope was weakened
to fit hosted CI.

## 6. Local quality suite (§6, this tree, 2026-09-07)

HEAD `40f6681`, branch `task/m7c-ci-evidence-final`, clean except
untracked `docs/reviews/M6-review.md` (M6 material, out of M7 scope).

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo test --workspace` | exit 0, zero failures |
| `cargo test -p boa_idb --features tracing --test observer_tests` | 4/4 PASS |
| `cargo check --manifest-path fuzz/Cargo.toml` | exit 0 (5 targets) |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | exit 0 |
| `cargo deny check` (`CARGO_DENY_DB_PATH=target/cargo-deny-advisories`) | advisories/bans/licenses/sources ok (cached DB; fresh-fetch note in `coverage-20260907/README.md`) |
| WPT `--backend memory/sqlite/fs --summary` | 482/482 ×3 PASS |
| `python scripts/test_bench_compare.py` | 15/15 OK |
| `git diff --check` | exit 0 |

## 7. Traceability sync (§5.3)

`docs/traceability.md` audited against this handoff and the M7-B evidence
table. No drift: every PASS cites a real local receipt/artifact; every
CI-dependent item stays PARTIAL.

| R | Status | Ground |
|---|---|---|
| R12.1 | PARTIAL | fixes + receipts + fail-closed comparator shipped; blocking enforcement needs labelled runner + baseline (§4 blocker) |
| R12.2 | PASS on local evidence | 1M matrix 12/12 with local receipts `RECEIPT-MATRIX-1M-20260906.md`; nightly run URL pending (not claimed) |
| R12.3 | PASS | tracing spans + observer + CLI covered by tests, CI tracing job present |
| R13.1 | PARTIAL | all levels wired, locally green; 4 h Linux fuzz evidence pending |
| R13.1-fuzz | PARTIAL | 5 targets compile; first 4 h evidence pending inaugural nightly run |
| R13.2 | PARTIAL | local 90.1/80.9 + enforcing absolute gate; CI coverage artifact pending |
| R13.4.1 | PASS | 10k differential suites green locally; nightly re-runs with long-lifecycle env |
| R13.6 | PASS | lifecycle + massif wiring + local lifecycle gates green |

## 8. Target misses, profiles, follow-ups (§5.4)

Per M7-B receipts: F1 (FS put 958 → 554 740 ops/s), F2 (SQLite 1M scan
847/s → 2 213 872/s), F4 (SQLite 1M open 310 ms → 24 µs) fixed with
same-machine before/after receipts + deterministic mechanism + DIAG-timed
proof (no sampling profiler on the dev host — samply needs Windows
Administrator; recorded as environmental limitation, not evidence).
F3 PASS on derived true cost 14.7 µs (thin ~2 % margin, load variance
documented); F5 PASS at the fsync floor (212–236 commits/s across runs,
nightly canary watches). No target weakened, no benchmark deleted.
Follow-ups (not blockers): stream FS WAL replay (open-time transient
~3.5× → ~1×); `dispatchEvent` `on*` handlers via `dispatchEvent` out of
scope; dead-`engine`-module removal proposal needs its own review.

## 9. Acceptance checklist (§7)

- [ ] Labelled runner + protected host ID provisioned; labelled baseline
  created on it and committed — **BLOCKED** (§4)
- [ ] `bench-regression` is an actual required PR check with a passing run
  on the baseline; comparator fail-closed cases tested — **code done
  (15/15), enforcement BLOCKED** (§4)
- [ ] Successful nightly evidence: coverage, 1M matrix, massif,
  differential/lifecycle, ≥4 h fuzz — **BLOCKED** (§5)
- [ ] Successful `nightly-fs-crash` evidence at 200 iterations — **BLOCKED** (§5)
- [x] Targets/misses have receipts/profiles/follow-ups without requirement
  substitution — **done** (§8)
- [x] `M7-handoff.md`, M7B handoff pointer, traceability in sync; every
  PASS has URL/artifact/command/SHA — **done** (§§5, 7)
- [x] Full local quality suite + memory/SQLite/FS WPT green — **done** (§6)

M7-C (and therefore M7) stays `EXTERNAL BLOCKER`; the next milestone
must not be marked as depending on an accepted M7.

## 10. Known limitations

1. All benchmark numbers are diagnostic (heterogeneous Windows dev box),
   never release evidence. Authoritative comparison lives on the pinned
   host only.
2. `cargo deny` freshness = cached advisory DB `5a0ebed` (2026-09-02);
   offline fetch is a no-op here. CI fetches fresh.
3. Fuzz `cargo-fuzz` link needs the Linux ASan runtime; Windows proves
   compile only.
4. Massif and the 90-min full 1M matrix need Linux CI (local matrix ran
   on Windows release instead).
5. Untracked `docs/reviews/M6-review.md` is M6 material, intentionally
   left out of M7-C scope.
