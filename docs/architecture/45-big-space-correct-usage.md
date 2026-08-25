# 45 — BigSpace and reference-frame contract

> Status: Active · Audience: contributors touching coordinates, cameras,
> physics, celestial placement, or terrain.

This is the as-built contract for the Bevy 0.19 `big_space` branch pinned in
`Cargo.lock`. `big_space` is the precision representation; it is not the
semantic reference-frame model.

## Semantic frames

`lunco-celestial::ReferenceFrame` is the only semantic frame tag used by the
runtime:

- `World` — the persistent scene/world frame.
- `EclipticJ2000 { center }` — non-rotating axes centred on a named NAIF body.
- `BodyFixed { body }` — IAU/WGCCRE rotating axes for a named body.

`ReferenceFrameIndex` resolves each declaration to exactly one `Grid`. Missing
or duplicate declarations return `None`; callers never choose the first grid,
infer a frame from entity order, or silently substitute identity.

Analytical conversions use the typed f64 frame tree. Projection between live
BigSpace grids uses `transform_pose_between_reference_frames`. The caller
declares source and target semantics; it never handles `CellCoord` directly.

## One precision hierarchy

`lunco_core::ensure_world_root` creates or returns the one persistent shell:

```text
WorldRoot (BigSpace + Grid)
└── WorldGrid (canonical scene mount)
    └── OriginAnchor (grid-direct precision frame + the one FloatingOrigin)
```

Celestial projection adds named nested grids under that shell. A body-fixed
grid rotates with its body; its body entity remains an identity child. The
matching inertial grid is a sibling and does not spin. Surface terrain,
rovers, and surface cameras are children of the body-fixed surface grid.
Inertial cameras and inertial trajectories use the non-rotating grid.

There is one `FloatingOrigin` per `BigSpace` root. It is permanently owned by
the grid-direct `OriginAnchor`. The viewport reconciler composes the selected
camera's authoritative f64 pose into `WorldGrid` and updates the anchor's
`(CellCoord, Transform)` split, so rendering recenters on the view without
moving the semantic world root, site frame, camera hierarchy, or avatar
identity.

The world shell validates this ownership and archetype contract every frame.
It also requires exactly one valid `WorldRoot` and exactly one direct-child
`WorldGrid`; an absent, duplicate, malformed, or misparented singleton is
published as a structured `world-shell` runtime error. `ensure_world_root`
fails fast if duplicate `WorldGrid` state already exists, so no consumer can
bind the first matching entity. The shell does not repair invalid origin or
world topology.

The world-grid precision parameters are set before the shell is created:
`WorldGridConfig::cell_edge_length` and `switching_threshold`. The latter is
a precision/hysteresis value, not a world-extent setting. A configuration
change does not mutate an existing `Grid`; restart the world shell to apply a
different grid definition. Non-finite or non-positive edge length, or a
negative threshold, is rejected at shell creation and published as a
`world-config` owner diagnostic if a live resource is later corrupted.

## Precision boundary

All authoritative positions, velocities, rotations, frame transforms, and
physics values are f64. The only final representation step is:

```text
semantic f64 pose → target Grid::translation_to_grid → (CellCoord, Transform)
```

`Transform` is cell-local render/bridge state. `GlobalTransform` is derived
and camera-relative; it is never an ephemeris, telemetry, network, or physics
source of truth. Use `ActiveFramePoseQuery`, `grid_relative_pose`, or the
typed frame conversion helpers for reads.

An entity migration is one atomic `(ChildOf, CellCoord, Transform)` operation
through `lunco_core::attach::migrate_to_grid`. Compute the complete f64 pose in
the destination frame first, then write the destination representation. Do
not reparent first and repair the pose next frame.

## Physics boundary

`ActivePhysicsFrame` identifies the single local frame used by Avian for the
currently mounted physical scene. `BigSpacePhysicsBridgePlugin` owns the f64
`Position`/`Rotation` ↔ BigSpace representation and collider propagation.
The persistent `WorldRoot`/`WorldGrid` shell does not install this resource;
the application binds `WorldGrid` explicitly for a flat scene, and scene
mounting replaces it with the authored site grid when that contract resolves.
Avian must not derive authority from camera-relative `GlobalTransform`.

Celestial ancestors may translate or rotate while the rover remains stable:
the bridge converts into the active body-fixed frame before physics and writes
the solved pose back through the same frame. Every physical body and collider
must belong to that active frame or be rejected by the bridge. The bridge
publishes an owning `usd-avian/physics-frame` diagnostic and raises the shared
`PhysicsHolds::FRAME_CONTRACT` hold for a missing, invalid, or disconnected
binding. The Avian `StepSimulation` set and bridge sync passes are directly
gated on the same contract, so no solver tick or force accumulation occurs
while the diagnostic is present.

## Placement and view rules

- A scene author supplies a geodetic/body anchor or another physical placement
  fact. The engine resolves the body-fixed frame and performs the mount.
- A camera request names a target and semantic frame. The camera system resolves
  the target grid and uses the atomic mount operation.
- A trajectory declares its reference frame. Its samples are converted once
  into the selected grid and rendered from cell-local segments.
- Surface mode uses the body-fixed frame and gravity-derived up. Moon/Earth
  view uses the corresponding inertial frame. Switching modes changes the
  camera's frame; it does not rotate or translate the entire world hierarchy.

## Invariants enforced in review and tests

1. No second `BigSpace` root or guessed `Grid` parent.
2. No raw f32 astronomical or physics position.
3. No site-pinning writer on the inertial solar hierarchy.
4. No duplicate semantic frame declaration.
5. No direct `GlobalTransform` read for authoritative state.
6. No per-frame repair, fallback frame, or next-frame reparent correction.
7. No physical entity outside `ActivePhysicsFrame`.
8. Scene replacement invalidates old roots before deferred despawns.

Celestial globe/site overlap is a surface-ownership problem, not a cell-size
problem. A site scene designates its finite ground owner through the USD
terrain contract; the globe LOD clips that authored footprint through
`GlobeHandoff`. No cell-size increase, depth bias, or camera-relative offset can
resolve two coincident render surfaces honestly.

Focused regression coverage lives beside the owning crates, notably
`lunco-celestial` frame/placement tests, `lunco-usd-avian` bridge tests, and
`lunco-core` world/lifecycle tests. The production check is:

```sh
RUSTC_WRAPPER= cargo build -p lunco-luncosim --bin luncosim -j 8
target/debug/luncosim --api 4101
```

Use the API `Exit` command and verify the port is released before another
session is started.
