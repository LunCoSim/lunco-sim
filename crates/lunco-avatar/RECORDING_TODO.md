# Recording — TODO & known limitations

Current state: screen recording ships behind the opt-in `recording` cargo feature,
built on Bevy 0.18.1's first-party `EasyScreenRecordPlugin`. See
`src/recording.rs`. It works, but the built-in encoder is deliberately minimal.
This doc records its limitations and the plan to move "real video" onto a
maintained, purpose-built crate.

## Limitations of `EasyScreenRecordPlugin` (Bevy 0.18.1)

These are constraints of bevy's built-in recorder, not our wiring:

1. **Raw `.h264` only** — writes a raw H.264 elementary stream, not an `.mp4`
   container. Must be remuxed (`ffmpeg -framerate 30 -i x.h264 -c copy x.mp4`).
2. **No output path/filename control** — hard-codes `<window-title>-<epoch_ms>.h264`
   in the **process working directory**. The `output_dir` field exists only on
   bevy `main`, not in released 0.18.1. Worked around (verified 2026-06-12): on
   stop, `encoder::spawn_relocator` runs a worker thread that waits for the
   encoder's flush then moves the file into the configured folder under a
   **sanitized** name (`encoder::sanitize_filename`, e.g. `sandbox-Listening-on-4013-<ms>.h264`).
   The thread is off the Bevy loop on purpose — an in-`Update` finalizer stalled
   when the winit loop went idle after stop. Filename is still derived from the
   window title (only the folder + sanitization are ours).
3. **No resolution control** — captures the window at whatever size it is. No
   fixed-resolution / offscreen target.
4. **Realtime only** — capture is coupled to the render clock. **No deterministic,
   frame-locked export** (can't render N frames at fixed dt regardless of machine
   speed). This blocks clean cinematic / camera-path video output.
5. **Platform-limited** — needs `bevy_dev_tools/screenrecording` → links system
   **libx264**. Native Linux/macOS only; **not Windows** (bevy issue #22132),
   **not wasm**.
6. **Single encoder/quality** — x264 with fixed preset/tune; no gif/PNG/codec choice.
7. **Toggle quirk** — its built-in toggle is a single `KeyCode` with no "disable";
   we park it on `KeyCode::F24` and drive via `RecordScreen` messages so Ctrl+Shift+R
   is the real control.

Keep `EasyScreenRecordPlugin` for the "grab my screen now" case — zero extra deps,
already working. It is NOT the path for shareable mp4 or cinematic export.

## Proper path: `bevy_image_export` (maintained, Bevy 0.18)

Chosen replacement for "real video". Crate `bevy_image_export`:
- **Alive & 0.18-current**: v0.16.1 (2026-05-31), depends on `bevy = "0.18"`,
  release cadence tracks each bevy version. Repo: https://github.com/paulkre/bevy_image_export
- Renders a target camera to an **image sequence** (PNG/JPEG/EXR) → `ffmpeg` →
  mp4/any codec.
- Supports an **offscreen render target at fixed resolution** → resolution
  independent of window, and **deterministic/frame-locked** when driven by a
  fixed-step loop. This is the correct base for camera-path / cinematic export
  (drive `SimTick` at fixed dt → render each tick → encode).

Rejected alternatives (do NOT adopt — dead/unmaintained):
- `bevy_capture` — pinned to bevy 0.17, last release Oct 2025, ~400 dl/mo.
- `bevy_capture_media` — v0.0.2, abandoned.
- `bevy_simple_screenshot` — screenshots only, no video.

First-party fallback if we ever want **zero third-party deps**: bevy's built-in
`bevy::render::gpu_readback::Readback` (+ `ReadbackComplete`) — readback the
render target to CPU ourselves and write frames. More code; never goes stale.
`bevy_image_export` essentially wraps this for us.

## TODO

- [ ] **Spike `bevy_image_export` 0.16.1** against our build: confirm it compiles
      with the workspace's slim bevy 0.18.1 feature set; note any feature gaps
      (it needs `bevy_render`, `bevy_asset`, `bevy_log`, plus `png` for PNG).
- [ ] **Offscreen capture camera**: spawn a second camera with a fixed-resolution
      image render target (configurable W×H in `RecordingSettings`), attach the
      exporter. Keep it separate from the live `Avatar` window camera.
- [ ] **Deterministic loop**: drive recording from a fixed-dt step keyed on
      `SimTick` (not wall clock) so output is frame-locked. Pair with the
      camera-path player (see `CAMERA_PATH_PLAN.md`).
- [ ] **Settings**: extend `RecordingSettings` with `width`, `height`, `fps`,
      `format` (png/mp4), and make filename first-class (now possible — we own
      the output path).
- [ ] **ffmpeg post-step**: on stop, invoke `ffmpeg` (if in PATH) to mux the
      PNG sequence → `<output_dir>/<name>.mp4`; fall back to leaving the sequence
      + logging the command if ffmpeg is missing. Native-only; never on wasm.
- [ ] **Backend selection**: route the existing `StartRecording`/`StopRecording`/
      `ToggleRecording` commands to either backend — `EasyScreenRecordPlugin`
      (quick realtime) or `bevy_image_export` (deterministic mp4) — via a
      `RecordingSettings.backend` enum. Keep the always-on control surface intact.
- [ ] **Remux helper**: replace the logged ffmpeg hint with an actual optional
      remux of the built-in encoder's `.h264` → `.mp4` when ffmpeg is present.

## Dependency-liveness rule

Before adopting any recording (or other) crate: check last-release date, bevy
version pin, and cadence. **Do not build on dead projects.** Prefer first-party
bevy APIs, then actively-maintained crates that track bevy releases closely.
