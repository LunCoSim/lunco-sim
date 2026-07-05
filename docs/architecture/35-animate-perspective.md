# 35 — Animate Perspective (timeline / sequence / scenario editor)

> Status: Draft · Audience: contributors building the animation/scenario editor
>
> **TL;DR.** A dedicated **Animate perspective** — a peer dock layout beside the
> Workbench/Sandbox views — that gives a timeline editor: a shared time ruler +
> draggable playhead, one lane per animated USD channel, a **camera track** that
> drives cuts as data (not imperative `set_camera()` calls), and an **event /
> marker lane** for named jump targets. The animation layer stays on **OpenUSD
> `timeSamples`**; the editorial layer (cuts, clips, markers) adopts
> **OpenTimelineIO (OTIO)** vocabulary. Almost every non-UI piece — seekable
> playhead, typed reversible keyframe write-back, journaling/undo, event bus —
> already exists; this doc scopes the missing **view/interaction layer** and the
> one piece of glue (a camera-track sampler).
>
> Extends [17-view-and-intent](17-view-and-intent.md),
> [18-unified-journal-and-history](18-unified-journal-and-history.md),
> [19-unified-time-and-clock](19-unified-time-and-clock.md),
> [21-domain-usd](21-domain-usd.md),
> [34-scenario-and-multidomain](34-scenario-and-multidomain.md).

## The target use case (driving example)

The lunar-descent cinematic (`scenes/sandbox/lander_cinematic.usda` +
`scenarios/lander_cinematic.rhai`) authors camera cuts and beat timing in
imperative rhai (`set_camera("TrackCam"); wait(6.0)`). You cannot **see** the
cut sequence, **scrub** back to review a beat, or **drag** a cut to retime it.
The Animate perspective turns that same cinematic into an editable timeline:

- a **camera track** whose clips are the cuts (WideTrack → TrackCam → DescentCam
  → PadCam → RoverCam), draggable to retime, driving `SetActiveCamera` at each
  cut boundary;
- **xform lanes** for the keyframed `OrbitView` dolly (already USD `timeSamples`);
- an **event lane** with the `lander_touchdown` / `rover_deployed` markers as
  labelled jump targets;
- a **transport** (play/pause/rate/seek) scrubbing the whole thing.

The rhai scenario keeps the *conditional / gated* logic (`wait_for`, arrival
predicates); the *timed, declarative* parts (cuts, keyframes, markers) move into
the timeline as data.

## Substrate this builds on (reuse, not core work)

| Capability | Mechanism | Notes |
|---|---|---|
| Seekable playhead w/ range + loop | `Playback { head, mode, rate, start, end, looping }` + `step_playhead` (`lunco-time/src/domain.rs:79,156`) | scrubs backward |
| Transport command surface | `ControlAnimation { playing, seek_secs, rate }` (`domain.rs:318`); world clock `SetTimeTransport` (`domain.rs:360`) | API+MCP+UI+rhai |
| Per-object independent clocks | `TimeDomain` / `TimeBinding` / `ResolvedDomains` (`domain.rs:50,118,126`) | |
| Preview domain auto-bind + range | `AnimationPreview` (`domain.rs:294`); `bind_animated_to_preview` grows `Playback.start/end` from clip spans (`usd-bevy/src/lib.rs:1938`) | |
| USD animation sampling | `sample_usd_animation` (`usd-bevy/src/lib.rs:1796`) reads domain time → evaluates `xformOp:*` / visibility / displayColor `timeSamples` → `Transform` | |
| Clip span per prim | `animated_time_range(reader, path) -> (f64,f64)` (`usd-bevy/src/lib.rs:1733`) | |
| Seconds ↔ timecode | `stage_time_codes_per_second` (`usd-bevy/src/lib.rs:1691`) | |
| **Typed reversible keyframe write** | `UsdOp::SetTimeSample` / `RemoveTimeSample` (`lunco-usd/src/document.rs:229,251`) — each the other's inverse, test-covered (`:1244`) | no UI callers yet |
| Journaled/undoable apply | `ApplyUsdOp { doc, op }` → `wire_usd_journal_recorders` records lossless (fwd,inv) pair (`lunco-usd/src/commands.rs:539,553`) | shared undo (UI+CLI+agent) |
| Attribute literal → typed value | `parse_attribute_value` (`usd-bevy/src/author.rs:186`) | UI only supplies a string |
| Camera switch (single target) | `SetActiveCamera{name}` → `ActivateCamera(Entity)` → `SceneViewport::active_camera`, reconciled each frame (`usd-bevy/src/camera_switch.rs`) | imperative only |
| Mounted follower cameras | `def Camera` under a body → `MountedCamera`, re-aimed each frame via `lunco:cameraLookAt` (`camera_mount.rs`) | |
| Event bus (jump-target source) | `TelemetryEvent { name, source, … }` (XTCE/YAMCS-aligned); `emit()`/`wait_for()`; `TriggerZone`/`portEvents` authored markers | |
| Declarative timeline data | JSON steps (`{wait}`,`{emit}`,`{cmd,params}`,`{move_to}`) persisted `<twin>/timelines/*.json`; `RunTimeline`/`Register`/`List`/`Get` (`lunco-scripting/commands.rs`) | |
| 1-D transport widget (the seed) | `animation_transport_section` — play/pause/rewind + scrub slider + rate (`sandbox-edit/src/ui/inspector.rs:588`) | |
| Reactive multi-instance panel host | `VizPanel` / `Panel2DCtx` read-only ctx + `defer` write (`lunco-viz/src/panel.rs,view.rs`) | pattern to copy |
| Pannable/zoomable 2D paint plane | `lunco-canvas` — Scene/Viewport/Selection/Tool/Layer/Overlay, `click_and_drag`, layered painters | track-canvas base |
| Value-curve lane | `egui_plot` via `LinePlot` (`lunco-viz/src/kinds/line_plot.rs`), time-on-X default | |
| Per-edit-category colours | `lunco_theme::JournalTokens` (`lunco-theme/src/lib.rs:313`) + `ColorAlpha` | colour keys/markers |

The **data + write-back + journaling + playhead** substrate is in place. What
remains is the **timeline VIEW/interaction layer** plus a small **camera-track
sampler** — a panel, not a system.

## What's missing (the actual work)

1. **Track/lane widget** — multi-row lanes over one shared time ruler.
2. **Ruler + draggable playhead** rendered on the tracks (today only a 1-D slider).
3. **Draggable keyframe markers** emitting `SetTimeSample`/`RemoveTimeSample` on
   create/drag/delete, with box-select + snap.
4. **Event / marker lane** — jump targets from `TelemetryEvent` names and journal
   `Marker`s; click-to-seek.
5. **Camera track + sampler** — a keyframed channel selecting the active camera
   over time, firing `SetActiveCamera` at cut boundaries. Converts the biggest
   opaque cinematic piece into data.
6. **The Animate perspective shell** — dock layout + panel registration.

## Decision 1 — Two layers: USD for curves, OTIO for editorial

Model the editor as **two composable layers**, exactly how a film pipeline splits
them (and why USD deliberately isn't an editorial format):

- **Animation layer — OpenUSD `timeSamples`.** Per-channel keyed values
  (`xformOp:translate`, `rotateXYZ`, `scale`, `visibility`, `displayColor`,
  material inputs). Already sampled, already writable via `SetTimeSample`. Each
  animated channel is one **lane**.
- **Editorial layer — OpenTimelineIO (OTIO) vocabulary.** Cuts, clips, gaps, and
  markers over the same time ruler: `Timeline` → `Track`s → `Clip`/`Gap` +
  `Marker`s, with `RationalTime`/`TimeRange` (value + rate). The **camera track**
  is an OTIO video track whose clips name a camera; the **event lane** is an OTIO
  marker track. We adopt OTIO's *concepts and naming*, not necessarily its file
  format (see Decision 4).

Rationale: USD gives lossless, journaled, per-channel animation for free; OTIO
gives the industry-standard shot/cut/marker model that USD lacks. Keeping them
separate means the camera track and event markers don't pollute the animation
schema, and either layer can be authored/edited independently.

## Decision 2 — Camera cuts become a data-driven track (slice #1)

Today the active camera is chosen imperatively (`SetActiveCamera` from rhai, a
hotkey, or avatar-add). Add a **keyframed camera channel** + a **sampler** so the
same selection is data on a timeline.

**Schema (on the scene's cinematic layer, an OTIO-shaped USD prim):**

```usda
def Scope "CameraTrack" (
    # An OTIO-style video track: clips select which camera is live over time.
    # `activeCamera` is a token channel keyed at cut boundaries (step-held).
    kind = "editorial"
)
{
    # step interpolation: the value holds until the next key (a cut, not a blend)
    token lunco:interp = "held"
    token lunco:activeCamera.timeSamples = {
        0:  "WideTrack",
        6:  "TrackCam",
        12: "DescentCam",
        # touchdown beat is gated in rhai, which writes the next key live, OR
        # a marker-anchored key resolves at run time (see Decision 3)
        28: "PadCam",
        32: "RoverCam",
    }
}
```

**Sampler** (`lunco-usd-bevy`, sibling to `sample_usd_animation`): a change-gated
system that, when the resolved domain time crosses a key boundary of an
`activeCamera` channel, fires the existing `ActivateCamera(Entity)` event (resolve
name→entity once, cache). Reuses the single-authority `reconcile_scene_viewport`
path — no new camera plumbing. Step ("held") interpolation only; a cut is
instantaneous. Backward seek re-evaluates which key is current, so scrubbing
shows the correct camera.

This slice is the highest-value first step: smallest surface, immediately
visible in the 3D view even before any timeline UI exists, and it converts the
worst opaque cinematic piece (the `set_camera` calls) into inspectable data.

## Decision 3 — Marker-anchored keys for gated beats

Some cuts are **not** at a fixed time — they fire on a sim event
(`wait_for("lander_touchdown")`). Two ways to reconcile with a fixed-time track:

- **Marker anchor (preferred).** A key can reference a named marker instead of an
  absolute time: `"@touchdown": "PadCam"`. The event lane owns marker→time
  resolution: when `TelemetryEvent{name:"touchdown"}` fires, the marker's live
  time is stamped and downstream keys shift. In authoring/preview (no live sim)
  the marker sits at its last-known or nominal time so the track is still
  scrubbable.
- **rhai writes the key.** The gated beat stays in rhai; on the event it issues
  `SetTimeSample(activeCamera, now, "PadCam")` so the track records what actually
  happened (good for replay/telemetry, not for pre-authoring).

Markers are the bridge between the **declarative timeline** (fixed beats) and the
**gated scenario** (conditional beats). The event lane renders both authored
markers (`TriggerZone`, `portEvents`, objective completions) and live emissions.

## Decision 4 — Persistence: USD-native first, OTIO interchange later

- **Authoring/runtime store = USD.** Camera track, event markers, and all
  animation keys live on the scene's USD layer(s) as `timeSamples` / typed prims,
  written through `ApplyUsdOp` (journaled, undoable, shared-undo). No new store.
- **Interchange = OTIO (optional, later).** An import/export bridge
  (`.otio` JSON ↔ our USD editorial prims) lets cuts round-trip with external NLEs
  / the wider pipeline. Deferred until the in-app editor is real; the schema in
  Decision 1–2 is chosen to map cleanly onto OTIO so this stays a straight
  translation.
- **The existing JSON timeline store** (`<twin>/timelines/*.json`,
  `RunTimeline`/`Register`/`List`/`Get`) remains the home for **imperative-ish
  step sequences**; the Animate perspective can surface and edit those as a
  fourth lane type, but the animation/camera/marker layers are USD-native.

## Decision 5 — Scrub scope: kinematic preview, not live sim

`Playback.head` scrubs backward; the physics `SimTick` is monotonic and does not.
So the editor scrubs the **kinematic preview** domain (`AnimationPreview`), not
live simulation:

- **Authoring / preview mode** — `TimeRegime::KinematicWarp`: tick frozen,
  animation sampled from `Playback.head`, full backward/forward scrub of keyed
  motion + camera cuts. This is the editor's normal mode.
- **Live / run mode** — `TimeRegime::RealtimePhysics`: physics runs forward, the
  timeline shows a read-only playhead + markers, scrubbing disabled. "Arm & run"
  transitions here.

To make the *descent itself* scrubbable (not just authored cameras), **bake** the
live physics run to `timeSamples` once (record `Transform` per frame → author via
`SetTimeSample`), then scrub the bake in preview mode. This is the deferred
"scrubbable baked timeline" from the cinematic work, now with a home.

## The Animate perspective (UI shell)

A peer perspective/dock-mode alongside Workbench/Sandbox. Registered like other
workbench panels (`VizPanel`-style multi-instance host, reactive `PanelCtx` read
+ `defer` write). Contents:

- **Timeline panel** (built on `lunco-canvas`, or direct egui `allocate_painter`
  like the Modelica diagram):
  - **Ruler** — time axis in seconds/timecode (`stage_time_codes_per_second`),
    frame ticks, current-time readout.
  - **Playhead** — draggable, bound to `ControlAnimation.seek_secs`; reads
    position from the preview `Playback.head`.
  - **Lanes** — one per animated USD channel (xform/vis/material), drawn from
    the channel's key list; the **camera track**; the **event/marker lane**.
  - **Keyframe markers** — draggable diamonds; create/move/delete →
    `ApplyUsdOp{SetTimeSample|RemoveTimeSample}`; coloured by op category via
    `JournalTokens`; box-select + snap-to-frame.
- **Transport bar** — reuse `animation_transport_section` verbs (play/pause,
  rate, rewind, seek).
- **Curve/value lane (optional)** — `egui_plot` `LinePlot` for numeric channels
  (a dope-sheet → graph-editor toggle).
- **Track header column** — per-lane name, add/mute/solo affordances.
- **Marker jump list** — click an event marker to `seek` to its time.

Frame discipline: read-only world access during the egui pass, all edits queued
via `defer`/`ApplyUsdOp` and applied after (per [42-ui-frame-discipline](42-ui-frame-discipline.md)).

## Standards alignment

| Concern | Standard | Mapping |
|---|---|---|
| Animation curves / keyframes | **OpenUSD** `timeSamples`, `timeCodesPerSecond` | the data model |
| Editorial: tracks / clips / cuts / markers | **OpenTimelineIO (OTIO)** — `Timeline`/`Track`/`Clip`/`Gap`/`Marker`, `RationalTime`/`TimeRange` | adopt vocabulary now (Decision 1–2); `.otio` interchange later (Decision 4) |
| Frame / time base | **SMPTE timecode** via `timeCodesPerSecond` | mapped |
| Telemetry / event dictionary | **XTCE / YAMCS** | `TelemetryEvent` aligned; event lane reads it |
| Mission structure | **SysML v2** | structure only — not sequencing |
| Interpolation for cuts | step ("held") vs USD linear/held/bezier | camera track = held; xform lanes = USD-native |

## Build order

1. **Camera track + sampler** (Decision 2) — keyframed `activeCamera` channel +
   change-gated system firing `SetActiveCamera`. Visible in 3D immediately, no UI
   needed. Migrate `lander_cinematic` cuts off rhai onto the track as the test.
2. **Timeline panel skeleton** — ruler + playhead on `lunco-canvas` inside a
   workbench panel; playhead bound to `ControlAnimation`. Read-only first.
3. **Lanes (read-only)** — render each animated channel's keys + the camera
   track as diamonds from the reader; no editing yet.
4. **Event / marker lane** (Decision 3) + click-to-seek jump targets.
5. **Draggable keyframes** — create/move/delete → `ApplyUsdOp{SetTimeSample}`
   (journaled/undoable). Box-select, snap.
6. **Bake mode** (Decision 5) — record a live descent to `timeSamples` so the
   physics motion scrubs.
7. **OTIO import/export** (Decision 4) — optional interchange bridge.

Steps 1–4 are data + small systems on existing substrate; 5–6 are the
interaction lift; 7 is optional pipeline interchange.

## Open questions

- **Marker-anchored keys vs. rhai-writes-key** (Decision 3) — start with rhai
  writing the key (records reality, zero new schema), add marker anchors when
  pre-authoring gated cuts is needed.
- **Camera blends** — OTIO transitions (`Transition` between clips) could drive a
  camera *dolly/blend* between cuts instead of hard cuts; held cuts only for now,
  but the schema leaves room (`lunco:interp`).
- **Multi-domain timelines** — per-vehicle preview domains already exist
  (`TimeBinding`); whether the perspective shows one global ruler or per-selection
  rulers is an open UI choice.

## Related

- [17-view-and-intent](17-view-and-intent.md) — camera selection / viewport
  single-authority (`SetActiveCamera` → `SceneViewport`).
- [18-unified-journal-and-history](18-unified-journal-and-history.md) — the
  journal that records every keyframe edit (undo/redo, shared authors).
- [19-unified-time-and-clock](19-unified-time-and-clock.md) — `Playback` /
  `TimeDomain` / `TimeRegime` the timeline scrubs.
- [21-domain-usd](21-domain-usd.md) — `timeSamples`, `UsdOp`, authoring engine.
- [34-scenario-and-multidomain](34-scenario-and-multidomain.md) — the rhai
  scenario/sequencer the timeline complements; the cinematic driving example.
- [42-ui-frame-discipline](42-ui-frame-discipline.md) — read/defer-write pass.
