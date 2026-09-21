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
   Treat packed surface-map AO as indirect-light visibility: it belongs in
   Bevy's PBR diffuse-occlusion input, not in authored albedo or direct-sun
   multiplication. If broad terrain colour patches match a low-frequency AO
   map, inspect that channel routing before touching assets or exposure.
4. Compare a settled frame sequence, not one screenshot. Fast color changes
   usually indicate changing inputs, repeated derived bakes, missing asset
   readiness, or unstable lighting—not a reason to clamp the image in a
   shader. Find the owner and gate the work by asset/revision events.

For celestial globes, verify the installed Earth/Moon imagery dataset and the
composed USD body look. A body shader look may add shader parameters, but it
must retain the installed dataset albedo unless USD supplies an explicit
albedo layer or `AuthoredBodyAlbedo`. The accelerated `CelestialTime` clock
owns globe and sky presentation; physical station, terrain, and link state
remain on `WorldTime`. In surface view, align the render-only presentation
hierarchy at the WorldTime camera while keeping relative body poses at
CelestialTime, so the local sky stays correct as the clocks diverge. The sky
clock readout must display the same celestial epoch that moves the
presentation. A visible surface marker may be copied under the
matching presentation body-fixed grid, but that copy is render-only. Show the
causal marker on its own body's surface and the copy in orbit or other-body
views so the fast globe has no stationary duplicate.
Procedural sky materials opt into the live Sun disc by declaring both
`sun_dir_view` and `sun_tan_radius` as engine inputs; no shader filename selects
the behavior. The direction is in the active camera's view coordinates, the
same frame as the shader's view rays. Its provider composes the camera's
`CellCoord` and `Transform` through `lunco_spatial::pose_in_grid`, while
BigSpace owns `GlobalTransform` propagation. Camera pose changes refresh the
direction even when CelestialTime is paused. Continuous renderer data stays in
Rust; author the background and material binding in USD.

Use `inspect-simulation`, `record-video`, or the API screenshot surface for
visual evidence. Preserve authored shadow maps, terrain resolution, BigSpace,
and physics settings. If an asset is missing, report the owning cache or
manifest error visibly instead of adding a procedural fallback.

For CPU-built RGBA8 mip chains, preserve the role-aware filtering above while
using the GPU texture extent rule `max(1, floor(size / 2))` independently on
each axis. Never use ceil-halving or silently clamp an oversized mip count:
the level data would no longer match the texture's legal subresources.
