---
name: record-video
description: >
  How to record deterministic video or PNG sequences from the luncosim —
  windowed or fully windowless. Trigger whenever the user asks to "record a
  video", "capture an episode", "render a cinematic", "make a recording of the
  scene/tutorial", to record without opening a window / on a headless box, or
  when a scenario needs frame-exact capture. Covers the one-command CLI takes
  (`--record-offline`, `--offscreen`), the video-vs-PNG destination rule, the
  rhai shot sequencer, and how to check progress and diagnose a black or
  stalled take.
---

# Record a deterministic video

The recorder supplies `1/fps` per frame through the shared transport. A held
simulation advances only through explicit `StepSimulation` requests. Output
sequencing uses frame index, independent of machine speed. Full design:
[`docs/offline-recording.md`](../../docs/offline-recording.md).

## The one-command take (start here)

```sh
# Windowless: no window/egui, renders offscreen, EXITS BY ITSELF when done
"$LUNCOSIM_BIN" --api 4101 \
  --offscreen --render-quality high \
  --record-offline ~/.cache/take.mp4 --record-fps 30 --record-frames 300

# Windowed variant (records the live window; you stop it, or pass --record-frames)
"$LUNCOSIM_BIN" --api 4101 --record-offline ~/.cache/take.mp4 --record-fps 30
```

- Recording starts **after the scene-visuals readiness gate**, not at process
  start — don't be surprised by a few seconds of warm-up before frame 0. For a
  USD scene this gate includes the stage's awaiting prims, queued visual
  projections, asynchronously generated meshes, and the selected image camera;
  a Rhai verdict or one early `Mesh3d` is not sufficient evidence that the render
  view has participants.
- **Destination picks the format**: `.mp4`/`.mkv`/`.mov` streams into `ffmpeg`
  (one small file); any other path is a directory of `frame_%06d.png`. No
  `ffmpeg` installed ⇒ loud warn + PNG fallback in `<name>.frames/`.
- `--record-size WxH` sets the offscreen resolution (default 1280x720, the
  windowed default). `--scene PATH` picks the scene, as usual.
- **Output goes under `~/.cache/...`**, never `/tmp` (root partition) and never
  the repo.

## Offscreen mode: what to know

- `--offscreen` is GPU-full windowless — NOT `--no-ui` (which drops the GPU and
  cannot capture anything).
- `--render-quality high` selects the highest shipped renderer budget for the
  process: shadows, sky cubemap, lunar terrain LOD, and tessellation. USD still
  owns the authored terrain/sky shader sources; this flag does not swap them.
- No workbench exists, so the recorder activates the scene's first **authored**
  `SceneCamera` (e.g. the luncosim `WideShot`). A scene with no authored camera
  records black and logs a warning — author a camera, don't fight the picker.
- The target-born offscreen camera mirrors the selected authored camera's transform,
  projection, exposure, tonemapping, MSAA, and camera-owned environment views
  (`Skybox` plus generated image-based-lighting intent), so PNG/video output uses the
  same authored grade and DomeLight background as the windowed presentation.
- `--offscreen --api PORT` (without `--record-offline`) gives a windowless
  interactive instance: `StartOfflineRecording` / `CaptureScreenshot` work over
  HTTP and read the offscreen target.
- Terrain appearance fixtures keep the DEM's authored elevation datum. Author
  the presentation camera near that datum; a camera aimed at elevation zero can
  validly cull every terrain tile. The terrain graphics gate also checks the
  full PNG and far/middle/near bands for real relief, not just file creation.

## Scripted episodes (rhai)

An unattended episode should replace an interactive scene's possession startup
with an observer-only `on_start` that holds simulation before its first tick.
If operator takeover must be excluded, use an isolated asset composition with
`control.authority.claim` returning false. Merely releasing control once does
not stop a later scene click from acquiring the vehicle. The shared default
policy permits normal interaction. Keep shot order and control inputs in the
native Rhai task tree; remote scene load and launch do not require a second
simulation/control loop.

Multi-shot episodes run on the shot sequencer — `shot_begin`/`shot_frame`/
`shot_end` from `prelude/recording.rhai`, sequenced by `lib/shots.rhai`. Shot
timing uses the recorder's `frame_index`. Declare `// @timing presentation`
so sequencing continues while `StepSimulation` holds the shared physics, Modelica
and simulation-timer clock. Explicit steps use the normal fixed-cycle runner
and respect all admission barriers. The sequencer
contract: [`docs/offline-recording.md`](../../docs/offline-recording.md) §5–§9.

## Check progress / diagnose

Use `GetOfflineRecordingStatus` and measurement queries during a take.
`CaptureScreenshot` rejects while recording is pending or active because the
recorder owns the capture target. For visual QA, capture before recording or
extract a frame from the finished video.

```rhai
query("GetOfflineRecordingStatus")
// #{ active, frame_index, is_waiting_for_frame, last_capture_sim_tick }
```

- **Stalled?** Check `active`, `frame_index`, `is_waiting_for_frame` and the
  readiness/encoder log. A pending readback holds the frame; a pending shared
  cycle or Modelica receipt holds capture until that cycle completes.
- **Black video?** Almost always "no rendering camera": offscreen without an
  authored `SceneCamera`, or every camera `is_active: false`. Check the log for
  the offscreen camera warning. Alpha-0 frames (white in PNG viewers, black in
  video) mean the camera never rendered at all.
- **Take aborted?** A failed frame save aborts loudly and names the frame; the
  sequence up to it is intact. Disk-full is the usual cause.
- A killed run may leave `.tmp` files next to frames — litter, not corruption;
  writes are atomic.
- Stop through `StopOfflineRecording`, including an armed request still waiting
  for visuals. It restores the prior native update source through `lunco-time`;
  returning to realtime admits no capture wall-time gap.

## Rules

- NEVER "fix" slowness by freezing the clock for readback or writing
  `TimeUpdateStrategy`/`WinitSettings` anywhere new — each knob has exactly one
  writer (doc §2).
- `--no-vsync` does NOT speed up recording (measured slower: presenting
  flat-out starves the save workers).
- Frame pacing, capture, and saving live in `lunco-capture`; the workbench and
  offscreen host install the same capability, while the CLI only arms the
  recorder state the commands use.
