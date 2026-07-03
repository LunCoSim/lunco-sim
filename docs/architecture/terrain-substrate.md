# Terrain Substrate — the Height Oracle

How LunCoSim represents planetary surfaces from orbit down to a rover wheel,
with one abstraction that keeps **visuals and physics in lockstep**, composes
under **USD layering**, and **scales to a whole solar system**.

This doc is the design narrative. Per-crate quick-starts live in
[`lunco-terrain-core`](../../crates/lunco-terrain-core/README.md),
[`lunco-terrain-surface`](../../crates/lunco-terrain-surface/README.md), and
[`lunco-terrain-globe`](../../crates/lunco-terrain-globe/README.md).

## The principle: height is a composable *function*, not a baked grid

A surface feature (a crater, a dune, the whole planet) is a **`HeightSource`** —
a pure function `height_at(x, z) -> y` (plus `normal_at`) evaluated in a
**local frame**. Features compose by **wrapping the source below them**:

```
composed = Craters(seed, placements) ∘ Dem(geotiff) ∘ Globe(radius)
height_at(x,z) = base.height_at(x,z) + Σ crater_delta(dᵢ, …)   // analytic, any resolution
```

The composed source is the **single source of truth**. Both consumers sample it:

- the **CDLOD visual baker** (`stream_viz` / `tile_mesh::bake_tile_mesh`) bakes
  each tile's mesh by sampling the oracle at that tile's resolution — fine near
  the camera, coarse far away;
- the **avian collider ring** (`collider_ring`) samples the *same* oracle at the
  collider resolution around each dynamic body.

Because both call one function, they **converge** — near a rover both are fine,
so there is no visual/physics mismatch. Crater crispness is no longer bounded by
a DEM mip; it's bounded by how deep you sample.

### Why (the bug this replaces)

The first crater implementation had **two materializations** of the surface: a
fixed-resolution `HeightGrid` that craters were *stamped* into (truth for tiles +
collider) **and** a separate high-fidelity *overlay mesh* floated over the near
craters with a constant vertical `lift` to win the depth test. Two surfaces can
never agree:

- the `lift` made craters sit visibly **above** the surrounding terrain (a
  pedestal/step at the crater edge);
- the overlay was built off the *smooth* pre-crater grid while the tiles were
  built off the *stamped* grid → craters **floated free** of neighbouring relief;
- the collider was the coarse stamped grid, never the crisp overlay → rovers
  drove a blocky bowl while *seeing* a sharp one → **contact felt wrong exactly
  on craters**.

All three symptoms are one root cause: more than one surface. The oracle has
exactly one. `crater_delta`
([`lunco-obstacle-field/field.rs`](../../crates/lunco-obstacle-field/src/field.rs))
is reused verbatim — as *math you call*, not pixels you stamp.

## USD describes the stack (two senses of "layer")

USD has no native heightfield prim, so terrain follows the
**describe-don't-store** split (as [Cesium-for-Omniverse] does): the stage
carries the *recipe*, geometry is procedural. Two distinct layering mechanisms
both feed the runtime oracle, and must not be conflated:

1. **USD composition layers** (sublayer stack / session / runtime layer) — the
   *authoring & merge* plane. Non-destructive, RBAC-scoped, journaled edits.
   A live crater-density tweak rides the **runtime layer**
   (`persist_property_to_runtime_layer`) → composition resolves → new params.
   This is where authored values live and how edits compose across peers.

2. **Terrain content layers** (`lunco:layer = "dem" | "craters" | "rocks" |
   "shader"` child prims under a `lunco:assetMode="layered"` terrain prim) — the
   *domain* stack. After USD composition resolves the stage, the bridge walks the
   composed `lunco:layer` child prims **in prim order** and folds each into the
   runtime source stack (`TerrainLayerStack` in
   [`terrain_layers/`](../../crates/lunco-terrain-surface/src/terrain_layers/mod.rs)).

USD's *stronger-layer-wins / prim-order* semantics **are** the fold order. A
session-layer override, or a new crater child prim, flows through composition to
a new stack; tiles re-bake lazily as they stream. No DEM re-read.

## Three channels: what a layer actually contributes

A terrain feature does not contribute *height* in general — it contributes to one
or more of **three orthogonal channels**, and the USD prim declares which. This
is what keeps genuinely-3D features (caves, arches, overhangs) from breaking the
cheap single-valued surface.

| Channel | Effect | Sampled by | Examples |
|---|---|---|---|
| **height** | displace the single-valued surface | tile baker + collider ring (the oracle) | crater, rille, dune, ridge |
| **carve/mask** | mark surface regions *absent* (a coverage / SDF field) | baker clips tris there; collider skips there | cave mouth, pit, skylight, vent |
| **geometry** | insert discrete 3D gprims + their own colliders | placed on `composed.height_at`, streamed as assets | rock, habitat, **cave interior**, arch |

- A **crater** is pure *height*.
- A **rock** is pure *geometry* (a mesh + sphere collider sitting on the surface).
- A **cave** is *carve* (punch the mouth) **+** *geometry* (insert the tube) **+**
  optionally a little *height* (a raised collapse rim). Nothing in the pipeline is
  cave-special-cased; a cave is "a layer prim that ticks two channels."

### The representational boundary (why caves aren't height modifiers)

A crater is single-valued (`y = f(x,z)`); a cave/overhang is **multi-valued** —
two surfaces stacked in `z`. No heightfield can express it. Going fully
volumetric (SDF/voxel everywhere) to "unify" would throw away the cheap
heightfield, the avian heightfield collider, and per-tile determinism for the 99%
of terrain that genuinely *is* single-valued — the wrong trade. Instead:

- the **oracle owns only the single-valued surface**;
- genuine 3D features are **USD-referenced geometry** (a `Xform` + payload to a
  tunnel/chamber asset, georef'd under the terrain prim, streamed by the existing
  asset stack with a `drawMode=bounds` proxy and a native trimesh collider);
- the **carve/mask channel is the single, well-defined seam** where the two meet.

The one net-new mechanism a cave forces is the carve channel: the baker clips
tris inside the mask, and — since **avian heightfield colliders cannot have
holes** — a tile touching a mouth swaps its collider *heightfield → trimesh* for
that patch (bounded to mouth tiles). The inserted tube brings its own collider
and takes over inside. Make the carve field a small composable **SDF** and "a
cave mouth carved into a crater wall" is just a smooth-union of the two, with no
special-casing.

## Detail on demand: earned by measured error

Detail should be **earned by measured error, not spent by camera distance.**
Today `Quadtree::geometric_error(depth)` is *uniform* (`root / 2^depth`) — it
assumes every tile at a depth is equally complex, so a flat plain near the camera
gets the same polygons as a jagged rim crest, and a central peak seen from far
rounds off. The oracle makes the fix cheap. Four levers, in priority order:

1. **Per-tile geometric error from the oracle.** When baking a node, sample the
   composed oracle over its bbox and measure `max |true − coarse|` (or a curvature
   integral); store that as the node's *real* geometric error. Then CDLOD
   selection (distance vs error at a fixed screen metric — the locked,
   view-independent, peer-deterministic design) **automatically refines tiles
   over peaks and rims deeper than tiles over plains at the same distance.** This
   is the main lever and it is currently the largest gap.

2. **Feature-declared concentration** (the honest version of the old overlay). A
   feature with a known shape — a crater's rim crest and central peak — declares a
   radial remesh, dense at rim + peak, sparse on the floor. Crucially it **samples
   the same composed oracle** (so it is *coincident* — no lift, no float) and
   *refines the tile patch* rather than floating a disc over a different surface.

3. **Procedural over-zoom.** Below the authored feature resolution (1 m off a 5 m
   DEM), the oracle **synthesizes** deterministic fractal micro-relief — a
   high-frequency height modifier gated by LOD depth (Outerra-style 5 m → 2.5 cm).
   Infinite zoom, nothing stored, `seed`-deterministic so visual, collider, and
   all peers agree.

4. **Collider parity.** Drive the collider ring's per-tile resolution by the *same*
   per-tile error metric, so a crater/peak under a wheel gets a finer collider tile
   matching the visual. Visual and contact refine together.

**Constraint that shapes all of it:** detail generation stays **CPU-side /
bakeable**, never GPU tessellation or mesh shaders. It must be wasm-safe (locked
no-VTF/no-compute rule), it must feed **avian colliders** (which cannot consume a
GPU-tessellated surface), and each detailed tile — however tessellated — must be
**content-addressable** as a `cache://` `TerrainTile` keyed by `(stack-hash, quad,
lod)`. GPU displacement would give visuals the physics and cache can't see.

*Worked example — a crater central peak:* a height term in the crater modifier
for large craters (`+peak_h · gauss(d / peak_r)`) → single-valued, so the oracle
owns it. It renders crisp because **(1)** error-driven CDLOD subdivides it,
**(2)** the crater's radial remesh guarantees ring density on the peak, **(3)**
over-zoom adds micro-relief up close, and **(4)** the collider refines there too.
One function, four LOD mechanisms feeding off its measured error.

## Scale: orbit to rover, one abstraction

The design scales to a full solar system *for the same reasons the crater fix
works* — height-as-function, composition, error-driven detail, content-addressing.

- **The frame hierarchy absorbs the 1e11 m dynamic range.** The oracle is
  *always* evaluated in a tile-local / body-local frame (`big_space` i64 cell
  grids per body, `translation_to_grid` per-CellCoord anchoring). A crater sample
  sees `(x,z)` of ±tile/2, never 1.5e11 m — so f64→f32 stays sub-mm even at 1 AU.
  Scale lives in the frame stack, not in the math.

- **Globe and surface are the *same* abstraction.** The locked "one LOD
  continuum, two height scales" model *is* the oracle at planetary scale: the
  **globe** (`lunco-terrain-globe` cube-sphere) is a radial `HeightSource`; the
  **surface** (`lunco-terrain-surface` DEM inset pinned to a georef'd lat/lon) is
  a tangent-plane `HeightSource` — and craters/carves/over-zoom are modifiers on
  *that* node. The **`CompositeHeightSource`**
  ([`core/source.rs`](../../crates/lunco-terrain-core/src/source.rs)) blends site
  DEM inside the georef region with globe height outside, crossover by altitude.
  A planet's full oracle is `globe ⊕ (DEM ⊕ craters ⊕ carves ⊕ over-zoom)`; the
  crater oracle is **one node in the planet's composite.**

- **Error-driven detail is the same law at every scale.** The globe's sphere
  metric (`subdivide_face`) and the surface's planar `Quadtree` differ, but both
  are "refine when the node's error subtends too many pixels." Over-zoom from a
  global DEM to a rover-scale rock is a continuous error-driven descent with no
  mode switch the user perceives.

- **Work scales with the *view*, replication with the *spec*.** Far bodies are a
  point sprite / globe L0 (USD payloads keep them lazy); the body you orbit is a
  few hundred near-face tiles; the site you stand on streams tiles + collider ring
  around the rover. Total work is `O(frustum)`, independent of solar-system
  extent — Jupiter costs nothing until you go there. And every tile (globe or
  surface) is `pure fn(source spec + coord)` → content-addressed → byte-identical
  across peers, so a shared solar system replicates a few KB of USD descriptors +
  ephemeris, never terrain.

- **USD composition *is* the solar-system model.** `SolarSystem → Body (Xform +
  ephemeris_id + georef) → Terrain (globe source + DEM-inset child prims) →
  features`. Bodies compose; payloads make far bodies lazy; layering adds a
  mission's landing-site inset or a session's cave non-destructively, per body.
  Geology is time-invariant, so the oracle is independent of ephemeris churn —
  bodies move, terrain functions don't.

## Current state & roadmap

**As-built (works today):** DEM ingest + crop/resample; static heightfield
collider; streamed CDLOD visual tiles (`stream_viz`) with vertex-morph geomorph
via `ShaderMaterial`; opt-in per-tile collider ring (`collider_ring`); big_space
per-tile anchoring; `TerrainLayerStack` composed from USD `lunco:layer` child
prims; `CompositeHeightSource` (core, pure); `TerrainGeoref` parsed from
`lunco:anchor:*`; derived surface/normal layers; `TerrainHeight` scripting query.

**Known gaps (in the order they should land):**

1. **Kill the two-surface crater path** — delete the floating overlay + `lift`;
   make craters a `HeightSource` *modifier* sampled by both the tile baker and the
   collider ring. This is the fix for "craters elevated / colliders suck," and
   step 1 of everything above.
2. **Per-tile geometric error** measured from the oracle → error-driven CDLOD
   (peaks/rims refine automatically); collider ring res driven by the same metric.
3. **Carve/mask channel** — the seam for caves/pits/skylights (baker clip +
   heightfield→trimesh fallback on mouth tiles).
4. **Orbit→surface bridge app-wiring** — build the `CompositeHeightSource` *live*
   from `lunco:anchor:lat/lon`, relate the globe and surface grids, swap by
   altitude. (`CompositeHeightSource` is done in core; the wiring + lat/lon↔XZ
   reprojection are deferred.)
5. **`cache://` `TerrainTile`** — content-addressed tile bake cache (one bake
   feeds visuals + physics), the L1/L2 LRU described in the caching strategy.

[Cesium-for-Omniverse]: https://github.com/CesiumGS/cesium-omniverse

## See also

- [`caching-and-precompute-strategy.md`](caching-and-precompute-strategy.md) — the
  `cache://` asset stack that terrain tiles ride on.
- [`21-domain-usd.md`](21-domain-usd.md) — USD as the description plane.
- [`mobility-substrate.md`](mobility-substrate.md) — the rovers that drive the
  collider ring.
