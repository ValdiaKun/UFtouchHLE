#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Tests for perf-report.py. Run: python3 dev-scripts/test_perf_report.py"""
import csv
import importlib.util
import io
import json
import os
import re
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("perf_report", os.path.join(HERE, "perf-report.py"))
pr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pr)


def rust_csv_header():
    with open(os.path.join(HERE, "..", "src", "performance_report.rs"), encoding="utf-8") as f:
        source = f.read()
    return re.search(r'pub const CSV_HEADER: &str = "([^"]+)";', source).group(1).split(",")


def write_text(path, text):
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def window(session, index, **over):
    row = {
        "session": session, "window_index": index, "build": "v1-gaaa",
        "mode": "Observe", "device_model": "iPhone4,1", "resolution": "960x640",
        "measurement_s": 5.0, "frames": 300, "average_fps": 60.0,
        "median_frame_ms": 16.0, "p95_frame_ms": 17.0, "p99_frame_ms": 20.0,
        "max_frame_ms": 30.0, "fps_1pct_low": 40.0, "cpu_ms_per_frame": 5.0,
        "gl_ms_per_frame": 3.0, "gl_calls_per_frame": 100.0,
        "present_ms_per_frame": 1.0, "readback_ms_per_frame": 0.0,
        "requested_wait_ms_per_frame": 0.0, "slow_frames": 0,
        "svc_per_frame": 40.0, "mem_callbacks_per_frame": 2.0,
        "code_reads_per_frame": 0.5, "cache_invalidations_per_frame": 0.0,
        "context_swaps_per_frame": 1.0, "run_calls_per_frame": 41.0,
        "peak_rss_mb": 500.0,
    }  # fmt: skip
    row.update(over)
    return row


def write_csv(directory, name, rows):
    path = os.path.join(directory, name)
    with open(path, "w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=pr.default_columns())
        writer.writeheader()
        for row in rows:
            writer.writerow({k: ("" if v is None else v) for k, v in row.items()})
    return path


class SchemaTests(unittest.TestCase):
    def test_python_columns_match_the_rust_header(self):
        self.assertEqual(pr.default_columns(), rust_csv_header())

    def test_every_metric_is_a_known_column(self):
        self.assertTrue(set(pr.METRICS) <= set(pr.default_columns()))


class LoadTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def test_roundtrip_with_quoted_model_and_empty_optionals(self):
        path = write_csv(self.dir.name, "a.csv", [window(1, 0, fps_1pct_low=None, peak_rss_mb=None)])
        rows = pr.load(path)
        self.assertEqual(rows[0]["device_model"], "iPhone4,1")
        self.assertIsNone(rows[0]["fps_1pct_low"])
        self.assertIsNone(rows[0]["peak_rss_mb"])

    def test_jsonl_with_nulls(self):
        path = os.path.join(self.dir.name, "a.jsonl")
        row = window(1, 0, fps_1pct_low=None)
        with open(path, "w") as f:
            f.write(json.dumps(row) + "\n\n")
        self.assertIsNone(pr.load(path)[0]["fps_1pct_low"])

    def test_rejects_bad_header_empty_required_and_disordered_percentiles(self):
        bad = os.path.join(self.dir.name, "bad.csv")
        write_text(bad, "a,b\n1,2\n")
        with self.assertRaisesRegex(pr.ReportError, "header"):
            pr.load(bad)
        with self.assertRaisesRegex(pr.ReportError, "empty"):
            pr.load(write_csv(self.dir.name, "e.csv", [window(1, 0, average_fps=None)]))
        with self.assertRaisesRegex(pr.ReportError, "exceeds"):
            pr.load(write_csv(self.dir.name, "o.csv", [window(1, 0, p95_frame_ms=50.0)]))
        with self.assertRaisesRegex(pr.ReportError, "not finite"):
            pr.load(write_csv(self.dir.name, "n.csv", [window(1, 0, average_fps="nan")]))
        with self.assertRaisesRegex(pr.ReportError, "no data rows"):
            pr.load(write_csv(self.dir.name, "z.csv", []))

    def test_cli_validate_exit_codes(self):
        good = write_csv(self.dir.name, "g.csv", [window(1, 0)])
        bad = os.path.join(self.dir.name, "b.csv")
        write_text(bad, "x\n")
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            self.assertEqual(pr.main(["validate", good]), 0)
            self.assertEqual(pr.main(["validate", good, bad]), 1)
            self.assertEqual(pr.main(["summarize", bad]), 2)


class AggregationTests(unittest.TestCase):
    def test_warmup_windows_are_skipped_per_session(self):
        rows = [window(1, i, average_fps=10.0 if i < 2 else 60.0) for i in range(6)]
        self.assertEqual(pr.per_run(rows)[1]["average_fps"], 60.0)  # median anyway
        runs = pr.per_run(rows, skip_seconds=10.0)  # drops the first two 5 s windows
        self.assertEqual(runs[1]["windows"], 4)
        self.assertEqual(runs[1]["average_fps"], 60.0)
        self.assertEqual(pr.per_run(rows, skip_seconds=1000.0), {})

    def test_peak_rss_is_a_maximum_not_a_median(self):
        rows = [window(1, i, peak_rss_mb=m) for i, m in enumerate([400.0, 500.0, 900.0])]
        self.assertEqual(pr.per_run(rows)[1]["peak_rss_mb"], 900.0)

    def test_variability_is_across_runs(self):
        runs = pr.per_run([window(s, 0, average_fps=f) for s, f in [(1, 58.0), (2, 60.0), (3, 62.0)]])
        stats = pr.across_runs(runs)["average_fps"]
        self.assertEqual(stats["median"], 60.0)
        self.assertEqual((stats["min"], stats["max"], stats["n"]), (58.0, 62.0, 3))
        self.assertAlmostEqual(stats["cv"], 2.0 / 60.0)


def runs_of(values, build="v1-gaaa", metric="p99_frame_ms"):
    return pr.per_run([window(i + 1, 0, build=build, **{metric: v}, max_frame_ms=max(v, 100.0)) for i, v in enumerate(values)])


def verdict(results, metric):
    return next(r["verdict"] for r in results if r["metric"] == metric)


class CompareTests(unittest.TestCase):
    def test_clear_improvement_and_regression_beyond_noise(self):
        base = runs_of([20.0, 20.2, 19.8])
        _, results = pr.compare(base, runs_of([15.0, 15.1, 14.9], build="v2-gbbb"))
        self.assertEqual(verdict(results, "p99_frame_ms"), "improved")  # lower is better
        _, results = pr.compare(base, runs_of([30.0, 30.1, 29.9], build="v2-gbbb"))
        self.assertEqual(verdict(results, "p99_frame_ms"), "REGRESSED")

    def test_small_change_is_within_noise(self):
        base = runs_of([20.0, 20.0, 20.0])
        _, results = pr.compare(base, runs_of([19.7, 19.7, 19.7], build="v2-gbbb"))  # -1.5%
        self.assertEqual(verdict(results, "p99_frame_ms"), "within noise")

    def test_noisy_runs_raise_the_bar(self):
        # Large spread (cv ~ 16%) -> a 20% change is still within 2*cv noise.
        base = runs_of([16.0, 20.0, 24.0])
        _, results = pr.compare(base, runs_of([12.8, 16.0, 19.2], build="v2-gbbb"))
        self.assertEqual(verdict(results, "p99_frame_ms"), "within noise")

    def test_higher_is_better_metrics(self):
        base = runs_of([60.0, 60.0, 60.0], metric="average_fps")
        _, results = pr.compare(base, runs_of([45.0, 45.0, 45.0], build="v2-gbbb", metric="average_fps"))
        self.assertEqual(verdict(results, "average_fps"), "REGRESSED")

    def test_too_few_runs_is_inconclusive(self):
        notes, results = pr.compare(runs_of([20.0]), runs_of([10.0], build="v2-gbbb"))
        self.assertEqual(verdict(results, "p99_frame_ms"), "inconclusive")
        self.assertTrue(any("fewer than 3 runs" in n for n in notes))

    def test_mismatched_configuration_and_same_build_are_flagged(self):
        base = runs_of([20.0] * 3)
        cand = pr.per_run([window(i + 1, 0, build="v2-gbbb", resolution="480x320") for i in range(3)])
        notes, _ = pr.compare(base, cand)
        self.assertTrue(any("not like-for-like" in n for n in notes))
        notes, _ = pr.compare(base, runs_of([20.0] * 3))
        self.assertTrue(any("same build" in n for n in notes))

    def test_compare_cli_fail_on_regression(self):
        with tempfile.TemporaryDirectory() as d:
            a = write_csv(d, "a.csv", [window(i, 0, p99_frame_ms=20.0) for i in (1, 2, 3)])
            b = write_csv(d, "b.csv", [window(i, 0, build="v2-gbbb", p99_frame_ms=40.0, max_frame_ms=60.0) for i in (1, 2, 3)])
            with redirect_stdout(io.StringIO()):
                self.assertEqual(pr.main(["compare", a, b]), 0)
                self.assertEqual(pr.main(["compare", "--fail-on-regression", a, b]), 1)


class LegacySchemaTests(unittest.TestCase):
    """v1 reports (before the CPU event rates) must stay interpretable."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def write_v1_csv(self, name, rows):
        path = os.path.join(self.dir.name, name)
        with open(path, "w", newline="", encoding="utf-8") as f:
            writer = csv.DictWriter(f, fieldnames=pr.legacy_columns())
            writer.writeheader()
            for row in rows:
                writer.writerow({k: row[k] for k in pr.legacy_columns()})
        return path

    def test_legacy_columns_are_current_minus_cpu_events(self):
        self.assertEqual(
            pr.legacy_columns(),
            [c for c in pr.default_columns() if c not in pr.CPU_EVENT_COLUMNS],
        )
        self.assertEqual(pr.legacy_columns()[-1], "peak_rss_mb")

    def test_v1_csv_loads_with_cpu_metrics_unavailable_not_zero(self):
        path = self.write_v1_csv("old.csv", [window(1, 0)])
        row = pr.load(path)[0]
        self.assertEqual(row["schema"], 1)
        for name in pr.CPU_EVENT_COLUMNS:
            self.assertIsNone(row[name])

    def test_current_csv_is_schema_2(self):
        path = write_csv(self.dir.name, "new.csv", [window(1, 0)])
        self.assertEqual(pr.load(path)[0]["schema"], 2)

    def test_v1_jsonl_loads(self):
        row = {k: v for k, v in window(1, 0).items() if k not in pr.CPU_EVENT_COLUMNS}
        path = os.path.join(self.dir.name, "old.jsonl")
        write_text(path, json.dumps(row) + "\n")
        loaded = pr.load(path)[0]
        self.assertEqual(loaded["schema"], 1)
        self.assertIsNone(loaded["svc_per_frame"])

    def test_partial_cpu_columns_in_jsonl_are_rejected_not_guessed(self):
        row = {k: v for k, v in window(1, 0).items() if k != "run_calls_per_frame"}
        path = os.path.join(self.dir.name, "partial.jsonl")
        write_text(path, json.dumps(row) + "\n")
        with self.assertRaisesRegex(pr.ReportError, "missing columns"):
            pr.load(path)

    def test_unknown_csv_layout_is_still_rejected(self):
        path = os.path.join(self.dir.name, "odd.csv")
        columns = pr.legacy_columns()[:-1]  # drops peak_rss_mb: neither schema
        with open(path, "w", newline="", encoding="utf-8") as f:
            writer = csv.DictWriter(f, fieldnames=columns)
            writer.writeheader()
            writer.writerow({k: window(1, 0)[k] for k in columns})
        with self.assertRaisesRegex(pr.ReportError, "known report schema"):
            pr.load(path)

    def test_compare_v1_baseline_with_v2_candidate_says_so_and_skips_cpu_metrics(self):
        old = self.write_v1_csv("old.csv", [window(i, 0) for i in (1, 2, 3)])
        new = write_csv(self.dir.name, "new.csv", [window(i, 0, build="v2-gbbb") for i in (1, 2, 3)])
        notes, results = pr.compare(pr.per_run(pr.load(old)), pr.per_run(pr.load(new)))
        self.assertTrue(any("v1 report schema" in n for n in notes))
        compared = {r["metric"] for r in results}
        self.assertIn("average_fps", compared)
        for name in pr.CPU_EVENT_COLUMNS:
            self.assertNotIn(name, compared)

    def test_compare_two_v2_reports_has_no_schema_note(self):
        a = write_csv(self.dir.name, "a.csv", [window(i, 0) for i in (1, 2, 3)])
        b = write_csv(self.dir.name, "b.csv", [window(i, 0, build="v2-gbbb") for i in (1, 2, 3)])
        notes, _ = pr.compare(pr.per_run(pr.load(a)), pr.per_run(pr.load(b)))
        self.assertFalse(any("schema" in n for n in notes))


if __name__ == "__main__":
    unittest.main()
