# 60 — Curvature, Elevation and Gravity

> Status: Design · Audience: contributors on terrain curvature and gravity
>
> Written out of a live defect on the Summer Space
> School twin, where a 1 km site rendered as kilometre-tall spikes. The diagnosis
below is measured and confirmed; the terrain edge fix is now shipped, while the
gravity work remains design-stage.

Companion to [`57-dem-georeferencing.md`](57-dem-georeferencing.md) (where a
raster's extent comes from) and
[`59-georeferenced-rasters-as-assets.md`](59-georeferenced-rasters-as-assets.md).

## 1. The measured defect: the edge feather replaced measured relief

`BodyCurvature::apply` (`lunco-terrain-core/src/modifier.rs`) folds the
tangent-plane DEM onto the body sphere. The old implementation additionally
replaced the outer part of the authored crop with a synthetic apron:

```rust
sag + h_in * f + self.edge_lift_m * (1.0 - f)   // f: 1 interior → 0 at edge
```

That interior feather made every crop boundary a level cap, erasing real canyon
and rille walls. The authoritative implementation is now only:

```rust
h_in + sag
```

It preserves every DEM sample and applies only the physical body-curvature
transform. The DEM square is a hard data boundary: the terrain renderer emits
no fabricated outer wall, and the globe renderer owns the surface outside it.

This is required for absolute DEMs as well as relative DEMs. An absolute Apollo
15 sample near −1917 m must not be blended toward zero or toward a guessed apron.

Measured against predicted on the live twin before the fix (half_extent 498 m,
with the former interior feather band):

| x (z = 0) | predicted | measured |
|---|---|---|
| −400 | −960 | −934 |
| −450 | −314 | −280 |
| −480 | −48 | −43 |
| −495 | ≈ −1 | −0.41 |

The model reproduces the numbers, so the mechanism is not in doubt.

### 1a. The feather is RADIAL; the DEM is SQUARE

`f` is computed from `√(x² + z²)` against `half_extent_m`, which is a half-SIDE.
Everything outside the inscribed circle — the four corners, ≈ 21 % of the patch —
falls beyond the feather end and is flattened to `edge_lift_m`.

This is what made the defect user-visible: the school twin authors its rover at
`(−382, −384)`, i.e. radius **541 m** against a 498 m half-extent. The rover is in
a corner, so the ground beneath it reads ≈ 1 m while the vehicle sits at −1918 m.

Scale hides it elsewhere. Moonbase is a ±4000 m patch, so the same ~1.9 km
descent is spread over a 1600 m band and reads as a distant rim rather than a wall.

### Historical candidate fixes

1. **Chebyshev feather** — drove `f` from `max(|x|, |z|)` so the band followed
   the square boundary, but still replaced measured rows.
2. **Edge-elevation apron** — kept the site datum instead of zero, but still
   invented values outside the authored raster.
3. **Exact crop plus globe ownership** — the shipped implementation. The site
   DEM owns its authored square; the globe owns the outside surface.

### Authoring guidance

The authored DEM square is preserved through its boundary. Do not place
scene-owned terrain content outside the measured raster unless a separate,
authored globe/site composition provides the surface there.

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
