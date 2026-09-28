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

This preserves every local DEM sample and applies only the physical
body-curvature transform. A site handoff uses only its installed cropped DEM.
For rendering, the active crop's border datum sets the visible globe shell
radius; the celestial body's canonical radius and all physical state remain
unchanged. The handoff measures the crop's edge heights and one-sided slopes,
continues them for one local posting, and feathers the remaining edge relief
into that datum-aligned sphere over a crop-specific collar. This avoids making
an unsupported body-scale ramp from a local absolute elevation to zero.

The render shell and collar close the finite crop; they do not describe measured
terrain outside it. The collar width is derived from the largest crop-edge
relief and slope at a bounded grade. Each Twin derives the shell datum and
transition from its own crop. No body-wide raster, download, or Apollo-specific
identity is part of this contract.

The handoff is parameterized by the active crop's georeference, grid spacing,
border datum, and measured boundary profile; it has no site or Twin identity
special case. The current globe owner accepts one built crop per body. If
multiple crops target that same body, it reports the ambiguous input and omits
the handoff rather than choosing one by size or load order.

The update order keeps the simulation input separate from visual handoff work:
USD terrain projection publishes georeferencing, celestial curvature is
published before `TerrainSurfaceSet::Build`, and deferred commands are applied
before the build captures its oracle inputs. The DEM-to-globe handoff,
appearance adoption, and globe LOD run in
`RuntimeCycleSet::Visualization`, after terrain builds and UI commands. Raster
loading and tile mesh generation are asynchronous; visualization only polls
without waiting and commits completed meshes in stable coarse-to-fine coordinate
order under per-frame count and byte budgets. Worker completion timing can
change when a visual tile appears, but it cannot change the physical oracle or
simulation state.

The globe cutout is projected in the site's local tangent chart at the crop's
border datum. Its inner edge is split at authored DEM postings. The generated
annulus continues those edge samples and slopes, then meets the datum-aligned
sphere at the outer edge with matching height and slope. Both the cutout and
full collar are clipped into each affected globe tile in the existing async
mesh bake. LOD refines the collar to a bounded fraction of its width; it does
not force high resolution across the whole body.

The visible shell radius changes with the active crop datum, while celestial
placement, gravity, terrain queries, and physics retain the canonical body
radius. The shell is a visualization approximation where no surrounding DEM is
available. DEM registration, reference radius, and height units remain part of
the geometry contract, not shader settings.

### Authoring guidance

The authored DEM square is preserved through its boundary; a Twin needs only
the cropped DEM used by its terrain. Do not place scene-owned terrain content
outside the measured local raster unless a separate authored source provides
it. A non-DEM site
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
