# Terrain Substrate — the Height Oracle

> Status: Active · Audience: contributors working on terrain, LOD, or surface physics

How LunCoSim represents planetary surfaces from orbit down to a rover wheel,
with one analytic source that composes under **USD layering** and **scales to a
whole solar system**. One immutable source supplies independently sampled physical,
visual and camera-navigation products. Physical support stays independent of camera
position, graphics settings and visual worker completion.

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

### Ray queries and terrain picking

`GridSurfaceQuery` resolves screen rays against the composed `SurfaceOracle`,
independent of which visual LOD tiles or physics colliders are resident. The
oracle visits DEM raster cells front-to-back and uses conservative height
enclosures for the bicubic base and each analytic modifier. It subdivides only
intervals that can contain an intersection, so narrow crater rims and ridges
cannot be skipped by a fixed-distance march. A ray with no DEM in the scene
returns no terrain hit, allowing physics-only placement to continue.

These point-ray queries return a certified hit/miss or an error for unavailable
bounds, missing active-frame poses or exhausted work. Callers preserve that
outcome. Point rays do not establish camera-volume clearance or resolve initial
penetration.

Camera navigation uses the terrain owner's typed sphere sweep against the full
composed surface. Its result distinguishes clear motion, witnessed contact
proximity with point and normal, initial overlap, and unavailable work. Conservative enclosures
prune clear intervals; an enclosure upper bound alone is not a collision. The
source returns one `HeightEnclosure`: scalar height bounds and a `HeightPlane`
with residual bounds for the same composed surface. Raster Bézier controls,
registered regional blends and analytic modifier gradients retain slope
correlation; additive modifiers may update the plane. The sphere solver combines restricted column bounds with an
upper-plane half-space bound, so shallow travel over a smooth slope does not
subdivide the whole path to clearance-sized rectangles. Only exact source samples
provide contact witnesses. Admission ends at a certified clear-prefix boundary
with an actual solid witness no farther than the sphere radius plus the
configured geometric separation tolerance. The returned normal samples the
same full source at the actual witness, including at grazing incidence. The
overzoom enclosure bounds each value-noise octave at rectangle and lattice-cut
corners, using monotone interpolation within each cell. Its warp bounds use the
same actual rectangle. Overzoom bounds value-noise partial derivatives, the warp
Jacobian and crater profile derivatives before fitting its affine contribution.
The residual encloses the remaining variation over the rectangle; the fit alone
never certifies separation. Source evaluation remains within the existing work
budget; neither an enclosure nor an exhausted search becomes a contact. The camera owner combines terrain
and native prop constraints through the existing movement mechanism. Movement
casts the physical camera radius; skin is a directional pullback from an
approaching contact, rather than an enlarged collision shell. Each projected
slide is recast against all sources and props. Camera terrain queries exclude the separate native DEM
collider realization, so navigation does not change at a rover's collider-ring
boundary. Requested lateral motion does not generate an automatic upward step.
The camera envelope includes its declared clearance and projection footprint;
free movement, scroll transit and follow booms consume the same geometry contract.
The shared interactive `lunar_surface_camera.usda` explicitly authors the standard
`clippingRange` near plane at 0.01 m, so an omitted USD near plane does not impose
a metre-scale navigation envelope. Scene-authored lens overrides remain authoritative.
Perspective-avatar celestial clipping preserves that source minimum; fixed
observers and mounted sensors retain their authored clipping range.
The presentation query then checks the retained draw meshes as solid terrain,
including the columns below their triangles, together with stitch uniforms and
camera-dependent morph interval. Camera and triangle motion use the same time
subinterval, so a later morph state cannot block an earlier camera position.
If coarse or preparing geometry intersects the
source-safe path, navigation holds its current pose and publishes a bounded
refinement corridor. It never slides on a coarse approximation.
An initially overlapping fixed convex column permits explicitly requested
non-inward motion only when its actual contact plane proves penetration cannot
increase along the segment; every other column remains checked.
A fixed contact on the current finest drawn surface joins source and prop
contacts in one bounded Avian contact-plane projection loop. Each projected step
is recast against all three owners. Its admitted prefix retains a normal-space
tolerance margin even at grazing incidence. This redirects requested motion
along a real contact; it never adds positional depenetration or an upward step.
Strict source-up travel can leave
drawn overlap because distance to every column of that heightfield is nondecreasing.
Terrain publication is a separate admission boundary. New meshes remain hidden
and same-coordinate replacements retain their published predecessor until the
complete candidate cover, morph windows and stitches clear the navigating camera.
One commit publishes geometry, material deformation and visibility together.
An unsafe candidate keeps the admitted cover and camera pose, reports
`presentation_pending`, and withdraws latest-cover readiness. Physics continues.
Candidate admission uses the same bounded solid-heightfield geometry query as
navigation; a plane above the camera cannot pass as a clear isolated triangle.
The continuous corridor remains pending across render frames while the camera
position and source/output/projection lease remain valid; orientation damping
alone does not cancel it. Capacity refusal retains the exact request and remains
terminal until its input or admission changes. Refused-node counts describe only
the current committed requirement and admission. Scroll retains
its exact request and source/frame lease, and retries after the draw revision
changes; input or lease changes retire that request. Deliberately requested,
source-certified upward recovery from initial overlap remains possible while
native props remain checked. Mounted cameras retain their authored pose; their
terrain-local clearance is available in `TerrainLodStatus.camera_cover`.
Runtime acceptance is tracked in
[the camera terrain review](../reviews/2026-11-12-camera-terrain-architecture.md).

### Independent visual views and physical support

The physics product chooses its authored collider lattice, filter and deterministic
quantization. Dynamic assemblies demand their own support and build-ahead nodes.
Graphics quality and camera output residency do not change that selection.

The visual product consumes finalized camera/output snapshots in `PostUpdate`.
Main viewport, visible feeds and explicit captures contribute only while their
output is consumed. Pose, projection, physical output dimensions, scene identity
and source lease belong to the snapshot. An inactive or retired output contributes
no visual demand. Each view evaluates its own projected error and owns its exact,
restricted draw cover, parent fallback, stitch topology and morph context. Mesh
bakes are shared by source, node and resolution; view state is separate. All color,
shadow and prepass passes use the same parent view context.

Perspective error uses output pixels, vertical FOV and three-dimensional camera
distance. Orthographic error uses pixels per metre from the finalized projection
area and output dimensions. Translating an orthographic camera along its viewing
axis preserves scale; zoom and lateral motion change detail or its footprint.
Both projections clip nodes against the finalized view volume using conservative
full-source height bounds. A projected posting-size floor keeps unresolved small
features eligible at every distance, including telephoto views where coarse probes
filter them out. The immutable oracle caches its full-domain enclosure; native
cover workers compute it, while synchronous web preparation only reads an already
cached enclosure and otherwise keeps uncertain nodes eligible. Selection retains
conservative local clip envelopes with hysteresis; angular tolerances derive from
the projection, output pixels and configured pixel error. The same retained envelope
owns invalidation and clipping, so subpixel rig jitter can reuse preparation while
meaningful telephoto pans and roll changes wake selection. Native lens/output
changes retire incompatible preparation. Orthographic tiles use static parent-edge stitching without distance
morphing and discrete zoom cover changes. Unsupported custom projection metrics
produce a diagnostic.

Visual tiles sample the oracle through their own visual band. Their parent targets
sample the parent lattice's band and indexed triangle interpolation. Visual posting
counts are odd, so parent and child grids nest. Graphics maximum depth describes the
local domain; a power-of-two regional expansion adds its exact depth offset while
preserving finest spacing in metres. Unsupported domain ratios or effective depth
are diagnosed before admission. Collider settings do not select visual resolution.

Each consumed view has its configured tile budget. Cover preparation and tile
bakes share bounded asynchronous admission, upload budgets and cache byte limits.
The graphics byte ceiling retains its GPU upload meaning. Accounting counts
unique uploaded geometry across views and sources, including live old generations
and retiring tiles; retained CPU vertex/index payload is reported separately. Uploads wait
before allocation if residents consume the configured limit. Lowering the limit
does not discard visible coverage; pressure remains explicit in streaming status.
The consumed-output registry and output pixel admission bound the number and size
of views. New demands withdraw obsolete queued work; already-running
work retains exact scene/source/view leases at publication. Ready ancestors remain
visible until replacements and materials are admitted. Camera retention is bounded
in three dimensions and scoped to its view and source. A requested minimum detail
tree that cannot fit the selected resource budget is reported as unfulfilled quality.
Physics never waits for visual preparation.

`TerrainLodStatus.camera_cover` reports each view's local pose, clearance, selected
depth, effective maximum depth, metric spacing, readiness, pin state, wanted,
resident and drawn counts, and refused refinements. `contacts` reports physical
support readiness and lattice parameters independently of rendered tiles.
`presented_ready` describes the retained drawable cover; `ready` additionally
requires the latest requested selection to be settled in its current generation.
`stream.selection_pending` counts unqueued follow-up work and newly consumed
outputs as well as unfinished selections, so retained residency cannot authorize
an unfinished numbered capture. Finalized demand publication updates this fence
before the Last-stage offline capture decision. One-shot screenshots retain their
explicit current-presentation semantics. Neither status proves image quality.
Near-camera visual/source error and physical/source
error require separate measured tolerances; exact equality between their sampled
triangles is not assumed.

`visual_foci` distinguishes perspective FOV from orthographic area and pixel
density. Streaming mesh accounting reports unique retained GPU upload estimates and separate CPU payload,
resident/cache/retired subsets, the configured ceiling and refused uploads.

Routes and wheel marks remain shared `SurfaceCurveAnnotation` material inputs in
terrain-local XZ. Each view's own terrain fragments consume them without rebuilding
annotation data for camera motion or LOD. Route overlays stay unlit; physical wheel
marks receive terrain lighting. Production acceptance covers physical support,
independent main/feed detail, contact appearance, and route/trail registration.

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

### Analytic detail and product fidelity

Measured DEM heights and analytic modifiers form one source function. A crater
modifier can resolve below the base DEM posting interval because the modifier is
analytic; the measured DEM does not acquire new measured detail through interpolation.
The visual mesh samples the combined function at its selected spacing. Explicitly
authored `overzoom` supplies deterministic synthetic craterlets and micro-relief,
with provenance distinct from measured topography.

Each product's sampling and filter determine which source frequencies it retains.
The renderer's pixel error controls visible approximation; the physics lattice
controls contacts; camera navigation uses full-source volume queries. Quantify their
height and normal discrepancies at triangle interiors, crater rims and tile edges,
including visible wheel contact. Correct inadequate physical sampling at the
physical owner independently of camera rendering quality.

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
DEM elevation samples remain `f64` in the native and worker paths and in the
worker's persistent grid cache. Cached and fresh grids therefore retain the
same values; conversion to `f32` occurs at the rendering boundary.
The DEM worker owns cache hashing, decoding, encoding and OPFS persistence, so
the browser page does not perform those raster-sized operations.

`lunco-terrain-bake::regional::RegisteredRegionalSurface` provides the pure
sampling and registration checks for a pair of centered DEM grids. It preserves
the local grid inside its footprint, continues the difference between the local
and regional edge surfaces through a finite blend, and keeps measured regional
relief throughout the transition. Edge derivative contributions are bounded
by the respective sample spacings. Registration
requires matching declared frames and projections plus an
explicit vertical offset. `SurfaceOracle` can compose that sampler with its
height modifiers, and its certified solar traversal uses the regional footprint
and conservative enclosures. Those enclosures consume the same cell budget on
cache hits and misses and return an error on exhaustion. Authored terrain layers
can declare a registered regional DEM alongside the local surface; the native
loader admits the pair together.

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

## Detail on demand: measured error and metric spacing

The visual selector measures source height against the mesh's indexed anti-diagonal
triangles at node vertices and interior probes. These samples estimate visual error
and the node height range; they are not conservative collision certificates. Each
consumed view projects that error using its own output height,
projection and terrain-local pose. Nearest three-dimensional distance,
restricted 2:1 neighbors and bounded retention keep the selected cover continuous.

For domain width `W`, depth `d` and odd posting count `n`, visual spacing is
`W / (2^d * (n - 1))`. Effective depth expands with a registered regional root so
local finest spacing is retained. The visual band retains wavelengths at least twice
its spacing; parent morph targets use twice the spacing and the corresponding band.
Source sampling and tile mesh generation remain CPU-side, bakeable and content keyed.

Physics parameters are explicit: `colliderDepth` and `colliderResolution` on the DEM
layer supply `DemTerrainRequest`. The same spacing formula determines the physical
band from that independent lattice. Author enough resolution for the smallest
intended physical feature and measure native contact/source discrepancy. The
physics sampler, scheduling and support readiness do not consume camera metrics.

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
`CanonicalStages::reader_for`, which selects the worker-prepared
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
a persistent restricted cover using memoized `measure_node_geometry` per node
(`TerrainNodeGeometry`). The existing vertex/interior probes provide both
geometric error and a sampled height range. Selection measures nearest 3D
camera distance to each node's square lifted by that range; the near-detail
floor and retention band use the same distance. Camera poses are resolved in
the terrain-local f64 frame through `lunco-spatial`'s shared-grid `big_space`
conversion. Range factor comes from the canonical screen metric (`pixel_error`),
and bounded deterministic split/merge edits retain an exact cover under the tile
budget for each consumed view; **streaming** — off-thread tile bakes (budgeted per frame,
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

**Remaining terrain extensions:**

1. **Carve/mask channel** — the seam for caves/pits/skylights (baker clip +
   heightfield→trimesh fallback on mouth tiles).
2. **Canonical-Stage read migration** — move `bridge_usd_dem_terrain` + the layer parsers onto
   `UsdRead`/`StageView`; replace the `AssetEvent<UsdStageAsset>` reload observer
   with a terrain `UsdAttrProjection` off StageSink; make regen a physics-atomic
   activation unit (see *Alignment* above).
3. **Tile bake cache** — **partly done**: visual tile meshes are
   content-addressed on disk (`tile_cache`, keyed on `SurfaceOracle::surface_key`
   + tile coord), so a warm reload of the same composed surface streams instead
   of baking. Shipping visual bakes as spec+hash across peers remains open. Physics and
   visual meshes retain independent product identities and sampling contracts.

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

Avian 0.8 owns native joint preparation, XPBD constraints, motors, passive
`JointDamping`, contact generation, contact solving, friction, and swept CCD.
The shared headless and graphical compositions install those native plugins.
Application integration owns authored SI conversion, validated admission,
BigSpace frame transport, ordered multi-body gear relations, and typed telemetry.
Contact provenance and impulses come from native `ContactGraph` pairs.
The `native_force_drives`, joint, differential, and continuous-contact production
scenes qualify behavior; compilation alone does not qualify physics.

World positions and authored/domain values retain f64. Avian's internal solver
state uses f32; its explicit conversion boundary and deferred precision work are
documented in [Data precision](../principles.md#data-precision).
