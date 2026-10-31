# Independent UFtouchHLE QA (development branch)

This project separates **implementation** from **independent validation**:

- Claude or other coding agents implement emulator changes on `perf-development`.
- The existing `.github/workflows/perf-validate.yml` runs standalone reporting tests, host compilation/unit tests and lint.
- `.github/workflows/qa-android.yml` independently builds a **debug** Android ARM64 APK and validates its package structure, native libraries, ELF architecture/alignment, manifest identifiers and debug signature.
- A reviewer checks test results and investigates regressions before any release decision.

All work remains on development branches. `trunk` is not modified by these workflows.

## Running CI

The Android QA workflow is triggered by pushes to `perf-development`, pushes to the setup branch `qa/android-validation`, and pull requests targeting `perf-development`. GitHub's **Actions** tab shows jobs and their logs. Once the workflow is available on the repository default branch, `workflow_dispatch` may also be used, but it is **not required** for development-branch testing.

Open:

- [Android QA runs](https://github.com/ValdiaKun/UFtouchHLE/actions/workflows/qa-android.yml)
- [Performance validation runs](https://github.com/ValdiaKun/UFtouchHLE/actions/workflows/perf-validate.yml)

The workflow must be enabled in repository settings, and GitHub-hosted runners need to be available. CI success is not guaranteed until the hosted jobs actually finish.

## Checks and artifact

1. Eight standard-library unit tests exercise `dev-scripts/verify-android-apk.py`.
2. Check out the current commit and all pinned source submodules.
3. Install Java 17, Android API 31, Gradle 8.11.1, NDK 28.2, Rust ARM64 and cargo-ndk, plus Boost development headers.
4. Fetch Cargo dependencies using `Cargo.lock` and build `:app:assembleDebug` with UF branding.
5. Check APK ZIP structure, bundled library names, ARM64 ELF machine type and 16 KiB PT_LOAD segment alignment. Reject unexpected guest IPA/ISO or key material in the archive.
6. Run `apksigner verify` and `aapt2 dump badging`; assert the expected package identifier, minSdk 29 and targetSdk 31.
7. Upload the APK, structured QA report, badging and SHA-256 as a **test-only artifact** retained for seven days.

The workflow has `contents: read` only; it does not access signing secrets, publish GitHub releases or auto-merge branches.

**Do not install CI's debug-signed APK over a production UFtouchHLE installation.** It has the same package ID but a different signing identity; testing may require a dedicated test device or isolated install. Never uninstall the original app without backing up saves. If side-by-side installation is needed, create an intentionally distinct package ID and corresponding data paths in a separate, reviewed experimental change.

## Offline APK check

```sh
python3 dev-scripts/test_verify_android_apk.py
python3 dev-scripts/verify-android-apk.py /path/to/test.apk --report apk-report.json
```

This check reads an APK but does **not** execute it, run games, measure FPS or confirm GLES runtime correctness. The ELF validator inspects 64-bit little-endian AArch64 program headers and checks that each PT_LOAD is aligned for Android's 16 KiB page-size requirement. The CI runner separately verifies the Android package metadata and APK signature.

## Hardware and FPS testing

See [BENCHMARKING.md](BENCHMARKING.md). The existing `--performance-test=observe` mode can export frame-time CSV/JSONL samples on a device. A physical-phone tester should collect at least three comparable runs with the same game/scene/device/profile and note thermals, RAM and Android version. Compare reports using `dev-scripts/perf-report.py`.

A successful CI build **does not** establish gameplay compatibility, measured speedups, low-end thermal behavior or on-device stability. Those must be reported separately as pending until real-device evidence exists.

## Reporting to the developer

For each commit, give the exact commit SHA and one of: `not run`, `pending`, `passed`, `failed`, `blocked`. Include the failing job and error when a run fails. Avoid claims of validated FPS gains without baseline and candidate data.
