#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Validate, summarise and A/B-compare touchHLE performance reports.

Input is the structured output of `--performance-test=...`:
`touchHLE_performance.csv` (or `.jsonl`). See
dev-docs/performance/BENCHMARKING.md for the protocol.

    perf-report.py validate FILE...
    perf-report.py summarize [--skip-seconds N] [--json] FILE...
    perf-report.py compare [--skip-seconds N] BASELINE.csv CANDIDATE.csv

A "run" is one session (one launch of the emulator). Each run is reduced to
one value per metric (the median over its windows), and variability is then
measured across runs, so use several launches per build.

Only the standard library is used. Frame times here are measured at the
guest's presentRenderbuffer: calls, not at the Android display.
"""
import argparse
import csv
import json
import math
import statistics
import sys

# Metrics: name -> True if a higher value is better.
METRICS = {
    "average_fps": True,
    "fps_1pct_low": True,
    "median_frame_ms": False,
    "p95_frame_ms": False,
    "p99_frame_ms": False,
    "cpu_ms_per_frame": False,
    "gl_ms_per_frame": False,
    "gl_calls_per_frame": False,
    "present_ms_per_frame": False,
    "peak_rss_mb": False,
}
# Metrics whose per-run value is the maximum over windows, not the median.
MAX_METRICS = {"peak_rss_mb"}
IDENTITY = ("build", "mode", "device_model", "resolution")
STRING_COLUMNS = set(IDENTITY)
INT_COLUMNS = {"session", "window_index", "frames", "slow_frames"}
# Columns that may be empty (CSV) / null (JSON) when unavailable.
OPTIONAL = {"fps_1pct_low", "peak_rss_mb"}
MIN_RUNS_FOR_CONCLUSION = 3
# The directive: a ~2% difference within noise is not proof of anything.
MIN_SIGNIFICANT_FRACTION = 0.02


class ReportError(Exception):
    pass


def _number(name, raw, where):
    if raw is None or raw == "":
        if name in OPTIONAL:
            return None
        raise ReportError(f"{where}: column {name!r} is empty")
    try:
        value = int(raw) if name in INT_COLUMNS else float(raw)
    except (TypeError, ValueError):
        raise ReportError(f"{where}: column {name!r} is not a number: {raw!r}")
    if isinstance(value, float) and not math.isfinite(value):
        raise ReportError(f"{where}: column {name!r} is not finite")
    return value


def _row(raw, where, columns):
    missing = [c for c in columns if c not in raw]
    if missing:
        raise ReportError(f"{where}: missing columns {missing}")
    row = {}
    for name in columns:
        if name in STRING_COLUMNS:
            row[name] = raw[name] if raw[name] is not None else ""
        else:
            row[name] = _number(name, raw[name], where)
    check_row(row, where)
    return row


def check_row(row, where):
    """Sanity checks that catch corrupted or mislabelled windows."""
    if row["frames"] <= 0:
        raise ReportError(f"{where}: frames must be positive")
    if row["measurement_s"] <= 0 or row["average_fps"] <= 0:
        raise ReportError(f"{where}: measurement_s and average_fps must be positive")
    # Percentiles must be ordered; 1e-6 tolerates float formatting.
    order = ["median_frame_ms", "p95_frame_ms", "p99_frame_ms", "max_frame_ms"]
    for lo, hi in zip(order, order[1:]):
        if row[lo] > row[hi] + 1e-6:
            raise ReportError(f"{where}: {lo} ({row[lo]}) exceeds {hi} ({row[hi]})")
    one_pct = row["fps_1pct_low"]
    if one_pct is not None and one_pct <= 0:
        raise ReportError(f"{where}: fps_1pct_low must be positive")


def load(path, columns=None):
    """Load and validate a CSV or JSONL report. Returns a list of row dicts."""
    columns = columns or default_columns()
    rows = []
    try:
        handle = open(path, newline="", encoding="utf-8")
    except OSError as error:
        raise ReportError(f"{path}: {error}")
    with handle:
        if path.endswith(".jsonl"):
            for number, line in enumerate(handle, 1):
                if not line.strip():
                    continue
                where = f"{path}:{number}"
                try:
                    raw = json.loads(line)
                except json.JSONDecodeError as error:
                    raise ReportError(f"{where}: invalid JSON: {error}")
                rows.append(_row(raw, where, columns))
        else:
            reader = csv.DictReader(handle)
            if reader.fieldnames != columns:
                raise ReportError(
                    f"{path}: header does not match the expected columns.\n"
                    f"  expected: {','.join(columns)}\n"
                    f"  found:    {','.join(reader.fieldnames or [])}"
                )
            for raw in reader:
                where = f"{path}:{reader.line_num}"
                if None in raw:  # more fields than header
                    raise ReportError(f"{where}: too many fields")
                rows.append(_row(raw, where, columns))
    if not rows:
        raise ReportError(f"{path}: no data rows")
    return rows


def default_columns():
    return [
        "session", "window_index", "build", "mode", "device_model",
        "resolution", "measurement_s", "frames", "average_fps",
        "median_frame_ms", "p95_frame_ms", "p99_frame_ms", "max_frame_ms",
        "fps_1pct_low", "cpu_ms_per_frame", "gl_ms_per_frame",
        "gl_calls_per_frame", "present_ms_per_frame", "readback_ms_per_frame",
        "requested_wait_ms_per_frame", "slow_frames", "peak_rss_mb",
    ]  # fmt: skip


def per_run(rows, skip_seconds=0.0):
    """Reduce windows to one value per metric per run (session).

    Windows that start before `skip_seconds` of cumulative measurement time in
    their session are dropped (warm-up). Returns {session: {identity,
    windows, metric: value}}.
    """
    by_session = {}
    for row in sorted(rows, key=lambda r: (r["session"], r["window_index"])):
        by_session.setdefault(row["session"], []).append(row)
    runs = {}
    for session, windows in by_session.items():
        elapsed, kept = 0.0, []
        for window in windows:
            if elapsed >= skip_seconds:
                kept.append(window)
            elapsed += window["measurement_s"]
        if not kept:
            continue
        run = {"identity": tuple(kept[0][k] for k in IDENTITY), "windows": len(kept)}
        for metric in METRICS:
            values = [w[metric] for w in kept if w[metric] is not None]
            if not values:
                run[metric] = None
            elif metric in MAX_METRICS:
                run[metric] = max(values)
            else:
                run[metric] = statistics.median(values)
        runs[session] = run
    return runs


def across_runs(runs):
    """{metric: {median, min, max, cv, n}} over runs (cv is stdev/mean)."""
    result = {}
    for metric in METRICS:
        values = [r[metric] for r in runs.values() if r[metric] is not None]
        if not values:
            continue
        mean = statistics.fmean(values)
        cv = statistics.stdev(values) / mean if len(values) > 1 and mean else 0.0
        result[metric] = {
            "median": statistics.median(values),
            "min": min(values),
            "max": max(values),
            "cv": cv,
            "n": len(values),
        }
    return result


def compare(base_runs, cand_runs):
    """Compare two sets of runs. Returns (notes, results)."""
    notes = []
    base_ids = {r["identity"] for r in base_runs.values()}
    cand_ids = {r["identity"] for r in cand_runs.values()}
    for label, ids in (("baseline", base_ids), ("candidate", cand_ids)):
        if len(ids) > 1:
            notes.append(f"{label} mixes several build/mode/device/resolution identities: {sorted(ids)}")
    # Same mode/device/resolution is required; the build is what differs.
    base_cfg = {i[1:] for i in base_ids}
    cand_cfg = {i[1:] for i in cand_ids}
    if base_cfg != cand_cfg:
        notes.append(
            "mode/device/resolution differ between baseline and candidate "
            f"({sorted(base_cfg)} vs {sorted(cand_cfg)}); the comparison is not like-for-like"
        )
    if {i[0] for i in base_ids} == {i[0] for i in cand_ids}:
        notes.append("baseline and candidate report the same build")
    base, cand = across_runs(base_runs), across_runs(cand_runs)
    few = min(len(base_runs), len(cand_runs)) < MIN_RUNS_FOR_CONCLUSION
    if few:
        notes.append(
            f"fewer than {MIN_RUNS_FOR_CONCLUSION} runs on at least one side "
            f"(baseline {len(base_runs)}, candidate {len(cand_runs)}): "
            "differences are not conclusive"
        )
    results = []
    for metric, higher_better in METRICS.items():
        if metric not in base or metric not in cand:
            continue
        b, c = base[metric], cand[metric]
        change = (c["median"] - b["median"]) / b["median"] if b["median"] else 0.0
        noise = max(MIN_SIGNIFICANT_FRACTION, 2 * max(b["cv"], c["cv"]))
        if few or abs(change) <= noise:
            verdict = "within noise" if not few else "inconclusive"
        elif (change > 0) == higher_better:
            verdict = "improved"
        else:
            verdict = "REGRESSED"
        results.append(
            {"metric": metric, "baseline": b["median"], "candidate": c["median"],
             "change": change, "noise": noise, "verdict": verdict}
        )  # fmt: skip
    return notes, results


def _fmt(value):
    return "-" if value is None else f"{value:.3f}"


def cmd_validate(args):
    ok = True
    for path in args.files:
        try:
            rows = load(path)
        except ReportError as error:
            print(f"INVALID {error}", file=sys.stderr)
            ok = False
        else:
            print(f"OK {path}: {len(rows)} windows, {len({r['session'] for r in rows})} runs")
    return 0 if ok else 1


def _load_all(files):
    rows = []
    for path in files:
        rows += load(path)
    return rows


def cmd_summarize(args):
    runs = per_run(_load_all(args.files), args.skip_seconds)
    if not runs:
        raise ReportError("no windows left after skipping warm-up")
    summary = across_runs(runs)
    identities = sorted({r["identity"] for r in runs.values()})
    if args.json:
        print(json.dumps({"runs": len(runs), "identities": identities, "metrics": summary}, indent=2))
        return 0
    print(f"runs: {len(runs)}  identities (build, mode, device, resolution): {identities}")
    print(f"{'metric':<24}{'median':>10}{'min':>10}{'max':>10}{'cv %':>8}{'runs':>6}")
    for metric, s in summary.items():
        print(f"{metric:<24}{_fmt(s['median']):>10}{_fmt(s['min']):>10}{_fmt(s['max']):>10}{s['cv'] * 100:>8.1f}{s['n']:>6}")
    if len(runs) < MIN_RUNS_FOR_CONCLUSION:
        print(f"note: only {len(runs)} run(s); variability is not meaningful below {MIN_RUNS_FOR_CONCLUSION}")
    return 0


def cmd_compare(args):
    base = per_run(load(args.baseline), args.skip_seconds)
    cand = per_run(load(args.candidate), args.skip_seconds)
    if not base or not cand:
        raise ReportError("no windows left after skipping warm-up")
    notes, results = compare(base, cand)
    for note in notes:
        print(f"note: {note}")
    print(f"{'metric':<24}{'baseline':>10}{'candidate':>11}{'change %':>10}{'noise %':>9}  verdict")
    for r in results:
        print(f"{r['metric']:<24}{_fmt(r['baseline']):>10}{_fmt(r['candidate']):>11}{r['change'] * 100:>+10.1f}{r['noise'] * 100:>9.1f}  {r['verdict']}")
    regressed = any(r["verdict"] == "REGRESSED" for r in results)
    return 1 if (regressed and args.fail_on_regression) else 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("validate", help="check report files")
    p.add_argument("files", nargs="+")
    p.set_defaults(func=cmd_validate)
    p = sub.add_parser("summarize", help="aggregate runs and report variability")
    p.add_argument("files", nargs="+")
    p.add_argument("--skip-seconds", type=float, default=0.0,
                   help="drop this much warm-up from the start of every run (protocol: 120)")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_summarize)
    p = sub.add_parser("compare", help="A/B compare a candidate against a baseline")
    p.add_argument("baseline")
    p.add_argument("candidate")
    p.add_argument("--skip-seconds", type=float, default=0.0)
    p.add_argument("--fail-on-regression", action="store_true")
    p.set_defaults(func=cmd_compare)
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except ReportError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
