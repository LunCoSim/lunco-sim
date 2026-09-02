# 60 — Curvature, Elevation and Gravity

> Status: Design · Audience: contributors on terrain curvature and gravity
>
> The terrain contract is implemented and measured; the gravity work remains
> design-stage.

Companion to [`57-dem-georeferencing.md`](57-dem-georeferencing.md) (where a
raster's extent comes from) and
[`59-georeferenced-rasters-as-assets.md`](59-georeferenced-rasters-as-assets.md).

## 1. Curvature and terrain ownership

`BodyCurvature::apply` (`lunco-terrain-core/src/modifier.rs`) folds the
tangent-plane DEM onto the body sphere. Its authoritative height is:

```rust
h_in + sag
```

This preserves every DEM sample and applies only the physical body-curvature
transform. The DEM square is a hard data boundary: the terrain renderer emits no
fabricated outer wall, and the globe renderer owns the surface outside it. The
boundary continuation preserves the measured one-sided edge slope over one
raster posting before the source blend reaches the globe.

This applies to absolute and relative DEMs. An absolute sample near −1917 m
remains at its authored datum; it is not blended toward zero or another guessed
elevation.

The authored data and body radius are the only inputs to the composed surface.
The tangent-plane footprint and the globe are separate ownership regions, joined
by the measured boundary source.

### Authoring guidance

The authored DEM square is preserved through its boundary. Do not place
scene-owned terrain content outside the measured raster unless a separate,
authored globe/site composition provides the surface there. A non-DEM site
must explicitly mark its standard, ENU-aligned finite Plane with
`lunco:terrain:surfaceRole = "flat-site"`; the same handoff then derives the
finite footprint from `UsdGeomPlane` width/length and its authored xform. Ramps and
other terrain-tagged solids are not implicitly treated as the site datum.
Missing, ambiguous, rotated, or non-square flat-site geometry is a runtime
contract error.

## 2. Gravity must follow the curved ground

**Not yet implemented — the substantive item on this page.**

Once the ground curves onto the body sphere, a single world-space "down" is wrong
by construction. Gravity is currently a constant vector; on a curved patch the
true direction is the local radial (from the body centre through the point), which
diverges from the patch's tangent-plane `−Y` as you move away from the site origin.

At the Moon's radius the divergence is `d / R` in radians:

| distance from site origin | tilt vs tangent-plane down |
|---|---|
| 1 km | ≈ 0.033° |
| 8 km | ≈ 0.26° |
| 50 km | ≈ 1.65° |

Negligible for a 1 km traverse, and NOT negligible for the long-range and orbital
work this engine also does — a vehicle 50 km downrange experiences gravity 1.65°
off from what the solver applies, which integrates into a steady lateral drift.

Consequences to work through before implementing:

- **Consistency with the surface.** `BodyCurvature` already curves the ground. If
  gravity stays planar, "downhill" and "down" disagree by the same angle, so a
  parked vehicle creeps and a slope reads as steeper or shallower than it drives.
  Whatever the curvature fold does, gravity must use the SAME body centre and
  radius, from the same resource, or the two go out of step silently.
- **Where it belongs.** Gravity is environment/domain state, not core — see the
  standing rule that domain config never moves into `lunco-core`. A radial gravity
  field is a property of the anchored body, so it belongs beside
  `TerrainBodyCurvature`, sharing its `radius_m`.
- **Cost.** Per-body radial gravity is a normalize per body per tick. Cheap, but it
  must not be recomputed per contact.
- **Orbital regime.** Radial gravity toward one body is still wrong for n-body and
  for anything already integrating its own ephemeris. This must be opt-in per
  scene, exactly as celestial is, and must not silently override a scene that owns
  its own dynamics.
- **Rollback/prediction.** Gravity direction becomes position-dependent, so it must
  be derived identically on client and server or predicted bodies will diverge.

## 3. Static visual path

`lunco:layer:lodViz = false` selects a static mesh instead of streamed LOD tiles —
The path now keeps the cropped native DEM as the authoritative oracle and static
heightfield collider, then derives the optional visual mesh from that oracle. An
authored `targetRes` can reduce only the visual mesh; it cannot move the query or
physics surface onto a lossy grid. The regression is covered by
`terrain::visual_product_tests::target_resolution_changes_only_the_static_visual_product`.
