---
name: render-quality
description: Diagnose or repair LunCoSim terrain appearance, albedo, shadows, lighting, material bindings, color flicker, overbright terrain, or missing visual layers while preserving image quality and authored rendering settings.
---

# Terrain and render-quality diagnosis

Read [`geo-assets`](../geo-assets/SKILL.md),
[`docs/architecture/50-usd-driven-visuals.md`](../../docs/architecture/50-usd-driven-visuals.md),
and the target scene's composed USD before changing a shader or asset. The
authored USD Material/Shader network and its assets own visual intent; the
runtime binder projects that intent to Bevy.

Make authored scene and material edits through the USD document tools: open the
exact source, inspect the composed value and edit target, apply typed
`ApplyUsdOp(s)` (or the owning schema-aware command), save, and read back. Do
not patch `.usd*` text directly, including for visual test scenes. When an
authoring operation is missing, add it at the owning USD API before changing
the scene.

For lunar-specific terrain data, regolith reflectance, multiscale detail, and
LOD approaches, consult the source-linked
[`lunar surface rendering research note`](../../docs/research/lunar-surface-rendering.md).

Check `RuntimeDiagnostics` for producer `render-recovery` before accepting a
rendered result. `presentation-stopped` rejects graphics evidence even if
`EnginePerformance` still returns numeric application-frame FPS. The renderer
preserves the simulation and API through that failure; inspect the native cause
and available GPU memory rather than changing authored quality automatically.

Use a task-owned `LUNCOSIM_CONFIG` directory for visual acceptance so restored
workbench documents cannot change the fixture. `LUNCOSIM_EPHEMERAL_SETTINGS=1`
alone does not isolate workbench restoration. Confirm physical viewport size
through `SceneCameraAudit` before comparing screenshots or FPS. Read the settled
active camera pose through `QueryEntity` before and after an FPS window; startup
view restoration can replace an early camera command.

Editor inspection uses a neutral studio rig, camera-local ambient fill and one
view-sized shadow cascade. Verify those cameras separately from physical scene
lighting; the mission sky and Sun calibration must not replace the CAD preview
background or its fill. Check front, back and underside views when tuning it.

Ambient authoring reads native composed DomeLights, including references,
schema defaults, exposure and activity. The dedicated runtime fill is excluded
from the sum; connected or time-varying inputs reject document authoring before
any edits. Run `scenarios/tests/native_ambient_authoring.rhai` through
`RunScenarioAsset` on the settled replacement fixture to verify that contract.

Use native WESL (`.wesl`) imports and conditional attributes. Headless
`ValidateAsset` checks syntax with the shared WESL frontend; full pipeline and
GPU acceptance still require the rendered fixture and a clean process log.

## Diagnose in order

1. Confirm the terrain prim, `material:binding`, Material surface connection,
   shader `info:wesl:sourceAsset`, and authored `inputs:albedo_map`/
   `inputs:normal_map`/weights in the composed stage. A composed-stage read is
   the runtime truth; do not open layers from a tutorial.
2. Inspect the actual asset role and color space. An illumination-bearing
   orthophoto is not intrinsic albedo: it must be processed with
   `kind = "albedo"`, which removes its broad source-light field and emits a
   stable linear material colour before PNG encoding. A calibrated reflectance
   raster may use the `texture` pipeline when its colour contract is known.
   `kind = "map"`, hillshade, slope, elevation colour, and mineral diagnostics
   are not direct albedo substitutes. Keep role-aware filtering and mip
   generation intact.
   For regular grain ripples, verify that DEM UVs and their derivatives use
   `terrain_half_extent`, independently of the shadow heightfield's `hf_size`.
   Compare settled frames with procedural relief and native shadow reception
   isolated separately. The High Gaussian filter uses normal bias 4.0,
   scaled by cascade texel size; keep its metre-valued depth bias and the
   physical Sun unchanged.
   Inspect the live light bias as well as the preset: saved Graphics values
   override changed preset defaults. For diagonal boundaries in close grain,
   isolate the raw procedural noise from lighting and geometry. Shared lattice
   corners must be identical; run the `noise_continuity` graphics fixture for
   the GPU value/gradient contract. Reload shader libraries with `ReloadShader`
   using an empty path so imported modules and their consumers refresh together.
3. Check the authored light and shadow contract, GPU shadow-resource status,
   and `primvars:doNotCastShadows` before changing material brightness. A
   renderer fallback that silently removes shadows is a failure to surface,
   not a quality setting to hide.
   For terrain shadows that separate at LOD transitions, verify that the
   authored vertex stage runs in both forward and `PREPASS_PIPELINE` variants.
   Use Bevy's `lod_view_world_position` for morph distance in shadow views;
   their ordinary view origin belongs to the light. Keep the shared material
   uniform bound in depth-only passes. Run the `vertex_shadow_alignment`
   graphics fixture: both lit-vacated and shadowed-displaced probes must appear,
   with no red disagreement pixels.
   Enable `DepthPrepass`, `NormalPrepass`, and `MotionVectorPrepass` through the
   existing reflected `add` interface when checking optional camera passes.
   Declare the camera dependency in the scenario. The terrain appearance and
   vertex-shadow fixtures exercise these variants; shader compilation errors
   fail the graphics gate. Motion-vector view flags occur on forward pipelines
   too, so previous-position varyings/bindings need the prepass guard as well.
   Include the separate diagnostic material: run
   `scripts/api/test_terrain_overlay_prepasses.py --output <evidence-dir>`
   (add `--windowed` for a desktop view). It exercises slope, LOD-depth, and
   restored production rendering with all prepasses active. Both vertex stages
   use the shared `terrain_morph_weight` law and the viewing camera's LOD origin.
   Reject GPU validation errors in the process log even when domain
   `RuntimeDiagnostics` is clear; a new screenshot can contain a retained frame
   after GPU submission fails.
   For scaled terrain lighting, run `terrain_normal_transform`: world-to-local
   normals use the transpose of the local-to-world linear transform, rather
   than the inverse used for positions.
   For measured shadow cost, use the diagnostics-enabled render owner's four
   `lunco_shadow_*` GPU spans described in `performance-profiling`. Confirm
   timestamp coverage and keep profiler attribution separate from unprofiled
   visual acceptance before changing shadow preparation or rendering.
   Static and streamed terrain remain native CSM casters and receivers so
   terrain shades objects and objects shade ground. The heightfield cache
   supplies ground self-shadow; cached tiles use `csm_far = 0` to avoid a
   fragment march at every distance. The `shadow_cache_sampling` fixture checks
   the GPU cache's endpoint-inclusive grid, including fractional interpolation
   and border clamping, plus the current below-horizon gate inside CSM range.
   Keep a committed cache bound across night; run the `sunrise_shadow_cache`
   editor fixture to verify repeated crossings, upload acknowledgements and one
   resident GPU allocation. Pending bakes retain a sampled presentation value;
   they must not block fixed physics. CPU light/cache agreement alone does not
   prove shader uniform freshness. Run `scripts/api/test_sun_material_precision.py`
   with an isolated `--output` directory; repeat for
   `--scene assets/scenes/tests/sun_material_precision_dem.usda` to cover streamed
   materials. `--windowed` uses the same gate in a desktop view. The GPU compares
   horizon signs with the native light and tests zero/positive angular radius.
   Retain daytime coalescing while always publishing local horizon crossings;
   static `HorizonMap` owners must enter the same material feed as DEM owners.
   Cache UVs must map
   the CPU bake nodes to texel centres before filtering.
   For panel power, read the wired probe's `sun_visibility`,
   `sun_irradiance_w_m2` and panel `available_power_w`.
   Physical samples use the f64 terrain oracle and selected optical proxies.
   See `docs/architecture/terrain-layered-rendering.md` for the production
   gates. Do not infer whole-body night occlusion from a finite DEM.
   The layered shader keeps measured landform normals and adds filtered
   sub-metre clump/grain normals. Each noise octave fades independently between
   two and four pixels per period, and its removed slope variance contributes
   to roughness. Inspect the fine-grain band as well as broad clumps in close
   views; a whole-stack cutoff can hide resolvable grain while leaving blobs.
   Relief remaps have continuous slopes at their clipping thresholds. The
   DEM normal-map weight selects the landform band without erasing the grain
   tangent slope; use full weight when the measured map owns those slopes.
   The layered material allows filtered `micro_albedo` grain only below an authored
   raster's texel size; broad procedural colour is suppressed by raster weight.
   `inputs:roughness` owns the base, with centred procedural variation.
   Scatter boulder reflectance is linear, and shared boulders cast shadows.
   For close terrain breakup, inspect the High profile's first-cascade bound
   separately from its maximum shadow distance. Keep sub-DEM synthetic crater
   geometry within the terrain tile's resolved sampling; measured orthophoto
   albedo and footprint-filtered shader detail provide stable close texture.
   Compare `TerrainLodStatus.focus_wanted` with `focus_resident`: a fully
   resident cover can still leave the active camera outside the DEM crop, where
   the near-field detail floor cannot refine the viewed ground. Verify the
   camera's terrain-local position against the authored DEM window before
   raising LOD budgets. For the `overzoom` layer, density zero disables
   synthetic craterlets and leaves only FBM relief; choose feature radii and
   density with High tile spacing in mind.
   Treat packed surface-map AO as indirect-light visibility: it belongs in
   Bevy's PBR diffuse-occlusion input, not in authored albedo or direct-sun
   multiplication. If broad terrain colour patches match a low-frequency AO
   map, inspect that channel routing before touching assets or exposure.
   For tile-shaped holes with a fully resident/prepared cover, inspect native GPU
   output-buffer stride before changing material lighting. `UninitBufferVec`
   allocates by `ShaderSize::SHADER_SIZE`, not Rust `size_of`; enhanced-determinism
   scalar math changes the host mesh layout. Keep the pinned renderer patch,
   native GPU culling and physics scalar math intact.
   Terrain selection uses nearest 3D distance to each node's sampled height
   envelope, including the near-detail/retention rules. Resolve camera and terrain
   through the existing shared-grid `big_space` path; ground clearance beneath
   the camera is not the elevation of every tile.
   For accelerated celestial failures, inspect the terminal solar diagnostic and
   run `celestial_terrain_clock.rhai` with the settled probe ID. Preserve physical
   fixed-step semantics and conservative bounds; do not raise quotas or fabricate
   sunlight to hide a traversal/certificate failure.
4. Compare a settled frame sequence, not one screenshot. Fast color changes
   usually indicate changing inputs, repeated derived bakes, missing asset
   readiness, or unstable lighting—not a reason to clamp the image in a
   shader. Find the owner and gate the work by asset/revision events. Camera-
   driven terrain cover selection runs at 30 Hz wall-clock cadence and uses
   shared bounded background admission on native visual hosts. The owner fences
   and commits current results in `Update`; tile-mesh bake admission still uses
   its per-terrain queue. Offline capture lockstep bypasses the cadence and runs
   selection synchronously for each captured frame. Profile remaining frame
   hitches before changing LOD budgets.

Standard `UsdPreviewSurface` input edits travel through
`UsdSceneChangeBatch`; the visual owner refreshes the bound `PbrLook` in place
and the renderer rebinds its material. If the authored value changes but the
surface stays stale, inspect that owner path before adding a scene rebuild.

Analytic primitives may carry `SharedMeshAsset` and share immutable mesh handles.
An entity-specific UV or geometry edit must detach before writing; quality
retessellation replaces keyed assets and point instances follow inherited handle
changes. Verify both the edited entity and an unchanged peer. Use the
[mesh ownership contract](../../docs/architecture/render-decoupling.md#immutable-primitive-mesh-assets)
when diagnosing cross-entity geometry changes or stale instance meshes.

For celestial globes, verify the installed Earth/Moon imagery dataset, composed
USD body look, and generated shell's shadow intent. Globe tiles and their
shared-surface DEM collars disable casting and receiving via `ShaderLook`; the
material binder alone applies the Bevy markers. DEM terrain and scene objects
retain their shadows, and shell surfaces retain normal sunlight shading.
The physical globe/collar cover uses the same f64 geometry and datum with
body-driven residency; camera LOD and shadow flags cannot remove ground contact.
For offscreen captures, the explicitly bound `SceneViewport.active_camera`
must drive globe LOD even when it renders to an image. A window-target filter
would omit the Moon and show the local DEM as an isolated square in space.
A body shader look may add shader parameters, but it must retain the installed
dataset albedo unless USD supplies an explicit
albedo layer or `AuthoredBodyAlbedo`. Body frames, globe tiles, stations,
terrain, links, sky, and lighting share `CelestialTime`, a child of `WorldTime`.
The ordinary render interpolation sample drives unbound USD time-sampled
animation. Use the body's single physical body-fixed grid for site and globe
content; do not align a parallel presentation hierarchy or copy a surface
marker into another frame. Modelica and Avian keep their ordinary fixed-step
cadence; physical celestial inputs evaluate that same affine domain at exact
communication boundaries through `CelestialClock`, independently of the render
solve gate.
Procedural sky materials opt into the live Sun disc by declaring both
`sun_dir_view` and `sun_tan_radius` as engine inputs; no shader filename selects
the behavior. Radius comes from the selected light's authored `inputs:angle`,
shared with terrain penumbra. Opaque scene depth clips the disc independently
of terrain shadow visibility; run `sun_disc_occlusion` for authored radius and
half-disc occlusion. The direction is in the active camera's view coordinates, the
same frame as the shader's view rays. `SunRenderState` comes from the
generic `sun` direction resolver after the scene light is finalized through
BigSpace. `SunState` supplies irradiance only. The renderer projects the
finalized light direction into camera space; do not add a second ephemeris
calculation for sky materials. Camera pose changes
refresh the direction while CelestialTime is paused. Continuous renderer data
stays in Rust; author the background and material binding in USD.

The globe's resident mesh cap defaults to 72 MiB. A Twin can override it in
`twin.toml` with the positive integer byte setting
`[settings] "celestial.globe_lod.max_resident_mesh_bytes" = 100663296`.
The application applies the override when the active Twin settings change,
not in the frame loop. An invalid value is reported and holds globe LOD
reconciliation; correct the Twin setting before diagnosing a coverage failure.
For a terrain-to-globe join, use the crop that already backs the local terrain.
Its retained DEM data stays unchanged, and its border datum sets the render-only
globe shell radius; celestial placement and physics keep the canonical radius.
Keep the composed terrain exact throughout the crop. One worker-prepared visual
collar starts on the measured crop boundary and blends its edge profile to the
matching sphere outside the crop. Derive its width from measured crop-edge
relief and one-sided slope, using a 0.60 relief-grade sizing target.
Continue the slope over one posting and fade the single nearest boundary
signal toward the sphere over at least one full crop width. A narrow exterior
can turn broad edge relief into a steep artificial dark rim. Check the
`terrain_surface_continuation` graphics verdict and ground views with the
physical Sun and shadow settings unchanged. Geometry and material fade share
one width sized from crop scale and maximum edge relief;
each corner contributes once. Preserve the native inner posting lattice, then
reduce exterior ring density to the outer boundary with at least 32 segments per side. Queries and colliders remain bounded to the DEM, whose samples
stay unchanged. Globe tiles clip against the collar's outer boundary with
bounded edge sampling. Add a camera-independent LOD floor only in the band
between the crop and the outer cutout, sized from the handoff footprint; keep
the rest of camera-driven globe LOD independent of DEM posting density. The
sphere beyond the crop is visual closure, not measured regional terrain. The
collar reads local maps from the DEM prim and appearance from the body's
USD-selected `material:binding`. Its shader asset path remains in USD. A
continuation shader declares matching WESL and USD
`lunco.lunar-surface-continuation.v1` interfaces; `RunLint` compares the authored
declaration with loaded WESL. Rhai holds simulation for incompatible sources.
Only the compositor admits a DEM collar's visibility after its composed look
is bound; globe cutout admission consumes that readiness and visibility.

Keep body curvature publication before the authoritative DEM build. Run the
DEM-to-globe handoff and globe LOD in
`RuntimeCycleSet::Visualization` after terrain builds and UI commands.
Raster loading and mesh generation stay off the visualization and physics
threads. Visualization keeps one keyed collar preparation per globe, polls
without waiting, rejects stale results, and installs current handoffs. Globe
meshes commit in stable coordinate order under count and byte budgets. Worker
timing may affect visual arrival, never the simulation oracle.

## High-profile near detail

For a fully prepared DEM screenshot or performance window, read
`TerrainLodStatus.derived` alongside `stream`: require `active == false`,
`pending == 0`, and `ready == total`, as well as a resident selected cover with
no pending stream work. Derived-map preparation is separate from simulation
readiness; a usable viewport alone does not prove that preparation has finished.

Query `TerrainLodStatus` and inspect `max_depth`, `tile_budget`, and
`budget_refused` before changing the High LOD. A resident count close to the
budget does not by itself prove that detail was refused. High currently uses
depth 9 with a 256-tile budget and 65 vertices per tile (about 3 cm per
interval at the deepest level over a 1 km crop). The shared terrain kernel
evaluates bump gradients analytically, filters each octave by its pixel
footprint, and transfers unresolved slope variance into roughness. The
`noise_continuity` GPU fixture checks fully resolved, partially resolved, and
unresolved spectra in both 2D and 3D. Apply lunar
photometry to the engine-selected Sun contribution only; putting it in
`base_color` also changes fill and earthshine.

For live terrain changes, trace the committed `TerrainSurfaceChange` record
from the replaced `SurfaceOracle` into the shared visual invalidator, collider
ring, static-collider debounce, and derived maps. Bounded reuse requires the
consumer's cached source key to match the record's previous key; the collider
ring also requires the immediately preceding revision. Reject static-collider
jobs whose oracle key is stale. Keep visual and physics sampling bands and
caches independent.

Use `inspect-simulation`, `record-video`, or the API screenshot surface for
visual evidence. Preserve authored DEM resolution, shadow ranges, BigSpace,
and physics settings while tuning render LOD through its quality profile. If an
asset is missing, report the owning cache or manifest error visibly instead of
adding a procedural fallback.

Authored RGBA8 shader rasters use `lunco-materials::ShaderTexture` typed prepared
sources. Retain their source handles for native reloads; the worker publishes a
complete native child before the renderer binds it. Its render-only usage moves
pixels through native GPU extraction while retaining main-world metadata; reload
the source to publish new texels or sampler settings. Generated derived maps use
complete native images directly. `load_raster` admits physical sources and rejects
container labels; bind imported labeled images through their owner's native
handle. Preserve native image/glTF loader settings and
validated request-time anisotropy. For CPU-built RGBA8 mip chains, preserve the
role-aware filtering above while
using the GPU texture extent rule `max(1, floor(size / 2))` independently on
each axis. Never use ceil-halving or silently clamp an oversized mip count:
the level data would no longer match the texture's legal subresources.

For DEM continuation colour seams, compare the site base colour and authored
raster weight with the exterior composition. Source reflection owns omitted
parameter defaults; the exterior must compose that site colour and lunar
photometry before fading to the body appearance. Preserve the DEM material
inside the crop.

For finite DEM joins, compare the collar native perimeter and the rendered DEM
boundary/morph vertices using bounded `TerrainLodStatus` pages. Inspect each
corner obliquely with shadows enabled. `ShaderLook` owns shadow intent and the
material binder owns Bevy cast markers; geometry systems must not compete with
that reader. Removing a shadow artifact is not proof of geometric continuity.

Exterior smoothing preserves the first native posting and filters only the
continuation in its preparation worker, without increasing mesh density. Keep
the DEM appearance visibly distinct. See [the geometry contract](../../docs/architecture/60-curvature-elevation-and-gravity.md)
for ownership and sampling.

Verify omitted and DEM-authored coordinates through the
[automatic-registration fixture procedure](../geo-assets/SKILL.md#verify-automatic-lunar-dem-registration).

USD light values, including colour, temperature controls, dimensions, and
normalization, come from native schema resolution. Change
scene photometry through authored USD, and use `editor_preview_illuminance`
only for editor helper lights. Quality updates preserve USD photometry and the
preview fill/key ratio. The focused production gate is
`lunco://scenarios/tests/native_light_schema_defaults.rhai`, attached through
`RunScenarioAsset` to the settled replacement fixture; its negative input must
produce a terminal command rejection without changing the document.
