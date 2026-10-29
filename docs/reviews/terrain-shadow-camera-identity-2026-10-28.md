# Terrain morph and shadow alignment

Directional shadow cascades and the visible terrain must use the same camera
origin for CDLOD deformation. The native view-uniform owner resolves each
cascade's retained camera `MainEntity` through an index of extracted camera
positions. The index is cleared and reused once per render frame, so all
cascades share the lookup without allocations after capacity settles. Point
and spot light origins keep their native shared-light contract.

Authoritative owner: Bevy `bevy_render::view::prepare_view_uniforms`, pinned by
`Cargo.toml` and `Cargo.lock` to `fc11207b6f179574575c23efd8e43e7b03d86cd2`.
The commit is published in `LunCoSim/bevy` on
`fix/gpu-shader-buffer-stride`, so other machines can fetch the pinned source.

The consumer is `terrain_geomorph.wesl`, which already uses
`view.lod_view_world_position` in the forward and shadow passes. The engine
change adds neither a terrain shader path nor shadow passes, geometry, texture
resolution, or physics work. Camera identity and GPU uniform construction are
renderer invariants rather than replaceable Rhai policy. Scope is the render
world; the reusable index contains only the current frame's camera positions.

The production `vertex_shadow_alignment` fixture now exercises camera-dependent
deformation. Its directional-light receiver projects the expected caster shadow
using the current Sun direction. The shared point-light fixture uses the same
vertex shader with the documented zero LOD weight. All native depth, normal,
and motion prepasses remain enabled by authored Rhai.

The negative baseline has 3,290 red disagreement pixels and no green/cyan
success probes in the inspected 1280 by 720 window capture. With the corrected
source, `scripts/api/test_shadow_lod_origin.py` passes twelve changing-Sun GPU
phases: both positive probes appear and every phase contains zero red pixels.
The three authored prepass checks pass; GPU logs and runtime diagnostics are
clear; owned API port 4784 is released. Evidence is under
`target/shadow-lod-regression/`, including `baseline.png`,
`baseline-pixels.json`, and `fixed/validation.json`.

The visual tile bake also owns the parent-surface interpolation used during
CDLOD transitions. Its morph heights and normals use the barycentric weights
of the parent's actual indexed triangles. Tile-cache revision 14 prevents
reuse of meshes baked with different interpolation math. This is asynchronous
geometry preparation; the vertex format, draw count, physics surface and
per-frame shader work are unchanged. The pure curved-surface regression
`morphed_child_matches_every_parent_triangle` failed before the repair with
a one-metre height disagreement at an interior vertex. Logs are
`target/terrain-shadow-repair/morph-baseline.log` and `morph-fixed.log`.
All nine `tile_mesh::tests` pass with the corrected bake.

The Griffin production session passed `celestial_terrain_clock.rhai`: six
checks across more than forty celestial days at 100,000x, preserving fixed
physics time and bounded solar samples. The pinned production binary also
passed the `shared_shadow_allocation` GPU fixture. These checks cover clock
and renderer invariants; they do not turn a 4 m elevation grid and roughly
1 m imagery into measured sub-metre landforms. Concurrent simulations and
compilation make the captured frame timings unsuitable for a
performance-improvement claim.

Nobile coverage gaps are processed at the native asset boundary. Polar adapters
preserve unresolved elevations, and `lunco-assets-processing` reconstructs only
missing heights while keeping measured postings fixed. Orthophoto zeroes are
excluded before resampling so they cannot create a dark fringe in neighbouring
material colour. The runtime consumes the resulting DEM and albedo through the
same composed USD terrain and material bindings.

The current numerical, asset and production-view evidence is recorded in
[Nobile crater reconstruction](nobile-crater-reconstruction-2026-10-29.md).
[DEM georeferencing](../architecture/57-dem-georeferencing.md) owns the processing
contract. These are offline preparation changes; terrain mesh density, shader
work, authored Sun, navigation coordinates and simulation clocks are unchanged.
