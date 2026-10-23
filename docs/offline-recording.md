# Offline Recording

How the engine renders a **deterministic, frame-exact image sequence** — the mechanism
that decouples Bevy's update loop from the system clock, and the rhai surface
(`prelude/recording.rhai` + `lib/shots.rhai`) that sequences shots on top of it.

- **Recorder:** `lunco-capture` — capture commands, GPU readback, frame pacing, and
  PNG/video sinks. `lunco-workbench` installs it for windowed hosts; the offscreen
  host installs the same capability without the workbench shell.
- **Prelude verbs:** [`prelude/recording.rhai`](../assets/scripting/prelude) —
  `shot_begin`, `shot_frame`, `shot_step`, `shot_end`, `recording_finish`, `shot_dir`.
- **Sequencer:** [`lib/shots.rhai`](../assets/scripting/lib/shots.rhai) — an episode as a
  task tree on the native behaviour-tree kernel.
- **Related:** [behaviour-trees.md](./behaviour-trees.md) (the kernel the sequencer runs
  on), [scripting-guide.md](./scripting-guide.md) (hooks, `cmd`/`query`),
  [architecture/19-unified-time-and-clock.md](architecture/19-unified-time-and-clock.md)
  (the clock this overrides),
  [architecture/51-cinematic-camera.md](architecture/51-cinematic-camera.md) (camera paths).

---

## 1. Why it exists

### Offline path-tracing samples

Settings → Graphics keeps raster rendering as its default and offers the
experimental reference path tracer without DLSS. Its positive sample target is
independent of output fps. A Twin may override preferences with
`[settings] "render.backend" = "path_tracer"` and
`"render.samples_per_pixel" = 128` in its manifest. The renderer validates
hardware and scene compatibility. Opaque standard materials are path traced;
supported sorted transparent materials, including custom jet effects, are
composited by Bevy's native forward pass before early post-processing. They do
not enter path accumulation or secondary-ray reflections. Opaque custom shaders,
alpha-tested surfaces, order-independent transparency and active local lights
block capture with an explicit error. Hidden or zero-intensity lights do not
contribute to the selected scene view.

After each logical image's fixed-step and participant barriers, the recorder
issues a fresh sampling revision and holds virtual time. The renderer gathers
samples for that revision; only a matching acknowledgement can admit the image.
Unsupported renderer intent or a GPU pipeline failure ends the armed/active
take through normal teardown and exposes `failure`; it does not wait forever.
Preparing material assets remain a distinct `renderer_pending_reason`. A new
request clears the prior per-take failure. CLI zero-image failures exit with an
error. The next `1/fps` duration is admitted after the image lands. A take locks backend
and sample settings, and `GetOfflineRecordingStatus` exposes progress and the
latest saved image's sample count. More samples cost wall time, never physics
steps. See the [qualification review](reviews/offline-path-tracing-2026-10-23.md)
for current runtime evidence and film compatibility limits.

Recording off the wall clock drops and duplicates frames: the renderer is at the mercy of
whatever else the machine is doing, and a stall becomes a visible hitch in the output.
Offline recording supplies one `1/fps` input duration per frame through the shared
transport. A held simulation advances only through explicit `StepSimulation`
requests. Frame numbering and capture completion determine shot sequencing.

The recorder saves the native update source before arming and restores it on
completion or cancellation, including requests cancelled before visual readiness.
Returning to automatic sampling uses the time owner's reanchoring operation:
elapsed time stays continuous and recording wall time creates no catch-up debt.

> **Wall-clock rate and output frame rate are fully decoupled.** Rendering faster changes
> only how long a capture takes, never what the video looks like — the output is locked to
> `fps` regardless of machine speed.

---

## 1a. One command in, one video out

The recording CLI arms through the same readiness gate and recorder state as the
commands below — never a parallel implementation:

Windowed UI recordings also require a clean rendered UI presentation. The
workbench publishes whether its current pass paints loading progress; the
render world acknowledges that clean revision after submission. `lunco-capture`
waits for both domain readiness and this acknowledgement before starting,
and checks them again before every image. Loading that returns during a take
holds the logical image without numbering a frame or admitting extra time.
This is a capture condition, not a hidden indicator or a post-production crop.
Hosts without a captured UI do not require the workbench acknowledgement.
`GetOfflineRecordingStatus` exposes `pending`, `ui_presentation_required` and
`ui_presentation_ready` alongside the frame/cycle receipt.

```sh
# Windowed: record the live window once the scene's visuals are ready
luncosim --record-offline take.mp4 --record-fps 30

# Windowless (offscreen): full GPU render stack, NO window/egui; renders into an
# offscreen target and EXITS BY ITSELF when the take drains
luncosim --api 4101 --offscreen --render-quality high \
  --record-offline take.mp4 --record-fps 30 --record-frames 300
```

| Flag | Meaning |
|---|---|
| `--record-offline <dir\|out.mp4>` | Arm recording; destination picks PNG sequence vs video (§3). |
| `--record-fps N` | Output frame rate (default 60). |
| `--record-frames N` | Stop automatically after N frames. |
| `--offscreen` | Windowless GPU mode. The scene renders into an offscreen target image; the process exits when the recording drains. Also usable with `--api` for a windowless interactive instance (API `CaptureScreenshot` reads the offscreen target). |
| `--record-size WxH` | Offscreen target resolution (default 1280x720 — the windowed default, so offscreen takes match windowed ones). |
| `--render-quality <low\|balanced\|high>` | Explicit renderer quality for this process. `high` is the highest shipped preset and raises shadow, sky-cubemap, lunar-terrain, LOD, and tessellation budgets. It does not replace shader sources authored by the USD scene. |

`CaptureScreenshot` rejects with `CommandRejected` while an offline take is
pending or active. The recorder owns its capture target for that interval;
competing window screenshots can cause Bevy to drop a capture and strand the
completed-cycle latch. Inspect a screenshot before the take or extract a frame
from the completed recording. Status and measurement queries remain available.

After the authored camera contract and the complete USD visual projection are
present, offscreen recording holds virtual time for a one-second cold-GPU render
warm-up before numbering frame 0. A CLI request made at process startup keeps
ordinary virtual time only until the authored scenario has selected its explicit
presentation camera; otherwise the readiness gate would freeze the clock before
that startup command could run. Once the camera is bound, scene completion includes
queued projections and asynchronously generated meshes, so the recorder cannot
begin with a valid camera and an incomplete render participant set. This fence lets
pipelines finish compiling without emitting a clear-color first frame.

Scene test observers use the same separation: a Rhai test under
`assets/scenarios/tests/` is headless unless it declares
`const TEST_KIND = "graphics";`, `const TEST_KIND = "render-contract";`, or
`const TEST_KIND = "editor";`. The composed USD scene only binds that observer
through `LunCoProgramAPI`/`info:sourceAsset`; `luncosim test --list` uses the
declaration to route headless tests to the deterministic CPU runner, pixel
graphics tests to the offscreen recorder, render-contract tests to the
production offscreen host without a pixel readiness gate, and editor tests to
the production windowed host. A render-contract test is appropriate for a
negative shader/material diagnostic whose authored scene intentionally has no
valid color-phase item; it must still publish a real Rhai verdict. Editor-domain
tests are not valid offscreen captures because preview and selection ownership
is UI-gated.

The graphics runner selects `--render-quality high` by default. Set
`RENDER_QUALITY=balanced` or `RENDER_QUALITY=low` for a deliberate comparison.
The ordinary Summer Space School behavior scripts remain CPU/headless tests;
invoke an offscreen run when the acceptance criterion includes shadows, sky, or
lunar-surface appearance.

Offscreen has no workbench, so no viewport camera exists: the scene must provide an
explicit active presentation camera, an authored `LocalEmbodiment` presentation camera,
or an authored camera track with a valid camera binding. The recorder consumes that
authored presentation contract and renders it through a target-born offscreen camera.
That camera mirrors the source camera's transform, projection, exposure, tonemapping,
MSAA, and camera-owned environment views (`Skybox` and generated image-based-lighting
intent); the authored camera remains the pose owner. Derived environment maps are
rebuilt by Bevy when the source cubemap changes. It does not select a camera by entity
order. A scene with no valid presentation camera fails the recording readiness contract
and reports the exact camera diagnostic instead of producing an incorrectly framed or
graded take.

### Terrain appearance fixtures

Graphics fixtures that review lunar terrain use the production DEM and shader path. The
repository-owned `assets/scenes/tests/terrain_shader_appearance.usda` fixture keeps the
DEM's absolute elevation datum and authors its camera at that same datum; a camera aimed
at elevation zero would validly produce a black, fully culled capture. The render runner
checks the resulting PNG's full-frame mean and standard deviation plus three horizontal
far/middle/near bands for independent tonal structure. This is a visual contract for
relief, not a substitute for `TerrainLodStatus` or a tile-residency diagnostic.

---

## 2. The three knobs — one writer each

Recording touches three independent pieces of engine state. **Each has exactly one
writer.** The failure this design exists to prevent is two systems writing the same knob,
with the last one each frame silently winning:

| Knob | Sole writer | Purpose |
|---|---|---|
| `TimeUpdateStrategy` | `drive_offline_clock` (in `Last`) | advance virtual time exactly `1/fps` per captured frame |
| `SimulationExecutionMode` | recorder activation/teardown | select `MaxSpeed` for the host while the take is active, then restore the prior mode |
| `WinitSettings` | `sim_focus_pace` (`lunco-modelica-ui`) | apply the execution mode and decide whether the app may sleep |
| `Window::present_mode` | the recorder, on start/stop | uncapped (`AutoNoVsync`) while recording |

`drive_offline_clock` runs in `Last`, so the strategy it writes is the one Bevy's
`TimeSystem` reads at the top of the next frame — the decision is made after every other
system has run.

`lunco-time` admits each running manual duration in full and drains at most 64
complete fixed cycles per app update. The recorder reads the same
`Time<Fixed>::overstep`: while a whole cycle remains, it writes zero additional
duration and waits before requesting readback. This preserves the logical frame
through budget and causal holds, including a 25 FPS frame at 64x. It does not
install or restore a separate virtual-clock cap. The rendered production
`fixed_budget_recording.rhai` gate exercises this boundary and rejects a
readback with whole cycles pending.

> [!IMPORTANT]
> **To keep the app awake, select `MaxSpeed` through the execution policy and hold a
> continuous `lunco_core_runtime::FramePacingDemand` request — never write
> `WinitSettings` directly.**
> `sim_focus_pace` is the sole Winit writer and applies the policy every frame. An
> unattended capture has no focused window, so the continuous request keeps the
> capture loop running at the selected execution mode.

---

## 3. The pipelined advance / capture cycle

Every render frame advances the clock exactly `1/fps` **and** requests a capture of the
frame just stepped — capture-before-advance, so a taken step is always captured. The
expensive halves overlap instead of serialising:

- **GPU readback** snapshots pixels at request time and delivers frames later — several
  captures are in flight at once, each carrying its own frame index and destination.
- **Encode + write** runs on worker tasks, never on the main thread (the main thread does
  zero per-pixel work; the image buffer is *stolen* from the delivery event in O(1)).
- **Back-pressure, not freezing:** when too many captures or saves are in flight
  (`MAX_OUTSTANDING_CAPTURES` / `MAX_IN_FLIGHT_SAVES`), the clock **pauses advancing**
  until the pipeline drains. Determinism is untouched — output is still a pure function
  of frame index; only wall-clock throughput varies.
- **Failure policy is drain-and-abort:** a failed save aborts the recording loudly and
  names the lost frame; frames already handed to workers still land. Never a silent
  mid-sequence hole. All writes go through `lunco_storage` (atomic temp+rename), so a
  killed run leaves ignorable `.tmp` files, never a corrupt frame.

The consequence for anything sequencing shots: **several `FixedUpdate` ticks can elapse
per captured frame**, and the count varies with machine speed. See §5.

### Destination decides the format

`output_dir` ending in `.mp4` / `.mkv` / `.mov` streams frames straight into a spawned
`ffmpeg` (libx264, one file, ~2 MB per 10 s at 720p instead of ~1.2 GB of PNGs); anything
else is a directory receiving a `frame_%06d.png` sequence. `ffmpeg` is probed at start —
missing, the recorder **warns loudly and demotes to a PNG sequence** in `<name>.frames/`;
it never crashes the take. The recorder writes each delivered frame synchronously to
the selected sink, so ffmpeg pipe back-pressure holds the deterministic clock just
like a slow PNG write; no writer thread or reorder buffer can change frame order.

---

## 4. Commands and queries

Fired from rhai as `cmd(name, #{…})` / `query(name)`; the full generated surface is in
[commands-reference.md](./commands-reference.md).

| Command | Params | Effect |
|---|---|---|
| `StartOfflineRecording` | `#{ output_dir: String, fps: i64 }` | Take over the virtual clock and begin step-by-step capture. `output_dir` ending in `.mp4`/`.mkv`/`.mov` streams to video (§3); else a PNG-sequence directory. |
| `StopOfflineRecording` | `#{}` | End or cancel capture and restore the prior native update source. |
| `StepSimulation` | `#{ hold: bool }` / `#{ steps: i64 }` | `hold` freezes/releases shared automatic admission; positive `steps` admit exact complete fixed cycles while held or user-paused, including physics, Modelica and simulation timers. |
| `ToggleInputOverlay` | `#{ enabled: bool }` | Show/hide the key-press readout (it is burnt into captured frames). |
| `CloseWindow` | `#{}` | Close the window and exit cleanly — what lets an unattended run terminate. |

```rhai
query("GetOfflineRecordingStatus")
// #{ active, frame_index, is_waiting_for_frame, last_capture_sim_tick }
```

> [!WARNING]
> **`query` returns the provider's data map UNWRAPPED** — there is no `#{status, data}`
> envelope — or `()` when the provider is missing or errored. Guard on a known key, never
> on a `status` field: reading a missing key in rhai yields `()`, so an envelope-shaped
> check (`st.status == "Ok"`) is true forever and stalls the sequence instead of erroring.
> `shot_frame()` returns `-1` as the honest "not recording yet".

---

## 5. Two invariants that fail silently

Both live in `prelude/recording.rhai` rather than in each scenario, because getting either
wrong looks exactly like a hang.

**1. Shot timing uses the recorder's completed-frame count.** The recorder
supplies one `1/fps` presentation interval per image. Automatic simulation uses
the shared accumulator; a frozen shot advances only by explicit fixed-cycle
requests. Callbacks run once for each pending image and never after the requested
count is complete. Capture waits for queued cycles and participant receipts.

**2. Shot sequencing uses `// @timing presentation`.** Its tasks, waits and
frame callbacks run on the presentation clock, including while shared simulation
admission is held. `StepSimulation` freezes physics, Modelica and simulation
timers together. Requested cycles use the normal fixed runner and honor scene
preparation, editor holds, participant barriers and terminal faults. Releasing
the manual hold retires pending requests; it does not alter user pause or rate.

---

## 6. The prelude verbs

| Verb | Meaning |
|---|---|
| `shot_begin(dir, fps, frozen)` | `StartOfflineRecording` into `dir` at `fps`, then `StepSimulation { hold: frozen }`. |
| `shot_frame()` | Frames captured so far in this shot, or `-1` if nothing is recording. |
| `shot_step()` | `StepSimulation { steps: 1 }` — advance a held world by exactly one shared simulation cycle. |
| `shot_end()` | `StopOfflineRecording`. |
| `recording_finish()` | Release the shared simulation hold, then `CloseWindow`. |
| `shot_dir(out, idx)` | `<out>/shot_NN/raw_frames` — **1-based and zero-padded** (`idx` 0 ⇒ `shot_01`). |

---

## 7. `frozen` — a determinism control, not an on/off switch for motion

This is the most commonly misread field in the whole surface.

> **`frozen: true` means physics advances ONLY by an explicit `shot_step()`, never by
> wall-clock.** It is not "this beat is motionless".

Three cases follow from that one rule:

| Beat | Result |
|---|---|
| `frozen: true`, no `frame` closure | Motionless. Physics is held and nothing steps it — a still under a voiceover. |
| `frozen: true` + a `frame` closure calling `shot_step()` | **Deterministically stepped**: exactly one sim step per captured frame, so the result is identical on any machine. |
| `frozen: false` | Physics free-runs on the virtual clock. Motion happens, but how much sim time elapses per captured frame is not pinned by the shot. |

The middle row is the reason for the **command, THEN step** ordering in a per-frame
closure:

```rhai
let fly = |m, f| {
    cmd("SetPorts", #{ target: m,
        writes: [["guidance_throttle", 0.6]], seq: 0, tick: 0 });
    shot_step();          // step AFTER commanding
};
```

Commanding before stepping means the setpoint this frame is the one the step integrates.
Reverse them and the step integrates the *previous* frame's setpoint, which is a one-frame
lag that varies with how the machine schedules ticks. **This ordering only means anything
under `frozen: true`** — with `frozen: false` there is no explicit step to order against.

> [!NOTE]
> **The word is used loosely elsewhere.** Scene comments and shot notes tend to say
> "frozen" to mean "motionless", which is the *first* row only. When reading an episode,
> check whether the beat also has a `frame` closure before concluding nothing moves.

---

## 8. The sequencer — `lib/shots.rhai`

An episode is a `seq` of shots on the native task-tree kernel
(`lunco-scripting-rhai-core/src/task_tree.rs`), which every scenario gets for free via `fn task(me, ctx)`.
**There is no `on_tick`** — the kernel advances the tree. `fn on_start(me, ctx)` still runs once.

```rhai
// @timing presentation
fn task(me, ctx) {
    import "/scripting/lib/shots" as shots;
    let out = "<episode>/shots";

    shots::episode([
        shots::shot(#{ out: out, idx: 0, frames: 200, frozen: true }),
        shots::shot(#{ out: out, idx: 1, frames: 300, frozen: true,
                       overlay: true, frame: fly }),
    ])
}
```

`shots::shot(spec)` fields:

| Field | Meaning |
|---|---|
| `out` | the episode's shots directory; `shot_dir(out, idx)` derives the rest |
| `idx` | 0-based shot index |
| `frames` | captured frames to hold the shot for (25 fps ⇒ 200 = 8 s) |
| `fps` | capture rate, default 25 |
| `frozen` | physics advances only by explicit `shot_step()` — see §7 |
| `overlay` | show the key-press readout; **only true on beats being flown**, since over a motionless shot it is an empty keyboard burnt into the frame |
| `begin` | optional `\|m\|`, runs once when the shot opens |
| `frame` | optional `\|m, f\|`, runs every tick with the recorder's frame index |

`shots::episode(list)` runs the shots in order, then appends `recording_finish()`.

### A shot ends after `frames` completed captures

The recorder increments `frame_index` after saving each image. A shot requesting
200 frames therefore writes exactly 200 images, numbered 0 through 199. The
per-frame callback receives those same indices, with no extra callback after
completion. A one-frame shot captures once before closing.

Production clock/capture checks use `shared_simulation_step.usda`, followed by
`RunScenarioAsset` for `shared_simulation_recording.rhai` in the same owned GPU
session. It checks coupled frame receipts for four-frame and one-frame takes,
plus a two-frame motionless take; inspect the encoded frame counts with ffprobe.
For cancellation before readiness, open and load the mesh-free
`modelica_initialization_network.usda` fixture, then run
`shared_simulation_recording_cancel.rhai`. It verifies the visual wait freezes
simulation and cancellation restores realtime task and simulation progress.

### Shot handoff is a handshake, not a bet on ordering

Closing a shot is not a bare `once(|m| shot_end())`. The kernel's `Sequence` ticks the next
child in the *same* tick a child succeeds, so a one-shot close would issue
`StopOfflineRecording` and the next shot's `StartOfflineRecording` in one tick with the
recorder's state machine in between. The close leaf instead holds until `shot_frame()`
reports "not recording" (`< 0`), making the handoff observable state rather than command
ordering.

### Task callback contract

Shot callbacks use the shared task contract: an anonymous closure with one
positional host id (`|me| ...`) or a named script callback (`Fn("name")`,
declared as `fn name(me)`). The driver supplies the persistent state map as the
bound `this` pointer for both forms, and the callback may call prelude verbs or
named helpers.

Imports used by a callback must be resolved through the normal scenario
execution context. Keep shot-library calls on the maintained prelude/library
surface rather than creating a second import or callback mechanism.

### The shot library follows the shared asset identity

```rhai
import "/scripting/lib/shots" as shots;
```

`lunco://scripting/lib/shots` is also valid. The Rhai asset loader resolves the
import through `lunco-assets` and declares it as a Bevy dependency, so the script
is ready before the task tree invokes its callback. A relative import stays under
the importing script's logical source; there is no project-wide Rhai preload.

> [!NOTE]
> **`lib/shots.rhai` is a temporary location.** It is campaign policy, not engine policy.
> It lives in the engine asset library only because consumers sit in separate twin roots
> and a twin root cannot import a sibling root. The durable fix is a shared twin root,
> which needs a way to register an extra root — roots arrive only via `TwinAdded` today,
> with no CLI flag and no environment variable. The module header carries the full note.

---

## 9. Driving a recording from a scene

A recording scene is self-contained: it carries its program as a `LunCoProgramAPI` child prim
and needs no external control loop (see
[scripting-guide.md § Persist it in the scene](./scripting-guide.md#persist-it-in-the-scene)).

```usda
def Xform "Vehicle" ( prepend references = @lunco://vessels/rovers/six_wheel_rover.usda@</SixWheelRover> )
{
    def Scope "Recorder" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @twin://my_episode/my_episode.rhai@
    }
}
```

> [!CAUTION]
> **Autoloading means loading the scene starts the recording.** A `LunCoProgramAPI` whose
> script calls `StartOfflineRecording` begins capturing — and overwriting the output
> directory — the moment the stage loads. Opening such a scene to inspect lighting or
> framing destroys the existing capture. To inspect without recording, load a
> non-recording variant of the scene, or deactivate the prim:
> ```usda
> over "Recorder" ( active = false ) {}
> ```

Relevant CLI flags (the full surface is in the [applications index](apps/README.md)):

| Flag | Effect |
|---|---|
| `--scene <path>` | Load a USD stage at startup — a relative or absolute filesystem path. The application services `load_startup_scene` resolves the containing directory as a Twin root, then mounts the canonical `twin://` address. |
| `--vertical` | `540x960` viewport, for vertical/mobile output. |
| `--no-ui` | Drop the egui overlay panels, leaving only the 3D viewport. |
| `--api <port>` | REST control listener. Not needed for a scene-driven capture. |

---

## 10. Determinism checklist

A capture is reproducible only if every source of wall-clock dependence is pinned:

- **Time** — the recorder pins the frame delta with `TimeUpdateStrategy::ManualDuration`.
- **Physics** — `frozen: true` plus one `shot_step()` per captured frame (§7). A
  free-running beat is not frame-reproducible.
- **Shot length** — driven off `shot_frame()`, never a tick counter (§5).
- **Terrain** — selection runs once per logical capture frame, and the recorder
  holds that frame's virtual time until the shared terrain-readiness status clears.
  On native hosts, cover preparation and tile baking stay asynchronous and mesh
  uploads are bounded per `Update`. Worker timing changes only the real time
  needed to produce a frame, not the simulation tick or captured LOD cover. Web
  cover preparation still needs worker transport. This is distinct from the
  authored per-terrain `LodFrozen`, which stops re-selection outright.
- **Camera** — camera paths are evaluated once per render frame on the path's own clock;
  the earlier fixed-cadence + `overstep_fraction()` smoothing was **not** reproducible
  because the residual is wall-clock derived. See
  [51-cinematic-camera.md](architecture/51-cinematic-camera.md).
