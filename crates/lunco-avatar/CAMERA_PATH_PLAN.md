# Camera-Path System — Phased Plan

Spline-keyframed cinematic camera moves, built on Bevy 0.18's built-in animation
(`AnimationClip` / `AnimationGraph` / `AnimationPlayer`) and the existing
`lunco-avatar` camera infrastructure. Scrub/seek and recording layer on top later.

## Goals / Non-goals

- **Goal:** author an ordered set of camera keyframes (time → position, look-at/FOV),
  interpolate them smoothly, play/seek them deterministically, and drive the workbench
  camera from the result.
- **Goal:** reuse the engine animation stack, not a bespoke tween loop.
- **Non-goal (this plan):** video export, timeline UI for *all* entities, recording.
  Those are follow-ups that consume what this plan produces.

## What we build on (existing infra)

- Camera modes + smoothing: `crates/lunco-avatar/src/lib.rs`
  (`SpringArmCamera`, `OrbitCamera`, `FreeFlightCamera`, `SurfaceCamera`, `FrameBlend`,
  `CameraDefaults`, `AdaptiveNearPlane`, `SurfaceRelativeMode`).
- Camera commands: `crates/lunco-avatar/src/commands.rs`
  (`FocusTarget`, `FollowTarget`, `PossessVessel` — pattern to follow for new commands).
- Deterministic clock: `SimTick(u64)` + `TimeWarpState` (`lunco-core`),
  `CelestialClock` (`crates/lunco-celestial/src/clock.rs`).
- Viewport sync: `crates/lunco-workbench/src/viewport.rs`.
- Big-space coords: keyframes authored in absolute solar coords, converted to the
  camera's current grid per frame (same approach `FrameBlend` already uses).

## Decisions to lock before Phase 1

- **D1 — Authoring frame:** keyframes store absolute solar-coord `DVec3` position +
  either a world-space look-at point OR a target entity (target-relative). Default:
  support both (`Aim::Point(DVec3)` / `Aim::Entity(Entity)`); convert to grid-local at
  eval time exactly like `FrameBlend`.
- **D2 — Time base:** track time is in **seconds of track-local time**, advanced by an
  `AnimationPlayer`. Recording later drives this player at fixed dt for frame-lock; live
  preview drives it from wall clock × `TimeWarpState`. Player owns seek.
- **D3 — Interpolation:** position = Catmull-Rom spline through keyframe points (C1,
  passes through points). Rotation = slerp of look-at-derived quats. FOV = eased scalar.
  Per-segment easing enum (linear / smoothstep / ease-in-out).
- **D4 — New camera mode:** add `CinematicCamera` marker alongside the 4 existing modes.
  While active it owns the camera transform; other mode systems must `run_if(not cinematic)`.

> Resolve D1/D3 specifics with one quick spike if uncertain; otherwise defaults above.

---

## Phase 0 — Spike: animate the camera via the engine (½ day)

Prove the engine path before committing structure.

- Implement `AnimatableProperty` for camera `Transform` (translation + rotation) — or
  confirm `animated_field!(Transform::translation)` covers it in 0.18.
- Hand-build a 3-keyframe `AnimationClip`, attach `AnimationPlayer` to the camera entity,
  confirm it plays and `seek` works.
- **Exit criteria:** camera visibly moves through 3 points under an `AnimationPlayer`,
  and `player.seek_to(t)` jumps deterministically. No spline yet — linear is fine.
- **Output:** throwaway `bin/camera_path_spike.rs` (untracked scratch), notes on whether
  `Transform` animates cleanly under big-space (it may need grid-local baking — see D1).

---

## Phase 1 — Data model + spline eval (pure, tested)

Author the camera-path types and a **pure** evaluator. No Bevy systems yet — this is the
testable core (mirrors how `proxy_wheel` helpers were extracted + unit-tested).

New module `crates/lunco-avatar/src/camera_path.rs`:

- `struct CameraKey { t: f32, pos: DVec3, aim: Aim, fov: f32, ease: Easing }`
- `enum Aim { Point(DVec3), Entity(Entity) }`
- `enum Easing { Linear, SmoothStep, EaseInOut }`
- `struct CameraPath { keys: Vec<CameraKey> }` (sorted by `t`; duration = last `t`)
- Pure fns:
  - `catmull_rom(p0,p1,p2,p3,u) -> DVec3` (with endpoint duplication for first/last)
  - `eval_pos(path, t) -> DVec3`
  - `eval_aim(path, t, resolve_entity) -> DVec3` (caller supplies entity→pos lookup)
  - `eval_fov(path, t) -> f32`
  - `apply_ease(e, u) -> u'`

**Tests** (`#[cfg(test)]` in the module): spline passes through keys at key times;
endpoints clamp; monotonic time; easing endpoints (0→0, 1→1); FOV interp; empty/single-key
guards. Target ~10 tests, all pure, no app.

- **Exit criteria:** `cargo test -p lunco-avatar` green (`-j2`).

---

## Phase 2 — `CinematicCamera` mode + clip baking (wiring)

Connect the pure model to the live camera through the engine animation stack.

- Add `CinematicCamera` marker component + register it as a camera mode in the avatar plugin.
- `bake_clip(path) -> AnimationClip`: sample the Phase-1 evaluator into an `AnimationClip`
  (either dense-sample curves at a fixed rate, or feed control points into Bevy curve types
  — pick per Phase-0 findings). Aim/look-at resolved to rotation at bake time for
  `Aim::Point`; `Aim::Entity` paths re-bake on demand or eval live (start with Point-only).
- System `drive_cinematic_camera` (PostUpdate, before `CameraUpdateSystems`,
  `run_if(active mode == Cinematic)`): reads the `AnimationPlayer` state, converts the
  animated absolute pose → camera's current big-space grid, writes camera `Transform`.
- Gate the 4 existing mode systems with `run_if(not cinematic)` so they don't fight it.
- Respect `AdaptiveNearPlane` while cinematic (large scenes).

- **Exit criteria:** a baked path plays in the running workbench; existing modes don't
  interfere; switching out of cinematic restores normal control.

---

## Phase 3 — Commands + API/MCP surface (control)

Make it drivable headlessly and from UI, following the `commands.rs` pattern.
(These are genuine product verbs, not test-only — consistent with the "real features =
commands" rule.)

- `#[Command] PlayCameraPath { path_id }` — activate `CinematicCamera`, start player.
- `#[Command] StopCameraPath` — restore prior mode.
- `#[Command] SeekCameraPath { t }` — `player.seek_to(t)` (deterministic scrub).
- `#[Command] SetCameraPath { keys: Vec<CameraKeyWire> }` — author/replace the active path
  (wire-friendly: `DVec3` as `[f64;3]`, `Aim::Entity` as `GlobalEntityId` per the id codec).
- Optional `AddCameraKey` / `RemoveCameraKey` for incremental authoring (matches
  "build node-by-node" preference) — can defer.
- Add matching MCP tools under `mcp/` (mirror `focus_target`/`follow_target`).

- **Exit criteria:** drive a full path play + seek via API; entity-id translation round-trips.

---

## Phase 4 — Authoring UI (lunco-viz panel)

Per the viz-layer convention, the timeline/scrubber panel lives in `lunco-viz`, surfaced as
a workbench panel — not in a binary.

- Scrubber: current `t`, play/pause, seek (drives `SeekCameraPath`).
- Keyframe list: add "key at current camera pose" (capture live transform → `CameraKey`),
  delete, reorder, edit `t`/FOV/easing.
- "Preview" toggle = play in viewport; gizmo line through keyframe positions in the scene.
- Read state inline (active `CameraPath` resource), no shadow entities (UI-data rule).

- **Exit criteria:** author a path by flying the camera + capturing poses, scrub it, play it,
  all in-app.

---

## Phase 5 — Handoff to recording (out of scope here, named for continuity)

This plan deliberately produces a **deterministic, seekable** camera animation so the
recording work can consume it without rework:

- Drive the `AnimationPlayer` from a fixed-dt loop keyed on `SimTick` (not wall clock).
- Feed each rendered frame to `bevy_capture` (headless encoder) → frame-locked video.
- `EasyScreenRecordPlugin` (Bevy 0.18, native/Linux) gives a free "record session" button
  in the meantime.
- Video encoding is **native-only** — never in wasm.

---

## Risk notes

- **Big-space / FloatingOrigin:** animating `Transform` directly may break across grid
  recentering. Mitigation: animate in absolute solar coords (a custom `AnimatableProperty`
  or a resource the drive system reads), convert to grid-local per frame like `FrameBlend`.
  Phase 0 must settle this.
- **Mode conflicts:** every existing camera system writes the transform; all must be gated
  off while cinematic or the result jitters.
- **`Aim::Entity` + baking:** a moving target can't be fully pre-baked. Start Point-only;
  add live-eval aim in Phase 2/3 if needed.
- **wasm:** the camera-path system itself is wasm-safe (pure math + engine animation).
  Only recording is native-gated.

## Build discipline

- Pure core (Phase 1) is the only place with heavy unit tests; verify with
  `cargo test -p lunco-avatar -j2`.
- Don't reflexively build per edit — build to verify a completed phase.
- One-crate blast radius for Phases 0–3 (`lunco-avatar`); Phase 4 touches `lunco-viz` +
  workbench panel registration.
