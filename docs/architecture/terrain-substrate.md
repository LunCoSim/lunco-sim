# Terrain Substrate — the Height Oracle

> Audience: contributors working on terrain, LOD, or surface physics.

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
  collider resolution around each dynamic body;
- **spawn placement** (`lunco-sandbox-edit`) samples the oracle (`dem_ground_height`)
  to drop a rover onto the surface. Because the oracle is analytic — not a collider
  raycast — it answers **before** the collider tile under the drop point has
  streamed/baked, so a spawn over un-baked terrain rests on the ground instead of
  free-falling. The GUI path takes `max(oracle, raycast)` so an obstacle rock poking
  up under the chassis still lifts the spawn; the API path snaps `y` to the surface
  (+ the asset's `lunco:spawnLift`) when DEM terrain covers the point.

Because they all call one function, they **converge** — near a rover the mesh,
collider, and spawn height agree, so there is no visual/physics mismatch. Crater
crispness is no longer bounded by a DEM mip; it's bounded by how deep you sample.

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

### Won't killing the overlay lose crater detail?

No — it *gains* detail. The overlay's original rationale was real: a crater should
be crisper than the coarse (~5 m) DEM mesh. But that rationale conflated two ideas:

- **"craters are a distinct composable layer"** — keep this, it was right;
- **"craters render as a separate floating mesh"** — drop this. A layer ≠ a second
  surface.

What let the overlay out-detail the DEM was **never that it was a separate mesh** —
it was that it sampled craters **analytically** (`crater_delta` as a function),
decoupled from the DEM's 5 m grid. The oracle keeps exactly that. In the composed
sample `height_at = dem.height_at(x,z) + crater_delta(…)`, the two terms have
different resolution limits: `dem.height_at` is bilinear off the coarse DEM (smooth,
low-frequency — correct, since there *is* no sub-5 m ground data to invent), while
`crater_delta` is **analytic → infinite resolution, no grid.** And a CDLOD tile's
vertex density is *not* the DEM's density: a near tile is 33² verts over a small,
deeply-subdivided patch, so the crater rim resolves as sharply as the tile
tessellates — well below 5 m via error-driven refinement + over-zoom — while the
ground under it stays smooth. The crater is crisper than the DEM mesh *because* its
contribution is a function, not baked pixels.

The overlay's one genuine win — **dense-rim tessellation** (many polys on the rim,
few on the floor, more poly-efficient than a uniform tile) — is preserved as the
**feature-declared radial remesh** (lever 2 under *Detail on demand*): the same
dense-rim/sparse-floor patch, but sampling the *same* composed oracle (coincident,
no `lift`) and stitched into the tile instead of floating over a different surface.

Net comparison:

| | Overlay (today) | Crater layer as `HeightSource` modifier |
|---|---|---|
| Detail source | analytic `crater_delta` | **same** analytic `crater_delta` |
| Sub-DEM resolution | yes (floated) | **yes (coincident)** |
| Dense-rim tessellation | yes | **yes** (feature remesh, honest) |
| Follows ground relief | no (smooth base + `lift`) | **yes** (sampled on the real surface) |
| Collider matches visual | **no** (visual-only) | **yes** (ring samples same source) |

The only casualties are `lift` and the smooth-base sampling — precisely the two
things that caused *elevated / doesn't-follow-ground / colliders-suck*. The
better-than-DEM detail is preserved and strengthened; it never actually required a
second surface.

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
**content-addressable** (via `lunco-precompute`, keyed on `(stack-hash, quad,
lod)`). GPU displacement would give visuals the physics and cache can't see.

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

## Alignment with the canonical USD Stage

The networking branch made the openusd **live composed `Stage` the runtime source
of truth** (server + client): one write path (`UsdOp → EditTarget → Stage`), one
read path (extractors read a `StageView` over the live stage via the `UsdRead`
trait), and `flatten → sdf::Data → ECS` demoted to a derived cache slated for
deletion (*"remove flatten entirely, canonical only"*). ECS is a **projection
membrane** — render / physics / cosim / **terrain** read Send ECS components, never
the `!Send` Stage. (Full design: see `21-domain-usd.md` §USD layers and the
canonical USD architecture notes in `docs/usd-source-of-truth-ecs-projection-design.md`.)

Terrain is **one more projection consumer** on that membrane, exactly like the
policy-as-projection pattern in networking. This is not a parallel pipeline;
it is three planes over the *one* Stage:

| Plane | Terrain content | Mechanism |
|---|---|---|
| **Authoring** (USD Stage) | terrain root + `lunco:layer` child prims (dem / craters / carve / rocks / shader) + georef / anchor | edits = `UsdOp` on an EditTarget; composition, reference-arc cascade, RBAC, journal, cross-peer sync all **free** from the canonical machinery |
| **Projection membrane** (StageSink → ECS) | `TerrainLayerStack`, `TerrainGeoref`, `DemTerrainRequest` components | a terrain `UsdAttrProjection`; change-driven — only the prims that resynced re-project |
| **Derived runtime** (oracle → geometry) | `HeightSource` stack → CDLOD tiles + collider ring | sampled on demand; content-addressed via `lunco-precompute`; regen = an atomic activation unit |

**Why this matches USD's dynamic nature:** terrain geometry is never authored or
stored — it is a **pure deterministic projection** of the composed Stage. So every
dynamic event — a layer edit, a session-layer override, a cross-peer journaled op,
**adding a landing-site DEM via a `references` arc** (*"spawn = add reference arc →
composes + instances + cascades free"*), a hot-reload — simply changes the
projected spec; the oracle swaps and tiles / colliders re-bake lazily and
atomically. There is nothing to invalidate by hand. The *composition* is USD layer
composition; the *rendering* is its downstream shadow — describe-don't-store
carried to its conclusion.

**Two hard couplings this forces (do them when terrain rebases onto networking):**

- **Migrate terrain's USD read off flatten.** `bridge_usd_dem_terrain` and
  `refresh_layered_terrain_layers` (`lunco-sandbox/lib.rs`) still read
  `Res<Assets<UsdStageAsset>>` via `UsdDataExt` — the flatten path being deleted.
  The read swap is mechanical (`reader.prim_attribute_value → view.value`, over the
  `UsdRead` surface the other extractors already use), but it also **retires
  terrain's private `AssetEvent<UsdStageAsset>::Modified` reload observer**: live
  edits become a StageSink re-projection of `TerrainLayerStack` → `Changed<…>` →
  rebake. Net *less* wiring.

- **Regen must be a physics-atomic activation unit, not despawn+rebuild.** The
  canonical spec names the *terrain double-bake* as the thing incremental
  projection exists to fix: **keep the old collider until the new one is ready,
  swap in one flush**, gated by a sim-time barrier so the swap is deterministic
  across peers. The height-oracle makes staging clean — swap the pure `HeightSource`
  first (cheap), stage the tile / collider bakes, activate the collider atomically.

## Dynamic modification: editing terrain with tools

*(By design, not yet built — this section records how live tool-driven terrain
edits fit the oracle so nothing here has to be retrofitted later.)*

A tool edit — dig a pit, raise a berm, flatten a pad, bore a skylight, drop a
boulder — is **not a mutation of a mesh or a heightfield**. It is one more
**modifier pushed onto the `HeightSource` stack**. Because features already compose
(`Craters ∘ Dem ∘ Globe`), an edit is just the newest wrap: `Edit_n ∘ … ∘ Edit_1 ∘
Dem ∘ Globe`. The oracle is *always* the current truth; there is no baked artefact
to keep in sync, so "modify the terrain" and "describe the terrain" are the same
operation. This is the single strongest payoff of the function-not-grid model.

**An edit is a USD layer op — nothing bespoke.** A tool authors the edit as a
`lunco:layer` prim (or attribute) on the terrain, written through the canonical path
(`UsdOp → EditTarget → Stage`) into the runtime/session layer. Everything the
editing story normally needs then comes **free** from machinery that already exists:

| Editing need | Provided by |
|---|---|
| Undo / redo, edit history | the USD journal (`lunco-twin-journal`) — revert the op, the oracle recomposes |
| Multiplayer sync | the edit is a small **spec** (brush centre/radius/profile/Δ), journaled + replicated like any op; peers re-derive identical geometry — no terrain transfer |
| Permissions (who may edit what) | layer-scoped RBAC on the terrain prim |
| Non-destructive layers | composition: edits ride the session/runtime layer over an untouched base DEM; drop the layer → base returns |
| Determinism | the [quantize](../../crates/lunco-terrain-core/src/quantize.rs) firewall keeps edited colliders byte-identical across peers |

**Reactivity is the same projection membrane.** The authored edit changes the
projected `TerrainLayerStack` → the composed `HeightSource` swaps → only the quads
overlapping the brush AABB are dirtied and re-baked (bounded footprint), then the
collider is swapped atomically (the same keep-old-until-new activation the regen path
uses). No manual invalidation; an edit is indistinguishable from any other USD edit
flowing through the membrane.

**Authoring tier vs runtime tier — USD + Fabric, the Omniverse pattern.** USD is the
source of truth, so edits author to it *by default*, but — like Omniverse, which never
runs physics/render off authored USD but off a runtime cache (**Fabric**) — the terrain
is **two tiers**: the **USD terrain doc** (committed edits as tiny param prims, the
truth) and the **ECS `EditsLayer` + bake** (the projection physics/render read, our
Fabric). Three disciplines follow, and the [command journal](command-journal.md) design
carries the full rationale:

- **Edit target is a runtime/session layer** (over the untouched base DEM), promotable
  to persistent on save — `UsdOp` carries `edit_target`. A scratch dig never bakes into
  the asset unless committed.
- **Commit-granularity, never per-frame.** A continuous sculpt drag edits the *runtime*
  projection live and authors **one** USD op on release (as Omniverse edits Fabric on
  drag, writes USD on mouse-up); a click-dig authors at once. Authoring per frame would
  thrash composition — the one hard rule.
- **One prim per edit** — affordable *because* USD holds only tiny parameter records
  (the oracle stores no geometry), so each edit is a prim addressable by path (its
  identity) and individually undoable, while the runtime folds them all into the single
  `EditsLayer`. So "one layer vs. per-edit" was a false tension: **prim-per-edit is the
  authoring tier; the one `EditsLayer` is the projection tier** — both at once.

**The three channels absorb the full toolset.** Height edits (dig / raise / flatten)
are height modifiers; carve edits (tunnel, skylight, pit shaft) are carve/mask;
place-object edits (rock, prefab, structure) are geometry gprims. One editing model,
the same three contributions a static layer makes.

**What this asks of the design (hooks, deliberately deferred):**

- **A generic `BrushField` modifier** in `lunco-terrain-core` — `CraterField` is
  already exactly this shape (a parametric radial profile over placements), so a
  brush generalises it to arbitrary profiles; the deterministic bucket index it uses
  gives the **bounded dirty-region lookup** an edit needs for free.
- **Freeform sculpt** (arbitrary per-vertex Δ, not parametric) is the one edit that
  *does* store samples: a `SparseEditField` — a hashmap/edit-raster of touched cells,
  itself a `HeightSource`. It stays bounded, composable, and content-addressable; it
  is a *layer over* the base, never a replacement of it.
- **An edit → dirty-quads signal** into the bake queue (the optimization branch's
  `BakeQueue`): an edit enqueues its affected quads, and error-driven detail
  ([measured error](../../crates/lunco-terrain-core/src/error.rs)) auto-refines a
  sharp edit locally while slope-limiting keeps its collider contact-stable.

In short: the height oracle handles dynamic modification natively. The zero-conflict
core landing now (`CraterField`, the bucket index, measured error, quantize, the
slope limiter) is already the substrate an editing toolset would stand on.

## Current state & roadmap

**As-built (works today):** DEM ingest + crop/resample; static heightfield
collider; streamed CDLOD visual tiles (`stream_viz`) with vertex-morph geomorph
via `ShaderMaterial`; opt-in per-tile collider ring (`collider_ring`); big_space
per-tile anchoring; `TerrainLayerStack` composed from USD `lunco:layer` child
prims; `CompositeHeightSource` (core, pure); `TerrainGeoref` parsed from
`lunco:anchor:*`; derived surface/normal layers content-addressed through
`lunco-precompute` (`derived_layers.rs`); `TerrainHeight` scripting query.

> **Note on the USD read path:** as-built terrain still reads USD via the *flatten*
> reader (`UsdStageAsset` / `UsdDataExt`). The canonical-Stage cutover (above) is a
> forced migration once the networking USD canonical-stage merge lands — see the two couplings.

**Known gaps (in the order they should land):**

1. **Kill the two-surface crater path** — delete the floating overlay + `lift`;
   make craters a `HeightSource` *modifier* sampled by both the tile baker and the
   collider ring. This is the fix for "craters elevated / colliders suck," and
   step 1 of everything above.
2. **Per-tile geometric error** measured from the oracle → error-driven CDLOD
   (peaks/rims refine automatically); collider ring res driven by the same metric.
3. **Carve/mask channel** — the seam for caves/pits/skylights (baker clip +
   heightfield→trimesh fallback on mouth tiles).
4. **Canonical-Stage read migration** — move `bridge_usd_dem_terrain` + the layer parsers onto
   `UsdRead`/`StageView`; replace the `AssetEvent<UsdStageAsset>` reload observer
   with a terrain `UsdAttrProjection` off StageSink; make regen a physics-atomic
   activation unit (see *Alignment* above).
5. **Orbit→surface bridge app-wiring** — build the `CompositeHeightSource` *live*
   from `lunco:anchor:lat/lon`, relate the globe and surface grids, swap by
   altitude. (`CompositeHeightSource` exists in core; the wiring + lat/lon↔XZ
   reprojection are follow-ups.)
6. **Tile bake cache** — extend the existing `lunco_precompute::Bake` pattern
   (already used by `derived_layers`) to tile + collider bakes, so one bake feeds
   visuals + physics and ships as a spec+hash across peers. (Not a bespoke
   `cache://` asset — it reuses the content-address substrate.)

[Cesium-for-Omniverse]: https://github.com/CesiumGS/cesium-omniverse

## See also

- [`caching-and-precompute-strategy.md`](caching-and-precompute-strategy.md) — the
  `lunco-precompute` content-address substrate terrain tiles ride on.
- [`21-domain-usd.md`](21-domain-usd.md) — USD as the description plane; the
  canonical live-Stage source-of-truth model terrain projects from.
- [`mobility-substrate.md`](mobility-substrate.md) — the rovers that drive the
  collider ring.
