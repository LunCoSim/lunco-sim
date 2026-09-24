# Terrain Editor preview: engineering handoff

Status: handoff for the terrain/runtime owner. This document records the
failure modes observed while integrating the USD terrain projection with the
isolated Editor preview, and the architecture required to fix them. It is
generic: it applies to every DEM-backed scene, not to one vehicle or mission.

The mounted-scene boundary now treats `UsdPreviewOnly` terrain as examined but
does not create a `DemTerrainRequest`. This prevents an isolated document preview
from adding collider work or holding mission physics. The preview still needs a
separate render-only terrain realization and a valid preview-grid camera demand
before it can show streamed relief.

## What is currently wrong

The isolated USD Editor preview can show the authored terrain prim, `ShaderLook`
metadata, and shader source identity, while showing no relief or only the
authored flat proxy. The runtime View may show streamed relief at the same
time. This is not a shader-selection problem: the preview has no streamed
terrain geometry to shade.

The observed symptoms are:

- A preview opened from a DEM scene has terrain metadata but no CDLOD tiles.
- The runtime View contains relief because its active world camera drives the
  terrain streamer; the isolated preview camera does not.
- A preview camera is unparented, so it has no shared `Grid` frame with the
  preview terrain. Big-space pose resolution therefore cannot produce a valid
  camera-to-terrain demand.
- `UsdPreviewOnly` is a scene-ownership marker, not a spatial or terrain role.
  Adding it alone does not create a grid, a camera demand, or a render-only
  terrain product.
- `lodViz = true` suppresses the static visual mesh. If CDLOD demand is empty,
  that correctly leaves no visual surface; installing a second flat mesh would
  hide the missing demand and create two geometry authorities.
- The terrain pipeline currently creates physics products from a DEM request:
  static colliders, collider rings, physics holds, physics-frame poses, and
  analytic terrain query candidates. A preview realization must not enter any
  of those paths or it can pause or alter the mission simulation.
- The preview has its own render layer, but generated terrain tiles need an
  explicit propagation rule so they cannot leak into the main window or a
  different preview.
- Editor lighting can make the result unreadable (overexposed white surfaces
  or black shadow cores). Lighting is a presentation concern; it must not be
  “fixed” by changing terrain data or installing a fallback shader.
- Reopening/recomposing a document must not leave old DEM tiles, jobs, or
  physics state attached to the replacement preview. The preview lease is the
  lifecycle boundary.

## Evidence in the current implementation

The following code paths explain the split between the runtime View and the
isolated preview:

| Evidence | Consequence |
| --- | --- |
| `crates/lunco-usd-viewport-runtime/src/lib.rs`, `create_preview_session` | The preview root carries `Transform`, `Visibility`, `UsdPreviewOnly`, and a render layer, but no `Grid`/`CellCoord`. |
| `crates/lunco-usd-viewport-runtime/src/lib.rs`, `create_preview_view` | The preview camera is spawned as a standalone entity. It is not a child of the preview spatial frame. |
| `crates/lunco-terrain-surface/src/stream_viz.rs`, `collect_terrain_detail_demands` | Demand requires an active perspective camera, a valid viewport/FOV, and a resolvable big-space world pose. Invalid or disconnected hierarchy yields no demand. |
| `crates/lunco-terrain-surface/src/stream_viz.rs`, `update_lod_tiles` | Empty visual demand returns before selecting or baking any tile. Terrain also needs an ancestor grid and a grid-relative pose. |
| `crates/lunco-usd-terrain/src/lib.rs`, `bridge_dem_prim_read` | USD correctly projects `demSource`, layer stack, georeference, shader intent, and streaming flags into the terrain request; it does not create a preview spatial frame. |
| `crates/lunco-terrain-surface/src/terrain.rs`, `assemble_dem_build` | A normal DEM build creates an oracle and may create static physics or a collider ring. The build function has no explicit render-only role. |
| `crates/lunco-terrain-surface/src/collider_ring.rs`, `hold_physics_until_dem_ready` | Physics readiness is global to the active simulation and observes DEM requests/rings. A preview request must be excluded by type, not by a name convention. |
| `crates/lunco-terrain-surface/src/surface_query.rs` and `src/query.rs` | Physics-frame terrain poses and `TerrainHeight`/field/raycast providers enumerate DEM oracles unless a render-only role is filtered out. |
| `crates/lunco-terrain-surface/src/terrain.rs`, `assemble_dem_build` and `stream_viz.rs`, `spawn_tile` | Streamed tiles are anchored to a grid and carry the authored `ShaderLook`; a flat fallback would be a second, divergent surface authority. |
| `docs/architecture/terrain-substrate.md` and `terrain-layered-rendering.md` | Terrain geometry is a deterministic projection of the DEM/oracle and layer stack; geometry is not authored as a giant USD mesh. |

The standard production shader remains the correct source for DEM appearance:
`lunco://shaders/terrain_layered.wgsl` with
`lunco://shaders/terrain_geomorph.wgsl`. The missing part is render geometry and
spatial demand, not an alternate material.

## Required architecture

### 1. Give the preview a real spatial frame

Create a generic, render-only preview frame owned by the viewport session:

1. Spawn a private `Grid` with the same validated cell-edge policy used by the
   world shell, plus a `CellCoord`/`Transform` origin contract.
2. Parent the preview USD root and its preview camera to that frame. The camera
   must be active only while its Editor panel is visible, but its hierarchy
   must exist before demand collection.
3. Keep the preview frame independent of `ActivePhysicsFrame` and the mission
   `WorldGrid`. It is a render coordinate context, not a physics world.
4. Propagate the preview render layer to every generated USD prim and terrain
   tile. Existing explicit render-layer components remain authoritative.

This is a generic viewport/spatial capability; it must not make the viewport
runtime depend on a particular Twin or terrain asset.

### 2. Add an explicit terrain realization role

The terrain domain needs a typed `PresentationOnly`/render-realization role.
It should be attached atomically with a preview DEM request by the USD→terrain
projector when the prim belongs to a `UsdPreviewOnly` hierarchy.

The role must be consumed by the terrain owner as follows:

- **Keep:** DEM decode, content-addressed grid cache, oracle, layer stack,
  georeference, derived visual maps, `TerrainLodViz`, and `ShaderLook`.
- **Exclude:** `RigidBody`, `Collider`, `TerrainColliderRing`, collider tile
  jobs, `PhysicsHolds::TERRAIN_READY`, `TerrainPoseInPhysicsFrame`, physics
  support validation, and analytic terrain query providers.
- **Never infer by name:** the role is a reflected ECS fact and is covered by
  the same scene-ownership lifecycle as the preview root.
- **Retire explicitly:** closing/recomposing a preview cancels its bake jobs,
  removes its LOD tiles and derived maps, and releases only that preview's
  cache/lease state.

The normal mission terrain remains unchanged and continues to own deterministic
physics products. The two realizations share the same source bytes and oracle
key, but they have separate consumers and lifecycles.

### 3. Feed the real Editor view into LOD selection

The preview streamer must consume a typed demand from the actual visible
off-screen Editor camera. The demand includes camera pose in the preview grid,
viewport height, FOV, near-detail radius, hysteresis, and preview render-layer
identity. It must not use the main window camera as an implicit substitute.

Selection and bake order must remain deterministic: sort demand and tile keys
with a total order, use the same lockstep policy as runtime recording, and key
mesh/cache products by DEM content, layer/oracle key, quadtree coordinate, and
quality profile.

### 4. Keep one USD→terrain source path

The Editor preview and runtime View must both use the same composed USD read:

`UsdShade`/shader identity → `DemTerrainRequest` → DEM resolver → layer stack →
oracle → visual LOD tiles.

Do not add a preview-only parser, a second DEM path, a hardcoded file ID, or a
flat mesh fallback. If a source or shader is missing, publish a structured
error in the preview status and leave the surface absent.

### 5. Make lighting a scoped presentation rig

Each preview view owns its key/fill lights and render layer. The rig must be
created and destroyed with the view, and its intensity/exposure must be
bounded by the validated presentation profile. Do not mutate physical sun
settings or terrain albedo to compensate for a preview lighting failure.

## Required Rhai-authored gates

Behavioral and asset-backed checks belong beside the authored scene, not in
Rust unit tests. Add a generic preview gate that loads the scene through the
production Twin/runtime host and reports typed evidence for:

1. The composed terrain prim has the expected DEM source, shader fragment, and
   CDLOD vertex source.
2. A visible preview has a valid camera demand and a shared preview-grid frame.
3. At least one resident LOD tile exists for the demanded area and carries the
   preview render layer.
4. The preview terrain has no rigid body, collider, collider ring, physics hold,
   physics-frame pose, or terrain-query contribution.
5. Runtime terrain in the same process still has its normal physics products;
   opening/closing the preview does not change them.
6. Recomposition/close removes preview jobs and tiles without touching another
   view or the mission terrain.
7. Missing DEM bytes and invalid shader identity produce a loud structured
   preview error, not a flat/procedural fallback.
8. Repeated runs under lockstep select the same tile coordinates and oracle/cache
   keys.

The gate should call the existing generic API/query and scene-test helpers.
Only add a Rust test for a pure lower-level invariant that Rhai cannot observe;
do not embed a repository asset path in such a test.

## Handoff checklist

- [ ] Implement the preview spatial-frame owner in the current viewport runtime.
- [ ] Add the typed render-only terrain role and filter every physics/query
      consumer at its authoritative owner.
- [ ] Attach the role atomically in `lunco-usd-terrain` using the preview
      hierarchy marker.
- [ ] Connect the visible Editor camera to terrain demand and propagate its
      render layer to generated tiles.
- [ ] Add the Rhai preview gate and negative fixtures.
- [ ] Update the terrain and viewport architecture docs plus the relevant
      skills in the same change.
- [ ] Validate with the production binary, one process per check, and inspect
      the live Editor projection; do not edit USD text directly.
