#!/usr/bin/env python3
"""Blocking >10% performance-regression gate (M7-B P1-5).

Compares fresh `cargo bench -p boa_idb --bench backends` criterion means
against the checked-in labelled-host baseline and FAILS (exit 1) when any
scenario regresses by more than 10% in mean time-per-iteration.

All bundled scenarios are time-based (lower is better), so the rule is
uniform: fail when new_mean_ns > 1.10 * base_mean_ns.

Modes:
  compare  --baseline FILE   run benches, compare, exit 1 on regression.
  write    --baseline FILE   run benches, (over)write FILE with fresh means
                             + provenance. For (re-)baselining the labelled
                             host only; the result must be committed.

The gate must run on the LABELLED (pinned, immutable-config) host: pass
`--runner self-hosted,bench` in CI. Heterogeneous runners must never quote
its numbers as evidence; on any other host use `--write` for diagnostics.

Criterion estimates layout:
  target/criterion/<group>/<function>/new/estimates.json
  -> {"mean": {"point_estimate": <ns float>, ...}, ...}
Scenario id: "<group>/<function>".
"""

import argparse
import datetime
import json
import os
import platform
import subprocess
import sys

REGRESSION_THRESHOLD = 1.10
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run_bench() -> None:
    cmd = ["cargo", "bench", "-p", "boa_idb", "--bench", "backends"]
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=REPO, check=True)


def collect_means() -> dict:
    crit = os.path.join(REPO, "target", "criterion")
    means = {}
    for group in sorted(os.listdir(crit)):
        gdir = os.path.join(crit, group)
        if not os.path.isdir(gdir):
            continue
        for func in sorted(os.listdir(gdir)):
            est = os.path.join(gdir, func, "new", "estimates.json")
            if not os.path.isfile(est):
                continue
            with open(est) as fh:
                data = json.load(fh)
            try:
                mean_ns = float(data["mean"]["point_estimate"])
            except (KeyError, TypeError, ValueError):
                continue
            means[f"{group}/{func}"] = mean_ns
    if not means:
        raise SystemExit("no criterion estimates found under target/criterion")
    return means


def provenance() -> dict:
    out = {
        "date_utc": datetime.datetime.now(datetime.timezone.utc)
        .date()
        .isoformat(),
        "os": platform.system(),
        "os_release": platform.release(),
        "arch": platform.machine(),
        "cpu": platform.processor(),
        "profile": "bench (inherits release, lto=thin)",
        "command": "cargo bench -p boa_idb --bench backends",
        "threshold": REGRESSION_THRESHOLD,
        # The gate only enforces on the pinned labelled host. Baselines
        # captured elsewhere MUST carry host_role "interim" and must be
        # replaced (same command, --write) once the labelled host exists.
        "host_role": os.environ.get("BOA_IDB_BASELINE_ROLE", "interim"),
    }
    try:
        rustc = subprocess.run(
            ["rustc", "--version"], capture_output=True, text=True, check=True
        )
        out["rustc"] = rustc.stdout.strip()
    except Exception:
        pass
    try:
        git = subprocess.run(
            ["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True, cwd=REPO
        )
        out["git_sha"] = git.stdout.strip()
    except Exception:
        pass
    return out


def cmd_write(path: str, no_run: bool) -> None:
    if not no_run:
        run_bench()
    means = collect_means()
    doc = {"provenance": provenance(), "scenarios": {}}
    for name in sorted(means):
        doc["scenarios"][name] = {"mean_ns": means[name]}
    with open(path, "w") as fh:
        json.dump(doc, fh, indent=2)
        fh.write("\n")
    print(f"wrote {len(means)} scenarios to {path}")


def cmd_compare(path: str) -> int:
    with open(path) as fh:
        doc = json.load(fh)
    base = {k: v["mean_ns"] for k, v in doc["scenarios"].items()}
    print(f"baseline: {path} ({len(base)} scenarios)")
    run_bench()
    means = collect_means()
    failed = False
    print(f"{'scenario':44s} {'base_ns':>14s} {'new_ns':>14s} {'ratio':>8s}  verdict")
    for name in sorted(set(base) | set(means)):
        if name not in base:
            print(f"{name:44s} {'—':>14s} {means[name]:14.1f} {'new':>8s}  NEW (no baseline)")
            continue
        if name not in means:
            print(f"{name:44s} {base[name]:14.1f} {'—':>14s} {'missing':>8s}  MISSING")
            failed = True
            continue
        ratio = means[name] / base[name] if base[name] else float("inf")
        verdict = "ok" if ratio <= REGRESSION_THRESHOLD else "REGRESSION"
        if verdict != "ok":
            failed = True
        print(f"{name:44s} {base[name]:14.1f} {means[name]:14.1f} {ratio:8.3f}  {verdict}")
    if failed:
        print("::error::performance regression >10% vs labelled baseline")
        return 1
    print("all scenarios within 10% of the labelled baseline")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("mode", choices=["compare", "write"])
    ap.add_argument("--baseline", required=True)
    ap.add_argument(
        "--no-run",
        action="store_true",
        help="write only: reuse existing target/criterion estimates",
    )
    args = ap.parse_args()
    if args.mode == "write":
        cmd_write(args.baseline, args.no_run)
        return 0
    return cmd_compare(args.baseline)


if __name__ == "__main__":
    sys.exit(main())
