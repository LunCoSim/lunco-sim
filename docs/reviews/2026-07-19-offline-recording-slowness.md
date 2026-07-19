# Offline recording — slowness analysis & handover

**Scope:** the offline frame-recording subsystem (`crates/lunco-workbench/src/screenshot.rs`
+ the rhai sequencer `assets/scripting/{prelude/recording,lib/shots}.rhai` + the CLI/scene
entry points in `crates/lunco-sandbox/src/lib.rs`), at branch `optimization` @ `1941ddda`.
**Symptom reported:** "when we record video/cinematic mode fps is very slow, feels like
slower than in realtime."
**Method:** static analysis only — read the cited code paths end-to-end. No profiling run,
no edits. Findings marked `CONFIRMED` are verified against the code as cited; `PLAUSIBLE` are
suspected but need a run to prove. Each finding names the file:line that establishes it.
**Benchmark:** judged against the project's own stated design intent in
[`offline-recording.md`](../offline-recording.md) and the frame-discipline rules in
[AGENTS.md §7](../../AGENTS.md) and the tunability mandate [AGENTS.md §3](../../AGENTS.md).

---

## ⚠️ Read this box first — the short version

1. **Recording is *designed* to be slower than realtime.** Output is locked to `fps`; wall
   clock is decoupled. "Slower than realtime" alone is not a defect (`offline-recording.md:29-31`).
   Disambiguate the symptom before changing anything (§1).
2. **There is one CONFIRMED bug** that matches the reported symptom precisely: the CLI
   `--record-offline` path skips `KeepAwake` + `PresentMode::AutoNoVsync`, which is exactly
   the documented "2–10 s/frame vs ~50 ms" failure mode (§2). Fix this first.
3. **There are two architectural issues** that the proposed "async PNG queue" actually
   addresses — but they are *not* "saving files faster." They are (a) the synchronous save
   sits inside the clock-freeze window, paying save cost in series with render instead of
   overlapped; and (b) two hand-maintained recording constructors have already drifted
   once and will drift again (§3).
4. **The proposed `buffer_depth` parameter is the right shape** (`0` = today's behavior,
   `N` = queue). But it forces four decisions that are easy to get wrong and must be
   designed, not bolted on (§4).
5. **Nothing in this document has been edited.** This is analysis for handover.

---

## §1. Symptom disambiguation — do this before anything else

"Feels slower than realtime" is ambiguous and the three readings have nothing in common:

| Symptom | Root cause class | Does an async PNG queue help? |
|---|---|---|
| Capture wall-time > playback duration | By design (or a bug on top) | Only if PNG encode dominates (unmeasured) |
| Output video plays slow / choppy / wrong speed | Framerate mismatch at encode | **No** — output is already locked to `fps`; capture speed is irrelevant to the bytes |
| App / UI sluggish **during** recording | Main-thread blocking, OR throttle bite | Partial — only the blocking part |

This document assumes the first reading. If the reporter actually meant the second, the
fix is at the encoder stage (not in `screenshot.rs` at all) and the rest of this document
does not apply.

A separate, real source of "feels slow" worth knowing about: **three different default
`fps` values across three surfaces**, and the one that wins for scene-driven recordings is
25 (`shots.rhai:107`), not 60. Lower `fps` does not speed up wall-clock — it shortens the
video for the same frame count — but it is invisible (no log line names the effective fps).
See §5.2.

---

## §2. CONFIRMED BUG — CLI path skips pacing knobs

The recording state machine has **two entry points** that construct `OfflineRecordingState`
independently. They have drifted:

| Entry point | Acquires `KeepAwake`? | Sets `PresentMode::AutoNoVsync`? |
|---|---|---|
| `StartOfflineRecording` command → `activate_recording` (`screenshot.rs:486-542`) | ✅ line 511 | ✅ line 523 |
| CLI `--record-offline` → direct resource insert (`lunco-sandbox/src/lib.rs:1761-1773`) | ❌ never | ❌ never |

The CLI path's own comment admits it:

```rust
// CLI-armed recording starts before `WinitSettings` exists to
// override; `StartOfflineRecording` is what forces Continuous.
prev_present_mode: None,
```

…then doesn't compensate. The comment's stated reason ("starts before `WinitSettings`
exists") applies only to the *present-mode* override, not to `KeepAwake` — which is a plain
`Resource`, initialized at `screenshot.rs:89`, and can be acquired at app-construction time.
Neither knob is applied for CLI recordings.

**Why this matches the symptom:**
- `pacing.rs:21-26` + `activate_recording`'s comment (`screenshot.rs:503-507`): an unfocused
  capture under `reactive_low_power` is **2–10 s per frame vs ~50 ms awake** — "turning a
  ~1 minute episode into hours."
- `sim_focus_pace` in `lunco-modelica` (`lib.rs:1585-1613`) is the *sole* writer of
  `WinitSettings.unfocused_mode` and consults `KeepAwake` at `lib.rs:1604`. Without the
  token, an unfocused CLI-record window gets throttled to ~1 FPS.
- `PresentMode::Fifo` (vsync, the default at `lunco-sandbox/src/lib.rs:306`) caps the render
  loop to the monitor refresh; the lockstep spends ≥2 render frames per captured frame →
  hard ~30 captured-FPS ceiling. Compounds with the throttle above.
- AGENTS.md §7 explicitly warns that a backgrounded window throttling to ~1 FPS is "not a
  hang, do not fix it" — but here we *are* trying to record through that throttle.

**Verify without rebuilding** — two log checks against a CLI-recorded run:
1. The command path logs `[offline-record] power saving disabled (KeepAwake acquired)`
   (`screenshot.rs:512`). A CLI recording will **not** emit this line. Absence = bug
   confirmed.
2. The command path overrides `PresentMode`; CLI does not. If the run started with
   `Fifo` and no override was logged, vsync is capping the loop for the whole capture.

**Fix shape (mechanical, small):** CLI-armed recording must acquire the same `KeepAwake`
token and apply the same present-mode override as `activate_recording`. The token can be
acquired at app construction (it's a plain counter). The present-mode override must be
applied on the first frame `state.active` is observed (an `Update` system or in
`start_recording_when_scene_ready`), because `WinitSettings`/`Window` don't exist at app
construction.

**Order:** fix this **before** any profiling or queue work. It is exactly the documented
"feels slower than realtime" failure mode, and no async scheme recovers time stolen by the
power-save throttle.

---

## §3. CONFIRMED architectural issues (independent of the §2 bug)

Read the per-frame cycle in `drive_offline_clock` (`screenshot.rs:822-883`) +
`deliver_offline_frame` (`screenshot.rs:886-928`). The sequence is **strictly serial**:

```
advance(1/fps) → spawn Screenshot → freeze(ZERO) → [GPU readback, 1-2 frames]
              → deliver: clone → convert → encode → write → unfreeze → advance …
```

### §3.1 No pipelining — save cost is in series with render cost

The freeze (`is_waiting_for_frame`, set `screenshot.rs:878`, cleared `screenshot.rs:924`)
covers the *entire* save. The clock stays at `ZERO` for the full deflate+write duration,
and `drive_offline_clock` won't request the next capture until `is_waiting_for_frame`
clears. **So the save cost is paid in series with the render cost, not overlapped.**

This is the architectural issue the proposed async queue actually addresses — *not* "saving
files faster." Win comes from overlapping save with the next render, not from making
deflate quicker.

### §3.2 The freeze conflates two signals

The freeze exists to keep the readback coherent. It is *also* being used as the save's
completion signal. Those are two different concerns. Splitting them (freeze clears when
readback delivers; save runs async after that) is the minimum change that unlocks
pipelining without touching determinism.

### §3.3 Two constructors, already drifted once

`StartOfflineRecording` → `activate_recording` vs CLI direct insert. They have already
drifted on `KeepAwake`/`PresentMode` (§2). Any new field (e.g. `buffer_depth`, §4) is a
third thing to drift on. **The constructor unification and the buffer parameter are the
same change** — do not add `buffer_depth` to two hand-maintained constructors.

### §3.4 Structural ≥2 render frames per captured frame (don't fix)

The advance/capture/wait cycle is ≥3 render frames per captured frame before any save cost
(frame A advance; frame B capture request; frame C wait; possibly more while Bevy's
readback lands; then deliver). At 60 Hz render that's a ~12–20 captured-fps structural
ceiling — already *below* the rhai default of 25 fps. Output stays locked to 25 fps for
determinism (correct), but wall-clock is `frames / ~15 fps`. This is by design
(`offline-recording.md §2-3`); re-deriving it reopens the "two runs differ at every frame"
class of bug the readiness gate exists to close. Leave alone.

### §3.5 `drive_offline_clock` polls every frame in `Last` (minor, AGENTS.md §7)

Unconditional per-frame system with a `Commands` buffer write on the no-op path. Not the
bottleneck, but a frame-discipline nit. Not worth touching unless restructuring §3.1/§3.2.

---

## §4. The `buffer_depth` parameter — right shape, four forced decisions

The proposed parameter (`buffer_depth: u32`, `0` = sync save = today, `N` = queue) is
correct. Default `0` is right because it (a) preserves the determinism contract verbatim,
(b) is a documented safety rollback if async ever misbehaves, and (c) makes A/B
measurement trivial.

But it forces four decisions that must be designed, not bolted on.

### §4.1 Decision 1 — what's in the buffer?

"Buffer N images" is ambiguous and the choice is invisible in the parameter name:

| Buffer holds | Encode runs on | Memory per slot | Win | Risk |
|---|---|---|---|---|
| **Encoded PNG bytes** | Main thread (still blocks) | ~1–2 MB @ 1080p | Disk I/O overlap only | Small |
| **Raw `DynamicImage`** | Worker pool | **~8 MB @ 1080p, scales with window res** | Deflate + I/O overlap | Closer to original OOM |

The real win is raw-image buffering (deflate is the expensive part, not `fs::write`). But
the prior OOM (`offline-recording.md:68-69`) was *unbounded* raw-image buffering — bounded
at N is what makes it safe, not the design. **Recommendation: ship bytes-buffered first**
(smaller win, near-zero risk), prove the queue discipline, then add image-buffered as a
second mode. Don't make the parameter choose contents on day one.

### §4.2 Decision 2 — drain on stop (the contract subtlety)

`on_stop_offline_recording` (`screenshot.rs:544-572`) flips `active=false` and restores
the clock immediately. With `buffer_depth > 0`, the last N frames are still in flight when
stop fires. The "no holes in the sequence" contract (`screenshot.rs:903-909`) means
**stop cannot complete until the queue drains** — otherwise the tail of every shot is
truncated and the next shot's `StartOfflineRecording` races the previous one's writes.

The rhai sequencer uses `shot_frame() < 0` (i.e. `!active`) as the "shot ended" signal, so
the rule is: **don't flip `active=false` until drain completes.** The wait is bounded by
`buffer_depth × per-save-time` — exactly the time the queue bought you mid-shot, paid back
once at the end. Net win unchanged; it just moves.

Implication: `StopOfflineRecording` becomes potentially blocking, or transitions through a
`draining` state that `GetOfflineRecordingStatus` (`screenshot.rs:930-943`) must expose.
The rhai prelude verb doesn't change (it already polls), but the status payload should grow
a `draining: bool`.

### §4.3 Decision 3 — the failure contract changes shape

Today: write fails → abort immediately (`screenshot.rs:910-919`). First bad frame = last
captured frame.

With buffering: write fails → failure arrives `N` frames late via a result channel. By
then `frame_index` has advanced past it, the next `buffer_depth` frames are queued, and
some may already be on disk out of order. **The "abort at the first bad frame to avoid
holes" contract no longer holds as stated.** Must be an explicit decision:

- **Drain-and-abort** (recommended): stop accepting new captures on first failure, drain
  the queue, log exactly which frames were lost, flip `active=false`. Sequence may have a
  tail after the gap; never a silent mid-sequence hole; failures loud and named.
- **Abort-and-discard**: throw away the queue on first failure. Cleaner sequence, loses
  work already done.

Either is defensible. The comment at `screenshot.rs:903-909` currently promises something
the buffered path cannot deliver verbatim — it must be updated as part of the change.

### §4.4 Decision 4 — the freeze-clear split is the actual architectural fix

Connects §4 to §3.2:

- `buffer_depth == 0` → freeze clears when sync save returns (today, contract preserved)
- `buffer_depth > 0` → freeze clears the instant bytes/image are handed to the queue; save
  runs async; next capture can start

So the parameter isn't really "how many to buffer" — it's **"should the freeze-clear wait
for the save, or just for the readback?"** That's §3.2's architectural fix, now gated
behind a number. Determinism (one time-step per captured frame, in order) is preserved in
both cases — only the *save tail* moves, not the *capture head*.

### §4.5 Where the parameter lives (drift surface — unify with §2)

The parameter becomes the third field both constructors must agree on (`fps`,
`KeepAwake`/`PresentMode` setup, `buffer_depth`). Adding it to two hand-maintained
constructors invites a third drift.

| Surface | Field | Today | Needs |
|---|---|---|---|
| `StartOfflineRecording` command | `buffer_depth: u32` | — | add |
| `OfflineRecordingState` resource | `buffer_depth: u32` | — | add (carries it through the run) |
| CLI `--record-buffer <N>` | parsed → state | — | add |
| rhai `shot(#{...})` | `buffer` | — | add; prelude passes through `shot_begin` → `StartOfflineRecording` |

Extract a single `RecordingConfig` builder both paths use. Doing `buffer_depth` without
that is adding a third thing to drift on.

### §4.6 Two flags to note, not solve

- **Wasm.** `AsyncComputeTaskPool` is single-threaded on wasm — "async" there is
  interleaving, not parallelism, and file I/O goes through `lunco_storage`. Win is
  marginal. `buffer_depth = 0` default means wasm sees no regression unless it opts in.
  One-line note in the field doc; not a design change.
- **Clamp.** A literal upper bound (say 32) prevents a future caller recreating the OOM by
  passing `u32::MAX`. Per AGENTS.md §3 it's a tunable parameter in the command/state, but
  should still clamp at a documented sane max.

---

## §5. Secondary costs (lower priority; revisit only after §2 fixed and §4 measured)

### §5.1 Per-frame image clone + format conversion

`deliver_offline_frame` does `event.image.clone().try_into_dynamic()` at `screenshot.rs:895`
— a full-buffer clone (~8 MB @ 1080p) plus a conversion to `image::DynamicImage`. ~10–20 ms
and meaningful allocation churn over a long shot.

**Lever:** encode directly from the captured `Image`'s bytes via
`image::codecs::png::PngEncoder`, skipping the `DynamicImage` round-trip and one of the two
per-frame copies. Cheaper than the queue and removes one allocation; do this *before* the
queue if instrumentation (§6) shows `clone` ≈ `save`.

### §5.2 The fps defaults — invisible, not a wall-clock lever

`state.fps` is written in **exactly one place** — `activate_recording` (`screenshot.rs:497`),
copied from the request. Nothing mutates it mid-recording. **No dynamic throttle to 25.**

But three defaults across three surfaces, and the one that wins for scene-driven
recordings is 25:

| Surface | Default fps | Where |
|---|---|---|
| rhai sequencer `shot(#{...})` | **25** | `assets/scripting/lib/shots.rhai:107` |
| CLI `--record-fps` | **60** | `lunco-sandbox/src/lib.rs:1744` |
| `StartOfflineRecording.fps` field | whatever the caller passes | `screenshot.rs:424` |

Lower fps does **not** speed up wall-clock — it shortens the video for the same frame count
(200 frames @ 25 fps = 8 s video; @ 60 fps = 3.3 s). Wall-clock is
`frame_count × per_frame_wall_cost` either way. **But** the 25 default is invisible: no log
line names the effective fps. Adding one to `activate_recording`'s existing info! (it
already prints fps at `screenshot.rs:538`) — make sure it's actually emitted in the scene
path too, not only the command path. The "feels slow" report may partly be "I got a 25 fps
video and didn't expect to."

### §5.3 PNG compression level

The `image` crate's default is aggressive; these are intermediates that get re-encoded to
video downstream. `PngEncoder` with a faster filter/compression preset typically ~2× faster
encode for ~10–20% larger files. Free win if encode is the bottleneck.

### §5.4 Lockstep terrain streaming on moving-camera shots

`mirror_recording_to_terrain_lockstep` (`lunco-sandbox/src/lib.rs:2411-2424`) mirrors
`OfflineRecordingState::active` onto `TerrainStreamLockstep` for determinism. Cost: every
captured frame blocks on in-flight terrain bakes instead of letting them land whenever.
Correct for determinism; not cheap on moving-camera shots; **no lever** — it's a
determinism requirement. Awareness only.

### §5.5 Window-resolution capture

`Screenshot::primary_window()` (`screenshot.rs:877`) captures at window resolution. Every
downstream cost scales with pixels. A separate `capture_resolution` knob (render to a
downsampled target) is a bigger change (needs a render target) but scales all downstream
costs. Only worth it after everything above is exhausted.

---

## §6. Instrumentation — apply this before changing anything

Split the per-frame cost in `deliver_offline_frame` (`screenshot.rs:886-928`) so we know
*which* of clone / convert / encode / write is the bottleneck. This is the cheapest answer
to "is the queue worth it." Changes no behavior — only adds `debug!` lines.

```rust
fn deliver_offline_frame(
    trigger: On<ScreenshotCaptured>,
    mut state: ResMut<OfflineRecordingState>,
) {
    if !state.active || !state.is_waiting_for_frame {
        return;
    }

    let event = trigger.event();
    let frame_idx = state.frame_index;

    // ── instrumentation: split the per-frame cost ──
    let t0 = web_time::Instant::now();
    let img_bytes = event.image.data.len();  // pre-encode, for context
    let t_clone_start = web_time::Instant::now();
    let Ok(dyn_img) = event.image.clone().try_into_dynamic() else {
        error!("[offline-record] failed to convert image for frame {}", frame_idx);
        state.is_waiting_for_frame = false;
        return;
    };
    let t_clone = t_clone_start.elapsed();

    let path = state.output_dir.join(format!("frame_{:06}.png", frame_idx));

    let t_save_start = web_time::Instant::now();
    if let Err(e) = dyn_img.save(&path) {
        error!(
            "[offline-record] failed to save frame {} ({e}) — aborting recording to \
             avoid a sequence with holes in it",
            frame_idx
        );
        state.active = false;
        state.is_waiting_for_frame = false;
        state.frame_just_captured = false;
        return;
    }
    let t_save = t_save_start.elapsed();
    let t_total = t0.elapsed();

    debug!(
        "[offline-record] frame {}: raw={}B clone={:.1}ms save={:.1}ms total={:.1}ms",
        frame_idx,
        img_bytes,
        t_clone.as_secs_f32() * 1000.0,
        t_save.as_secs_f32() * 1000.0,
        t_total.as_secs_f32() * 1000.0,
    );
    // ── /instrumentation ──

    state.frame_index += 1;
    state.is_waiting_for_frame = false;
    state.frame_just_captured = true;
}
```

Run with `RUST_LOG=lunco_workbench=debug` (or the existing `--log-diag` mechanism).

**Decision rule:**

| Observation | Conclusion |
|---|---|
| `save` ≫ `clone` AND `save` ≫ render-frame time | PNG queue (§4) is worth it |
| `clone` ≈ `save` | Do §5.1 first (PngEncoder-direct encode) — cheaper, removes one allocation |
| Both small, capture still slow | Cost is elsewhere: §2 bug, §5.4 terrain lockstep, or §3.4 structural floor |

---

## §7. Profiling notes (`scripts/perf/profile.sh` exists)

The repo already has a samply-based profiler with a `--diag-only` fallback. Notes for
applying it to recording specifically:

- It launches with `--no-vsync` (`PresentMode::Mailbox`), which **masks the very cost §2
  diagnoses** (missing `AutoNoVsync`). For a faithful recording profile, drive the binary
  with `--record-offline <dir>` and *don't* add `--no-vsync` — the harness's Mailbox
  stands in for the missing override. Separately, reproduce the throttle case
  (backgrounded window, no `--no-vsync`, no `KeepAwake`) to confirm seconds-per-frame.
- Capture warmup must cover the readiness gate (`READY_TIMEOUT` 20 s, `SETTLE_PERIOD`
  500 ms — `screenshot.rs:605, 635`). The script's default 8 s warmup is too short; pass
  `--warmup 25`.
- Hot-function view (`symbolicate_samply.py --skip-start`) drops warmup → steady-state
  capture cost. That's where `image::codecs::png` / deflate / `fs::write` should surface
  if PNG is the bottleneck.

---

## §8. Recommended order of work

1. **Disambiguate the symptom** (§1). If the reporter meant "output video plays wrong
   speed," stop — the fix is at the encoder stage, not here.
2. **Fix the §2 CLI constructor drift.** Pure correctness; no architecture change. Acquire
   `KeepAwake` at app construction for CLI-armed recording; apply `PresentMode::AutoNoVsync`
   on the first observed-active frame. This alone should recover the bulk of "feels slower
   than realtime" if the user records via `--record-offline`.
3. **Unify the two constructors** into a single `RecordingConfig` builder. Required before
   §4 — otherwise `buffer_depth` becomes a third drift surface.
4. **Add the effective-fps log line** to the scene-driven path (§5.2). Cheap, removes a
   class of "why is my video 25 fps" confusion.
5. **Apply the §6 instrumentation.** Run one representative shot. Read the clone/save split.
6. **Decide between the queue (§4) and PngEncoder-direct (§5.1)** based on the
   instrumentation. Only build the queue if `save` dominates.
7. **If building the queue:** ship bytes-buffered first (§4.1), with the drain phase
   (§4.2), the delayed-failure drain-and-abort (§4.3), and the freeze-clear split (§4.4)
   as one coherent change. Image-buffered mode as a follow-up after the queue discipline
   is shown correct.
8. **Leave §3.4 (structural floor) and §5.4 (terrain lockstep) alone** unless
   instrumentation exonerates everything else. Both exist for determinism.

---

## §9. File:line reference index

All citations in one place for quick navigation:

| Concern | Location |
|---|---|
| Recorder plugin + commands | `crates/lunco-workbench/src/screenshot.rs:76-119` |
| `StartOfflineRecording` command + handler | `screenshot.rs:419-482` |
| `activate_recording` (command path constructor) | `screenshot.rs:486-542` |
| `StopOfflineRecording` handler | `screenshot.rs:544-572` |
| Readiness gate (`PendingShotStart`, `scene_visuals_ready`) | `screenshot.rs:574-745` |
| `drive_offline_clock` (clock writer, advance/capture cycle) | `screenshot.rs:822-883` |
| `deliver_offline_frame` (sync save call site) | `screenshot.rs:886-928` |
| `GetOfflineRecordingStatus` query | `screenshot.rs:930-943` |
| CLI `--record-offline` direct insert (§2 bug) | `crates/lunco-sandbox/src/lib.rs:1741-1776` |
| CLI present-mode default (`Fifo`) | `crates/lunco-sandbox/src/lib.rs:303-307` |
| `KeepAwake` resource definition | `crates/lunco-core/src/pacing.rs:26-45` |
| `sim_focus_pace` (sole `WinitSettings` writer) | `crates/lunco-modelica/src/lib.rs:1584-1619` |
| `mirror_recording_to_terrain_lockstep` | `crates/lunco-sandbox/src/lib.rs:2411-2424` |
| rhai prelude verbs | `assets/scripting/prelude/recording.rhai` |
| rhai sequencer (default fps = 25) | `assets/scripting/lib/shots.rhai:107` |
| Design doc | `docs/offline-recording.md` |
| Profiler | `scripts/perf/profile.sh` |

---

**Status:** analysis only, as requested. No code or docs outside this file have been
touched. Ready to turn §2, §3, §4, or §6 into an implementation plan on request.

---

## §10. Outcome (implemented + measured later on 2026-07-19)

- **§2 fixed and verified live.** The pacing knobs are no longer applied by the
  activation/teardown paths at all: a single `enforce_recording_pacing` system
  (`screenshot.rs`) acquires/releases `KeepAwake` on the edges of `state.active` and
  applies/restores `AutoNoVsync` with a retry until the window exists. A CLI
  `--record-offline` run now logs both knob lines and captured 299 frames in 47 s
  (~6.3 fps wall) **unfocused** — the throttle case that used to run at 2–10 s/frame.
- **§3.3 fixed.** Both entry points build through `OfflineRecordingState::start()`;
  the CLI no longer hand-lists fields.
- **§6 instrumentation shipped** (`debug!` clone/save split per frame). Measured on
  the default sandbox scene: clone+convert ≈ 5–8 ms, save ≈ 67–77 ms of a ~150 ms
  cycle — save dominates, per-frame cost is real.
- **§5.3 tried and REJECTED, measured.** A/B at 1920×1200 on the default sandbox
  scene, ~390 steady frames each: default compression save = 65.0 ms; `Fast` +
  `Adaptive` save = 77.5 ms — *slower* — at the same ~4 MB/frame. Strictly worse on
  this content; do not re-try. The remaining lever for save cost is the §4
  buffered-save design (overlap, not cheaper deflate).
- **Found while verifying the video: the CLI path bypassed the readiness gate.**
  It inserted an `active` state at app construction, so frames 0..N captured a
  not-yet-loaded scene (black opening frames in the encoded video). Fixed: the CLI
  now arms via `arm_recording_at_startup` → same `PendingShotStart` gate as the
  command path. This also retired the CLI's direct use of the state constructor.
- End-to-end verified: 396-frame CLI capture → ffmpeg (`-framerate 30 … libx264`)
  → 13.2 s 30 fps video, full-bleed 3D with no workbench chrome burnt in
  (`render_layout`'s clean-capture pass), simulation motion present.
- **§4 BUILT — and then §3.4 re-derived, deliberately.** Measurements after the
  async save queue landed showed the real dominant cost was neither throttle nor
  PNG: **GPU readback latency (~50–100 ms/frame)**, which the serial design paid
  in full every frame by freezing the clock until delivery. But a screenshot's
  pixels are snapshotted on the GPU at request time — waiting for the copy buys
  nothing. The cycle is now PIPELINED: every render frame advances exactly one
  `1/fps` step and captures it; each capture carries its slot index + destination
  path in its own `OfflineFrameCapture` component (order/state-independent
  delivery, safe across stop and even into the next shot); readbacks and saves
  overlap, bounded by `MAX_OUTSTANDING_CAPTURES`/`MAX_IN_FLIGHT_SAVES`
  back-pressure that pauses *advancing*, never skips a taken step. Determinism
  contract (N frames = N consecutive steps, no gaps) verified on a 327-frame run:
  contiguous indexes, zero duplicate adjacent frames, correct motion in the video.
- **Measured progression at 2560×1552, plain CLI launch, unfocused:**
  serial+sync save 119 ms/frame → +async queue 110 (readback exposed at ~63 ms)
  → +pipelining **41.5 ms/frame (24 fps captured)** — a 10 s @ 30 fps episode
  captures in ~13 s. `--no-vsync --no-throttle` measured SLOWER with the pipeline
  (69.6 ms — flat-out Mailbox rendering competes with readback/encode for CPU);
  the recorder's own AutoNoVsync override is the right mode, and
  `enforce_recording_pacing` now leaves an already-uncapped window alone.
- Remaining known cost, in order: GPU readback of a 16 MB window-sized frame
  every render frame, then the ~10 ms main-thread `Image` clone. §5.5
  (capture_resolution knob) is now the biggest untouched lever.
- KILL CAVEAT: SIGKILL/SIGTERM mid-recording can truncate the last
  `MAX_IN_FLIGHT_SAVES` PNGs (workers die mid-write). A normal
  `StopOfflineRecording` + running app drains cleanly; `pending_saves` in
  `GetOfflineRecordingStatus` exposes the drain.
