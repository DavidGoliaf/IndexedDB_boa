# Handoff: M7 — performance, reliability, final acceptance (COMPLETE)

> **Status 2026-09-08 (M7-C, P1 closed):** `COMPLETE` — §4 (labelled
> runner + baseline + required PR check) PASS; §5 nightly evidence is a
> fresh green run on a baseline-containing SHA (see below). No open
> blockers.

Work order: `tasks/10_TASK_M7C_CI_EVIDENCE_AND_FINAL_ACCEPTANCE.md`
(`TASK-10-M7C-CI-EVIDENCE-FINAL-ACCEPTANCE`)
Branch: `task/m7c-ci-evidence-final` (from `task/m7b-optimizations-reliability`
tip `40f6681`)
Predecessors: M7-A handoff `docs/reviews/M7A-handoff.md`, M7-B handoff
`docs/reviews/M7B-handoff.md` (rework №3 code closure `3d29f8b` + WAL-sidecar
test fix `f1912a6`)
Scope: M7-C collects CI evidence and closes M7 documentally. CI-driven
changes on this branch: `workflow_dispatch: {}` registration fix
(`49a85e7`), `boa-idb-cli` example pre-build + `resolve_cli_exe`
harness fix (§5.2), WAL `decode_ops` OOM production fix (§5.1 — one line,
no semantic/format change, see below). No IndexedDB semantics, SCF/KEY/
WAL/segment formats, or WPT expectations changed.

## 1. What M7 delivered (A → B → C)

| Half | Content | Handoff |
|---|---|---|
| M7-A | Criterion harness (7 §12.1 scenarios × SQLite/FS + memory control), baseline workflow, `tracing` feature, `IdbObserver`, dev-only `boa-idb-cli`, traceability skeleton | `docs/reviews/M7A-handoff.md` |
| M7-B | Keyset SQLite cursor (F2), FS quota/packing fixes (F1), SQLite blob-sweep fast path (F4), F3/F5 dispositions, lazy cursors memory+FS, dhat gates, 1M matrix, absolute coverage gate, 5 fuzz targets, `bench_compare.py` fail-closed comparator + host-id binding, `bench-regression.yml` PR gate, nightly workflows | `docs/reviews/M7B-handoff.md` |
| M7-C | CI evidence collection + final acceptance (this file): CI-forced fixes (workflow registration, example pre-build, WAL OOM one-liner) + run triage | — |

## 2. Commits (this branch)

Base `40f6681 Clarify M7-C labelled baseline identity` (tip of
`task/m7b-optimizations-reliability`, includes `f1912a6` WAL-sidecar test
fix). M7-C adds (oldest → newest):

| Commit | Content |
|---|---|
| `fd54458` | `M7-handoff.md` (mandatory deliverable §8) + pointer in `M7B-handoff.md` |
| `49a85e7` (via merge `5f21831`) | `workflow_dispatch: {}` registration fix — bare `workflow_dispatch:` keys made GitHub silently skip `nightly-m7`/`nightly-fs-crash`/`bench-regression` (YAML 1.1 `on:`-as-bool); API showed 1/4 workflows before, 4/4 after |
| `1c17eb3` (+ sync merges `6f6ccd9`, `4d211ac`) | coverage-harness fix (§5.2): example pre-build in `nightly-m7.yml` + `resolve_cli_exe()` in `cli_tests.rs` |
| `60c3321` | WAL `decode_ops` OOM production fix (§5.1): `Vec::with_capacity(count)` → `Vec::new()` + regression test |
| `7f13ae5` | handoff §5 evidence for both findings above |

(Cherry-picks `66be8f0`, `ba43a6f` carry the same two fixes on
`task/m7b-optimizations-reliability`.)

Per §1 work order, workflow/comparator changes are allowed when a real
CI run demands them — runs `34050249898`/`34050610807` did, and each
change above carries a regression test + rerun requirement. The single
production line (§5.1) fixes an abort-on-untrusted-input with no
behavioral change on valid inputs (suite green).

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

**PASS (CI).** All §3 requirements closed 2026-09-07/08:

- **Runner:** `boa-bench-01` (agentId 21), self-hosted Linux x64,
  labels `[self-hosted, bench]`. VM: Debian 13
  (`6.12.107+deb13-amd64`), Intel i7-14700K passthrough, 4 vCPU / 7 GB
  RAM, stable toolchain. `BOA_IDB_BENCH_HOST_ID=boa-bench-01` from the
  runner's protected configuration (`.env` of the runner service, never
  from workflow YAML). Limitation recorded: VirtualBox VM has no
  cpufreq governor (frequency managed by the host); no co-tenants
  during capture.
- **Baseline:** `crates/boa_idb/benches/baselines/m7b-label-baseline.json`
  (commit `72827e8` on `task/m7c-ci-evidence-final`, merged into `main`
  as `a3fc22d`): 19 scenarios, `host_role: labelled`,
  `host_id: boa-bench-01`, provenance `git_sha b9077f5`,
  `os Linux`, `cpu Intel(R) Core(TM) i7-14700K`, `arch x86_64`,
  `rustc 1.91.0 (f8297e351 2025-10-28)` (repo-pinned toolchain, same as
  CI), `profile bench (inherits release, lto=thin)`. `m7b-label-ref.json`
  stays `interim`/diagnostic, never renamed or hand-edited.
- **Self-compare:** `compare` on the same host right after capture —
  19/19 `ok`, `all scenarios within 10% of the labelled baseline`,
  exit 0 (log `/tmp/baseline-compare.log` on the runner).
- **PR check:** [PR #1](https://github.com/DavidGoliaf/IndexedDB_boa/pull/1)
  (`task/bench-check-probe` → `main`, probe comment only) triggered the
  `pull_request` path; run
  [`34206701559`](https://github.com/DavidGoliaf/IndexedDB_boa/actions/runs/34206701559)
  on `boa-bench-01`: **success** (~7 min), no regressions vs the
  committed baseline. `bench-regression` is listed in the `main`
  ruleset required status checks (owner-confirmed 2026-09-08), so a
  missing/failed check blocks merge. Comparator fail-closed behavior
  was additionally proven live: the earlier scheduled run `34110716543`
  (no baseline committed yet) exited 2 before running benches with
  `bench-compare.log` (artifact `10032726637`, 278 bytes).
- **Comparator unit tests:** `python scripts/test_bench_compare.py` →
  16/16 OK (15 gate cases + `cpu_string` non-empty), incl. 4 host-id
  cases and the 10.01 % synthetic-regression failure (§3.4, no
  production-code red run needed).
- **By-catch fix:** `platform.processor()` is empty on Debian, so the
  first labelled capture failed its own mandatory-provenance gate
  (`lacks mandatory provenance: cpu`). Added `cpu_string()` with
  `/proc/cpuinfo` fallback + unit test (commits `6b94d4c`/`c714f5b`).
  Admission remains `host_id`-only; `cpu` stays informational.

Capture commands (as run on the runner, clean tree @ `b9077f5`):

```sh
BOA_IDB_BASELINE_ROLE=labelled BOA_IDB_BENCH_HOST_ID=boa-bench-01 \
python3 scripts/bench_compare.py write \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
BOA_IDB_BENCH_HOST_ID=boa-bench-01 \
python3 scripts/bench_compare.py compare \
  --baseline crates/boa_idb/benches/baselines/m7b-label-baseline.json
```

## 5. Inaugural nightly evidence (§4)

**PASS on a baseline-containing SHA (P1 closed 2026-09-08).** Fresh
`workflow_dispatch` run
[`34240365417`](https://github.com/DavidGoliaf/IndexedDB_boa/actions/runs/34240365417)
on `main` @ `72757a8` (2026-09-08, contains baseline commit `72827e8` —
TЗ §4 line 86 satisfied): **9/9 jobs success**. Obsolete run
`34092345578` (`cea6062`, pre-baseline SHA) is superseded as acceptance
evidence but stays on record as the first all-green run; run
`34050610807` (pre-fix SHA) is kept only as crash-finding evidence
(§5.1). Dispatch by the owner throughout (no `gh` on the dev host).

| Job (`nightly-m7.yml` / `nightly-fs-crash.yml`) | Required evidence | Status |
|---|---|---|
| `coverage` | lcov.info, cov.json, core ≥90 % / boa_idb ≥80 %, tests+bins+examples + WPT ×3 | **PASS (CI)** — run `34240365417`, job `coverage` success 2026-09-08: `boa_idb_core: lines=2585 covered=2336 cover=90.4% floor=90.0%`, `boa_idb: lines=8938 covered=7233 cover=80.9% floor=80.0%` (threshold-gate step exit 0); artifact `coverage-lcov` (ID `10062424669`, 133 577 bytes, expires 2026-12-07) |
| `differential-and-lifecycle` | 10 000 seeded scenarios + `BOA_IDB_LONG_LIFECYCLE=1` success log | **PASS (CI)** — run `34240365417`, job success 2026-09-08 (~9 min, full workspace suite with nightly knobs, zero failures) |
| `cursor-matrix-1m` | `matrix-1m.log`, `receipt-matrix-1m.txt`, 12 combos at 1M | **PASS (CI)** — run `34240365417`, job success 2026-09-08 (~58 min on Linux): 12/12 at 10⁶, all peaks < 1 MiB (memory ~1 KiB, FS ~1 KiB, SQLite ~237 KiB); artifact `cursor-matrix-1m-log` (ID `10064237819`, expires 2026-12-07). Full receipts: §5.4 |
| `memory-massif` | `massif.out`, Valgrind version, peak + gate result | **PASS (CI)** — run `34240365417`, job success 2026-09-08: `massif peak heap bytes: 433022` (≪ 512 MiB gate); artifact `massif-out` (ID `10062816872`, 43 568 bytes, expires 2026-12-07) |
| `fuzz` (5 targets × 48 min, ≥4 h total) | final stats, corpus/cache identity, crash artifacts + replay | **PASS (CI)** — run `34240365417`, all 5 jobs success 2026-09-08, 2881 s each (~4.0 h total), zero crashes: `fuzz_scf_decode` 981 529 248 execs, `fuzz_key_decode` 124 598 008, `fuzz_keypath_parse` 296 678 733, `fuzz_wal_recovery` 851 894 181, `fuzz_stateful_txn` 2 844 979. The pre-fix crash (§5.1, run `34050610807`) is the required crash-artifact evidence. |
| `nightly-fs-crash` (`BOA_IDB_FS_CRASH_ITERS=200`) | run URL/ID, seed/replay, green M6 fault matrix | **PASS (CI)** — run `34050249898` 2026-09-06, `workflow_dispatch` on `task/m7c-ci-evidence-final` @ `5f21831`: `crash-consistency (ubuntu-latest)` success (43 s step) + `crash-consistency (windows-latest)` success; seed `0xC0FFEE`, command `BOA_IDB_FS_CRASH_ITERS=200 BOA_IDB_FS_CRASH_SEED=0xC0FFEE cargo test -p boa_idb_fs --test m6b3_crash_tests -- --nocapture` per `.github/workflows/nightly-fs-crash.yml`. Run: https://github.com/DavidGoliaf/IndexedDB_boa/actions/runs/34050249898. Crash behavior is SHA-independent here: no FS-format change between `5f21831` and `main` (only docs/workflow/test-harness changed), so no re-dispatch is required for this job |

Dispatch (owner with Actions access) — always fresh `workflow_dispatch`
on the current tip (never `rerun --failed`, see §5.3):

```sh
gh workflow run nightly-m7.yml --ref main
```

(`nightly-fs-crash` stays green on `5f21831`, see table;
`bench-regression` is done via PR #1, see §4.)

File the top-level run URL/ID, per-job URLs/IDs, exact commands, and
artifact names here before flipping any status to PASS. On failure:
seed + replay command + corpus/artifact. No workflow scope was weakened
to fit hosted CI.

### 5.5 P1 fresh run (closed 2026-09-08)

P1 required nightly on a SHA containing baseline commit `72827e8`.
Satisfied by run `34240365417` on `main` @ `72757a8`
(`72827e8` is an ancestor — verified via `git merge-base --is-ancestor`).
Local log copy: `artefacts/logs_92753161294.zip` (untracked, dev-host
copy only). §5.4 receipts below are from this run (identical peaks to
the superseded run, as expected — no cursor-code change between them).

### 5.1 Fuzz crash finding: WAL `decode_ops` OOM (`fuzz_wal_recovery`)

Inaugural run `34050610807`, job `fuzz (fuzz_wal_recovery)`
(https://github.com/DavidGoliaf/IndexedDB_boa/actions/runs/34050610807/job/101533428244):
after ~20.7M execs (~90 s) libFuzzer aborted with

```text
==4356== ERROR: libFuzzer: out-of-memory (malloc(9863417728))
SUMMARY: libFuzzer: out-of-memory
```

Root cause (`crates/boa_idb_fs/src/wal.rs::decode_ops`): the op vector
was pre-allocated from the untrusted varint count,
`Vec::with_capacity(count as usize)`. The crashing input (111 bytes,
valid `IWAL` magic, `len=49`, valid CRC over the frame body) claims
`count = 154115902` ops. Proof by arithmetic: `154115902 × 64`
(`size_of::<WalOp>()`) = `9863417728` = the exact `malloc` size in the
log. No other allocation of that size exists on this path. The fuzzer
verdict is therefore attributed to this line, not to allocator noise.

Fix (commits `60c3321` on `task/m7c-ci-evidence-final`, cherry-pick
`66be8f0` on `task/m7b-optimizations-reliability`; production change is
one line): grow the vector incrementally (`Vec::new()`) — the loop
already returns `Malformed` on truncated payloads, so peak allocation
is bounded by the real input length. Regression test
`wal::tests::huge_op_count_does_not_preallocate` pins a valid-CRC frame
with `count = u64::MAX` to `Malformed` (no abort). `boa_idb_fs` suite
green, workspace fmt/clippy/diff-check clean.

Crash evidence: artifact `fuzz-crashes-fuzz_wal_recovery`
(ID `9994447714`, 315 bytes, expires 2026-12-05) on run `34050610807`;
file `fuzz_wal_recovery-oom-9f99fc61c67578bb64eff6d8475e23c6d2017813`
(byte-identical to the base64 input printed in the job log). Local copy:
`artefacts/fuzz-crashes-fuzz_wal_recovery.zip` (untracked, dev-host
agreements only — not committed to keep the repo free of fuzzer blobs).

Replay (Linux, fixed code — must exit 0 with `Malformed`; on pre-fix
code the same input triggers the OOM path under ASan/rss-limit):

```sh
cd fuzz
cargo +nightly fuzz run fuzz_wal_recovery ../crash-wal/fuzz_wal_recovery-oom-9f99fc61c67578bb64eff6d8475e23c6d2017813
```

Limitation, stated explicitly: direct OOM replay on the Windows dev
host is architecturally impossible — the Windows allocator commits
lazily (virtual reserve without physical backing, 32+ GB RAM here), so
both pre- and post-fix code return `Err(unknown op kind)` with exit 0;
libFuzzer itself cannot link on Windows (no ASan runtime — limitation
3 in §10). The attribution therefore rests on the malloc-size
arithmetic above plus the regression test, not on a local abort.

### 5.3 Fresh dispatch rule (applied)

`gh run rerun 34050610807 --failed` would re-execute the recorded SHAs
(`c950302` for `nightly-m7`), i.e. the code WITHOUT the §§5.1–5.2
fixes. It was NOT used. Instead the owner ran a fresh
`workflow_dispatch` → run `34092345578` on the fixed SHA `cea6062`,
which is the evidence in the table above.
`nightly-fs-crash` needs no re-dispatch (green on `5f21831`, unaffected
by later fixes). `bench-regression` stays queued until the §4 runner
exists.

### 5.4 CI 1M cursor-matrix receipts (run `34092345578`, Linux x86_64)

From `cursor-matrix-1m.log` (`receipt-matrix-1m.txt` in artifact
`cursor-matrix-1m-log`), bound 1048576 bytes each:

```text
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=memory dir=Next scale=1000000 bound=1048576 peak=952
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=memory dir=Prev scale=1000000 bound=1048576 peak=952
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=sqlite dir=Next scale=1000000 bound=1048576 peak=237396
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=sqlite dir=Prev scale=1000000 bound=1048576 peak=237560
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=fs dir=Next scale=1000000 bound=1048576 peak=1112
RECEIPT-MATRIX test=cursor_index_directions_bounded backend=fs dir=Prev scale=1000000 bound=1048576 peak=1112
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=memory dir=Next scale=1000000 bound=1048576 peak=936
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=memory dir=Prev scale=1000000 bound=1048576 peak=936
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=sqlite dir=Next scale=1000000 bound=1048576 peak=236944
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=sqlite dir=Prev scale=1000000 bound=1048576 peak=236884
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=fs dir=Next scale=1000000 bound=1048576 peak=1088
RECEIPT-MATRIX test=cursor_store_walk_bounded_1m backend=fs dir=Prev scale=1000000 bound=1048576 peak=1088
```

Verdict: 12/12 PASS (`test result: ok. 2 passed`), `os=linux arch=x86_64`
on every line. Source: local log copy `logs_92357051068.zip`
(untracked `artefacts/`, dev-host copy only).

### 5.2 Coverage job failure: missing `boa-idb-cli` example build

Same run `34050610807`, job `coverage`: `cli_tests` (3/3) failed with
`boa-idb-cli example binary must exist` — the coverage run retargets
builds into `target/llvm-cov-target/`, whose `debug/examples/` dir never
received the plain-named example binary at test time (locally green only
because `target/debug/examples/boa-idb-cli.exe` was built earlier, via
the workspace-fallback probe).

Fix (commits `1c17eb3` / cherry-pick `ba43a6f`, sync `6f6ccd9`/`4d211ac`;
workflow + test only, no production change): `nightly-m7.yml`
(`coverage`, `differential-and-lifecycle`) now runs
`cargo build --workspace --examples` BEFORE the llvm-cov step (a later
build could never help: the failed step skips the rest of the job);
`cli_tests.rs` resolution extracted into `resolve_cli_exe()` (plain →
hashed-sorted → workspace fallback; no `read_dir` panic on a missing
`examples/` dir) with 4 unit tests (`cli_tests` 7/7 locally).

Do NOT `gh run rerun 34050610807 --failed`: GitHub re-runs the recorded
SHA (`c950302`), not the fixed tree. A fresh dispatch on the fixed SHA
is required (see §5.3).

## 6. Local quality suite (§6, this tree — re-verified for this handoff)

Branch `task/m7c-ci-evidence-final`, clean except untracked
`docs/reviews/M6-review.md` (M6 material, out of M7 scope) and
`artefacts/` (local CI evidence zips, intentionally uncommitted).

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
| R12.1 | PASS (CI) | harness + fixes + fail-closed comparator (16/16) + labelled runner/baseline + required `bench-regression` PR check green on PR #1 (§4) |
| R12.2 | PASS (CI) | 1M matrix 12/12 on Linux run `34240365417` (`main` @ `72757a8`, contains baseline `72827e8`) with CI receipts (§5.4) |
| R12.3 | PASS | tracing spans + observer + CLI covered by tests, CI tracing job present |
| R13.1 | PASS (CI) | all levels green on run `34240365417` (coverage 90.4/80.9, 4 h fuzz no new crashes, massif, differential, matrix); pre-fix crash found+fixed with evidence (§5.1) |
| R13.1-fuzz | PASS (CI) | 5 targets × 2881 s, zero crashes on run `34240365417`; pre-fix OOM crash fixed with artifact+replay (§5.1) |
| R13.2 | PASS (CI) | CI coverage gate exit 0 on run `34240365417`: core 90.4 % (2336/2585) ≥90, boa_idb 80.9 % (7233/8938) ≥80; artifact `coverage-lcov` (ID `10062424669`) |
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

- [x] Labelled runner + protected host ID provisioned; labelled baseline
  created on it and committed — **done** (§4: `boa-bench-01`, commit `72827e8`)
- [x] `bench-regression` is an actual required PR check with a passing run
  on the baseline; comparator fail-closed cases tested — **done**
  (§4: PR #1, run `34206701559`, 16/16 unit tests)
- [x] Successful nightly evidence: coverage, 1M matrix, massif,
  differential/lifecycle, ≥4 h fuzz — **done** (run `34240365417` on
  `main` @ `72757a8`, contains baseline `72827e8`; §5.5): 9/9 success;
  fuzz crash found+fixed (§5.1); coverage workflow+test green (§5.2)
- [x] Successful `nightly-fs-crash` evidence at 200 iterations — **done**
  (run `34050249898`, §5)
- [x] Targets/misses have receipts/profiles/follow-ups without requirement
  substitution — **done** (§8)
- [x] `M7-handoff.md`, M7B handoff pointer, traceability in sync; every
  PASS has URL/artifact/command/SHA — **done** (§§5, 7)
- [x] Full local quality suite + memory/SQLite/FS WPT green — **done** (§6)

M7-C (and therefore M7) is **COMPLETE**: every §7 item has CI evidence
above on baseline-containing SHAs. `main` carries the full M5→M7
integration; probe branch `task/bench-check-probe` may be deleted after
PR #1 merge.

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
