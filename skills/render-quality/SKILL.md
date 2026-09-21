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
   Treat packed surface-map AO as indirect-light visibility: it belongs in
   Bevy's PBR diffuse-occlusion input, not in authored albedo or direct-sun
   multiplication. If broad terrain colour patches match a low-frequency AO
   map, inspect that channel routing before touching assets or exposure.
4. Compare a settled frame sequence, not one screenshot. Fast color changes
   usually indicate changing inputs, repeated derived bakes, missing asset
   readiness, or unstable lighting—not a reason to clamp the image in a
   shader. Find the owner and gate the work by asset/revision events.

## High-profile near detail

Query `TerrainLodStatus` and inspect `max_depth`, `tile_budget`, and
`budget_refused` before changing the High LOD. A resident count close to the
budget does not by itself prove that detail was refused. High currently uses
depth 9 with a 1024-tile budget and 49 vertices per tile (about 4 cm per
interval at the deepest level over a 1 km crop). The shared terrain kernel evaluates bump gradients
analytically and transfers unresolved slope variance into roughness. Apply lunar
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
