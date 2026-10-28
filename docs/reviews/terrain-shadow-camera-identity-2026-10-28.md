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

Unlit albedo and geometric-normal captures isolated another defect in the
northern crater: missing orthophoto coverage coincides with irregular filled
DEM slopes. The native `lunco-assets-processing::process::fill_dem_voids` owner
now solves bounded gaps harmonically with measured postings fixed. The polar
adapter in lunar-base-model leaves unresolved coverage for that owner. There
is no second fill in the adapter. The native negative slope regression failed
before the correction; all four void-processing tests pass, including the
oversized-gap rejection. Dataset pipeline revision 9 invalidates prior stamps.

The production asset CLI regenerated Nobile's 513 by 513 raster. All 261,442
measured nodes and all 2,601 nodes in the central 200 m square remain bitwise
identical. Only 1,727 missing nodes change, by at most 6.8241043 m. These remain
interpolated estimates, not additional lunar measurements. The verified TIFF
is installed in both the Twin's authored and cached terrain directories; its
4 m node spacing, 2048 m extent and vertical datum are preserved. Evidence:
`target/terrain-shadow-repair/void-data/bake-validation.json` and
`void-process.log`. This is offline asset preparation with no added rendering
cost. The Twin's workflow and provenance describe reproduction and coverage.

The rebuilt app rendered the repaired crater at three matched distances with
no GPU or runtime errors. The central artificial ridge and its black shadow
strip are absent in `complete/crater-0.png` through `crater-2.png`, compared
with `final/crater-0.png` through `crater-2.png` before asset regeneration.
The missing image coverage still has neutral albedo; the repair does not
invent photographic detail there.

Final acceptance used the rebuilt production binary after asset installation:
`celestial_terrain_clock.rhai` passed all six checks again at 100,000x; runtime
readiness is true and diagnostics contain zero errors or warnings. The owned
window remains open on API port 4783 with celestial speed restored to 1x.
Its PID and evidence are recorded under `target/terrain-shadow-repair/complete/`.
The separate `verify_griffin_bundle.py` check passes for cached sources,
georeferencing, DEM and albedo. Formatting and diff whitespace checks pass;
the 43-skill catalogue validates. Unrelated concurrent worktree edits are not
part of this change or its validation claim.
