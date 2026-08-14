---
name: gui-integration-test-video
description: Record screenshots and video from Smash's Rust GUI integration-test harness, including keyboard and pointer overlays.
---

# Smash GUI integration-test recording

Use this skill only for the Rust integration-test recording pipeline. For an ordinary screenshot or recording of the running app, use the computer-control tooling instead.

The implementation lives under `crates/integration` and `crates/warpui_core/src/integration`.

## Run a recording test

```bash
SMASHUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
cargo run -p integration --bin integration -- test_video_recording
```

To enable driver-managed recording for matching tests:

```bash
SMASHUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
SMASH_INTEGRATION_TEST_VIDEO=test_video_recording \
cargo run -p integration --bin integration -- test_video_recording
```

`SMASH_INTEGRATION_TEST_VIDEO` accepts `1`, `all`, or a comma-separated list of test names. A test that explicitly uses `with_start_recording()` and `with_stop_recording()` does not need it.

Set `SMASH_INTEGRATION_TEST_ARTIFACTS_DIR` to override the artifacts root. Otherwise artifacts are written below `$TMPDIR/smash_integration_test_artifacts/<test-name>/<timestamp>/`.

Set `SMASH_INTEGRATION_TEST_VIDEO_DIR` only when overriding the lower-level recorder output root. Its default is `$TMPDIR/smash_integration_video_captures`.

## Authoring

- Use `Builder::new().with_real_display()`.
- Start and stop recording with `TestStep::with_start_recording()` and `with_stop_recording()`.
- Capture still images with `TestStep::with_take_screenshot("filename.png")`.
- Use event-driving steps for visible mouse and keyboard overlays.
- Inspect `recording.mp4`, requested PNGs, and `recording.log` before reporting success.

If MP4 finalization fails, inspect the fallback `recording_frames/` PNG directory.
