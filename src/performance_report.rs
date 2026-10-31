/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Pure statistics and structured-report formatting for [crate::performance].
//!
//! This module deliberately depends only on `std` (no `crate::` items), so its
//! tests can be run on their own with
//! `rustc --edition 2021 --test src/performance_report.rs` without building
//! the whole emulator.
//!
//! All times are host wall-clock milliseconds measured around the guest's
//! `presentRenderbuffer:` calls. They describe frames the guest *generated*,
//! not frames the Android compositor actually displayed. Display-side
//! timestamps must be captured separately (see
//! `dev-docs/performance/BENCHMARKING.md`).

/// Smallest number of frames for which a "1% low" is meaningful: below this
/// the slowest 1% is less than one whole frame.
pub const MIN_FRAMES_FOR_1PCT_LOW: usize = 100;

/// Nearest-rank percentile of `values` (`fraction` in 0.0..=1.0). Returns 0.0
/// for an empty slice. NaN values sort last (`f64::total_cmp`).
pub fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
}

/// Summary of one measurement window of frame intervals.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameStats {
    pub frames: usize,
    pub window_seconds: f64,
    /// Frames divided by window wall time.
    pub average_fps: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    /// Average FPS over the slowest 1% of frames (1000 / mean of the worst
    /// `ceil(1%)` intervals). `None` if fewer than
    /// [MIN_FRAMES_FOR_1PCT_LOW] frames were recorded.
    pub fps_1pct_low: Option<f64>,
}

/// Summarise frame intervals (milliseconds). `None` if there are no frames or
/// the window length is not positive.
pub fn summarize(intervals_ms: &[f64], window_seconds: f64) -> Option<FrameStats> {
    if intervals_ms.is_empty() || window_seconds.is_nan() || window_seconds <= 0.0 {
        return None;
    }
    let frames = intervals_ms.len();
    let fps_1pct_low = (frames >= MIN_FRAMES_FOR_1PCT_LOW).then(|| {
        let mut sorted = intervals_ms.to_vec();
        sorted.sort_by(f64::total_cmp);
        let worst = ((frames as f64 * 0.01).ceil() as usize).max(1);
        let mean = sorted[frames - worst..].iter().sum::<f64>() / worst as f64;
        1000.0 / mean
    });
    Some(FrameStats {
        frames,
        window_seconds,
        average_fps: frames as f64 / window_seconds,
        median_ms: percentile(intervals_ms, 0.5),
        p95_ms: percentile(intervals_ms, 0.95),
        p99_ms: percentile(intervals_ms, 0.99),
        max_ms: percentile(intervals_ms, 1.0),
        fps_1pct_low,
    })
}

/// Identity of the run that a window belongs to.
pub struct RunInfo<'a> {
    pub session: u128,
    pub window_index: u64,
    /// `crate::VERSION` (`git describe --tags --always` at build time).
    pub build: &'a str,
    pub mode: &'a str,
    pub device_model: &'a str,
    pub resolution: &'a str,
}

/// Per-frame averages of the subsystem timers over one window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Subsystems {
    pub cpu_ms: f64,
    pub gl_ms: f64,
    pub gl_calls: f64,
    pub present_ms: f64,
    pub readback_ms: f64,
    pub requested_wait_ms: f64,
    pub slow_frames: u64,
}

pub const CSV_HEADER: &str = "session,window_index,build,mode,device_model,resolution,measurement_s,frames,average_fps,median_frame_ms,p95_frame_ms,p99_frame_ms,max_frame_ms,fps_1pct_low,cpu_ms_per_frame,gl_ms_per_frame,gl_calls_per_frame,present_ms_per_frame,readback_ms_per_frame,requested_wait_ms_per_frame,slow_frames,peak_rss_mb";

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number for CSV: empty when unavailable or non-finite.
fn csv_num(value: Option<f64>) -> String {
    match value {
        Some(v) if v.is_finite() => format!("{v:.3}"),
        _ => String::new(),
    }
}

/// A number for JSON: `null` when unavailable or non-finite.
fn json_num(value: Option<f64>) -> String {
    match value {
        Some(v) if v.is_finite() => format!("{v:.3}"),
        _ => "null".to_string(),
    }
}

/// One CSV data row matching [CSV_HEADER].
pub fn csv_row(
    run: &RunInfo,
    stats: &FrameStats,
    sub: &Subsystems,
    peak_rss_mb: Option<f64>,
) -> String {
    [
        run.session.to_string(),
        run.window_index.to_string(),
        csv_field(run.build),
        csv_field(run.mode),
        csv_field(run.device_model),
        csv_field(run.resolution),
        csv_num(Some(stats.window_seconds)),
        stats.frames.to_string(),
        csv_num(Some(stats.average_fps)),
        csv_num(Some(stats.median_ms)),
        csv_num(Some(stats.p95_ms)),
        csv_num(Some(stats.p99_ms)),
        csv_num(Some(stats.max_ms)),
        csv_num(stats.fps_1pct_low),
        csv_num(Some(sub.cpu_ms)),
        csv_num(Some(sub.gl_ms)),
        csv_num(Some(sub.gl_calls)),
        csv_num(Some(sub.present_ms)),
        csv_num(Some(sub.readback_ms)),
        csv_num(Some(sub.requested_wait_ms)),
        sub.slow_frames.to_string(),
        csv_num(peak_rss_mb),
    ]
    .join(",")
}

/// One JSON object (single line, for JSONL) with the same content as
/// [csv_row].
pub fn json_line(
    run: &RunInfo,
    stats: &FrameStats,
    sub: &Subsystems,
    peak_rss_mb: Option<f64>,
) -> String {
    format!(
        "{{\"session\":{},\"window_index\":{},\"build\":{},\"mode\":{},\"device_model\":{},\"resolution\":{},\"measurement_s\":{},\"frames\":{},\"average_fps\":{},\"median_frame_ms\":{},\"p95_frame_ms\":{},\"p99_frame_ms\":{},\"max_frame_ms\":{},\"fps_1pct_low\":{},\"cpu_ms_per_frame\":{},\"gl_ms_per_frame\":{},\"gl_calls_per_frame\":{},\"present_ms_per_frame\":{},\"readback_ms_per_frame\":{},\"requested_wait_ms_per_frame\":{},\"slow_frames\":{},\"peak_rss_mb\":{}}}",
        run.session,
        run.window_index,
        json_string(run.build),
        json_string(run.mode),
        json_string(run.device_model),
        json_string(run.resolution),
        json_num(Some(stats.window_seconds)),
        stats.frames,
        json_num(Some(stats.average_fps)),
        json_num(Some(stats.median_ms)),
        json_num(Some(stats.p95_ms)),
        json_num(Some(stats.p99_ms)),
        json_num(Some(stats.max_ms)),
        json_num(stats.fps_1pct_low),
        json_num(Some(sub.cpu_ms)),
        json_num(Some(sub.gl_ms)),
        json_num(Some(sub.gl_calls)),
        json_num(Some(sub.present_ms)),
        json_num(Some(sub.readback_ms)),
        json_num(Some(sub.requested_wait_ms)),
        sub.slow_frames,
        json_num(peak_rss_mb),
    )
}

/// Extract the peak resident set size (`VmHWM`, in MiB) from the text of
/// `/proc/self/status`.
// Only called on platforms that have /proc; still unit-tested everywhere.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub fn parse_vm_hwm_mb(status: &str) -> Option<f64> {
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let mut parts = line["VmHWM:".len()..].split_whitespace();
    let value: f64 = parts.next()?.parse().ok()?;
    match parts.next() {
        Some("kB") | Some("KB") | Some("kb") => Some(value / 1024.0),
        _ => None,
    }
}

/// Peak resident set size of this process in MiB, where the OS exposes it
/// (Linux and Android). `None` elsewhere or on read failure.
pub fn peak_rss_mb() -> Option<f64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        parse_vm_hwm_mb(&std::fs::read_to_string("/proc/self/status").ok()?)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal quote-aware CSV field splitter (RFC 4180 subset).
    fn split_csv(row: &str) -> Vec<String> {
        let (mut fields, mut cur, mut quoted) = (Vec::new(), String::new(), false);
        let mut chars = row.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, quoted) {
                ('"', true) if chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                ('"', _) => quoted = !quoted,
                (',', false) => fields.push(std::mem::take(&mut cur)),
                (c, _) => cur.push(c),
            }
        }
        fields.push(cur);
        fields
    }

    fn run() -> RunInfo<'static> {
        RunInfo {
            session: 1700000000000,
            window_index: 3,
            build: "v0.3.21-5-gabc1234",
            mode: "Observe",
            device_model: "iPhone4,1",
            resolution: "960x640",
        }
    }

    #[test]
    fn percentile_is_nearest_rank() {
        let v = [10.0, 50.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&v, 0.5), 30.0);
        assert_eq!(percentile(&v, 0.95), 50.0);
        assert_eq!(percentile(&v, 1.0), 50.0);
        assert_eq!(percentile(&v, 0.0), 10.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn summarize_rejects_empty_and_bad_windows() {
        assert_eq!(summarize(&[], 5.0), None);
        assert_eq!(summarize(&[16.0], 0.0), None);
        assert_eq!(summarize(&[16.0], f64::NAN), None);
    }

    #[test]
    fn steady_60fps_stats() {
        let intervals = vec![1000.0 / 60.0; 300];
        let s = summarize(&intervals, 5.0).unwrap();
        assert_eq!(s.frames, 300);
        assert!((s.average_fps - 60.0).abs() < 1e-9);
        assert!((s.median_ms - 16.6667).abs() < 1e-3);
        assert!((s.p99_ms - s.median_ms).abs() < 1e-9);
        assert!((s.fps_1pct_low.unwrap() - 60.0).abs() < 1e-6);
    }

    #[test]
    fn one_percent_low_reflects_the_slowest_frames() {
        // 200 frames: 198 at 10 ms, 2 at 100 ms. Slowest 1% = ceil(2.0) = 2
        // frames, mean 100 ms -> 10 FPS.
        let mut intervals = vec![10.0; 198];
        intervals.extend([100.0, 100.0]);
        let s = summarize(&intervals, 4.0).unwrap();
        assert!((s.fps_1pct_low.unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(s.max_ms, 100.0);
        // p99 of 200 samples is the 198th sorted value, still 10 ms.
        assert_eq!(s.p99_ms, 10.0);
        assert_eq!(s.median_ms, 10.0);
    }

    #[test]
    fn one_percent_low_needs_enough_frames() {
        let s = summarize(&vec![10.0; MIN_FRAMES_FOR_1PCT_LOW - 1], 1.0).unwrap();
        assert_eq!(s.fps_1pct_low, None);
        let s = summarize(&vec![10.0; MIN_FRAMES_FOR_1PCT_LOW], 1.0).unwrap();
        assert!(s.fps_1pct_low.is_some());
    }

    #[test]
    fn csv_row_matches_header_column_count() {
        let stats = summarize(&vec![16.0; 150], 5.0).unwrap();
        let sub = Subsystems {
            cpu_ms: 4.0,
            gl_ms: 2.5,
            gl_calls: 120.0,
            present_ms: 1.0,
            readback_ms: 0.0,
            requested_wait_ms: 0.5,
            slow_frames: 0,
        };
        let columns = CSV_HEADER.split(',').count();
        // The model "iPhone4,1" contains a comma, so it must be quoted and
        // still count as one field.
        let row = csv_row(&run(), &stats, &sub, Some(512.25));
        let fields = split_csv(&row);
        assert_eq!(fields.len(), columns, "{row}");
        assert_eq!(fields[4], "iPhone4,1");
        assert_eq!(fields.last().unwrap(), "512.250");
        // Unavailable RSS is an empty trailing field, not "NaN"/"None".
        let fields = split_csv(&csv_row(&run(), &stats, &sub, None));
        assert_eq!(fields.len(), columns);
        assert_eq!(fields.last().unwrap(), "");
    }

    #[test]
    fn csv_quotes_fields_that_need_it() {
        let mut r = run();
        r.device_model = "weird,\"model\"";
        let stats = summarize(&vec![16.0; 10], 1.0).unwrap();
        let row = csv_row(&r, &stats, &Subsystems::default(), None);
        assert!(row.contains("\"weird,\"\"model\"\"\""), "{row}");
        assert_eq!(csv_field("plain"), "plain");
    }

    #[test]
    fn json_line_escapes_and_uses_null() {
        let mut r = run();
        r.build = "a\"b\\c\n";
        let stats = summarize(&vec![16.0; 10], 1.0).unwrap(); // <100 frames
        let line = json_line(&r, &stats, &Subsystems::default(), None);
        assert!(line.starts_with('{') && line.ends_with('}'));
        assert!(!line.contains('\n'));
        assert!(line.contains("\"build\":\"a\\\"b\\\\c\\n\""), "{line}");
        assert!(line.contains("\"fps_1pct_low\":null"));
        assert!(line.contains("\"peak_rss_mb\":null"));
    }

    #[test]
    fn non_finite_numbers_never_leak() {
        assert_eq!(csv_num(Some(f64::NAN)), "");
        assert_eq!(csv_num(Some(f64::INFINITY)), "");
        assert_eq!(json_num(Some(f64::NEG_INFINITY)), "null");
        assert_eq!(json_num(Some(1.5)), "1.500");
    }

    #[test]
    fn vm_hwm_parsing() {
        let status =
            "Name:\ttouchHLE\nVmPeak:\t  900000 kB\nVmHWM:\t  524288 kB\nVmRSS:\t  400000 kB\n";
        assert_eq!(parse_vm_hwm_mb(status), Some(512.0));
        assert_eq!(parse_vm_hwm_mb("VmRSS:\t1 kB\n"), None);
        assert_eq!(parse_vm_hwm_mb("VmHWM:\tabc kB\n"), None);
        assert_eq!(parse_vm_hwm_mb("VmHWM:\t12\n"), None); // no unit
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn peak_rss_is_available_on_linux() {
        assert!(peak_rss_mb().is_some_and(|mb| mb > 0.0));
    }
}
