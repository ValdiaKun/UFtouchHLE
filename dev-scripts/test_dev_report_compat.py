#!/usr/bin/env python3
"""Independent QA checks against the current perf-development report parser.

Run on a QA checkout containing a sibling 'development/' checkout:
    UF_DEV_ROOT=development python3 dev-scripts/test_dev_report_compat.py
"""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest


DEV_ROOT = Path(os.environ.get("UF_DEV_ROOT", "development")).resolve()
SCRIPT = DEV_ROOT / "dev-scripts" / "perf-report.py"
spec = importlib.util.spec_from_file_location("qa_dev_perf", SCRIPT)
perf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(perf)


def window(schema):
    row = {}
    for key in perf.default_columns() if schema == 2 else perf.legacy_columns():
        if key in ("session", "frames", "window_index", "slow_frames"):
            row[key] = {"session": 42, "frames": 100, "window_index": 0, "slow_frames": 0}[key]
        elif key in ("build", "mode", "device_model", "resolution"):
            row[key] = {"build": "sha", "mode": "Observe", "device_model": "iPhone4,1", "resolution": "960x640"}[key]
        elif key == "measurement_s":
            row[key] = 5.0
        elif key == "average_fps":
            row[key] = 20.0
        elif key == "median_frame_ms":
            row[key] = 10.0
        elif key == "p95_frame_ms":
            row[key] = 15.0
        elif key == "p99_frame_ms":
            row[key] = 20.0
        elif key == "max_frame_ms":
            row[key] = 30.0
        else:
            row[key] = 1.0
    return row


class IndependentCompatibilityTests(unittest.TestCase):
    def setUp(self):
        td = tempfile.TemporaryDirectory()
        self.addCleanup(td.cleanup)
        self.path = Path(td.name) / "report.jsonl"

    def write(self, *records):
        self.path.write_text("".join(json.dumps(r) + "\n" for r in records), encoding="utf-8")

    def test_v1_reports_unavailable_counters(self):
        self.write(window(1))
        row = perf.load(str(self.path))[0]
        self.assertEqual(row["schema"], 1)
        self.assertTrue(all(row[k] is None for k in perf.CPU_EVENT_COLUMNS))

    def test_v2_reports_numeric_counters(self):
        self.write(window(2))
        row = perf.load(str(self.path))[0]
        self.assertEqual(row["schema"], 2)
        self.assertTrue(all(row[k] is not None for k in perf.CPU_EVENT_COLUMNS))

    def test_v1_then_v2_jsonl_must_not_silently_drop_cpu_counters(self):
        # A benchmark file can be appended to across emulator versions.
        # Either reject mixed schemas or represent the later v2 metrics.
        # The older parser used the first row's schema for the whole file,
        # silently dropping CPU metrics from later v2 records.
        old = window(1)
        new = window(2)
        new["window_index"] = 1
        self.write(old, new)
        try:
            rows = perf.load(str(self.path))
        except perf.ReportError:
            return  # Explicitly rejecting mixed layouts is safe.
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[1]["schema"], 2, "Later v2 row was silently treated as v1")
        for key in perf.CPU_EVENT_COLUMNS:
            self.assertEqual(rows[1][key], new[key], f"Lost v2 CPU event metric {key}")


if __name__ == "__main__":
    unittest.main()
