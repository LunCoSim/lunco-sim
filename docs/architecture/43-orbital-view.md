# 43 — Celestial bodies and orbital views

> Status: Active · Audience: contributors adding celestial bodies, sites,
> spacecraft, or body-relative views.

`lunco-celestial` owns natural-body facts, reference frames, and analytic
propagation contracts. `lunco-celestial-ephemeris` supplies the built-in
Sun/Earth/Moon analytic model. `lunco-celestial-spatial` projects those facts
into BigSpace. Reusable frame lookup and surface-coordinate contracts live in
`lunco-celestial-spatial-core`, so camera, avatar, networking, telemetry, and
USD projection consumers do not install terrain/globe/link runtime merely to
read or publish a celestial fact. USD owns scene composition and authored
motion; Rhai owns any scene-specific asset selection.

## Body catalog

`CelestialBodyRegistry` is the single catalog of body facts. Each
`BodyDescriptor` contains:

- human name and canonical NAIF/ephemeris id;
- mean radius and gravitational parameter;
- optional sphere-of-influence and parent id;
- the IAU/WGCCRE rotation model.

Known ids are named in `lunco_celestial::ephemeris_id` (`SUN`, `EARTH`,
`MOON`, and the barycentre ids). Runtime code must use those constants, not
repeat integer literals. A projected `CelestialBody` is created with
`BodyDescriptor::body_component`, so name/id/radius cannot drift between
spawners.

The IAU rotation elements are the only rotation authority. Pole, prime
meridian, spin rate, and body-fixed quaternion are derived from those elements
at the requested epoch. Missing catalog/ephemeris data returns `None`; it is
never replaced with the Sun's centre or identity rotation.

## Physical placement

User-facing components describe physical intent:

- `GeodeticAnchor` places a point on a named body's surface;
- `KeplerOrbit` describes a body-centred orbit;
- `LibrationAnchor` describes an Earth–Moon/Sun–body libration point;
Natural bodies are evaluated from the shared `EphemerisResource`; moving scene
objects use ordinary USD time-sampled transforms. A model's catalog and geometry
do not make it a control target: possession follows its authored control surface,
and camera framing operates on the requested scene entity.

### Authored scene motion

A moving scene object is an ordinary `UsdGeomXformable` prim with a standard
`double3 xformOp:translate` time-sample channel. A long mission path may live in
a separate USD asset referenced by the scene; the referenced prim owns its
samples and remains addressable through normal USD composition.

`UsdAnimationPlugin` samples unbound USD animation from
`SimulationPresentationTime`, derived from completed physical ticks. The
presentation sample stays between completed states and holds when the physical
clock pauses. After the scene and queued projections settle, the scene-time
policy uses the TDB epoch authored on the physical scene root, or current
computer UTC converted to TDB when the root has no epoch. Time-sampled motion
waits for that selection. Author `LunCoEpochAPI` and its non-zero
`lunco:time:epochJd` for a repeatable sample origin. USD time codes map to elapsed
seconds through the stage's `timeCodesPerSecond` metadata (24 when omitted by
USD); authored sample codes are elapsed seconds multiplied by that rate.
Keep source timestamps and state values in USD's native double precision.
For a moving prim directly beneath the world `Grid`, keep translation in a
`double3 xformOp:translate`; the USD projection splits that f64 position into
BigSpace cells before narrowing the cell-local render transform. More complex
double-precision transform stacks that cannot be split without losing their
meaning fail visibly at projection.

`FrameTree` is the f64 hub-and-spoke conversion layer. It converts through the
solar inertial frame and requires the epoch, body registry, and ephemeris.
`pose.rs` resolves these components; placement then converts the complete pose
into the selected destination frame and performs one atomic BigSpace mount.

## Reference-frame hierarchy

The concrete hierarchy is:

```text
WorldRoot / Solar inertial
├── body inertial grid (non-rotating)
│   └── spacecraft and other inertial scene objects
└── body-fixed grid (rotates with IAU body rotation)
    └── surface grid
        └── terrain, ground stations, rovers
```

The body entity is an identity child of its body-fixed grid. The grid, not the
body mesh, carries the rotation. `lunco-celestial-spatial-core::ReferenceFrameIndex` maps each semantic
frame to one unique concrete grid and fails closed for missing/duplicate
declarations.

Surface terrain and rovers use the body's body-fixed frame. A star-fixed
camera or inertially moving scene object uses the body's inertial sibling.
Camera and placement systems resolve the semantic frame and use the common f64
conversion/migration path. Orbital camera views remain avatar presentation
state; moving scene objects use the USD animation path above.

## Orbital camera poses

`OrbitCamera` is an avatar-owned presentation mode. `OrbitViewHistory` stores
the last user-controlled pose per stable celestial ephemeris id, so switching
between Moon, Earth, or another body—and re-entering orbit from that body's
surface—restores that body's own yaw, pitch, distance, damping, and vertical
offset. The first surface-to-orbit entry has no saved pose and derives its arm
from the live radial position for continuity. The history is transient and is
cleared when the active Twin closes or when the avatar is demoted; it is not a
scene-wide celestial fact and is never persisted as USD.

When a body has no saved user pose, `FocusTarget` seeds the orbit direction
from the camera's current region in the target's explicit inertial BigSpace
grid. It does not use a fixed world-axis or Sun-facing arrival. The existing
f64 frame conversion and atomic grid migration remain the only placement path;
the arrival marker is consumed by the orbit writer once and cannot become a
second transform writer.

## Coordinate and physics boundary

Ephemeris, anchors, orbits, velocities, and rotations remain f64 until the
destination `Grid::translation_to_grid` split. `CellCoord` and cell-local
`Transform` are storage/render representation only. `GlobalTransform` is not
telemetry or physics authority.

Avian uses the explicitly bound `ActivePhysicsFrame`. The
`BigSpacePhysicsBridgePlugin` owns f64 pose exchange and collider propagation,
so body motion above a lunar surface does not become rover motion. Physical
entities outside the active frame are rejected instead of being silently
reinterpreted.

## Adding a celestial object

1. Add or reuse a `BodyDescriptor`; use a named `ephemeris_id` constant.
2. Author a physical intent component or USD metadata, not a grid/cell.
3. Let `FrameTree` and the placement systems resolve the f64 pose.
4. Attach it atomically to the semantic destination grid.
5. Add a frame round-trip and a real placement/physics regression.

Do not add a second body catalog, cached rotation-rate copy, raw f32 absolute
position, guessed grid parent, or fallback for missing ephemeris data.

The production scene gate exercises authored double-precision USD animation,
its BigSpace cell split, and its physical-clock pause behavior.
