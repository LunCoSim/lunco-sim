# Terrain Layered Rendering & Analysis Overlays

> Status: Active · Audience: contributors working on terrain rendering, scientific overlays, or the
> comms/illumination map layers. Companion to
> [`terrain-substrate.md`](terrain-substrate.md) (the height oracle) — this doc is
> about what gets **drawn on** the surface, not how the surface is shaped.

How LunCoSim renders **many layers on one terrain** — textures, scientific
overlays (slope, minerals), a lat/lon graticule, and **time-dependent maps**
(connectivity, illumination) — as a single extensible, live-tunable, USD-authored
system, without a bespoke shader per layer.

The renderer uses the unmodified Bevy Render package and its supported math
feature layout. Physics retains enhanced determinism without enabling global
scalar math, preserving the renderer's GPU ABI and native culling and shadows.
See [the physics integration review](../reviews/upstream-only-physics-integration.md)
for dependency feature selection and current runtime qualification.

## Terrain material decision record (2026-09-21)

The current source inventory has one authored terrain appearance contract, one
optional geometry stage, one explicit non-authored material, and one general-purpose
regolith material. Runtime ownership matters here: USD selects the fragment
source and may select the vertex source on the same Shader prim. The terrain
runtime consumes that projected look; it does not choose a shader filename.

| Path | Authoritative source | Current contract | Design defect to remove |
|---|---|---|---|
| Authored DEM fragment | `assets/shaders/terrain_layered.wesl` | The single canonical DEM material: authored albedo/layers, DEM relief, filtered detail, lunar photometry, and terrain self-shadow | None; this is the only production appearance implementation for authored DEMs. |
| Streamed DEM vertex stage | `assets/shaders/terrain_geomorph.wesl` | Optional CDLOD morphing and crack-free edge stitching; no fragment entry point and no second material law | Its `Material` declaration is only the shared uniform ABI; all appearance remains in the fragment above. |
| Explicit non-authored terrain material | `assets/shaders/terrain_shadow.wesl` | Plain authored/display albedo, scene PBR, and heightfield sun visibility | It may be selected by an authored Rhai/tool policy for a terrain with no authored material; Rust never installs it to hide a missing or invalid authored source. |

`assets/shaders/regolith.wesl` is excluded from this inventory: it is the
general `ShaderMaterial` for ordinary regolith meshes, not a terrain source
selection. `assets/shaders/terrain_debug.wesl` is already the separate
diagnostic material and must remain outside the production material contract.

### USD stage authoring

An authored terrain material uses standard `UsdShade` stage metadata:

```usda
uniform token info:implementationSource = "sourceAsset"
uniform asset info:wesl:sourceAsset = @lunco://shaders/terrain_layered.wesl@
uniform asset info:wesl:vertexAsset = @lunco://shaders/terrain_geomorph.wesl@
```

`sourceAsset` is the canonical fragment/material contract. `vertexAsset` is
optional: omit it for a static or otherwise ordinary mesh vertex path, and use
it for streamed CDLOD geometry. The USD projection resolves both assets into
one `ShaderLook`; Rust does not contain a terrain shader selector or a hidden
default for this production path. The explicit `SpawnDemTerrain` command
remains a generic tool API and requires its caller to provide a look.

The authored vertex stage runs in the main, depth/normal, and native shadow
passes. It selects Bevy's forward or prepass output interface using
`PREPASS_PIPELINE`; the material group remains bound in depth-only passes so
the same morph uniforms are available. CDLOD distance uses
`view.lod_view_world_position`, the viewing camera's origin shared with its
shadow cascades. Edge stitching and morph interpolation therefore describe the
same surface in every pass. The `vertex_shadow_alignment` GPU fixture displaces
an authored caster and requires its native shadow at the displaced position,
with the vacated position lit.

Native depth, normal, and motion-vector components are registered by the camera
binder and can be enabled through the shared reflected component interface.
Motion-vector definitions also occur on forward pipelines; previous-position
outputs and previous-view bindings belong only to `PREPASS_PIPELINE`. The terrain
appearance and vertex-shadow fixtures enable all three native prepasses and
reject shader compilation failures as well as missing image probes.
The separate diagnostic vertex stage obeys the same pass interfaces, including
previous-frame positions for motion vectors. Both vertex stages consume
`terrain_morph_weight` from the shared surface module so analysis preserves the
production morph and parent-edge stitching law. The production GPU driver
`scripts/api/test_terrain_overlay_prepasses.py` switches through production,
slope, LOD-depth, and restored production views with all prepasses active.
The render wrapper runs it for each selected quality profile. GPU validation
callbacks must also be checked in the process log: domain `RuntimeDiagnostics`
does not include those renderer failures, and a captured PNG can retain stale
pixels after a failed submission.

Static and streamed terrain therefore differ only in geometry delivery. A
static mesh is effectively the non-morphing/root LOD of the same appearance
contract; the streamed ECS quadtree may still manage residency, colliders, and
LOD selection independently.

Procedural relief operates in the terrain-local normal frame. Bevy's
inverse-transpose maps a local normal to world space; the reverse mapping uses
the transpose of the mesh's local-to-world linear transform. This preserves the
normal direction under nonuniform scale as well as rotation. The
`terrain_normal_transform` GPU fixture checks that round trip on a rotated,
nonuniformly scaled mesh.

DEM-global UVs use the terrain owner's `terrain_half_extent` for procedural
detail coordinates, their pixel footprint, and authored albedo texel size.
`hf_size` describes the independent shadow heightfield and must not choose
material scale. The High profile's Gaussian shadow filter uses normal bias
4.0, scaled by each cascade's shadow texel size, to keep grazing-angle terrain
from sampling its own surface inside the filter footprint. Depth bias remains
0.03 metres.

### USD-authored DEM-to-globe continuation

Exterior smoothing preserves the first native posting and filters only the
continuation in its preparation worker. Geometry and material fades multiply
the two axial weights, keeping corner diagonals smooth. The existing bounded
ring tessellator accounts for the product fade's curvature. Keep
the DEM appearance visibly distinct. See [the geometry contract](60-curvature-elevation-and-gravity.md)
for ownership and sampling.


The generated collar has two independent scene inputs: the DEM prim supplies
the crop dimensions and local map images, while the celestial body's composed
`UsdShade` binding selects the surface appearance. The collar clones the body's
current `ShaderLook` and adds the DEM material base colour, cropped albedo, and packed surface images in
the dedicated continuation texture roles. Rust carries the USD-selected asset
paths and typed values; it contains no lunar fragment or vertex shader path.
The source reflection publishes typed shader defaults, so an omitted DEM
albedo uses its shader-owned default. The exterior composes the site base
colour with its authored raster weight before fading to the body appearance.
Both continuation shaders use `terrain_continuation_gradients` for the colour
and packed surface maps: inside the crop it retains the screen footprint;
outside, the footprint grows with distance from coverage. Mip filtering averages
the clamped edge before the spatial fade, preventing edge texels from stretching
into long stripes. This is an appearance continuation, not additional measured
coverage.
`ShaderLookSourceInterface` belongs to the render binder's source-reflection
path, independently of material binding and render readiness. Retiring a
material or invalidating `ShaderLookReady` preserves those loaded-source facts.
Source replacement/removal and reload publication are handled by the reflection
owner. Source request admission precedes reflection; material readiness
invalidation precedes readiness promotion. Required consumers must not lose a
valid interface while their material waits for binding or repacking.
The exterior also carries the site lunar photometry parameters and fades
its direct-sun response to the body response. Shared PBR and terrain lighting
helpers own lighting; no heightfield shadow march runs for the collar. The
packed terrain uniform remains within 256 bytes.

A body fragment shader that accepts these local roles declares
`//!@interface lunco.lunar-surface-continuation.v1` in WESL and authors the same
identifier as `info:wesl:interface` on its USD Shader prim. The optional vertex
asset remains USD-owned. Omitting it uses Bevy's standard mesh vertex stage;
the generated collar has its own final geometry and carries no CDLOD morph
attributes. The globe shader guards forward lighting imports and fragment
entrypoints out of `PREPASS_PIPELINE`; its native prepass vertex entrypoint
remains available for depth, normal, motion-vector and shadow passes. Shader assets remain hot-reloadable and the USD binding can be
changed while the scene is running.

The required Rhai hook `terrain.lunar_surface_continuation` receives the USD
declaration and the reflected WESL interface after the source loads. It returns
`compose` only for a valid match, `wait` while source reflection is pending,
and `hold` for an invalid interface. The compositor owns DEM collar visibility:
it admits only the composed look after material binding is ready. Invalid
interfaces raise `RuntimeFaults` and leave the collar hidden. Globe cutout
admission consumes that visibility and material readiness, so a cutout cannot
precede its admitted continuation. `RunLint` reports a declared/actual mismatch
and a missing required continuation interface from the same reflected facts.
The composed body look is carried by `GlobeLod`; its authored declaration entity
is retained only as provenance for validation. Generated body entities are not
USD material sources.

This composition is visualization-only. It never resamples, blends, edits, or
replaces DEM heights, terrain queries, or colliders. The separate collar mesh
continues the measured edge profile outside the crop and reaches the render
sphere; local DEM appearance maps fade across that same visual shoulder. The
shoulder width comes from the active crop's measured boundary relief and
one-sided slope with a 0.60 relief-grade sizing target. One measured perimeter
signal continues over a posting and fades to the sphere over at least one full
crop width, preserving broad landform scale outside the measured boundary.
Crop width and maximum relief size one shared width for exterior geometry and
material fade, independently of the Sun and camera. A unique corner sample closes
the transition around corners. The rectangular outer boundary keeps the
collar mesh and globe cutout watertight; this changes neither the DEM nor its
physics surface.

### Decision

The production target is one authored DEM appearance contract with one
diagnostic replacement:

1. A shared DEM base owns lunar albedo, DEM normal, authored-map presence,
   anti-aliased detail, roughness, and sun visibility. Streamed and static
   delivery may retain different vertex stages, but they must not choose
   different physical surface laws or distance-dependent colour/normal rules.
   When a heightfield is present, the engine marks the look with the explicit
   `terrain_geometry_on` source fact. The DEM mesh remains authoritative for
   geometry and collision, while a co-registered authored/derived DEM normal
   owns the measured normal band for visible lighting whenever it is present.
   The tile mesh normal is the geometric fallback for materials without that
   map; it must not be mixed into a mapped DEM's lighting law because its
   interpolation changes at a CDLOD boundary. A separate footprint-filtered
   sub-metre clump and grain normal is allowed on every DEM as shading-only
   detail. It does not modify height or collision and fades before it aliases.
   The layered material recovers the grain tangent slope independently of the
   geometric normal and projects it onto the selected DEM-map normal. Raising
   the map weight to one therefore preserves grain while removing tile-normal
   interpolation from the landform band.
   Analytic noise gradients evaluate each layer once. Relief uses a cubic height
   remap with continuous slopes at its clipping thresholds; colour-only ramps
   remain linear. Each FBM octave filters its own footprint between two and four
   pixels per noise period, retaining resolvable close grain while suppressing
   finer slopes. Filtered octaves converge to their mean without renormalizing
   the remaining spectrum. An estimate of the removed per-octave slope variance
   transfers into GGX roughness using the same squared resolution weights. Authored
   `inputs:roughness` owns the base, with centred clump variation. The mid
   procedural hummock band is restricted to non-measured surfaces, so it cannot
   duplicate measured landforms.
   When USD supplies an illumination-bearing grayscale orthophoto, the native
   asset pipeline's `kind = "albedo"` bake removes its low-frequency source
   illumination, anchors local detail at a neutral regolith value, sRGB-encodes
   the linear material colour, and lets the texture loader decode it back to
   linear. The terrain shaders use that authored material directly at its
   weight; they do not apply an orthophoto transfer at runtime. `kind = "map"`
   remains an analysis/display product and is not a valid direct albedo input.
   The static layered path scales its broad procedural dust/mottle colour by
   `1 - weight_albedo`. The measured raster owns the colour frequencies it
   resolves. Authored `micro_albedo` modulates its mean only for grain periods
   smaller than a raster texel, with a smooth frequency gate derived from the
   bound albedo dimensions and terrain width. Clump, micro and fine colour
   bands share the already evaluated relief samples, each with its own footprint
   and raster-frequency gate; a disappearing fine band does not remove resolved
   clump texture. The same stable coordinates and
   pixel-footprint fade filter grain colour and normals. Relief normals,
   roughness, ambient occlusion, and photometry remain independent, so camera
   footprint or CDLOD replacement cannot introduce unrelated colour changes.
   The packed surface map's G channel is ambient occlusion and is sent to
   Bevy's `PbrInput.diffuse_occlusion`, where it modulates indirect diffuse
   light only. It must never be multiplied into base albedo: doing so turns a
   broad derived field into false colour patches and also darkens direct sun.
   Lunar powder materials use diffuse lunar scattering without an additional
   smooth dielectric GGX reflection. That separate lobe can dominate dark
   regolith at grazing angles and erase map contrast. The continuation blends
   the site's zero dielectric reflectance into the body's authored `reflectance`
   along with its photometry. The standard Moon material authors zero; the
   generic blueprint default remains 0.5 for other materials.
   Exposure remains authored camera/environment intent, independent of albedo.
2. `terrain_debug.wesl` remains the only diagnostic replacement. LOD depth and
   slope are analysis data, not branches or uniforms added to the production
   material.
3. `terrain_shadow.wesl` remains an explicit material for non-authored terrain.
   A Rhai assembly or recovery policy may select it when that is the authored
   scenario behavior. Rust never selects it automatically and never uses it to
   hide missing or invalid authored source data.

The existing `terrain_surface.wesl` and `lunar_brdf.wesl` modules remain the
authoritative owners for shared procedural detail and lunar photometry. Heavy
orthophoto decode/filter work belongs to the native
`lunco-assets-processing` Rust asset processor. The
authored Rhai assembly tools may select the resulting standard `albedo_map`,
normal, and weight inputs, but must not reproduce the raster math or create a
second material representation. USD-authored `UsdShade` source and map roles
remain the source of intent, while the terrain reconciler supplies only the
engine-derived products that USD did not author.

### Procedural noise continuity

The shared GPU value noise hashes integer lattice identities with Bevy's PCG
generator before floating-point interpolation. Adjacent cells must reuse exactly
the same corner value, including at negative, high-frequency terrain coordinates;
rounding in the hash must not become a visible lattice boundary. Scalar noise and
analytic-gradient noise share that owner. The `noise_continuity` graphics fixture
checks both the 2D and 3D value/gradient contracts on the production GPU path.

### Shadow ownership

Static and streamed terrain both keep native directional-shadow casting and
receiving. Native CSM carries terrain shadows onto objects and object shadows
onto ground. A heightfield cache only supplies ground self-shadow; it cannot
replace the terrain caster needed by object materials. `wire_terrain_materials`
sets cached streamed materials' `csm_far` to zero so the cached ground visibility
covers every distance without a fragment march. Native shadow-map resolution,
range, and cascade count remain owned by Graphics. Static materials retain
their native CSM range.

The terrain uniform feed covers both DEM owners and static meshes carrying a
`HorizonMap`. Ordinary meshes without a heightfield use the separate global Sun
feed. Daytime angular drift is coalesced, but a change across local `sun_dir.y = 0`
always reaches the material because it changes the shader's night branch.
Authored angular-radius changes compare exact GPU values, including zero.

The visibility cache stores an endpoint-inclusive nodal grid, matching the CPU
bake's `i / (resolution - 1)` footprint coordinates. Its shader maps those nodes
to texel centres before bilinear sampling and clamps at the footprint boundary;
raw normalized UVs would shift shadow edges by up to half a cache texel. The
`shadow_cache_sampling` graphics fixture checks nodes, fractional interpolation,
and borders against independent integer texel loads on a rectangular grid.

The cache binding stays resident across the local horizon. A shared shader gate
checks the current Sun direction before sampling: at or below the DEM's horizontal
plane, visibility is zero even inside native cascade range. This finite-site
point-source gate is separate from whole-body occlusion and from sky visibility.
Sunrise uses the same angular replacement scheduler as daytime; small direction
changes use chord distance so f32 cosine rounding cannot suppress a refresh.
The shared light projector compares its rendered quaternion directly, preserving
small direction changes through the horizon without an angular dead zone.
One native worker per terrain retains the last completed angular presentation
sample until the next bake finishes. It does not delay physics or claim that
old pixels represent the current Sun exactly. Completed results are fenced by
heightfield identity and sampling quality. Equal-size R8 updates use the shared
`lunco-render::data_texture` queue, preserving GPU allocation and material binding. Uploads run strictly
before the render submission set because the GPU queue uses interior mutability.
`TerrainLodStatus.shadow_caches` exposes sampled direction, pending work, binding,
and acknowledged upload/allocation counts. The `sunrise_shadow_cache` editor
fixture exercises repeated crossings and fixed physics cadence.

Procedural sky disc radius and heightfield penumbra use the same authored
`UsdLuxDistantLight.inputs:angle`. Opaque scene depth clips the disc from the
viewer's position; terrain shadow visibility never hides the sky globally.
The `sun_disc_occlusion` graphics fixture checks the authored angular size and
an opaque half-disc occluder.
`sun_material_precision` and its DEM counterpart compare the terrain uniform's
horizon side to Bevy's native light on the GPU. Their live driver
`scripts/api/test_sun_material_precision.py` exercises tiny crossings and the
zero/positive authored-angle boundary while keeping material bindings resident.
The High render gate invokes that driver after the initial-frame pixel check.


Physical illumination is independent of the camera and GPU shadow cache.
`LunCoEnvironmentProbeAPI.outputs:sun_visibility` is the fraction of explicitly
authored sampling points with a clear ray to the Sun centre. A point probe is
an `Xform`; an area probe uses standard `UsdGeomPoints.points`. The shared panel
uses nine equal-area cell centres just above its collecting face. The USD bridge
composes their metric offsets with the native double transform, including scale and `resetXformStack`;
the live framed pose supplies motion. This declares a spatial approximation,
not exact cell coverage or solar-disc penumbra.

`lunco-terrain-surface::solar_visibility` publishes demanded samples before the
fixed co-simulation exchange. Initialization waits for irradiance, terrain
preparation and the shared direction resolver's physical observer/source poses.
Celestial irradiance alone does not establish pose readiness. Missing,
ambiguous or coincident direction sources still reach the sampling owner's
terminal diagnostic; fixed-tick sampling remains strict after admission.
Terrain queries traverse native raster cells and
check coarse cells before breadth-first refinement using conservative enclosures
of the same bicubic surface and analytic modifiers used by the terrain oracle.
The raster owner converts Catmull–Rom patches to tensor-product Bézier controls
and uses their restricted convex hull for conservative bicubic bounds, preserving
coefficient correlation. If a reusable canonical cell enclosure is unresolved,
the cache attempts bounded dyadic cuts, preferring a half that contains the
complete current segment. When the segment crosses both cell midplanes, it
clips both halves and certifies their complete interval cover. The shared slab
clipper preserves fresh f64 endpoints for both cache preparation and the outer
breadth-first traversal. A reusable scratch queue and two child slots avoid
per-cut allocation. The complete surface key fences every certificate. Whole
raster cells use their two integer indices as their cache identity; refined
rectangles additionally retain their exact coordinate bits. Nearby rays can share
these spatial bounds while retaining their full f64 coordinates. The
`SurfaceOcclusionBudget.max_ray_cache_depth` bound limits this preparation
(eight cuts by default; zero skips preparatory cuts);
every attempted cut and classification follows the same order on a hit or miss.
Each query fences retained bounds against its complete immutable surface key
and current capacity before any enclosure read. The surface remains unchanged
during that query, so bounds lookups share this one validation boundary.
An unresolved certificate proceeds to the same oracle's exact footprint bounds
before interval subdivision. Terrain beside that footprint cannot block the ray. Work
quotas and unavailable/error semantics remain independent of cache residency.
Actual surface samples certify blockers; clear results require complete
interval coverage. Raster traversal advances integer cell identities at the
crossed boundaries, advancing both axes at a corner. It does not recover the
next cell from a rounded ray position or perturb the ray with an epsilon.
Zero-length entry spans advance the cell without consuming a sampling interval.
Within one ray, exact coordinate-bit identities reuse
height samples at shared endpoints and refinement positions. This cache is
discarded with the ray and holds at most three samples per admitted interval;
it changes neither interval budgets nor the classification order. This search
order keeps an uncertain contact edge from starving a later definite blocker.
Unsupported bounds, numerical
ambiguity and exhausted work return unavailable data. `SurfaceOcclusionBudget`
bounds each ray, total interval work per fixed tick, canonical ray-enclosure
preparation and retained spatial enclosures.
Probe admission follows unique composed USD paths; terrain intervals follow
canonical geometry keys. Scene teardown clears sampled visibility, cell bounds,
and optical geometry before the next Twin loads.
Canonical cell subdivisions are shared across sampling points and moving probes, with bounded FIFO
admission and complete surface-key invalidation. Exact pose, direction, surface
identity and participating shape revisions fence complete-sample memo reuse.
Spatial certificates and per-ray point samples use Bevy's maintained hash map
for exact-key lookup. Neither map is iterated for query decisions; the separate
FIFO queue and geometric traversal retain admission and evaluation order.
The Overzoom owner derives signed global crater-band enclosures from its actual
bowl, rim, apron, reach-tail and population limits, intersected with regional
enclosures. Positive and inverted bowls retain distinct upper/lower limits.
Tighter certificates avoid unnecessary canonical cuts and exact-footprint
refinement without changing the sampled terrain, fresh ray, work budgets or
unavailable-data behavior. The population generator and enclosure share their
rim-ratio and per-cell count constants.

`scenarios/tests/celestial_terrain_clock.rhai` attaches to a settled celestial DEM
through `RunScenarioAsset`, with the wired environment probe's `probe_id`. Its
40-second fixed-step window at 100,000× covers more than forty celestial days and
checks every demanded solar sample, physical elapsed time and transport rate.

Equipment optical proxies are explicitly selected with standard
`CollectionAPI:occluders` on the probe. Members must expose an admitted collision
shape and pose. The physics owner supplies a reusable f64 Parry BVH; neither
camera LOD nor streamed DEM colliders select physical terrain visibility.
Omitting the collection selects no equipment proxies. Authors must select
geometry that represents their optical occluders; collision geometry does not
imply opacity. Each sampling ray tests the union of terrain and its selected
proxies, so overlapping shadows cannot be counted twice.

The Sun provider supplies `outputs:sun_irradiance_w_m2` separately: direct-normal
physical irradiance in W/m2, calibrated by `LunarSun.irradiance_w_m2` at one AU and
scaled by inverse-square distance for a celestial source. Render lux and exposure
are separate quantities. The electrical network explicitly routes irradiance,
visibility and direction through its root boundary to `SolarPanel.mo`. Modelica
owns incidence and electrical conversion, and publishes incident irradiance and
radiant power for thermal models to apply absorption and losses. This panel is a
lumped current source; string mismatch and bypass diodes require an authored
Modelica electrical network with independently sampled zones.

Missing demanded physical samples are withdrawn and produce a structured terminal
fault. `RequiredOutputSamples` prevents the generic exchange from substituting
zero, and the Modelica worker does not dispatch another step after an earlier
fault in the same fixed transaction. Physical queries cover admitted finite DEMs
and explicitly selected proxies; they do not establish whole-body eclipse or
occlusion beyond that geometry. The terrain shader's below-horizon check remains
necessary for its finite heightfield domain; it is not a global Sun-hide rule.

Production gates: `terrain_solar_visibility` exercises raised, partially shaded,
moving-proxy, fully terrain-shaded and clear panel states through Modelica;
`terrain_solar_visibility_night` verifies below-surface sunlight with a panel
that faces the Sun; `terrain_solar_visibility_invalid_admission` declares the expected runtime fault
before loading `terrain_solar_visibility_invalid`, then requires the terminal
`solar-sample-unavailable` diagnostic from the sampling owner.

The canonical fragment applies lunar photometry and heightfield visibility
only to the selected Sun's direct contribution, using Bevy's directional BRDF
and native shadow for that term. It does not multiply `base_color` or the
completed PBR result. Ambient light and authored earthshine remain independent
when a terrain horizon blocks direct Sun.

### Distance and evidence contract

| Distance | Required appearance | Allowed work |
|---|---|---|
| Near | DEM relief plus authored normal when present; resolved granular regolith detail; stable CSM/horizon visibility; no tiled or plastic look | Measured DEM geometry owns relief; footprint-filtered sub-metre clumps and grains supply shading-only detail. |
| Middle | Same albedo/roughness law and DEM relief; authored surface maps remain registered; no shader-path or LOD seam | Procedural detail fades analytically by footprint, not by mesh depth or morph state. |
| Far | Stable authored albedo and large-scale relief with horizon visibility; no noisy high-frequency colour detail or mode switch | Use the existing derived/authored map source contract and pre-baked horizon cache. |

Missing authored maps have one semantic: the source-presence/weight contract
selects the documented derived product or the base material inside the authored
shader. A missing tile or map is not converted into a second Rust-selected
visual material. An invalid shader stage is different: it is an authoring error,
reported through `RuntimeDiagnostics`, and leaves the material unbound until an
explicit USD/Rhai edit repairs or replaces it. Height, collider, streaming, and
lighting/time ownership remain unchanged by this decision.

Filterable authored RGBA8 roles use `ShaderTexture` prepared-source requests.
The native asset worker decodes and builds the full CPU mip chain before
publishing the native child image, with distinct typed source identities for
color, scalar and normal roles. All tiles share those handles. Color roles filter
in linear light, scalar roles in their stored linear space, and normals are
renormalized. Native image settings carry linear/trilinear filtering and the
validated anisotropy profile. Generated derived maps already supply complete
native images. The render binder consumes these products without copying pixels;
changing a terrain tile or camera does not rebuild the chain.

The shared performance reference is the open High-quality Apollo target:
200 FPS sustained, 5.0 ms p95 frame time. It is a budget to measure against,
not a claim about this shader pass. The optimization checkout currently has no
maintained DEM graphics fixture or settled near/middle/far capture set, so this
design pass records the source-level decision and does not claim visual
acceptance. A production capture must be added only when a repo-owned DEM
fixture is available; the dirty external Summer Space School scene is not a
safe substitute for a reproducible fixture.

## The principle: one pipeline, not one shader per layer

A slope overlay, a mineral map, an elevation ramp, and a comms-connectivity map
look different but are the *same three decoupled pieces*:

```
layer = Data source  →  Transfer function  →  Blend
        (a field)        (field → RGBA)        (over the base)
```

You never write "a slope shader" and "a mineral shader" and "a connectivity
shader." You implement a **fixed, small set of transfer functions once**, and every
scientific layer is a `(data source, transfer function, parameters)` triple. Adding
temperature or illumination later is a data channel plus a colormap — no new code.

This split is also what makes **real-time tuning free**: bake the *data* once (a
texture), evaluate the *transfer* every frame from **uniforms**. Changing a critical
slope angle, a colormap, or an opacity is a uniform write — instant, no re-bake.

The streamed terrain material keeps its source selection explicit. USD-authored
albedo, mineral, surface, and normal roles are projected once into
`TerrainAuthoredMaps`; the engine-derived surface/normal product fills only roles
that USD did not author with a positive weight. The existing fixed texture slots
are reused, and source-presence uniforms select the corresponding shader terms.
Initial Lit tiles are not exposed until the USD material projection is complete
and every role has a selected source. A fully authored surface/normal pair can
start immediately; otherwise the streamed terrain waits for the off-thread
derived product. This prevents either producer from triggering a whole-terrain
material replacement and GPU upload spike after the scene is already visible.
Subsequent source changes use the same single reconciliation path for all
resident tiles.

The streamed runtime keeps this ownership explicit at the ECS boundary. The
terrain pass reads and mutates only entities marked `DemTerrainSurface`; tile
look updates target only entities marked `LodTileOf`. Bevy cannot prove those
markers are mutually exclusive from filters alone, so the pass exposes the
two views through one `ParamSet` and applies the collected tile-look changes
after the terrain state pass. This preserves one authoritative terrain reader
and one tile writer without allowing an ambiguous overlapping query.

## Fields are data; render is one consumer

The single most important rule: **an analysis layer's field is computed CPU-side as
data, and rendering is one downstream consumer of it — never its definition.** A
slope map is not "a shader that colours by `normal.y`"; it is a **`SurfaceField`** —
`value_at(x, z) -> f32` (scalar or categorical), evaluated purely on the
`SurfaceOracle`, headless, deterministic, content-keyed. Slope, aspect, elevation,
AO, hazard, and connectivity are all `SurfaceField`s — the data-side parallel to the
height channel's `HeightSource`.

This is the same relationship height already has: the mesh is the *shadow* of the
oracle, not the source of truth. One field, many consumers:

| Consumer | Uses the field for |
|---|---|
| **render** | materialise to overlay tiles (the tiled model) → transfer → colour |
| **query** (API / rhai / MCP) | point / region sample — **headless, no GPU** |
| **planner / cosim** | slope → traversability, hazard → no-go, connectivity → comms windows |
| **USD** | a `lunco:layer = "field"` prim *describes* it → addressable / composable / replicated |

Consequences that shape everything below:

- **Author the field once, in Rust, on the oracle** — not in WESL. It then runs
  headless (the field exists with no renderer), is unit-testable and deterministic,
  and bakes to tiles for the GPU. `derive.rs::slope_map` + the oracle's `normal_at`
  are already exactly this; `query.rs` already returns `slope` for a point with no GPU.
- **WESL only samples + colourises.** For a purely geometric field a shader *may*
  re-derive it live (zero-bake), but the Rust stays authoritative — a determinism
  contract like the tile bake, not a second definition.
- **A field is a USD layer, describe-don't-store** — `{ field, params }`, no pixels;
  any tool resolves it by prim path, queries it headless, or renders it. A dynamic
  field (connectivity) adds only a time binding.

So a slope "view" is the *last, optional* step: the field is real data first, useful
to headless tools, the planner, cosim, and scripting whether or not anything draws it.

## Two planes: shading vs annotation

An "overlay" is composited in one of two places. Choosing the right plane per layer
is the first design decision.

| Plane | What it is | Belongs here | Mechanism |
|---|---|---|---|
| **1 — terrain material** | recolours the surface *per pixel*, lit + occlusion-correct, inside the terrain material's fragment stage | albedo/mosaic texture, mineral map, slope/AO/roughness, elevation ramp, connectivity coverage | a standard USD `UsdShade` material supplies the shader source and authored maps; the render-free `ShaderLook` reconciler adds only engine-derived map inputs |
| **2 — diagnostic/annotation** | either temporarily replaces the terrain material for analysis, or marks it with surface strokes or independent geometry | slope/LOD diagnostics, lat/lon graticule, region/ROI boundaries, traverse paths, landing rings, coordinate labels | a separate authored diagnostic material for analysis; sparse fragment strokes for terrain routes; overlay meshes/labels for other annotations |

Rule of thumb: **does the layer change how the ground *looks under light* (Plane 1),
or is it a temporary diagnostic/marking tool (Plane 2)?** The terrain diagnostic is
Plane 2 and swaps a separate material; the lat/lon grid is also Plane 2 but remains
independent geometry. Neither adds analysis branches to the production shader.

## Data sources — including dynamic, time-dependent ones

A **data source** is a field sampled over the terrain with a **type** (scalar /
categorical / vector) and a **refresh policy**:

| Kind | Example | Refresh |
|---|---|---|
| **static baked** | slope, elevation, AO, hazard | bake once off-thread, content-addressed by `SurfaceOracle::surface_key` (like `derived_layers`) |
| **authored raster** | mineral classification, FeO/TiO₂ abundance, NASA albedo mosaic | load a USD-referenced image once |
| **in-shader derived** | slope = `acos(normal.y)` | a *render optimisation* of a geometric field — zero bake, but the field's authoritative definition stays the CPU `SurfaceField` (headless), which the shader must match |
| **dynamic / sim-driven** | **connectivity, illumination-over-time, temperature** | recompute off-thread on a **cadence**, driven by sim time / ephemeris / observer state |
| **manual / painted** | user-drawn regions, a custom hazard / no-go map, hand annotations | edited in-app via the **brush pipeline**; persisted as a USD layer — a sparse paint field or an authored raster (see *Manual maps*) |

The dynamic kind is the only structural addition beyond a static texture stack: a
`Dynamic { driver, cadence }` provider re-evaluates its data texture on a throttled
cadence (the same async-bake + debounce machinery the horizon and derived-map bakes
use, re-triggered by *time* instead of an oracle swap). Everything downstream —
transfer, blend, legend, controls — is identical to a static layer.

### Connectivity ≡ the horizon-shadow cache, generalised

The sun-visibility `HorizonShadowCache` (an R8 "is the sun above the local horizon
in direction D at time t" texture) **is already a dynamic, time-dependent coverage
map.** A **connectivity map is the same computation with a comms target instead of
the sun**: per terrain cell, is the relay satellite (position from the deterministic
ephemeris) / Earth / a base station above the local horizon — the `HorizonMap`
oracle already gives the skyline per direction — and within range/SNR at sim-time
`t`? Output a coverage scalar `0..1` (or categorical connected / marginal / blocked).

So connectivity, illumination-over-time, and Earth-visibility all reuse the existing
**horizon oracle + ephemeris**, and connectivity specifically should **derive from
the existing comms/connectivity substrate** (`comms-connectivity` design), not
reimplement line-of-sight.

### Determinism & caching for dynamic maps

A dynamic map is keyed on `(surface_key, time-bucket, targets)` → content-addressable
**per time bucket** → cacheable and **peer-deterministic**: at the same virtual
sim-time every peer computes the identical map, so it replicates as spec+hash, never
pixels (the ephemeris is deterministic). Cadence tracks how fast the *driver* moves
(satellite motion), not the frame rate; scrubbing `lunco-time` updates the map on
cadence (respecting pause ≠ speed-0). Aggregation variants fall out for free:
instantaneous coverage, time-integrated ("% of a lunar day with comms"), or forecast
("coverage over the next orbit") — all just how the provider reduces over time.

## Tiled overlay layers — LOD, coverage, and memory in one move

The naïve delivery — one full-terrain draped texture per layer — fails three ways at
once: it can't match the terrain's sub-5 m *near* tessellation (blur), it can't express
a layer that covers only part of the site, and N such textures blow the memory budget.
**Tiling the overlay data the way the terrain is already tiled solves all three
together** — the Cesium / planetary-renderer approach: an imagery quadtree over the
terrain quadtree.

Each layer's data is a **tile pyramid keyed by the terrain's `QuadCoord`.** A resident
terrain tile at `(depth, x, z)` samples its layer's tile at the matching coord (or the
nearest coarser ancestor the layer provides). Consequences:

- **LOD parity — free.** The overlay tile's resolution tracks the terrain tile's
  tessellation depth: crisp near, coarse far, refining as you move. No draped-texture
  blur, no giant full-res texture.
- **Partial coverage — intrinsic.** A terrain tile with *no* tile in a layer → that
  layer contributes nothing there (transparent). **Missing tile = NoData — no separate
  mask.** Ragged edges *within* a tile use a per-texel **alpha / coverage** channel in
  the overlay tile. So: tile-presence = coarse coverage; per-texel alpha = fine coverage.
- **Memory — view-bounded.** Overlay-tile residency = terrain-tile residency: stream a
  layer's tiles with the terrain tiles that need them, evict on tile eviction. Memory
  scales with `view × active layers`, not `full terrain × all layers`, and reuses the
  terrain tile cache / OPFS / streaming machinery wholesale.
- **Multimap — natural.** Each layer owns its pyramid, extent, max-depth, and CRS. A
  landing-site mineral inset has tiles only there; a global map has them everywhere;
  each contributes only where it has coverage. Different source resolutions → different
  max pyramid depth per layer (a layer clamps / over-zooms to its finest tile beyond its
  own resolution, exactly like the DEM over-zoom).

The tile is the unit of **bake, cache, stream, and dirty** — identical to terrain tiles
— so every data-source kind slots in per-tile: *derived* (slope/AO baked per tile),
*authored* (sliced/resampled from a source raster into the tile), *painted* (the brush
writes the tiles under its footprint, dirtying only those), *dynamic* (connectivity
computed per tile per time-bucket, cached on `(tile, time-bucket)`). A simple small-site
layer degenerates to a **one-tile pyramid** (a single draped texture at depth 0), so
tiling never taxes the easy case.

**External geodata** maps straight onto this: an external tiled source (WMTS / slippy)
*is* a pyramid; an untiled GeoTIFF is reprojected + sliced into the terrain pyramid on
ingest (the georef reprojection step). Either way the renderer only ever sees
terrain-aligned tiles.

## Efficiency & performance — design invariants

- **Derive in-shader where the field is geometric** (slope/aspect from `normal`,
  elevation from position): zero bake, per-pixel, no texture, no LOD mismatch. Reserve
  baked/streamed tiles for data not recoverable from geometry (minerals, AO,
  connectivity, paint).
- **Connectivity reuses the baked horizon skyline, never a fresh ray-march.** Per cell:
  `target_elevation(t) > horizon[azimuth]` — a LUT read off the existing `HorizonMap`,
  on a coarse tile depth (coverage varies slowly), not per-pixel. A naïve per-cell
  ray-march per target per time-bucket would be ~1 M marches per recompute.
- **Dynamic-map recompute runs in the web worker on wasm**, not the AsyncCompute pool
  (which degrades to the page main thread and would freeze the tab — the DEM-decode trap).
- **One uniform-driven pipeline, not a permutation per active-layer-count / transfer.**
  Branch on an `active_layers` uniform; keep all transfer/blend params in uniforms so a
  tweak never mints a new material in the `(entity, mode, depth, band)` cache or triggers
  a first-use pipeline compile (stutter).
- **Paint writes partial tiles** — rasterize a stroke into the AABB of the tiles it
  touches (`write_texture` on the dirty rect); never re-upload a whole layer.
- **The graticule is a retained line mesh** rebuilt only on anchor/zoom change once it is
  dense; per-frame gizmos stay fine only for a sparse grid.
- **Layer residency is explicit** — a layer's tiles bake/upload only while it is visible;
  toggling a layer off evicts its tiles.

## Manual maps — drawing on the terrain

A hand-drawn map is the **same edit pipeline the height brush already uses**
(`terrain-substrate.md` → *Dynamic modification*), retargeted from the height channel
to a data channel (raster paint) or to overlay geometry (vector annotation). Two forms:

- **Raster paint (Plane 1).** A paintable data source. The brush — reusing
  `raycast_surface` onto the oracle, the tool palette + arm gate, and the footprint
  math — writes values into a **`SparsePaintField`**: a bounded sparse edit-raster of
  touched cells, the paint analogue of the deferred `SparseEditField` for height. It
  feeds any transfer (paint colours directly = Passthrough; paint a scalar "hazard
  0–1" → Ramp). This is how a user hand-authors a custom no-go / region / hazard map.
- **Vector annotation (Plane 2).** Click to drop vertices on the surface
  (`raycast_surface` → oracle hit) → a waypoint / traverse route / ROI polygon, stored
  as USD prims, drawn as gizmos / overlay mesh.

Everything the height-edit story provides comes for free:

| Editing need | Provided by |
|---|---|
| Undo / redo, history | the USD journal — revert the op |
| Multiplayer sync | the stroke is a journaled + replicated `lunco:layer` op |
| Permissions | layer-scoped RBAC on the terrain prim |
| Non-destructive | edits ride the session/runtime layer over an untouched base |
| Commit granularity | a paint drag edits the runtime texture live; authors **one** USD op on release (a click authors at once) — never per-frame |

**Replication caveat.** Parametric strokes (a circular brush at centre/radius/value)
replicate as a tiny **spec**, like the height brush. But **freeform per-pixel paint is
genuine data** — it cannot be spec'd — so a painted layer ships as a bounded sparse
raster / authored image asset (exactly like the mineral mosaic), not as parameters.
Keep it sparse (touched cells only) and content-addressed.

## Transfer functions — a fixed set, all uniforms

The transfer maps `data → RGBA`, fully parameterised by uniforms (so tuning is
instant). Four cover everything:

| Transfer | Params (uniforms) | For |
|---|---|---|
| **Ramp** | colormap LUT (256×1), domain `[min,max]`, gamma | continuous scalars — elevation, abundance, coverage |
| **Threshold / Classify** | stops, **critical value(s)**, discrete\|smooth | **slope hazard** (green < safe, amber < critical, red beyond), connectivity connected/marginal/blocked |
| **Palette** | class → colour table | categorical — mineral classes |
| **Passthrough** | — | an authored RGB mosaic, no remap |

Concretely: **slope-hazard = slope data + Threshold (critical angle uniform)**;
**minerals = classification raster + Palette**; **elevation = elevation data + Ramp**;
**connectivity = coverage data + Ramp or Threshold (min-SNR uniform)**.

Unify the slope-viz threshold with the sim's actual hazard classification
(`hazard_from_slope`'s safe/cliff angles in `lunco-terrain-core/src/derive.rs`) so the
coloured overlay shows *exactly* the hazard the rover planner uses — one source of
truth, live-tunable for both.

## Real-time parameter control — one param, three surfaces

Every transfer/blend parameter is a **reflected uniform** on the dynamic
`ShaderMaterial` (the `//!@`-annotated self-describing material, same path
`terrain_debug.wesl` uses). Exposing a param as a reflected uniform gives, for free:

- **Inspector** — a slider (critical angle, opacity, domain, colormap picker);
- **Scripting / API** — `cmd("SetObjectProperty", { critical_angle: 0.35 })` over rhai / HTTP / MCP;
- **USD** — the layer prim carries defaults (`lunco:layer:slope:criticalAngle`); live tweaks ride the runtime/session layer, journaled + replicated like any edit.

Changing the value writes a uniform → instant, no re-bake. Changing the data channel
swaps the sampled texture; changing the colormap swaps the LUT.

## Legends

Each transfer function *implies* a legend: Ramp → gradient bar + domain labels;
Threshold → safe/caution/hazard bands with the live angle; Palette → mineral swatches
+ class names. An egui legend panel reads the active layer's transfer function and
re-renders as parameters change.

## Extensibility — reuse the USD layer stack

Layers ride the existing `TerrainLayerStack` (`lunco:layer` child prims, folded in
prim order). Extend the `TerrainLayer` contribution kinds — today height_modifier /
stamp / scatter / configure — with two rendering arms:

- `texture_layer() -> Option<TextureLayer>` → folds into the **in-material** stack (Plane 1);
- `overlay_layer() -> Option<OverlayLayer>` → spawns/updates an **on-top** entity (Plane 2).

A new layer type = impl `TerrainLayer` + a parser, registered via
`add_terrain_layer("<type>", parser)` — no changes to the build/scatter/regen
systems. `Changed<TerrainLayerStack>` makes add / remove / reorder **live**, and the
`LayerId` (USD prim path) is the addressable handle.

**The material constraint & fix.** `ShaderMaterial` today has fixed named
`#[texture(N)]` slots (height/albedo/mineral/surface/normal/shadow), already tight
against WebGPU binding limits — it cannot grow one slot per dynamic layer. For an
arbitrary-N in-material stack, replace the growing named slots with **one
`texture_2d_array` binding + a per-layer uniform array** `[{ weight, blend_mode,
transfer, params }; MAX_LAYERS]`, where — under the tiled model above — each array
slice is *this tile's* overlay tile for that layer (small, view-resident), not a
full-terrain texture. The fragment loops the active slices, generalising the current
fixed `mix()` sequence. Fold order = array z-order; a slice with no resident tile is
skipped (its layer is transparent here — intrinsic NoData).

## Georeferencing — matching real lunar terrain

Overlays that carry lat/lon (the graticule) or geographic data (minerals,
connectivity) need a projection that matches the source data. Do **not** pick one
arbitrarily — match the DEM's own CRS and the lunar cartographic standard, and store
it in `TerrainGeoref`:

- **Reference body:** a **sphere, R = 1737.4 km** (IAU mean lunar radius — the Moon
  is mapped as a sphere, not an ellipsoid), in the **Mean Earth / Polar Axis (ME)**
  body-fixed frame = `IAU_MOON`. Keep this consistent with the celestial body's frame
  (`CelestialBody::radius_m`) so the surface DEM registers onto the globe correctly.
- **Projection — read from the DEM, per site:**
  - global / equatorial products (LOLA LDEM) → **Equirectangular** (simple cylindrical);
  - **polar / Artemis south-pole sites** (Shackleton, Connecting Ridge) → **Polar
    Stereographic** (the LOLA polar-DEM standard). Equirectangular and the flat
    tangent-plane approximation both degenerate at the poles, so the site DEMs in play
    need real polar stereographic.
- `TerrainGeoref` should therefore carry: projection type
  (`Equirectangular | PolarStereographic { south }`), centre lat/lon, reference
  radius, scale / standard parallel, and implement **forward + inverse**. The DEM's
  GeoTIFF tags already declare all of it — read them
  (`lunco_geotiff` / `dem::read_geotiff_transform`), never restate them. The inverse
  (XZ → lat/lon) is what both the graticule *and* the orbit→surface
  `CompositeHeightSource` need, so implement it once here.

## Coordinate grid — Plane 2

A `TerrainGraticule` component + a system that steps integer lat/lon lines, maps them
to XZ via `TerrainGeoref`, samples the `SurfaceOracle` for Y so the lines hug relief,
and draws them with `Gizmos`. Per-viewer toggle, no material change, works in every
shader mode.

## Current status and next work

Implemented: deterministic `SurfaceField` and `TransferFn` queries, the
`TerrainField` region query, a separate streamed-CDLOD diagnostic material, live
`SetTerrainOverlay` partial updates, and the inspector legend/sliders. The
headless field is not yet addressable as a USD layer; production terrain materials
are authored through standard USD `UsdShade` bindings and the same map-source
reconciler serves static and streamed paths.
Open DEM ground is picked through the same `bevy_picking` backend using the
analytic `GridSurfaceQuery`; it does not retain CPU mesh vertices or require a
separate click path per scene tool.

Remaining work, in dependency order:

1. Add the `lunco:layer = "field"` USD descriptor and tiled field materialisation.
2. Add `TerrainGeoref` forward/inverse projection and the lat/lon graticule.
3. Add a tiled overlay stack with terrain-LOD residency and NoData coverage.
4. Add dynamic providers and illumination-over-time maps.
5. Add manual raster and vector authoring on the existing edit-tool pipeline.

## Resolved by the tiled model

- **Overlay LOD parity** — overlay tiles refine with the terrain quadtree (was the
  biggest technical gap).
- **Coverage / NoData / partial maps** — missing tile = transparent; per-texel alpha for
  ragged edges. No separate mask system.
- **Memory for N layers** — view-bounded residency reusing terrain tile streaming.
- **Multiple simultaneous overlays** — per-tile `texture_2d_array` slices.

## Still open (designed-but-not-detailed / analyst-facing half)

- **External geodata ingest** — the reproject + crop + resample of a real mineral /
  compositional product (own CRS, resolution, extent) into the terrain tile pyramid. The
  practical blocker for authored geodata; needs a georef-aware tiler.
- **Data probe / readback** — hover/click reads the layer value ("slope 22°, anorthosite,
  comms blocked") and highlights the legend. Extends `query.rs` (height/slope today) to
  sample the active layers. Core analyst tool, not yet specced.
- **Layer-manager UI** — the GIS "layers panel": list / toggle / opacity / reorder / solo
  per layer, with legends. The stack + inspector exist; the management surface does not.
- **Lit-vs-unlit diagnostic presentation** — analysis false-colour is usually **unlit**
  (slope reads the same regardless of sun) in a separate diagnostic material; a
  per-layer lit/unlit toggle and replacement policy need pinning down.
- **Time-control UX + range integration** — a scrubber tied to dynamic maps, and the cost
  model for "integrate coverage over an orbit / lunar day" (many time samples).
- **Dynamic-map cadence** — fixed sim-time step vs adaptive driver-motion threshold
  (satellite angular rate); likely the latter.

### Mesh inspection

`TerrainLodStatus.stream` reports selected-cover fulfilment and render-resource
publication. Its `derived` object reads `TerrainDerivedStatus` directly:
`active`, `ready`, `total`, and `pending` describe the optional surface/normal-map
lifecycle. A fully prepared quality measurement requires no pending stream or
derived work, a resident selected cover, and `derived.ready == derived.total`.
These read-only facts do not add a simulation hold or change tile admission.

`TerrainLodStatus` exposes admitted continuation sources, material values, native
corners, and corner spokes in the active physics frame. `include_geometry: true`
returns a bounded mesh page: `vertex_offset` and `vertex_limit` (1..4096), plus
`index_offset` and `index_limit` (1..12288). Positions are available in local and
active physics coordinates; index values address the complete mesh. This
explicit query reads retained CPU collar geometry and performs no per-frame
readback. Finite DEM boundary tiles retain CPU mesh data within the mesh-cache
budget; interior tiles remain GPU-only. `tiles` lists current visible tiles and
whether their boundary geometry is inspectable. Supply `mesh_entity` with the
same paging parameters to read a selected tile's positions and morph positions
in the active physics frame. `boundary_only: true` selects the finite crop's UV
perimeter before paging; `vertex_indices` retains original mesh indices. Index
pages continue to reference the full mesh. Collar and DEM pages use one reader.
Persistent visual tiles include the current sampling/layout revision in their
content key; changing shared boundary math requires updating that revision.
`include_shadow_meshes: true` lists locally visible shadow-casting meshes.

Generated globe/collar appearance derives render-shell shadow intent once from
`GlobeLod`; image updates and mesh creation consume that same look. The canonical
material binder is the sole writer of `NotShadowCaster`, including streamed DEM
cache transitions. Geometry owners do not write competing shadow markers.

Run `lunco://scenarios/tests/terrain_surface_continuation.rhai` on the admitted
DEM prim with `RunScenarioAsset.target` set to that prim's entity. A scene-owned
host supplies simulation query authorization. The scenario pages the complete
native perimeter and compares it to the existing `TerrainHeights` mission
query; visualization data does not become a second elevation oracle.
