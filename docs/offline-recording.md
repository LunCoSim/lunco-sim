# Offline Recording

How the engine renders a **deterministic, frame-exact image sequence** — the mechanism
that decouples Bevy's update loop from the system clock, and the rhai surface
(`prelude/recording.rhai` + `lib/shots.rhai`) that sequences shots on top of it.

- **Recorder:** `lunco-workbench` — the capture systems, `drive_offline_clock`, the
  screenshot readback.
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

Recording off the wall clock drops and duplicates frames: the renderer is at the mercy of
whatever else the machine is doing, and a stall becomes a visible hitch in the output.
Offline recording inverts the relationship — **virtual time is advanced by the recorder,
one `1/fps` step per frame it actually captures**, so the output is a function of the
frame index and nothing else.

> **Wall-clock rate and output frame rate are fully decoupled.** Rendering faster changes
> only how long a capture takes, never what the video looks like — the output is locked to
> `fps` regardless of machine speed.

---

## 1a. One command in, one video out

The recording CLI arms through the same readiness gate and recorder state as the
commands below — never a parallel implementation:

```sh
# Windowed: record the live window once the scene's visuals are ready
sandbox --record-offline take.mp4 --record-fps 30

# Windowless (offscreen): full GPU render stack, NO window/egui; renders into an
# offscreen target and EXITS BY ITSELF when the take drains
sandbox --offscreen --record-offline take.mp4 --record-fps 30 --record-frames 300
```

| Flag | Meaning |
|---|---|
| `--record-offline <dir\|out.mp4>` | Arm recording; destination picks PNG sequence vs video (§3). |
| `--record-fps N` | Output frame rate (default 60). |
| `--record-frames N` | Stop automatically after N frames. |
| `--offscreen` | Windowless GPU mode. The scene renders into an offscreen target image; the process exits when the recording drains. Also usable with `--api` for a windowless interactive instance (API `CaptureScreenshot` reads the offscreen target). |
| `--record-size WxH` | Offscreen target resolution (default 1280x720 — the windowed default, so offscreen takes match windowed ones). |

Offscreen has no workbench, so no viewport camera exists: it activates the scene's first
**authored** `SceneCamera` whose render pipeline is bound (the scene's own framing intent
— cinematic paths drive authored cameras anyway) and points every window-targeting camera
at the offscreen image. A scene that authors no camera records black and says so in the
log.

---

## 2. The three knobs — one writer each

Recording touches three independent pieces of engine state. **Each has exactly one
writer.** The failure this design exists to prevent is two systems writing the same knob,
with the last one each frame silently winning:

| Knob | Sole writer | Purpose |
|---|---|---|
| `TimeUpdateStrategy` | `drive_offline_clock` (in `Last`) | advance virtual time exactly `1/fps` per captured frame |
| `WinitSettings` | `sim_focus_pace` (`lunco-modelica`) | whether the app may sleep |
| `Window::present_mode` | the recorder, on start/stop | uncapped (`AutoNoVsync`) while recording |

`drive_offline_clock` runs in `Last`, so the strategy it writes is the one Bevy's
`TimeSystem` reads at the top of the next frame — the decision is made after every other
system has run.

> [!IMPORTANT]
> **To keep the app awake, hold a `lunco_core::KeepAwake` token — never write
> `WinitSettings` directly.** `sim_focus_pace` rewrites it every frame and is the last
> writer, so any direct write is reverted on the next frame. An unattended capture has no
> focused window, and under the `reactive_low_power` throttle the app sleeps between
> redraws: **measured 2–10 s per frame versus ~50 ms awake.**

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
it never crashes the take. A dedicated writer thread owns frame ordering (video is the
one strictly-sequential sink in the pipeline) and finalizes the container when the
recording drains.

---

## 4. Commands and queries

Fired from rhai as `cmd(name, #{…})` / `query(name)`; the full generated surface is in
[commands-reference.md](./commands-reference.md).

| Command | Params | Effect |
|---|---|---|
| `StartOfflineRecording` | `#{ output_dir: String, fps: i64 }` | Take over the virtual clock and begin step-by-step capture. `output_dir` ending in `.mp4`/`.mkv`/`.mov` streams to video (§3); else a PNG-sequence directory. |
| `StopOfflineRecording` | `#{}` | End capture, restore automatic real-time ticking. |
| `StepPhysics` | `#{ hold: bool }` / `#{ steps: i64 }` | `hold` freezes/releases physics integration; `steps` advances a held world by exactly that many fixed steps. |
| `ToggleInputOverlay` | `#{ enabled: bool }` | Show/hide the key-press readout (it is burnt into captured frames). |
| `CloseWindow` | `#{}` | Close the window and exit cleanly — what lets an unattended run terminate. |

```rhai
query("GetOfflineRecordingStatus")
// #{ active, frame_index, is_waiting_for_frame, video,
//    outstanding_captures,   // GPU readbacks not yet delivered
//    pending_saves }         // encode/write workers still running
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

**1. Shot timing is the recorder's `frame_index`, never a local tick counter.** Virtual
time advances `1/fps` per *captured* frame and several fixed ticks elapse per frame — a
tick counter ends every shot early and desyncs any narration cut against the shot list.

**2. Freezing a beat is a PHYSICS hold, never a world-clock pause.** Pausing the world
clock stops `FixedUpdate`, so the scenario that paused it cannot run again to unpause
itself: the shot hangs and the recorder spools frames until the process is killed. A
physics hold leaves the scenario ticking.

---

## 6. The prelude verbs

| Verb | Meaning |
|---|---|
| `shot_begin(dir, fps, frozen)` | `StartOfflineRecording` into `dir` at `fps`, then `StepPhysics { hold: frozen }`. |
| `shot_frame()` | Frames captured so far in this shot, or `-1` if nothing is recording. |
| `shot_step()` | `StepPhysics { steps: 1 }` — advance a held world by exactly one physics frame. |
| `shot_end()` | `StopOfflineRecording`. |
| `recording_finish()` | Release the physics hold, then `CloseWindow`. |
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
        writes: [["external_throttle", 0.6]], seq: 0, tick: 0 });
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
(`lunco-scripting/src/task_tree.rs`), which every scenario gets for free via `fn task(me)`.
**There is no `on_tick`** — the kernel advances the tree. `fn on_start(me)` still runs once.

```rhai
fn task(me) {
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

### A shot ends at `frames - 1`

Frame indices `0..frames-1` are captured, so a 200-frame shot writes **199** files. This
matches the hand-rolled advance the module replaced. **Do not "fix" this off-by-one** — a
complete shot is `frames - 1` files, and any verification that counts frames must expect
that.

### Shot handoff is a handshake, not a bet on ordering

Closing a shot is not a bare `once(|m| shot_end())`. The kernel's `Sequence` ticks the next
child in the *same* tick a child succeeds, so a one-shot close would issue
`StopOfflineRecording` and the next shot's `StartOfflineRecording` in one tick with the
recorder's state machine in between. The close leaf instead holds until `shot_frame()`
reports "not recording" (`< 0`), making the handoff observable state rather than command
ordering.

### Closures cannot see imports

A top-level or in-`fn` `import` **is** visible inside a named `fn` — the runtime re-runs an
imports-only AST per hook (`top_level_import_source` in `lunco-scripting/src/world_bridge.rs`).
It is **not** visible inside a closure invoked through `FnPtr::call`, which is exactly how
the task kernel runs leaf callbacks: `FnPtr::call` takes an `AST` and has no `eval_ast`
switch, so there is nowhere to re-run the imports.

**A `begin` or `frame` closure may therefore only call prelude verbs** (`cmd`, `query`,
`get`, `shot_step`, …). Those are registered as an engine **global module**, not resolved
through the import stack, so they resolve from any calling context. A module alias inside
one of these closures is a runtime error mid-shot, which stalls the beat and spools frames.

### The import is spelled `/scripting/lib/shots`, not `lunco://`

```rhai
import "/scripting/lib/shots" as shots;
```

**MEASURED:** `import "lunco://scripting/lib/shots"` fails with `Module not found`. Script
ids are registered by `asset_path::anchor_of`, which returns a **bare relative path** for
Bevy's default asset source — the engine library registers as `scripting/lib/shots.rhai`,
with no scheme. `lunco://…` has a scheme, so `canonicalize` passes it through untouched and
the lookup misses.

**This is asymmetric with USD references**, where `lunco://` *is* the way to name an engine
asset (see [56-asset-resolution-and-cache.md](architecture/56-asset-resolution-and-cache.md)),
so the mistake is an easy one to make twice.

The **leading slash** is what makes the import work from a twin: it is the "absolute from
the assets root" form, which `canonicalize` resolves without the importing script's anchor.
A bare `"scripting/lib/shots"` would anchor to the importer's own root and look for
`twin://<importer_root>/scripting/lib/shots.rhai`.

> [!NOTE]
> **`lib/shots.rhai` is a temporary location.** It is campaign policy, not engine policy.
> It lives in the engine asset library only because consumers sit in separate twin roots
> and a twin root cannot import a sibling root. The durable fix is a shared twin root,
> which needs a way to register an extra root — roots arrive only via `TwinAdded` today,
> with no CLI flag and no environment variable. The module header carries the full note.

---

## 9. Driving a recording from a scene

A recording scene is self-contained: it carries its program as a `LunCoProgram` child prim
and needs no external control loop (see
[scripting-guide.md § Persist it in the scene](./scripting-guide.md#persist-it-in-the-scene)).

```usda
def Xform "Vehicle" ( prepend references = @lunco://vessels/rovers/six_wheel_rover.usda@</SixWheelRover> )
{
    def LunCoProgram "Recorder"
    {
        uniform asset lunco:program:sourceAsset = @twin://my_episode/my_episode.rhai@
    }
}
```

> [!CAUTION]
> **Autoloading means loading the scene starts the recording.** A `LunCoProgram` whose
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
| `--scene <path>` | Load a USD stage at startup — a path relative to the `assets/` root, **or an absolute path anywhere on disk**, in which case `load_startup_scene` mounts the containing directory as a twin root. |
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
- **Terrain** — LOD streaming runs in **lockstep** while a recording is active
  (`TerrainStreamLockstep`, set by `lunco-sandbox` off `OfflineRecordingState::active`).
  Normally a tile bake lands whenever its async task finishes, so the frame a given
  tile pops in is wall-clock dependent; in lockstep the frame blocks on the bake
  instead. Selection stays live — the shot still refines as the camera moves — only
  its timing is pinned. Distinct from the authored per-terrain `LodFrozen`, which
  stops re-selection outright.
- **Camera** — camera paths are evaluated once per render frame on the path's own clock;
  the earlier fixed-cadence + `overstep_fraction()` smoothing was **not** reproducible
  because the residual is wall-clock derived. See
  [51-cinematic-camera.md](architecture/51-cinematic-camera.md).
