#!/usr/bin/env python3
"""Unit tests for bench_compare.py (M7-B P1-1.4).

No benchmark is executed: `load_baseline` is exercised against synthetic
JSON files and `evaluate` against synthetic means. Run:
    python3 scripts/test_bench_compare.py
"""

import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from bench_compare import (
    BaselineError,
    cpu_string,
    evaluate,
    host_id,
    load_baseline,
)

HOST = "pinned-host-01"

PROVENANCE = {
    "git_sha": "abc123",
    "os": "Linux",
    "cpu": "Test CPU",
    "arch": "x86_64",
    "rustc": "rustc 1.91.0",
    "profile": "bench (inherits release, lto=thin)",
}


def write_doc(tmp, doc):
    path = os.path.join(tmp, "baseline.json")
    with open(path, "w") as fh:
        json.dump(doc, fh)
    return path


def labelled_doc(scenarios=None, host=HOST):
    return {
        "host_role": "labelled",
        "host_id": host,
        "provenance": dict(PROVENANCE),
        "scenarios": scenarios
        if scenarios is not None
        else {"a/x": {"mean_ns": 100.0}, "b/y": {"mean_ns": 200.0}},
    }


class LoadBaselineTests(unittest.TestCase):
    def test_labelled_matching_id_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_doc(tmp, labelled_doc())
            doc = load_baseline(path, current_host=HOST)
            self.assertEqual(len(doc["scenarios"]), 2)

    def test_missing_env_id_rejected(self):
        # current_host="" simulates BOA_IDB_BENCH_HOST_ID unset: run_bench
        # must never start.
        with tempfile.TemporaryDirectory() as tmp:
            path = write_doc(tmp, labelled_doc())
            with self.assertRaises(BaselineError):
                load_baseline(path, current_host="")

    def test_missing_baseline_id_rejected(self):
        doc = labelled_doc()
        doc.pop("host_id")
        with tempfile.TemporaryDirectory() as tmp:
            path = write_doc(tmp, doc)
            with self.assertRaises(BaselineError):
                load_baseline(path, current_host=HOST)

    def test_mismatched_id_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_doc(tmp, labelled_doc(host="other-host"))
            with self.assertRaises(BaselineError):
                load_baseline(path, current_host=HOST)

    def test_host_id_reads_protected_env(self):
        os.environ["BOA_IDB_BENCH_HOST_ID"] = HOST
        try:
            self.assertEqual(host_id(), HOST)
        finally:
            del os.environ["BOA_IDB_BENCH_HOST_ID"]
        self.assertEqual(host_id(), "")

    def test_interim_rejected(self):
        doc = labelled_doc()
        doc["host_role"] = "interim"
        with tempfile.TemporaryDirectory() as tmp:
            path = write_doc(tmp, doc)
            with self.assertRaises(BaselineError):
                load_baseline(path)

    def test_missing_role_rejected(self):
        doc = labelled_doc()
        del doc["host_role"]
        with tempfile.TemporaryDirectory() as tmp:
            path = write_doc(tmp, doc)
            with self.assertRaises(BaselineError):
                load_baseline(path)

    def test_missing_file_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(BaselineError):
                load_baseline(os.path.join(tmp, "nope.json"))

    def test_provenance_gap_rejected(self):
        for key in ("git_sha", "os", "cpu", "arch", "rustc", "profile"):
            doc = labelled_doc()
            del doc["provenance"][key]
            with tempfile.TemporaryDirectory() as tmp:
                path = write_doc(tmp, doc)
                with self.assertRaises(BaselineError, msg=key):
                    load_baseline(path)

    def test_empty_scenarios_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_doc(tmp, labelled_doc(scenarios={}))
            with self.assertRaises(BaselineError):
                load_baseline(path)


class CpuStringTests(unittest.TestCase):
    def test_returns_nonempty_string(self):
        # platform.processor() is "" on many Linux installs; the
        # provenance gate requires a non-empty cpu, so cpu_string()
        # must never return ""/whitespace (M7-C labelled capture).
        self.assertTrue(cpu_string().strip())


class EvaluateTests(unittest.TestCase):
    def test_within_threshold_passes(self):
        failed, _ = evaluate(labelled_doc(), {"a/x": 105.0, "b/y": 220.0})
        self.assertFalse(failed)

    def test_exactly_10_percent_passes(self):
        failed, _ = evaluate(labelled_doc(), {"a/x": 110.0, "b/y": 200.0})
        self.assertFalse(failed)

    def test_10_01_percent_regression_fails(self):
        failed, lines = evaluate(
            labelled_doc(), {"a/x": 110.01, "b/y": 200.0}
        )
        self.assertTrue(failed)
        self.assertTrue(any("REGRESSION" in line for line in lines))

    def test_missing_scenario_rejected(self):
        failed, lines = evaluate(labelled_doc(), {"a/x": 100.0})
        self.assertTrue(failed)
        self.assertTrue(any("MISSING" in line for line in lines))

    def test_new_scenario_rejected(self):
        failed, lines = evaluate(
            labelled_doc(), {"a/x": 100.0, "b/y": 200.0, "c/z": 1.0}
        )
        self.assertTrue(failed)
        self.assertTrue(any("NEW" in line for line in lines))


if __name__ == "__main__":
    unittest.main(verbosity=2)
