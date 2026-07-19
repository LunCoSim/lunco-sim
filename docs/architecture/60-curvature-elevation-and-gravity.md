# Curvature, Elevation and Gravity — planned improvements

**Status:** PLANNED. Written 2026-07-19 out of a live defect on the Summer Space
School twin, where a 1 km site rendered as kilometre-tall spikes. The diagnosis
below is measured and confirmed; the fixes are proposals, not decisions.

Companion to [`57-dem-georeferencing.md`](57-dem-georeferencing.md) (where a
raster's extent comes from) and
[`59-georeferenced-rasters-as-assets.md`](59-georeferenced-rasters-as-assets.md).

## 1. The measured defect: the edge feather descends ABSOLUTE relief

`BodyCurvature::apply` (`lunco-terrain-core/src/modifier.rs`) folds the
tangent-plane DEM onto the body sphere and feathers its edge so the patch meets
the surrounding globe tiles instead of floating the sagitta above them:

```rust
sag + h_in * f + self.edge_lift_m * (1.0 - f)   // f: 1 interior → 0 at edge
```

`h_in * f` scales the height toward zero across the feather band, so at the edge
the surface sits at `edge_lift_m` (1 m) above the sphere.

**That is only meaningful when `h_in` is relative to the reference sphere.** For a
DEM carrying ABSOLUTE elevations the term interpolates between "the real ground"
and "the datum", which is not a physical surface at all. The Apollo 15 site sits
at ≈ −1917 m, so the patch descends 1917 m inside the band.

Measured against predicted on the live twin (half_extent 498 m, `feather_from`
0.6 ⇒ band r = 299…498 m):

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

### Candidate fixes

1. **Chebyshev feather** — drive `f` from `max(|x|, |z|)` so the band follows the
   square DEM boundary. Fixes the corner flattening outright. Does not on its own
   rescue a small site whose content lives inside the band.
2. **Feather toward the patch's own edge elevation** rather than the datum: blend
   `h_in` toward the mean height of the DEM border, so an absolute-elevation patch
   stays at its own elevation and only the curvature `sag` applies. Principled for
   absolute DEMs; changes how a patch meets globe tiles, which moonbase relies on.
3. **Apply curvature only when there is a globe to meet.** A standalone
   site-anchored DEM with no surrounding body tiles has nothing to blend onto, and
   the feather is pure damage.

Preference at time of writing: **3 + 1**. (2) is a larger call about what a
site-anchored DEM means and should not be made casually.

### Authoring guidance until this is fixed

Keep scene content well inside `feather_from × half_extent` (default 0.6). On a
1 km site that is a 300 m radius — smaller than authors expect, and the reason the
school twin's 380–395 m placements fell off the usable surface.

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

## 3. Related open question: the no-LOD path

`lunco:layer:lodViz = false` selects a static mesh instead of streamed LOD tiles —
conceptually "load it all at once". On the school twin that path rendered **nothing
at all**, while `lodViz = true` rendered (revealing the feather defect above).

The static-mesh branch does build a mesh: `needs_static_grid = !collider_ring ||
!lod_viz` is true, and `mesh = if lod_viz { None } else { Some(...) }` yields
`Some`, with a material confirmed applied by `wire_terrain_materials`. Mesh and
material both exist, yet nothing draws.

**Unresolved.** If "no LOD" is meant to be the simple, always-correct fallback —
the tool you reach for when the streaming path misbehaves — then it is currently a
regression and should be the first thing fixed, since it is the path with the
fewest moving parts to reason about.
