/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Opt-in performance comparisons. Timings are host wall time, not GPU queries.

use crate::options::Options;
use crate::performance_report::{self, percentile};
use crate::Environment;
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use touchHLE_dynarmic_wrapper::touchHLE_CpuCounters;

pub const REPORT_FILE: &str = "touchHLE_performance_tests.txt";
/// Structured per-window results (CSV with [performance_report::CSV_HEADER],
/// and JSON Lines), written next to [REPORT_FILE].
pub const CSV_FILE: &str = "touchHLE_performance.csv";
pub const JSONL_FILE: &str = "touchHLE_performance.jsonl";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Measure only: every option (resolution, filtering, frame limit, ...)
    /// is left exactly as the user or device profile configured it. Use this
    /// for baselines and A/B comparisons of code changes.
    Observe,
    Reference,
    LowResolution,
    QuietLog,
    LowResolutionQuietLog,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "observe" => Ok(Self::Observe),
            "reference" => Ok(Self::Reference),
            "low-resolution" => Ok(Self::LowResolution),
            "quiet-log" => Ok(Self::QuietLog),
            "low-resolution-quiet-log" => Ok(Self::LowResolutionQuietLog),
            _ => Err(format!("Unknown performance test {value:?}")),
        }
    }
    pub fn quiet(self) -> bool {
        matches!(self, Self::QuietLog | Self::LowResolutionQuietLog)
    }
    pub fn low_resolution(self) -> bool {
        matches!(self, Self::LowResolution | Self::LowResolutionQuietLog)
    }
}

/// Applied after device selection so all comparisons use the same model and
/// canvas. Only Retina and short-read logging differ between the four modes.
pub fn configure(options: &mut Options) {
    let Some(mode) = options.performance_test else {
        return;
    };
    if mode == Mode::Observe {
        return;
    }
    options.retina = !mode.low_resolution();
    options.device_screen_scale = Some(if mode.low_resolution() { 1 } else { 2 });
    options.scale_hack = std::num::NonZeroU32::new(1).unwrap();
    options.fps_limit = Some(60.0);
    options.filter_mode = crate::options::FilterMode::Sharp;
}

#[derive(Default)]
pub struct State {
    pub cpu: Duration,
    pub gl: Duration,
    pub gl_calls: u64,
    pub short_reads: u64,
    last_frame: Option<Instant>,
    start: Option<Instant>,
    intervals: Vec<f64>,
    present: Duration,
    readback: Duration,
    requested_wait: Duration,
    slow_frames: u64,
    session: Option<u128>,
    reported_io_error: bool,
    /// Number of structured windows written so far this session.
    windows: u64,
    /// CPU event counters at the start of the current window.
    window_counters: touchHLE_CpuCounters,
}

/// Append compact audio events to the same report as the performance tests.
pub fn audio_event(line: &str) {
    log!("{}", line);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(crate::paths::user_data_base_path().join(REPORT_FILE))?;
        writeln!(file, "{line}")
    })();
    if let Err(error) = result {
        log!("Could not append audio test report: {}", error);
    }
}

/// Append one line to a file in the user data directory, writing `header`
/// first if the file is new or empty.
fn append_line(file_name: &str, header: Option<&str>, line: &str) -> std::io::Result<()> {
    let path = crate::paths::user_data_base_path().join(file_name);
    let is_empty = std::fs::metadata(&path).map_or(true, |m| m.len() == 0);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if let (true, Some(header)) = (is_empty, header) {
        writeln!(file, "{header}")?;
    }
    writeln!(file, "{line}")
}

impl State {
    /// Write one measurement window to the CSV and JSON Lines reports.
    fn append_structured(&mut self, csv_row: &str, json_line: &str) {
        let result = append_line(CSV_FILE, Some(performance_report::CSV_HEADER), csv_row)
            .and_then(|()| append_line(JSONL_FILE, None, json_line));
        if let Err(error) = result {
            if !self.reported_io_error {
                self.reported_io_error = true;
                log!("Could not append structured performance report: {}", error);
            }
        }
    }

    fn append(&mut self, line: &str) {
        log!("{}", line);
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(crate::paths::user_data_base_path().join(REPORT_FILE))?;
            writeln!(file, "{line}")
        })();
        if let Err(error) = result {
            if !self.reported_io_error {
                self.reported_io_error = true;
                log!("Could not append performance report: {}", error);
            }
        }
    }

    fn clear_window(&mut self, now: Instant, counters: touchHLE_CpuCounters) {
        self.window_counters = counters;
        self.cpu = Duration::ZERO;
        self.gl = Duration::ZERO;
        self.gl_calls = 0;
        self.short_reads = 0;
        self.present = Duration::ZERO;
        self.readback = Duration::ZERO;
        self.requested_wait = Duration::ZERO;
        self.slow_frames = 0;
        self.intervals.clear();
        self.start = Some(now);
        self.last_frame = Some(now);
    }
}

/// Record only actual presentations. The readback component is included in
/// present time; CPU, GL calls and requested pacing are separate measurements.
/// GL calls measure driver submission/stalls, not asynchronous GPU execution.
pub fn frame(
    env: &mut Environment,
    started: Instant,
    slow: bool,
    readback: Duration,
    requested_wait: Option<Duration>,
) {
    let Some(mode) = env.options.performance_test else {
        return;
    };
    let now = Instant::now();
    let model = env.options.device_model.unwrap_or("iPhone1,1");
    let size = env.window.as_ref().unwrap().size_unrotated_unscaled();
    let scale = env.options.screen_scale();
    // A cheap copy of the CPU wrapper's event counters; only reached when a
    // performance test mode is active.
    let cpu_counters = env.cpu.counters();
    let state = &mut env.framework_state.performance;
    if state.session.is_none() {
        let session = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        state.session = Some(session);
        // Observe mode leaves the configuration untouched, so it must not
        // claim the fixed comparison settings the other modes apply.
        let config = if mode == Mode::Observe {
            "config=unmodified"
        } else {
            "scale=1 fps_limit=60"
        };
        state.append(&format!(
            "\nSESSION {session} mode={mode:?} version={} model={model} pixels={}x{} {config}; times=host_wall_ms; readback_included_in_present; timings_may_overlap_do_not_add; diagnostic_overhead_present; GL_is_submission_not_GPU; use_same_gameplay_scene_30s",
            crate::VERSION, size.0 * scale, size.1 * scale,
        ));
    }
    // Do not count a long loading/pause gap as a normal gameplay frame.
    let Some(previous) = state.last_frame else {
        state.clear_window(now, cpu_counters);
        return;
    };
    let interval = now.saturating_duration_since(previous);
    if interval > Duration::from_secs(2) {
        state.append("Sampling restarted after a loading/pause gap longer than 2s.");
        state.clear_window(now, cpu_counters);
        return;
    }
    state.last_frame = Some(now);
    state.intervals.push(interval.as_secs_f64() * 1000.0);
    state.present += now.saturating_duration_since(started);
    state.readback += readback;
    state.requested_wait += requested_wait.unwrap_or_default();
    state.slow_frames += u64::from(slow);
    let elapsed = now.saturating_duration_since(state.start.unwrap());
    if elapsed < Duration::from_secs(5) {
        return;
    }
    let frames = state.intervals.len() as f64;
    let per_frame = |time: Duration| time.as_secs_f64() * 1000.0 / frames;
    let line = format!(
        "session={} mode={mode:?} seconds={:.1} frames={} FPS={:.2} frame_p50_ms={:.2} frame_p95_ms={:.2} CPU_ms/frame={:.2} GL_ms/frame={:.2} GL_calls/frame={:.1} present_ms/frame={:.2} readback_path_ms/frame={:.2} slow_frames={} requested_wait_ms/frame={:.2} short_reads={} frame_p99_ms={:.2}",
        state.session.unwrap(), elapsed.as_secs_f64(), state.intervals.len(),
        frames / elapsed.as_secs_f64(), percentile(&state.intervals, 0.5),
        percentile(&state.intervals, 0.95), per_frame(state.cpu), per_frame(state.gl),
        state.gl_calls as f64 / frames, per_frame(state.present), per_frame(state.readback),
        state.slow_frames, per_frame(state.requested_wait), state.short_reads,
        percentile(&state.intervals, 0.99),
    );
    state.append(&line);
    if let Some(stats) = performance_report::summarize(&state.intervals, elapsed.as_secs_f64()) {
        let cpu_events = cpu_counters.since(&state.window_counters);
        let per_frame_count = |count: u64| count as f64 / frames;
        let subsystems = performance_report::Subsystems {
            cpu_ms: per_frame(state.cpu),
            gl_ms: per_frame(state.gl),
            gl_calls: state.gl_calls as f64 / frames,
            present_ms: per_frame(state.present),
            readback_ms: per_frame(state.readback),
            requested_wait_ms: per_frame(state.requested_wait),
            slow_frames: state.slow_frames,
            svc_exits: per_frame_count(cpu_events.svc_exits),
            mem_callbacks: per_frame_count(cpu_events.callback_reads + cpu_events.callback_writes),
            code_reads: per_frame_count(cpu_events.code_reads),
            cache_invalidations: per_frame_count(cpu_events.cache_invalidations),
            context_swaps: per_frame_count(cpu_events.context_swaps),
            run_calls: per_frame_count(cpu_events.run_calls),
        };
        let mode_name = format!("{mode:?}");
        let resolution = format!("{}x{}", size.0 * scale, size.1 * scale);
        let run = performance_report::RunInfo {
            session: state.session.unwrap(),
            window_index: state.windows,
            build: crate::VERSION,
            mode: &mode_name,
            device_model: model,
            resolution: &resolution,
        };
        let rss = performance_report::peak_rss_mb();
        let csv_row = performance_report::csv_row(&run, &stats, &subsystems, rss);
        let json_line = performance_report::json_line(&run, &stats, &subsystems, rss);
        state.windows += 1;
        state.append_structured(&csv_row, &json_line);
    }
    state.clear_window(now, cpu_counters);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observe_mode_leaves_every_option_untouched() {
        let mode = Mode::parse("observe").unwrap();
        assert_eq!(mode, Mode::Observe);
        assert!(!mode.quiet() && !mode.low_resolution());
        let mut options = Options {
            performance_test: Some(mode),
            device_model: Some("iPhone4,1"),
            retina: true,
            device_screen_scale: Some(2),
            fps_limit: None,
            ..Options::default()
        };
        let scale_hack = options.scale_hack;
        let filter_mode = options.filter_mode;
        configure(&mut options);
        assert!(options.retina);
        assert_eq!(options.device_screen_scale, Some(2));
        assert_eq!(options.fps_limit, None);
        assert_eq!(options.scale_hack, scale_hack);
        assert_eq!(options.filter_mode, filter_mode);
    }
    #[test]
    fn modes_change_only_the_two_comparison_variables() {
        for (name, low, quiet) in [
            ("reference", false, false),
            ("low-resolution", true, false),
            ("quiet-log", false, true),
            ("low-resolution-quiet-log", true, true),
        ] {
            let mode = Mode::parse(name).unwrap();
            assert_eq!(mode.low_resolution(), low);
            assert_eq!(mode.quiet(), quiet);
            let mut options = Options {
                performance_test: Some(mode),
                device_model: Some("iPhone4,1"),
                ..Options::default()
            };
            configure(&mut options);
            assert_eq!(options.device_model, Some("iPhone4,1"));
            assert_eq!(options.retina, !low);
            assert_eq!(options.scale_hack.get(), 1);
        }
        assert!(Mode::parse("unknown").is_err());
    }
    #[test]
    fn frame_percentiles_and_resets_are_deterministic() {
        assert_eq!(percentile(&[10.0, 50.0, 20.0, 30.0, 40.0], 0.5), 30.0);
        assert_eq!(percentile(&[10.0, 50.0, 20.0, 30.0, 40.0], 0.95), 50.0);
        let now = Instant::now();
        let mut state = State {
            cpu: Duration::from_secs(1),
            short_reads: 12,
            session: Some(123),
            ..State::default()
        };
        let counters = touchHLE_CpuCounters {
            svc_exits: 77,
            ..Default::default()
        };
        state.clear_window(now, counters);
        assert_eq!(state.window_counters.svc_exits, 77);
        assert_eq!(state.cpu, Duration::ZERO);
        assert_eq!(state.short_reads, 0);
        assert_eq!(state.session, Some(123));
        assert_eq!(state.last_frame, Some(now));
    }
}
