---
name: record-video
description: >
  How to record deterministic video or PNG sequences from the sandbox —
  windowed or fully windowless. Trigger whenever the user asks to "record a
  video", "capture an episode", "render a cinematic", "make a recording of the
  scene/tutorial", to record without opening a window / on a headless box, or
  when a scenario needs frame-exact capture. Covers the one-command CLI takes
  (`--record-offline`, `--offscreen`), the video-vs-PNG destination rule, the
  rhai shot sequencer, and how to check progress and diagnose a black or
  stalled take.
---

# Record a deterministic video

The recorder advances virtual time exactly `1/fps` per captured frame — output
is a function of frame index, never machine speed. Full design:
[`docs/offline-recording.md`](../../docs/offline-recording.md).

## The one-command take (start here)

```sh
# Windowless: no window/egui, renders offscreen, EXITS BY ITSELF when done
cargo run -p lunco-sandbox --bin sandbox -- \
  --offscreen --record-offline ~/.cache/take.mp4 --record-fps 30 --record-frames 300

# Windowed variant (records the live window; you stop it, or pass --record-frames)
cargo run -p lunco-sandbox --bin sandbox -- --record-offline ~/.cache/take.mp4 --record-fps 30
```

- Recording starts **after the scene-visuals readiness gate**, not at process
  start — don't be surprised by a few seconds of warm-up before frame 0.
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
- No workbench exists, so the recorder activates the scene's first **authored**
  `SceneCamera` (e.g. the sandbox `WideShot`). A scene with no authored camera
  records black and logs a warning — author a camera, don't fight the picker.
- `--offscreen --api PORT` (without `--record-offline`) gives a windowless
  interactive instance: `StartOfflineRecording` / `CaptureScreenshot` work over
  HTTP and read the offscreen target.

## Scripted episodes (rhai)

Multi-shot episodes run on the shot sequencer — `shot_begin`/`shot_frame`/
`shot_end` from `prelude/recording.rhai`, sequenced by `lib/shots.rhai`. Shot
timing MUST use the recorder's `frame_index` (never a tick counter), and beats
freeze via a PHYSICS hold (never a clock pause). Both traps + the sequencer
contract: [`docs/offline-recording.md`](../../docs/offline-recording.md) §5–§9.

## Check progress / diagnose

```rhai
query("GetOfflineRecordingStatus")
// #{ active, frame_index, video, outstanding_captures, pending_saves, … }
```

- **Stalled?** `outstanding_captures`/`pending_saves` pinned at their caps means
  back-pressure (slow disk/encoder) — the clock pauses advancing by design and
  resumes when the pipeline drains.
- **Black video?** Almost always "no rendering camera": offscreen without an
  authored `SceneCamera`, or every camera `is_active: false`. Check the log for
  the offscreen camera warning. Alpha-0 frames (white in PNG viewers, black in
  video) mean the camera never rendered at all.
- **Take aborted?** A failed frame save aborts loudly and names the frame; the
  sequence up to it is intact. Disk-full is the usual cause.
- A killed run may leave `.tmp` files next to frames — litter, not corruption;
  writes are atomic.

## Rules

- NEVER "fix" slowness by freezing the clock for readback or writing
  `TimeUpdateStrategy`/`WinitSettings` anywhere new — each knob has exactly one
  writer (doc §2).
- `--no-vsync` does NOT speed up recording (measured slower: presenting
  flat-out starves the save workers).
- Frame pacing, capture, and saving live in `lunco-workbench`'s screenshot
  module; the CLI only arms the same recorder state the commands use.
