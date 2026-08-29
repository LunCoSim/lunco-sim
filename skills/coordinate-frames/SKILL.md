---
name: coordinate-frames
description: >
  Use when a rover, camera, terrain tile, trajectory, planet, or link jitters,
  moves in the wrong direction, changes altitude while stationary, loses its
  orientation after a view switch, or when adding a new orbital/body-fixed
  reference frame. Also use for BigSpace, CellCoord, FloatingOrigin,
  ActivePhysicsFrame, or frame-conversion work.
---

# Coordinate frames and BigSpace

Use this runbook for coordinate changes. The concise design contract is
[`docs/architecture/45-big-space-correct-usage.md`](../../docs/architecture/45-big-space-correct-usage.md).

## Find the semantic owner first

Every astronomical or surface pose has a semantic `ReferenceFrame`:

- `World` for the persistent scene frame;
- `EclipticJ2000 { center }` for non-rotating body-centred work;
- `BodyFixed { body }` for a rotating surface frame.

Resolve it through `ReferenceFrameIndex`. Never select the first `Grid`, walk
an arbitrary parent, or add a second frame marker to a precision sub-grid.
Missing and duplicate declarations must remain errors (`None`).

## Use the existing conversion path

1. Read the authoritative f64 pose from USD, ephemeris, Modelica, or Avian.
2. Convert source → target semantic frame with the existing f64 frame helpers.
3. Resolve the target `Grid` from `ReferenceFrameIndex`.
4. Split once with `Grid::translation_to_grid`.
5. Attach/migrate atomically with `lunco_core::attach::migrate_to_grid`.

`CellCoord`, `Transform`, and `GlobalTransform` are private projection state.
They never cross a user/API/network/model boundary and never become the
authoritative source of an astronomical or physics value.

For a camera, compose the selected camera's authoritative f64 pose into the
persistent `WorldGrid` and update the grid-direct `OriginAnchor`'s
`(CellCoord, Transform)` split. `OriginAnchor` is the sole owner of
`FloatingOrigin`; cameras never receive or transfer that marker. For a
site-anchored scene, celestial placement mounts only the authored site root and
binds its physical descendants; it does not query or migrate an avatar/camera.
An authored avatar camera inherits the site's frame through the hierarchy, and
all explicit camera frame changes stay with the camera subsystem through the
same atomic migration helper. For a physical entity, keep it under
`ActivePhysicsFrame` and let
`BigSpacePhysicsBridgePlugin` own the Avian f64 pose exchange. For a trajectory
or connection line, convert both endpoints into one semantic frame before
generating cell-local geometry. Treat trajectory visibility as a work boundary:
sample ephemeris and rebuild cell-local mesh only for an active trajectory view,
and keep explicit geometry/sampling/presentation revisions. Compute results must
carry their input revisions and stale results must be discarded; missing or empty
inputs must resolve once until a frame/provider/input revision changes. When the
Celestial domain is in high-rate transport, including an independent Celestial
clock scale, hold an existing curve sample while continuing current-epoch frame
alignment; do not rebuild thousands of points on a wall-clock cadence. Trajectory
workers are polled without waiting from the main schedule, and their visualization
must not become a UI-cycle dependency.

For transform gizmos, use `transform-gizmo-bevy` only as a render-space
frontend on an unparented proxy. Capture through `SimulationPoseQuery`, keep
the proposed pose in the explicit `ActivePhysicsFrame`, convert the complete
pose back with the canonical render/grid and parent-local helpers, and commit
through one `TransformEntity` scene command. Never apply render deltas to a
parent-local `Transform`, read `GlobalTransform` as physics authority, or write
Avian `Position`/`Rotation` from editor code. Reproject from the active-frame
transaction pose after BigSpace origin/cell changes; scale handles remain
disabled until scale has its own authored contract.

For USD geometry, `xformOpOrder` is the authoritative ordered transform stack.
Read the complete composed local transform through the shared USD transform
decoder, including scale; do not inspect individual `xformOp:*` attributes in a
second path.

For waypoint labels, author `lunco:billboard*` on the waypoint and let the
generic billboard renderer consume its propagated `GlobalTransform`. The
renderer uses the shared BigSpace world-pose machinery for the camera/subject
range check; route projection does not add another distance or coordinate
conversion owner. The terrain-grid/BigSpace hierarchy and the existing
billboard path already own that conversion.
Editor-created waypoints use the canonical USD billboard authoring helper;
runtime-only waypoints attach the same `UsdBillboard` data plus the generic
`BillboardIndex` fact to the shared marker root. Keep both paths on this one
renderer; do not overwrite `Name` or add a waypoint-specific overlay.

## Do not patch symptoms

Do not add a per-frame position correction, a fallback frame, a guessed parent,
an epoch-specific offset, a raw f32 absolute position, or a second transform
writer. Those hide the ownership error and will reappear at a grid boundary or
view transition.

## Required tests

Add the smallest real regression at the owning boundary:

- frame index rejects missing and duplicate semantic grids;
- f64 pose conversion round-trips position and rotation;
- atomic migration preserves the pose across a cell boundary;
- the Avian bridge is invariant to BigSpace re-splitting and celestial-parent
  rotation;
- surface ↔ inertial camera transfer preserves target pose and up direction;
- the selected camera projects through the persistent `WorldGrid` into the sole
  `OriginAnchor`, while duplicate or missing world-shell entities fail closed.

Run focused checks first:

```sh
scripts/run_rust_tests.sh -p lunco-core --lib -j 4
scripts/run_rust_tests.sh -p lunco-celestial -j 4
scripts/run_rust_tests.sh -p lunco-usd-avian -j 4
RUSTC_WRAPPER= cargo build -p lunco-luncosim --bin luncosim -j 4
```

For visual acceptance, launch the built production binary head-full with an
explicit free API port, inspect surface and inertial views, then send the API
`Exit` command and verify the process and port are gone. Use `--no-ui` only for
headless deterministic checks.
