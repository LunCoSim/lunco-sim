# lunco-terrain-core

Projection-agnostic **terrain LOD spine** — the shared core both terrain crates
build on.

The pure, **render-free, physics-free** heart of the terrain system. It depends
on nothing but `std` + `serde` — no bevy, avian, big_space, DEM, or sphere
projection — so it's wasm-safe and unit-tested, and lets both terrain scales
share one selection algorithm instead of duplicating it.

## Modules

| Module | Role |
|--------|------|
| `quadtree` | CDLOD quadtree selection over an abstract square region: distance-range refinement from a fixed canonical screen metric (view-independent → deterministic across peers), 3D-Tiles geometric error, and CDLOD geomorph bands. `select_3d` takes eye-height so altitude coarsens LOD. |
| `tile` | uniform planar tile-grid math: world↔tile mapping, the resident ring of tiles around a focus (the physics-collider-ring substrate). |
| `source` | the `HeightSource` trait (`height_at` as a pure function of position) + `normal_at`, a deterministic analytic FBM source for bring-up / tests, and **`CompositeHeightSource`** — the orbit→surface bridge (site DEM inside a georeferenced region, globe height outside, smoothstep collar). |

## The height oracle

A `HeightSource` is the atom of the terrain model: features (a crater, a DEM, a
whole planet) compose by wrapping the source below them. The composed source is
the **single terrain truth** sampled by visual and physics products, while each
product owns its own sampling lattice and scheduling. Visual quality cannot
change physics work or contact geometry, and physics does not require render
assets. See the design narrative:
[`docs/architecture/terrain-substrate.md`](../../docs/architecture/terrain-substrate.md).

## Built on by

- **`lunco-terrain-surface`** — surface scale: DEM-backed `HeightSource` +
  avian heightfield colliders + big_space per-tile anchoring for local ground.
- **`lunco-terrain-globe`** — globe scale: cube-sphere region map + radius
  `HeightSource` for whole bodies seen from orbit.

The orbit→surface bridge is `CompositeHeightSource` (above): it returns the site
DEM inside its georeferenced region, a composed collar at the measured boundary,
and the globe height outside it. `lunco-celestial` supplies the live oracle and
tangent frame to the globe projection; the core remains pure and render-free.
