# 43 — Celestial bodies, anchors, and orbital views

> Status: Active · Audience: contributors adding celestial bodies, sites,
> spacecraft, trajectories, or body-relative views.

`lunco-celestial` owns the solar-system semantic model. `lunco-celestial-
ephemeris` supplies the concrete ephemeris provider. USD authors the physical
intent; the engine resolves it into the existing reference-frame hierarchy.

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
- `MissionTrajectoryDecl` selects an inertial or body-fixed trajectory view.

`FrameTree` is the f64 hub-and-spoke conversion layer. It converts through the
solar inertial frame and requires the epoch, body registry, and ephemeris.
`pose.rs` resolves these components; placement then converts the complete pose
into the selected destination frame and performs one atomic BigSpace mount.

## Reference-frame hierarchy

The concrete hierarchy is:

```text
WorldRoot / Solar inertial
├── body inertial grid (non-rotating)
│   └── spacecraft and inertial trajectories
└── body-fixed grid (rotates with IAU body rotation)
    └── surface grid
        └── terrain, ground stations, rovers, surface trajectories
```

The body entity is an identity child of its body-fixed grid. The grid, not the
body mesh, carries the rotation. `ReferenceFrameIndex` maps each semantic
frame to one unique concrete grid and fails closed for missing/duplicate
declarations.

Surface terrain and rovers use the body's body-fixed frame. A star-fixed
camera or orbit trajectory uses the body's inertial sibling. A view request
names the semantic target/frame; camera and placement systems resolve the grid
and use the common f64 conversion/migration path.

## Coordinate and physics boundary

Ephemeris, anchors, orbits, velocities, and rotations remain f64 until the
destination `Grid::translation_to_grid` split. `CellCoord` and cell-local
`Transform` are storage/render representation only. `GlobalTransform` is not
telemetry or physics authority.

Avian uses the selected `ActivePhysicsFrame`. The
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
