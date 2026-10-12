# Terrain Substrate — the Height Oracle

> Status: Active · Audience: contributors working on terrain, LOD, or surface physics

How LunCoSim represents planetary surfaces from orbit down to a rover wheel,
with one analytic source that composes under **USD layering** and **scales to a
whole solar system**. Visual rendering and physics are independent products:
they may sample the same source, but neither product's quality or scheduling
controls the other.

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
- **spawn placement** (`lunco-luncosim-edit-core`) samples the oracle (`dem_ground_height`)
  to drop a rover onto the surface. Because the oracle is analytic — not a collider
  raycast — it answers **before** the collider tile under the drop point has
  streamed/baked, so a spawn over un-baked terrain rests on the ground instead of
  free-falling. The GUI path takes `max(oracle, raycast)` so an obstacle rock poking
  up under the chassis still lifts the spawn. The GUI uses the asset's composed
  whole-assembly `UsdPhysics` collision envelope, including nested rigid bodies;
  an asset without one is rejected instead of
  receiving invented dimensions or a lift. The API path uses the explicit
  position supplied by the caller.

Because they all call one function, they **converge on terrain facts** while
remaining independent products. The visual product chooses its camera-driven
sampling; the physics product chooses its authored collider lattice. A graphics
quality change cannot add physics work or change contact geometry. Crater
crispness is no longer bounded by a DEM mip; it is bounded by each product's
own sampling contract.

### One committed-change contract, independent product caches

Every committed oracle swap records its source key, destination key, revision,
and terrain-local dirty bounds once in `TerrainSurfaceChange`. Native restamps
and progressive worker upgrades publish the same record. The visual owner
applies it through one invalidation function for tile generations, in-flight
bakes, and cached meshes. The collider ring accepts bounded reuse only when its
cached source key and revision match the record's source; otherwise it refreshes
the full ring. Derived maps carry their bake source key, so maps from an older
surface are dropped instead of being retained across a later bounded edit. The
monolithic static collider consumes the same change record to debounce its
rebuild, and rejects an async result whose oracle key is stale.

Each product still applies the change according to its own contract: visual
tiles invalidate overlapping meshes and retain compatible in-flight work;
physics keeps stale colliders supporting bodies until replacements are ready;
derived maps stay published only while their source is the surface immediately
being replaced. The visual and physics quad lattices share the core
`Square::overlaps_aabb` spatial rule, while their resolution, bands, scheduling,
and cache lifetimes remain independent.

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

> **Invariant: there is exactly one surface builder — the DEM terrain.** It owns
> crater/rock generation from the spec; the obstacle-field crate is a pure generator
> that spawns nothing. The rebuild trigger is `UpdateObstacleFieldSpec`, observed by
> the terrain crates.
>
> This used to be a *mode*: `ObstacleFieldMode::Standalone` built its own flat-slab
> arena, and it was the default once, costing **43× the frame rate**. It was demoted
> to opt-in, then became unreachable — nothing (scene attribute, API, or rhai) could
> select it — and the mode enum plus the whole slab-build path have since been
> deleted. Recorded because the lesson outlives the code: a default that is the
> pathological path is a fuse, not a fix, and it will be found by whoever forgets to
> set the mode, which is everyone.

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
| Collider uses the visual-quality contract | **no** (independent product) | **no** (ring samples the same source through authored physics settings) |

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
   typed runtime-layer shader-parameter command → composition resolves → new params.
   This is where authored values live and how edits compose across peers.

2. **Terrain content layers** (`lunco:layer = "dem" | "craters" | "rocks" |
   material-bound terrain prim under a `lunco:assetMode="layered"` terrain prim) — the
   *domain* stack. After USD composition resolves the stage, the bridge walks the
   composed `lunco:layer` child prims **in prim order** and folds each into the
   runtime source stack (`TerrainLayerStack` in
   [`terrain_layers/`](../../crates/lunco-terrain-surface/src/terrain_layers/mod.rs)).

USD's *stronger-layer-wins / prim-order* semantics **are** the fold order. A
session-layer override, or a new crater child prim, flows through composition to
a new stack; tiles re-bake lazily as they stream. No DEM re-read.

Collider ownership follows the authored terrain mode. `lunco:assetMode="mesh"`
is realized by the USD-to-Avian bridge after its asynchronous `Mesh3d` loads;
`"dem"` and `"layered"` are realized by `lunco-terrain-surface` from the
`SurfaceOracle`. The bridge does not install a mesh placeholder for DEM-backed
terrain, so a visual mesh cannot replace the oracle-derived physics surface.

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

The `rocks` geometry layer uses the composed terrain footprint as its default
scope: its optional `lunco:layer:regionM` is a half-extent, and zero (the USD
schema default) means the whole composed terrain. A positive value is an
explicit near-field scope. Density is expressed per hectare, then the
authoritative rendering-quality profile applies the total instance cap. The
sampler is one deterministic field for the layer, so coverage does not restart
at tile boundaries or duplicate when the camera moves; shared meshes and
visibility ranges provide the bounded presentation cost.
- A **rock** is pure *geometry* (a mesh + sphere collider sitting on the surface).
- A **cave** is *carve* (punch the mouth) **+** *geometry* (insert the tube) **+**
  optionally a little *height* (a raised collapse rim). Nothing in the pipeline is
  cave-special-cased; a cave is "a layer prim that ticks two channels."

### The representational boundary (why caves aren't height modifiers)

A crater is single-valued (`y = f(x,z)`); a cave/overhang is **multi-valued** —
two surfaces stacked in `z`. No heightfield can express it. Going fully
volumetric (SDF/voxel everywhere) to "unify" would throw away the cheap
heightfield oracle, sampled triangle colliders, and per-tile determinism for the 99%
of terrain that genuinely *is* single-valued — the wrong trade. Instead:

- the **oracle owns only the single-valued surface**;
- genuine 3D features are **USD-referenced geometry** (a `Xform` + payload to a
  tunnel/chamber asset, georef'd under the terrain prim, streamed by the existing
  asset stack with a `drawMode=bounds` proxy and a native trimesh collider);
- the **carve/mask channel is the single, well-defined seam** where the two meet.

The one net-new mechanism a cave forces is the carve channel: the baker clips
tris inside the mask in both the visual mesh and the sampled collision mesh
for that patch (bounded to mouth tiles). The inserted tube brings its own collider
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
   all peers agree on the source function while retaining independent product
   sampling contracts.

4. **Physics parameters are explicit.** The DEM generator authors
   `colliderDepth` and `colliderResolution` on the `dem` layer. They are copied
   into `DemTerrainRequest` and are the only quality inputs to the collider ring;
   visual LOD and `RenderingQualitySettings` are not read by physics.

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
  *that* node. The local oracle keeps its composed DEM heights throughout the
  finite crop. In Visualization, one worker-prepared exterior collar continues
  the measured crop boundary and joins it to a datum-aligned render sphere. The
  crop border datum sets that render-only shell radius; celestial and physical
  state keep the canonical radius. Globe tiles clip against the collar's outer
  square in the same orthographic tangent chart with bounded edge sampling, so
  camera-driven globe LOD does not track DEM posting density. Physics and
  terrain queries remain bounded to the crop. This closes the finite crop
  without modifying its DEM or requiring a body-wide raster.
  **`CompositeHeightSource`**
  ([`core/source.rs`](../../crates/lunco-terrain-core/src/source.rs)) remains a
  generic pure blending primitive; it does not select or load terrain assets.

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
  NAIF body identity (lunco:body) + georef) → Terrain (globe source + DEM-inset child prims) →
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
canonical USD architecture notes in `usd-source-of-truth.md`.)

Terrain is **one more projection consumer** on that membrane, exactly like the
policy-as-projection pattern in networking. This is not a parallel pipeline;
it is three planes over the *one* Stage:

| Plane | Terrain content | Mechanism |
|---|---|---|
| **Authoring** (USD Stage) | terrain root + standard `UsdShade` material + `lunco:layer` child prims (dem / craters / carve / rocks) + georef / anchor | edits = `UsdOp` on an EditTarget; composition, reference-arc cascade, RBAC, journal, cross-peer sync all **free** from the canonical machinery |
| **Projection membrane** (StageSink → ECS) | `TerrainLayerStack`, `TerrainGeoref`, `DemTerrainRequest` components on mounted scene roots | a terrain `UsdAttrProjection`; change-driven — only the prims that resynced re-project; `UsdPreviewOnly` terrain prims are examined without creating mission DEM requests |
| **Derived runtime** (oracle → geometry) | `HeightSource` stack → CDLOD tiles + collider ring | sampled on demand; content-addressed via `lunco-precompute`; regen = an atomic activation unit |

The mission physics-admission mirror reports the exact mounted DEM request or
dataset that holds activation; it excludes `UsdPreviewOnly` ancestry even if a
preview request was already present when the hierarchy was tagged.

The initial DEM bridge reads generation-zero facts through
`CanonicalStages::reader_for_entity`, which selects the worker-prepared
projection plan. It indexes candidate paths by terrain API schema and authored
`lunco:` properties, then examines only changed or explicitly retrying prims.
After a live stage generation advances, it classifies only each changed prim by
the terrain API, `lunco:assetMode`, and `lunco:terrain:surfaceRole` facts the
bridge consumes. Read-only startup projection does not open a live stage on the
app thread or rescan the live stage during edits.

An Editor preview shares composed USD paths with the mounted scene, but it is
not a terrain simulation participant. Its DEM does not build collider rings,
publish analytic query sources, or hold the mission physics transport. An
isolated preview that needs relief requires a separate render-only terrain
realization and preview-grid camera demand.

**Why this matches USD's dynamic nature:** terrain geometry is never authored or
stored — it is a **pure deterministic projection** of the composed Stage. So every
dynamic event — a layer edit, a session-layer override, a cross-peer journaled op,
**adding a landing-site DEM via a `references` arc** (*"spawn = add reference arc →
composes + instances + cascades free"*), a hot-reload — simply changes the
projected spec; the oracle swaps and tiles / colliders re-bake lazily and
atomically. There is nothing to invalidate by hand. The *composition* is USD layer
composition; the *rendering* is its downstream shadow — describe-don't-store
carried to its conclusion.

**One remaining hard coupling:**

- **Regen must be a physics-atomic activation unit, not despawn+rebuild.** The
  collider ring already does the local half of this: on an oracle swap it marks
  tiles **stale instead of despawning them**, keeps the old collider live while
  the replacement bakes off-thread, and swaps the new `Collider` onto the same
  entity — regen never opens a physics hole. Still open: the sim-time barrier
  that makes the swap frame deterministic across networked peers.

## Dynamic modification: editing terrain with tools

Live tool edits are built: brush raise/dig (`BrushTerrain`) and flatten
(`FlattenTerrain`) — armed from the Tools palette, applied by scene click, or
driven over rhai/HTTP/MCP — author edit records through the document, project
into the composed `EditsLayer` modifier, and re-bake **only the tiles whose
region the edit touches** (`TerrainDirty`). Doc-backed terrains carry
`LiveRebuildExempt`: a USD edit confined to the terrain subtree
(`edit_confined_to_exempt_subtree`) refreshes the terrain in place and never
reloads the scene. The rest of this section records how the editing model maps
onto the oracle.

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

**Hook status:**

- **Parametric brush edits** — built: `EditsLayer` is the generic
  radial-profile-over-placements modifier (`CraterField`'s shape generalised),
  and its bucket index gives the bounded dirty-region lookup.
- **Edit → dirty-surface signal** — built: an edit records its footprint as a
  `TerrainDirty` region; the committed `TerrainSurfaceChange` sends that same
  region to overlapping visual and collider tiles, while the monolithic static
  collider rebuild and derived-map bake use their own source-keyed refresh rules.
  Error-driven visual detail auto-refines a sharp edit locally.
- **Freeform sculpt** (arbitrary per-vertex Δ, not parametric) — still deferred:
  a `SparseEditField` — a hashmap/edit-raster of touched cells, itself a
  `HeightSource`. It stays bounded, composable, and content-addressable; a
  *layer over* the base, never a replacement of it.

## Bounded rock scatter admission

Procedural rock placement and shared mesh selection remain owned by the terrain
layer. New and recycled rock bodies enter the ECS in stable batches of at most
8 per `Update`; render-only components are admitted in batches of at most 64.
Each body batch is committed in deterministic placement order while the current
simulation continues; rock admission adds no physics hold.
`TerrainScatterContent` records the admitted field fingerprint as soon as the
ordered queue is accepted. `TerrainLayersPending` remains until both body and
visual queues finish, then `TerrainLayersApplied` replaces it. Recomposition
compares the admitted fingerprint during loading as well as after completion;
unchanged content cannot cancel and hide a partially displayed field. A changed
field replaces the fingerprint when its new queue is accepted. Refresh and scene
teardown cancel queued work.

## Current state & roadmap

**As-built (works today):** DEM ingest + crop/resample; **the live-analytic
oracle** — `SurfaceOracle` (`oracle.rs`: raster base + ordered analytic
`HeightModifier`s, per-modifier content keys) retained as `DemHeightField` and
sampled by the tile baker, the collider ring (through the
`prepare_collider_heights` slope-limit + quantize firewall), the derived-layer
texture bakes, the rock scatter, the `TerrainHeight` query, and **UI picking**
(`raycast_surface`: the spawn ghost, footprint corner probes, and terrain brush
march the oracle instead of physics colliders, so placement hugs the drawn
ground even where no collider exists) — craters and runtime edits are analytic
modifiers (`Craters`, `EditsLayer`), no longer raster stamps, so rim crispness
is bounded by tile tessellation, not grid resolution; **the crater model** —
octave-stratified overprint (same-scale
craters overprint via min/max, scale-separated ones superpose, so small bowls
survive inside large ones), power-law size-frequency population (2–60 m,
exponent −1.8) with azimuth-lobed secondaries around large parents, and a
degradation morph (fresh paraboloid → soft flat dish via `bowl_power` 2–6, rim
decay), all band-limited per consumer; **error-driven CDLOD** —
`select_with_error` over memoized `measure_node_error` per node
(`TerrainNodeErrors`), range factor from the canonical screen metric
(`pixel_error` knob) with **budget fitting by metric coarsening** (tile budget
exceeded → raise `pixel_error`, so morph bands move with the detail radius),
morph bands snapped to log-lattice buckets so batching survives per-parent
bands, and **dynamic bodies as forced visual refinement foci** (the ground under
a rover always draws at max visual depth); **streaming** — off-thread tile bakes (budgeted per frame,
bounded in flight) keeps a root fallback queued first while refined tiles
publish, then orders selected work by screen-space benefit weighted toward the
camera heading, with a reveal morph settling each child onto its parent's lattice,
a content-addressed on-disk tile cache (`tile_cache`, keyed on the oracle's
`surface_key`) so warm reloads stream instead of baking, and
`TerrainStreamStatus` driving a status-bar progress indicator whose pending count
includes selected meshes and the root fallback until `ShaderLookReady` confirms
the shader layout and declared image dependencies; in-place image updates do not
toggle that latch, and a cached edge-stitch material variant preserves it when
its own shader and image dependencies are already ready; **the status mirror
remains live while `pending > 0` even when `resident == wanted`, and presents
that render-publication phase as an indeterminate preparation state rather than
a completed tile bar**; **procedural USD DEM surfaces** keep their standard
`UsdShade`/`PbrLook` intent on the terrain Xform before the asynchronous DEM mesh
is assembled, so the render-free terrain crate never invents a material and a
static surface cannot arrive black solely because geometry was late; **procedural
over-zoom** — `Overzoom` in core (hashed craterlet bands ≤ 2 m on the lunar
size-frequency shape + FBM micro-relief, Nyquist-gated per consumer via
`SurfaceOracle::detail_limited`), **explicit opt-in** — an authored
`lunco:layer = "overzoom"` prim is the sole source of synthetic detail; streamed CDLOD visual
tiles (`stream_viz`) with vertex-morph geomorph via `ShaderMaterial`, tiles
`NotShadowCaster` while remaining directional-shadow receivers (terrain
self-shadow rides the horizon cache, while the cascade carries dynamic-object
shadows onto the ground);

Overzoom's band traversal prepares the Poisson distribution once for adjacent
cells in the same density group. This query-local value retains the exact f64
initial probability; independent cell hashes and the inverse-CDF operation order
are unchanged. It stores no population or random state between queries.

**the collider ring** (`collider_ring`) — 3×3 independently selected
physics tiles around each focus, with authored `colliderDepth` and
`colliderResolution`, `FIX_INTERNAL_EDGES`, baked off-thread with stale-swap on
regen (old collider lives until the replacement lands on the same entity);
rock colliders are spheres at 0.6× the bucket-visual radius so physical contact
matches the drawn rock;
big_space per-tile anchoring; `TerrainLayerStack` composed from USD
`lunco:layer` child prims; `CompositeHeightSource` (core, pure);
`TerrainGeoref` parsed from `lunco:anchor:*`; derived surface/normal layers
content-addressed through `lunco-precompute` (`derived_layers.rs`, keyed on
base heights + the oracle's modifier content key); `TerrainHeight` scripting
query.

The bake itself (GeoTIFF decode → native crop → optional coarse-preview resample
→ crater stamp → `HeightGrid`) is the pure, bevy/avian-free `lunco-terrain-bake`
crate, so
the SAME code runs off-thread on both platforms: native inside an
`AsyncComputeTaskPool` task; on wasm — where that pool degrades to the page's main
thread and the ~40 MB decode + stamp froze the tab — dispatched to the `dem_worker`
Web Worker over `lunco-worker-transport`, which decodes once then streams a coarse
preview then the full grid (coarse-then-full progressive, applied via the live
re-stamp swap). Only the avian collider + Bevy mesh derive stays in
`lunco-terrain-surface`, where those types live.

> **Note on the USD read path:** terrain projects from the live canonical `Stage`
> (`CanonicalStages` → `StageView`, read through `UsdRead`) — the cutover is done and
> the flatten reader is gone. `UsdDataExt` still appears in `lunco-usd-terrain`, but
> only for the *authoring* plane: a raw authored `sdf::Data` layer, deliberately
> pre-composition. Two read planes, two traits; don't conflate them.

**Known gaps (in the order they should land):**

1. ~~Kill the two-surface crater path~~ — **done**: craters are an analytic
   `Craters` modifier on the `SurfaceOracle`, sampled by the tile baker and the
   collider ring at their own resolutions. The collider ring's depth and tile
   resolution are authored on the DEM layer and are never selected from the
   graphics-quality profile; rendered tile count and physics contact density
   therefore remain independent in both windowed and headless execution.
2. **Carve/mask channel** — the seam for caves/pits/skylights (baker clip +
   heightfield→trimesh fallback on mouth tiles).
3. **Canonical-Stage read migration** — move `bridge_usd_dem_terrain` + the layer parsers onto
   `UsdRead`/`StageView`; replace the `AssetEvent<UsdStageAsset>` reload observer
   with a terrain `UsdAttrProjection` off StageSink; make regen a physics-atomic
   activation unit (see *Alignment* above).
4. **Tile bake cache** — **partly done**: visual tile meshes are
   content-addressed on disk (`tile_cache`, keyed on `SurfaceOracle::surface_key`
   + tile coord), so a warm reload of the same composed surface streams instead
   of baking. Still open: sharing the bake with the collider ring (one sample
   pass feeding visuals + physics) and shipping bakes as spec+hash across peers
   via the `lunco_precompute::Bake` pattern.

[Cesium-for-Omniverse]: https://github.com/CesiumGS/cesium-omniverse

## See also

- [`caching-and-precompute-strategy.md`](caching-and-precompute-strategy.md) — the
  `lunco-precompute` content-address substrate terrain tiles ride on.
- [`21-domain-usd.md`](21-domain-usd.md) — USD as the description plane; the
  canonical live-Stage source-of-truth model terrain projects from.
- [`efficiency-and-maintainability.md` § Substrate C](efficiency-and-maintainability.md#substrate-c--mobility-the-structurevolatility-classifier)
  — the mobility classifier behind the rovers that drive the collider ring.

## Sampled collision surface

`lunco-terrain-surface::collision` cooks every static DEM and streamed collider
ring tile through the same `terrain_collider` owner. The height oracle remains
the source of samples, quantization, tile boundaries, and rebasing. Parry's
`HeightField::to_trimesh` supplies its exact grid triangulation; the native
triangle mesh merges shared vertices and fixes internal-edge normals.

This collision representation supports translation and rotation sweeps for
Avian's nonlinear CCD, including offset compound colliders. Baking remains in
the existing worker pipeline, and completed ring batches enter in canonical
coordinate order. Invalid samples or failed cooks raise `terrain-collision-cook`
and hold the runtime before admitting an incomplete batch.

`lunco-physics::avian_backend_pose_is_valid` owns native pose admission:
positions must be finite and representable by Avian's spatial backend, and
rotations must satisfy Glam's existing unit-quaternion tolerance. The USD/Avian
bridge checks body, ancestor and target-frame transform rotations before spatial
composition; normalization cannot turn invalid authored input into admitted
state. Invalid rotations and failed bridge pose resolution raise
`avian-backend-pose-invalid` and hold physics before solver admission. Structural
frame disconnection retains its `physics-frame` diagnostic and hold. The production
`invalid_physics_rotation` scene exercises this boundary through a public Rhai
transform edit and requires the terminal fault without a process crash.
Hierarchy preflight shares the coordinate reader's
`MAX_SPATIAL_HIERARCHY_DEPTH` bound and reports cycles and excessive depth.
Admission and fixed preparation use the same frame diagnostic owner and
structural invalidation contract; a refused frame publishes its diagnostic and
hold at either boundary.
Collider-transform preparation checks rotations before composition and raises
`avian-collider-topology-invalid` for ancestry failures. A rejected preparation
leaves the collider transform unchanged and holds dynamics before integration.

The shared simulation installs `lunco-physics::ContinuousCollisionPlugin`
with the same `UsdCollisionFilter` as Avian's broad phase. Continuous contact
inequalities run inside each native substep, after position integration and
ordinary joint solving, before velocity projection. Candidate identity is ordered
by `PhysicsOrderKey`; each pass bounds the whole solved COM rotational arc and
recasts the current motion independently of velocity-based broad-phase edges.
Compound leaves are swept in their admitted native order and body frame. Contact
by one leaf cannot hide a different leaf's new crossing through the same body.
Pairs originate from CCD-enabled bodies, so static terrain/scatter populations
do not generate a quadratic static pair set. Endpoint AABBs expanded by the
rotational chord's Taylor remainder conservatively enclose the whole arc.
For stationary composite shapes, that bound queries Parry's existing BVH in
the composite frame. Native casts run on each selected part in stable part order;
each part retains its own impact and witness frame. Initial contact by one mesh
triangle cannot hide a different triangle's later crossing. Moving composites
bound the other whole-arc AABB in their local COM frame with a sphere enclosing
both COM-path endpoints, then query the same native BVH and per-part kernels.
Swept witnesses consume Parry's native normal constraints, matching contact
manifolds' internal-edge correction and rejection of oriented mesh backfaces.
Native support points and a triangle's face axis can certify a gap within the
continuous position tolerance over the whole rotating path. Endpoint supports include the projected second-derivative
remainder; clear endpoints alone never skip a cast. Ball geometry has no
orientation-dependent term. Cylinder geometry uses parallel transport to remove
axial spin from the material correspondence: with axis `a` and angular increment
`omega`, `k = omega - a*(omega dot a)` and the geometry acceleration is bounded
by `radius*(|k|^2 + |k'|)`. The occupied cylinder is unchanged by that spin;
COM-offset acceleration and changing tilt remain in the bound.
Each immutable sweep computes each body's solved motion once and shares it
across its leaves. Each unique leaf's whole-arc bound is computed once.
Admitted candidate queries run in a synchronous Bevy compute-pool scope over
immutable leaf trajectories and shapes. Each chunk owns its contact caches and
reusable native EPA workspace. `ContinuousCollisionSettings.minimum_query_batch_size`
sets the minimum queries per worker (default 64); smaller populations execute
the identical kernel directly. Zero permits one query per worker. Joined results, diagnostics and failures are
consumed in prepared pair order before any body correction; worker completion
order cannot select an authoritative result. The same query path serves one
and multiple compute threads. Geometry caches remain local to the immutable
starting state of one substep. Candidate separation directions and EPA capacity
can survive later substeps of the prepared physics step; directions supply no
geometry or clearance and require a fresh full-shape whole-arc bound.
Each convex leaf query shares native endpoint poses and local bounding spheres
across its candidate-axis tests. This immutable interval is rebuilt whenever
the sweep's motion or geometry changes; it retains no result across corrections.
Support gaps use native relative poses before adding local occupied geometry,
so a large common world translation cannot erase a small clearance. Endpoint
gap and curvature bounds include outward arithmetic allowances; nonfinite
intermediate bounds reject the query before they can certify an interval.
Material-witness gaps subtract body centers before adding their relative
offsets. New native witnesses subtract collider and COM origins before adding
rotated local points, preserving crossings under a large common translation.
Parry's native BVH admits overlapping swept bounds for active CCD leaves;
the prepared pair table retains collision hooks, layers and pair-specific
dynamic-body eligibility. Candidates execute in prepared stable pair order,
independent of BVH traversal order. Its topology is built once per prepared
physics step; every changed trajectory replaces exact leaf bounds and refits
ancestors with zero change-detection margin. Refitting expands and shrinks bounds.
Unchanged poses cannot create a new crossing; prepared native inequalities
still own their existing contacts. Initial geometry is allocated lazily for admitted candidates and
retains its immutable substep lifetime. Native support maps
certify separation along triangle axes over the complete arc. Native finite
triangle projections supply inexpensive candidate edge axes. Native closest
contacts provide additional axes for separated finite features; their full-shape
support bound must stay clear over the entire arc. Cylinder radial features
supply an additional axis with the cap component removed. It is a candidate,
not a geometric assumption: the complete occupied shapes must still pass the
same full-arc support bound. Successful separation axes are checked first on
later closure sweeps, and invalidated when changed endpoints no longer pass.
An initially
intersecting convex leaf may also omit a directional cast when a native contact
normal, matching full-shape support depth, certifies no increase in that depth.
The bound includes feature changes. A finite triangle's initial depth may also
be certified from native supports and finite geometry. A real triangle point
inside a cylinder owns an interior ball whose radius is its native distance to
the nearest boundary. Every shorter translation retains intersection with that
point. Alternatively, the whole convex projection can clear all three triangle
edges by the support depth plus tolerance. A matching support upper bound
proves the minimum translation depth; the whole-arc gap must remain above the
proven lower depth minus tolerance. Finite-edge and deep overlaps without either
proof retain the native contact query. Cylinder point projection chooses a
nearest feature even when cap/side distances tie.
Contact with another compound leaf supplies no certificate.
A strictly interior cylinder witness at a real finite triangle point
can also prove initial intersection without its minimum translation depth.
For tilted cylinders whose center projection is outside the solid, opposite
native support points provide a chord crossing the triangle's plane. The finite
triangle projection must still supply a real point strictly inside the cylinder,
with the native arithmetic margin. Plane crossing alone proves nothing about
finite overlap; touching and uncertified witnesses retain native queries.
Projection candidates are reconstructed from finite nonnegative native
barycentric weights, so triangle membership does not depend on the conditioning
of the closest-point calculation. The interior-radius margin includes pose,
triangle and cylinder arithmetic; uncertain candidates retain native queries.
When the conservative point-displacement bound cannot meet Parry's unchanged
directional impact criterion, that proven overlap needs no query. Parry owns
this criterion for both native casts and the continuous owner. Separated shapes
retain their positive-time queries, and prepared material inequalities still
enforce their existing position tolerance. Changed trajectories revalidate the
motion bound; an initial intersection certificate is never a clear-space cache.
Initial depth certificates and native contact geometry are cached per exact composite leaf within one
substep. Position closure changes endpoints, so every sweep recomputes its
whole-arc support bound; unchanged starting geometry does not repeat GJK.
Starting geometry expires before the next substep captures its starting poses,
including every composite leaf's overlap proof, depth certificate and native
contact. Only advisory separation directions survive; all pair caches expire
when the next physics step rebuilds its admitted geometry. A cached
minimum translation depth beyond tolerance rules out a clear separating axis at
the initial pose, so closure need not repeat other separation-only axes.
Convex triangle starting contacts seed Parry's native GJK/EPA with their finite
feature axis. The native cast, bisection and fresh contact kernel own the geometry;
the search direction supplies no assumed plane penetration. Native
triangle/cylinder and triangle/cuboid references specialise that same entry
point, retaining both pair orientations and the generic kernel's exact geometry.
Native dispatcher witnesses retain their respective local frames until the initial
contact owner transforms them to world space. Nonfinite witnesses, invalid
normals, unsupported queries, or missing geometry from an unbounded cast query
or an enclosing initial query fault the
continuous solve. The adapter retains those failures across Parry's optional-hit
boundary, so an incomplete query cannot certify a clear trajectory. Initial
query witnesses are validated before entering their immutable cache. Native EPA
scratch retains buffer capacity across queries, with a reset of current geometry
on every call. A single-point touching simplex retains actual material witnesses
and its generating support direction. Missing direction provenance rejects the
query instead of inventing a normal.
The native upward-rounded relative-point displacement bound can also certify a
convex leaf's residual tolerance directly. Every occupied-shape support gap,
including its initial contact axis, changes by at most that complete bound.
The proof includes both offset pivots and native arithmetic margin at the actual
coordinate and geometry scale, and is re-evaluated after corrections;
larger or uncertain bounds retain native geometry. This does not enlarge the
contact tolerance or replace prepared material-contact checks.
The pinned Parry f64 owner uses maintained GJK distance bounds to classify
stalled simplices, retaining intersections for overlapping axisymmetric shapes.
Its enhanced determinism feature enables scalar vector math alongside libm and
ordered maps. The native Bevy/glam type contract remains unchanged.
Its nonlinear motion evaluator preserves the exact supplied starting pose and
evaluates the analytic COM-pivot trajectory without cancelling world transforms.
The native directional initial-overlap handler uses an upward-rounded relative
point-speed bound, including both offset pivots, to prove when its unchanged
thickness criterion cannot be reached. That proof omits only impossible initial
overlap sampling; positive-time casts and zero-thickness shapes retain their
native queries. Development builds explicitly optimise the workspace math and
physics owners while retaining assertions and overflow checks.
Contact projection alternates with the native hard-geometry joint schedule,
restricted to the affected connected bodies. It does not reintegrate motors or
repeat compliant channels. Its symmetric sweeps reverse both constraint kinds
and identities, propagating corrections through the prepared topology in both
directions. The ordered native passes execute on one thread;
closure acquires the body query once for all ten forward/reverse native passes
and does not dispatch workers between sequential constraints.
Native independent body preparation, integration, inertia refresh and writeback
use Bevy's adaptive parallel-query batching with
`SolverConfig.minimum_body_batch_size` (default 64). Populations at or below
that bound execute the same kernel directly on the caller; larger populations
retain native adaptive batching. A conservative query upper bound chooses
dispatch without visiting entities. Both native body and integrator plugins initialize the existing solver
configuration without replacing a supplied value. This controls work assignment;
per-body arithmetic and ordered contact/joint passes remain unchanged.

Ordinary substeps integrate each motor once. Admitted
material inequalities and native joints close before a full geometry refresh;
only a clear refreshed sweep can commit within `ContinuousCollisionSettings`.
Material-witness constraints bound separation throughout the trajectory,
including contact paths with clear endpoints. A whole-arc chord remainder can
certify a clear path directly; uncertain paths retain the extremum search.
Their SO(3) Jacobian maps corrections to the solved endpoint.

Prepared native contact points supply their own anchors and separation offsets
to continuous closure. Their trajectories may recover initial penetration but
cannot deepen it. Projection publishes its applied normal impulse to the same
native point accumulator used by friction, restitution and warm starting;
velocity projection applies it once. Native contact relaxation follows XPBD
velocity projection, so friction consumes completed suspension and continuous
corrections. Zero-time casts never supply witness
geometry. Positive-time impact witnesses cover new crossings and remain admitted
through the rest of the physics step until native preparation owns the pair.
Applied impulses for these new crossings are retained with exact collider
identities in the continuous owner. `contact_debug` publishes them separately as
`continuous_normal_impulse`, alongside the native discrete `total_normal_impulse`;
pair filtering uses the same collider hierarchy scope for both. Their sum is the
observed normal response, without double counting prepared contact projection.
Unsupported kernels, inconsistent witness statuses, invalid order or clocks,
and exhausted convergence bounds raise structured runtime faults.
The physics schedule guards joint-force publication, restitution, final pose/velocity writeback and impulse
storage in that same tick, so a rejected solve cannot publish a partial pose.
Native Avian body-only CCD is disabled. DEM, ring and mesh-backed terrain all use
triangle meshes with internal-edge correction.

`PhysicsPerformance` reports the continuous solve, geometry-sweep and joint-closure elapsed times, substep count,
cast-pair count and position iterations. Projected collider identities include
referenced-instance scope. Standalone terrain commands allocate identities in
ordered admission, so repeated use of one DEM asset cannot alias generated
colliders or scatter layers. These diagnostics cannot select solver
actions. Angular integration increments must remain below pi per substep;
otherwise endpoint quaternions cannot identify the complete rotational path and
the continuous owner refuses writeback.
Sweeps, convergence and native velocity projection share the native `atan2` SO(3)
logarithm, preserving small rotations and ignoring quaternion norm drift.
Witness checks prepare each rotational motion once; endpoint Jacobians are
computed only when a contact needs projection.
Projected acceleration bounds certify monotonic witness paths directly; paths
whose derivative can change sign retain the bounded interval minimization.
