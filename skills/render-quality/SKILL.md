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

## Diagnose in order

1. Confirm the terrain prim, `material:binding`, Material surface connection,
   shader `info:wgsl:sourceAsset`, and authored `inputs:albedo_map`/
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
3. Check the authored light and shadow contract, GPU shadow-resource status,
   and `primvars:doNotCastShadows` before changing material brightness. A
   renderer fallback that silently removes shadows is a failure to surface,
   not a quality setting to hide.
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

For celestial globes, verify the installed Earth/Moon imagery dataset and the
composed USD body look. A body shader look may add shader parameters, but it
must retain the installed dataset albedo unless USD supplies an explicit
albedo layer or `AuthoredBodyAlbedo`. Body frames, globe tiles, stations,
terrain, links, sky, and lighting share `CelestialTime`, a child of `WorldTime`.
The ordinary render interpolation sample drives unbound USD time-sampled
animation. Use the body's single physical body-fixed grid for site and globe
content; do not align a parallel presentation hierarchy or copy a surface
marker into another frame. Modelica and Avian keep their ordinary fixed-step
cadence while their celestial-derived inputs change from the shared sample.
Procedural sky materials opt into the live Sun disc by declaring both
`sun_dir_view` and `sun_tan_radius` as engine inputs; no shader filename selects
the behavior. The direction is in the active camera's view coordinates, the
same frame as the shader's view rays. `SunRenderState` comes from the
CelestialTime-driven `SunState` after the scene light is finalized through
BigSpace. The renderer projects that one semantic direction into camera space;
do not add a second ephemeris calculation for sky materials. Camera pose changes
refresh the direction while CelestialTime is paused. Continuous renderer data
stays in Rust; author the background and material binding in USD.

## High-profile near detail

Query `TerrainLodStatus` and inspect `max_depth`, `tile_budget`, and
`budget_refused` before changing the High LOD. A resident count close to the
budget does not by itself prove that detail was refused. High currently uses
depth 9 with a 256-tile budget and 65 vertices per tile (about 3 cm per
interval at the deepest level over a 1 km crop). The shared terrain kernel
evaluates bump gradients analytically and transfers unresolved slope variance
into roughness. Apply lunar
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

For CPU-built RGBA8 mip chains, preserve the role-aware filtering above while
using the GPU texture extent rule `max(1, floor(size / 2))` independently on
each axis. Never use ceil-halving or silently clamp an oversized mip count:
the level data would no longer match the texture's legal subresources.
