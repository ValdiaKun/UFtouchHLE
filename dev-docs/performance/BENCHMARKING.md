# UFtouchHLE benchmarking: instrumentation, reports and A/B comparison

Companion to [LOW_END_ANDROID_PLAN.md](LOW_END_ANDROID_PLAN.md). This describes
what the built-in instrumentation measures, how to collect a baseline, and how
to decide whether a change is a real improvement. **No performance numbers are
recorded here**: there are no baseline measurements from physical hardware yet.

## What is measured

Enable with `--performance-test=observe` (see `OPTIONS_HELP.txt`). It is off by
default and costs nothing when off. `observe` changes no setting (resolution,
filtering, frame limit and device profile stay as configured), so it is the mode
for baselines and code A/B tests. The older `reference`, `low-resolution` and
`*-quiet-log` modes force fixed comparison settings and answer different
questions (resolution and logging cost).

A *window* is about 5 seconds of frames. For each window the emulator appends:

- `touchHLE_performance_tests.txt`: the human-readable line (unchanged format,
  with `frame_p99_ms` appended at the end).
- `touchHLE_performance.csv` and `touchHLE_performance.jsonl`: one structured
  row per window with identical content. The CSV header is
  `CSV_HEADER` in `src/performance_report.rs`.

Files are written to the user data directory (on Android,
`/storage/emulated/0/Android/data/org.touchhleuf.android/files/`).

| Column | Meaning |
|---|---|
| `session` | Start time of the emulator run (ms since epoch). One launch = one run. |
| `window_index` | Window number within the run, from 0. |
| `build` | `git describe --tags --always` at build time (identifies the commit). |
| `mode`, `device_model`, `resolution` | Test mode, emulated device model, emulated pixel size. |
| `measurement_s`, `frames` | Wall time and number of frames in the window. |
| `average_fps` | `frames / measurement_s`. |
| `median/p95/p99/max_frame_ms` | Nearest-rank percentiles of frame-to-frame intervals. |
| `fps_1pct_low` | Average FPS over the slowest 1% of frames (`1000 / mean of the worst ceil(1%) intervals`). Empty when the window has fewer than 100 frames. |
| `cpu_ms_per_frame` | Host time spent running guest code. |
| `gl_ms_per_frame`, `gl_calls_per_frame` | Time inside host GL calls (driver *submission*, not asynchronous GPU execution) and call count. |
| `present_ms_per_frame` | Time inside `presentRenderbuffer:`; includes `readback_ms_per_frame`. |
| `requested_wait_ms_per_frame` | Sleep requested by the frame limiter. |
| `slow_frames` | Frames that took the slow readback present path. |
| `svc_per_frame` | JIT exits for SVCs per frame, i.e. guest-to-host function calls (and returns to the host). |
| `mem_callbacks_per_frame` | Memory read + write *callbacks* per frame. These are the slow path: with fast memory on, only null-page accesses should land here, so a large value means the inline page-table path is being bypassed. |
| `code_reads_per_frame` | Code words read while dynarmic translates blocks. High values after warm-up indicate JIT recompilation churn. |
| `cache_invalidations_per_frame`, `context_swaps_per_frame`, `run_calls_per_frame` | Calls to invalidate the JIT cache, guest thread context switches, and `run_or_step` calls. |
| `peak_rss_mb` | Process peak resident memory (`VmHWM`), Linux/Android only. Empty elsewhere. |

Timers overlap and must not be added together. CPU and GL timing add some
overhead of their own, so only compare runs that both use the instrumentation.
Gaps longer than 2 seconds (loading, pause) restart the window instead of
counting as a slow frame.

### What this does *not* measure

- Intervals are taken at the guest's `presentRenderbuffer:` calls: they are
  frames the **guest generated**, not frames the Android compositor displayed.
  If the host drops or delays frames after that point, only a display-side
  capture shows it (Perfetto / `dumpsys SurfaceFlinger` / `dumpsys gfxinfo`
  on a device). Never read a display refresh rate as emulation FPS.
- No GPU execution time, CPU frequency, battery or temperature. Record
  thermal state by hand or with Perfetto for now.
- Games that draw with UIKit/Core Graphics rather than EAGL do not go through
  `presentRenderbuffer:` and produce no windows.

## Collecting a baseline

1. Build the commit under test and note `build` from the report (do not trust
   the file name of an APK; the report shows what code actually ran).
2. Enable it for the game in `touchHLE_options.txt` (per-app line, see that
   file's header), e.g. `com.example.game: --performance-test=observe`.
3. Same device, game version, save, scene, settings and charge state for every
   run. Warm up for 2 minutes, then sample at least 5 minutes. Launch the
   emulator **at least 3 times** per build: variability is measured across
   launches. Keep cold-start runs separate from sustained ones.
4. Pull `touchHLE_performance.csv` (for example with `adb pull`) and keep it
   with notes: device model/SoC/RAM, Android version, game and version,
   scene, thermal conditions. These are not recorded by the emulator.

## Analysing runs

`dev-scripts/perf-report.py` uses only the Python standard library.

```
python3 dev-scripts/perf-report.py validate  touchHLE_performance.csv
python3 dev-scripts/perf-report.py summarize --skip-seconds 120 base.csv
python3 dev-scripts/perf-report.py compare   --skip-seconds 120 base.csv cand.csv
```

- `validate` rejects malformed rows and impossible values (for example p99
  above max).
- `summarize` reduces each launch to one value per metric (median over its
  windows; max for peak RSS), then reports median, min, max and coefficient of
  variation across launches. `--skip-seconds` drops warm-up per launch.
- `compare` judges each metric against a noise threshold of
  `max(2%, 2 x the larger coefficient of variation)`. Smaller changes are
  "within noise". With fewer than 3 launches on either side it reports
  "inconclusive". It warns if mode, device or resolution differ, or if both
  files are the same build. `--fail-on-regression` makes regressions exit 1.

A change that is neutral, harmful or within noise should be reverted or kept
behind an experimental setting.

## Tests

```
# Statistics and report formatting (std only, no emulator build needed):
rustfmt --edition 2021 --check src/performance_report.rs
rustc --edition 2021 --test -D warnings src/performance_report.rs -o /tmp/report_tests && /tmp/report_tests

# Full unit tests (needs the normal build environment):
cargo test --release --lib performance

# Report tooling:
python3 dev-scripts/test_perf_report.py
```

`dev-scripts/test_perf_report.py` also checks that the Python column list
matches `CSV_HEADER` in the Rust source, so the two cannot drift apart.

## Status

| Item | State |
|---|---|
| Statistics/report module (`src/performance_report.rs`) | Tested on host (standalone `rustc --test`) |
| `observe` mode and structured output in `src/performance.rs` | Type-checked and unit/end-to-end tested against stand-ins for the emulator types; **not yet built inside the real crate** |
| `dev-scripts/perf-report.py` | Tested on host |
| Android build, on-device behaviour, any FPS/frame-time baseline | **Pending hardware validation** |
