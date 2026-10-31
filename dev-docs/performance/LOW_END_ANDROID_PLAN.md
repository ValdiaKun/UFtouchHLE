# UFtouchHLE: Low-End Android Performance Plan

Status: **planning and baseline setup**. This document describes experiments, **not measured speedups**.

## Goal and scope

Improve UFtouchHLE's real-world responsiveness on entry-level ARM64 Android devices without sacrificing game compatibility, correctness, save-data integrity, or stability. Work in `perf-development` and submit reviewed PRs to `trunk`; do not change `trunk` directly. Treat this fork as independent from upstream touchHLE; follow the existing `AGENTS.md`, licenses, and rules against submitting AI-generated work to upstream.

Do not assume that a lower render resolution or a new graphics backend will always be faster. UFtouchHLE runs **32-bit ARM games** on an ARM64 host and uses Dynarmic for CPU translation; improving emulator performance does not expand the set of supported guest architectures.

## Confirmed starting points in the fork

- `src/cpu/dynarmic_wrapper/lib.cpp` provides Dynarmic A32 JIT callbacks, memory read/write paths, an optional direct-memory fast path, and an existing `check_halt_on_memory_access` setting. **Do not disable safety/correctness checks without controlled tests.**
- `src/cpu/dynarmic_wrapper/lib.rs` bridges the JIT to Rust.
- `src/mem.rs` owns guest memory abstractions (per the current `AGENTS.md`).
- `src/gles/` contains GLES-related code; inspect its actual layout before selecting draw-call or state-change optimizations.
- `android/`, `dev-docs/building.md`, and `dev-scripts/build-android-uf.sh` provide the existing Android build path; the release script asks for a local signing secret. **Never put signing passwords or keystores in GitHub or CI.**
- Existing `AGENTS.md` documents tests and packaging. The upstream release workflow referenced in some documentation is absent from this fork.

## Phase 0 — reproducible baseline (first mandatory PR)

1. Record the current `trunk` commit and source/release version, and establish a working ARM64 Android **debug** build before changing core code. Confirm that native `libtouchHLE` and assets actually correspond to the checked-out commit; investigate cached JNI libraries.
2. Run available lint/format/unit tests (`dev-scripts/lint.sh`, `dev-scripts/format.sh --check`, `cargo test --release --lib`, where supported). State which tests could not run and why.
3. Implement optional, low-overhead performance instrumentation, OFF by default, around: frame start/end, guest CPU/JIT execution, host API calls, GLES draw/state/texture operations, audio, and resource loading. Use monotonic clocks, avoid per-frame allocations in hot paths, and avoid logging every guest call. Keep any measurement overhead quantifiable.
4. Produce a CSV/JSON results format and a reproducible benchmark runner or recording instructions. Do not claim FPS improvements before A/B measurements on physical hardware.

## Benchmark protocol

For each **legally obtained**, compatible test game, record bundle ID/version/hash, save state and exact scene, game settings/device profile/resolution, render backend, device model/SoC/RAM, Android version, thermals, build commit, and profiler version.

- Use the same device, scene, settings, battery/charging state and ambient conditions for baseline and candidate; reboot/cool down if needed.
- Warm up for at least 2 minutes, then sample at least 5 minutes; repeat 3 times when practical and report variability. Include an extended 15–20-minute thermal run for finalists.
- Record median, p95 and p99 frame time in ms, presented FPS and 1% low FPS where valid, CPU load, peak RSS / RAM, dropped frames, startup time, crash/hang count, and temperature/thermal-throttling signals when available.
- Use Android Perfetto and Simpleperf for CPU/scheduling. Add GPU profiling where the device and driver support it.
- Maintain a compatibility matrix for input, audio, display, loading, resume/background, save/import/export, and game-specific rendering fixes. Never commit copyrighted IPA/game data or user saves.

Suggested CSV columns:

`commit,device,android_version,game_bundle_id,game_version,scene,device_profile,render_backend,resolution,run_index,warmup_s,measurement_s,median_frame_ms,p95_frame_ms,p99_frame_ms,average_fps,fps_1pct_low,peak_rss_mb,cpu_pct,thermal_status,crashes,correctness_notes`

## Phase 1 — investigate measured bottlenecks (one variable per PR)

1. **CPU/JIT:** benchmark Dynarmic callbacks, fault handling, page-table/fastmem behavior, JIT invalidation and cache. Compare changes to existing fastmem rather than replacing Dynarmic. Preserve memory permissions, null-page faults and guest correctness.
2. **GLES:** measure redundant state calls, shader compilation/cache hits, buffer/texture uploads, draw counts, driver stalls and unnecessary composition. Cache only where state identity and invalidation are proven; validate on Mali, Adreno, and PowerVR where possible.
3. **Frame pacing:** test vsync and a user-selectable 30 FPS cap only for games where they make sense; report smoothness, input latency and actual frame times. Do not claim a frame limiter increases peak FPS.
4. **RAM and load:** profile texture and decoded-asset lifetime, allocator overhead, caches and temporary buffers. Avoid allocator swaps without before/after evidence. Guard against OOM on 3–4 GB devices.
5. **Thermal:** consider Android ADPF/thermal signals only where supported, with no privileged commands, root requirements or forced clock changes.
6. **Build:** consider release optimization tuning and profile-guided optimization after a reproducible build/benchmark pipeline exists; preserve correct unwinding, symbols when profiling and deterministic dependency revisions.
7. **Experimental:** ANGLE, GL4ES-derived strategies or alternate render backends only as isolated opt-in experiments after profiling demonstrates a reason. Respect licenses and never promise they will outperform GLES.

## Success gates

- **Correctness first:** no new known crashes, memory faults, save corruption, rendering/audio/input regressions, or ABI/packaging breakage against the baseline scenes.
- **Measurable benefit:** prefer changes with repeatable improvement in p95/p99 frame time, 1% lows, RAM use, or sustained temperature at comparable visual quality. If gains are within noise, revert or keep experimental.
- **Repeatability:** preserve baseline logs, hardware data and benchmark methodology; separate cold start from sustained gameplay.
- **Deployment:** no unsigned-release confusion, no copied upstream signing key, no auto-updating users into experimental builds, and no automatic merging into `trunk`.

## Next deliverable

Create a small, isolated PR containing the benchmark schema, optional timing/instrumentation and test instructions. Capture a baseline on a real budget Android device **before** changing Dynarmic or rendering logic.

Useful references: `AGENTS.md`, `dev-docs/building.md`, `dev-scripts/build-android-uf.sh`, and this project's source tree.
