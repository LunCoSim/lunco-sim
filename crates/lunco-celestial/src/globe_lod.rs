//! Camera-driven cube-sphere **live LOD** for celestial bodies (globe scale).
//!
//! Replaces the old fixed 24-tile shell (6 faces × 2×2 at level 1) with a
//! recursive quadtree subdivision per face: tiles refine near the camera and
//! coarsen far away, so a body shows planetary curvature from orbit and finer
//! relief as you approach. The selection is the globe crate's sphere-correct
//! `subdivide_face` (camera distance vs tile arc-size) — kept there as the pure
//! spine; this module is the scene integration (spawn/despawn + material), which
//! lives in `lunco-celestial` because that's what owns the bodies, textures, grids
//! and the blueprint `ShaderMaterial`.
//!
//! Per body, [`GlobeLod`] carries the params + the surface grid + material;
//! [`GlobeTiles`] tracks the resident tile set; [`update_globe_lod`] diffs the
//! desired set against it each frame. Tile placement replicates the proven static
//! pattern verbatim (mesh body-local, entity anchored at the tile centre via the
//! surface grid's `translation_to_grid`, `set_parent_in_place`) so correctness is
//! preserved — only *which* tiles exist becomes dynamic.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::camera::visibility::NoFrustumCulling;
use big_space::prelude::*;
use lunco_materials::ShaderMaterial;
use lunco_terrain_globe::quad_sphere::{cube_to_sphere, subdivide_face, tile_center_uv};
use lunco_terrain_globe::{create_quadsphere_tile_mesh, TerrainTile, TileCoord};

/// Per-body live-LOD context. Inserted on a celestial body entity in place of the
/// old fixed tile loop; [`update_globe_lod`] reads it to stream cube-sphere tiles.
#[derive(Component)]
pub struct GlobeLod {
    /// Body radius (m) — tile vertices ride this sphere.
    pub radius_m: f64,
    /// The surface grid the tiles anchor into (its own `CellCoord` per tile).
    pub surface_grid: Entity,
    /// Material applied to every tile (the body's blueprint `ShaderMaterial`).
    pub material: Handle<ShaderMaterial>,
    /// Vertices per tile side.
    pub res: u32,
    /// Deepest subdivision level near the camera.
    pub max_lod: u32,
    /// `refine when dist < tile_arc · factor` — larger = refine from farther.
    pub lod_distance_factor: f64,
}

/// The cube-sphere tiles currently resident for a body, keyed by quadtree node.
#[derive(Component, Default)]
pub struct GlobeTiles {
    /// Live tiles (in the desired LOD set).
    pub resident: HashMap<TileCoord, Entity>,
    /// Tiles that left the desired set, kept alive for a few frames while
    /// their replacements' meshes reach the GPU. Despawning old and spawning
    /// new in the SAME frame opened a one-frame hole per swap (a fresh
    /// `Mesh3d` renders only after render-world extraction/prepare) — with a
    /// moving camera the LOD churns continuously and the whole sphere
    /// flickered ("still blinking"). The brief overlap of coplanar identical
    /// surfaces is invisible; a hole is not.
    pub retiring: Vec<(Entity, u8)>,
}

/// Frames an outgoing tile stays alive after its replacement spawned.
const TILE_RETIRE_FRAMES: u8 = 3;

// TODO(globe-invisible): In luncosim's dev `cargo run`, the globe is NOT
// visible — the viewport renders black even though this system spawns the
// correct tile entities (verified via list_entities: f0-f5 L0 + L1 refinements
// for Earth & Moon, camera auto-focused Earth at 3x radius). The viewport CHROME
// is fixed (ViewportPanel + auto_tag → Camera3d active, black clear) and the
// 2x-radius tile placement bug is fixed (tiles now built centre-relative). But
// nothing renders. Prior notes say spacecraft glTFs were also invisible, so the
// remaining cause is likely GLOBAL, not tile-specific. Suspects to investigate:
//   - avatar camera clip planes: `update_avatar_clip_planes_system`
//     (lunco-avatar) only adapts near/far for cameras WITH AdaptiveNearPlane +
//     CellCoord + ChildOf(Grid). If the Observer Camera misses one, projection
//     stays default (far≈1000 m) → everything at orbital distance is clipped.
//   - blueprint.wgsl ShaderMaterial actually producing visible output for the
//     globe tiles (backface winding / cull mode / `transition` mode).
//   - big_space GlobalTransform propagation for tiles under the surface grid.
// NOTE: luncosim screenshots (MCP + HTTP CaptureScreenshot) render the viewport
// WHITE — they do not composite the Camera3d pass — so this must be verified in
// the real window, not via screenshot. See memory
// project_luncosim_viewport_and_globe_fix.
/// Per-frame: stream each body's cube-sphere tile set against the camera.
pub fn update_globe_lod(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    cameras: Query<(&Camera, &GlobalTransform, &bevy::camera::RenderTarget), With<Camera3d>>,
    transforms: Query<&GlobalTransform>,
    grids: Query<&Grid>,
    mut bodies: Query<(Entity, &GlobeLod, &mut GlobeTiles)>,
) {
    // ONLY the active window camera may steer the LOD. `iter().next()` picked
    // an arbitrary Camera3d — including offscreen preview cameras — and
    // archetype moves can flip iteration order between frames, alternating
    // the LOD focus point and thrashing the whole tile set every frame.
    let Some(cam) = cameras
        .iter()
        .filter(|(c, _, target)| {
            c.is_active && matches!(target, bevy::camera::RenderTarget::Window(_))
        })
        .map(|(_, gt, _)| gt)
        .next()
    else {
        return;
    };
    let cam_pos = cam.translation().as_dvec3();

    for (body_ent, lod, mut tiles) in &mut bodies {
        // Camera relative to the body centre (= the surface grid origin, inertial),
        // in the frame the tiles live in. f32 render-space is plenty for choosing
        // the LOD; tile PLACEMENT below stays f64-precise via `translation_to_grid`.
        let Ok(sg_gt) = transforms.get(lod.surface_grid) else { continue };
        let Ok(sg_grid) = grids.get(lod.surface_grid) else { continue };
        let camera_body_local = cam_pos - sg_gt.translation().as_dvec3();

        // Desired leaf set: recurse all six faces from the root. The resident
        // set feeds the split/merge dead band (no per-frame flapping when the
        // camera parks exactly on a threshold — e.g. the 3.0-radii focus snap).
        let resident: HashSet<TileCoord> = tiles.resident.keys().copied().collect();
        let mut desired: HashSet<TileCoord> = HashSet::new();
        for face in 0..6u8 {
            subdivide_face(
                &mut desired,
                &resident,
                body_ent,
                face,
                0,
                0,
                0,
                camera_body_local,
                lod.radius_m,
                lod.max_lod,
                lod.lod_distance_factor,
            );
        }

        // Move tiles no longer desired to the retirement queue (despawned a
        // few frames later, once their replacements are renderable).
        let mut newly_retired: Vec<(Entity, u8)> = Vec::new();
        tiles.resident.retain(|coord, ent| {
            let keep = desired.contains(coord);
            if !keep {
                newly_retired.push((*ent, TILE_RETIRE_FRAMES));
            }
            keep
        });
        tiles.retiring.extend(newly_retired);
        tiles.retiring.retain_mut(|(ent, frames)| {
            if *frames == 0 {
                commands.entity(*ent).despawn();
                false
            } else {
                *frames -= 1;
                true
            }
        });

        // Spawn newly-desired tiles — placement verbatim from the proven static
        // path: mesh in body-local (tile_center = ZERO), entity anchored at the
        // tile centre via the surface grid, reparented in place.
        for coord in &desired {
            if tiles.resident.contains_key(coord) {
                continue;
            }
            let (u, v) = tile_center_uv(coord.face, coord.level, coord.i, coord.j);
            let tile_center_dir = cube_to_sphere(coord.face, u, v);
            let tile_body_local = tile_center_dir * lod.radius_m;
            let (tile_cell, tile_local_pos) = sg_grid.translation_to_grid(tile_body_local);
            // Build the mesh RELATIVE to the tile centre (pass `tile_body_local`,
            // not `DVec3::ZERO`): the entity is placed at the tile centre via the
            // grid, so the mesh must carry only the small offset of each vertex
            // FROM that centre. Passing ZERO leaves vertices at full body-local
            // magnitude (~radius) which then *adds* to the entity's ~radius
            // placement → every tile rendered at ≈2× radius, a broken offset
            // shell (the long-standing "globe invisible" bug). Centre-relative
            // coords also keep vertex magnitudes small (≪ radius), avoiding f32
            // precision loss at 6.4e6 m.
            let mesh = create_quadsphere_tile_mesh(
                body_ent, coord.face, coord.level, coord.i, coord.j, lod.radius_m, lod.res, tile_body_local,
            );
            // Atomic (ChildOf, CellCoord, Transform) — the authored grid-local
            // pose IS the placement. `set_parent_in_place` here was the globe
            // corruption: it OVERWRITES the child Transform from its current
            // GlobalTransform, which at spawn is `default()` (never propagated),
            // so every tile's placement was replaced with
            // `identity.reparented_to(surface_grid_global)` — zero at startup
            // (all tiles collapsed to the body centre = the long-standing
            // "globe invisible" TODO above) and camera-distance garbage once
            // the view moves (exploded tile shards from orbit).
            let ent = commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(lod.material.clone()),
                    *coord,
                    TerrainTile,
                    tile_cell,
                    Transform::from_translation(tile_local_pos),
                    GlobalTransform::default(),
                    Visibility::Visible,
                    InheritedVisibility::default(),
                    NoFrustumCulling,
                    // The globe is a FEATURELESS sphere of planetary size; as a
                    // shadow caster it contributes nothing (its night side is
                    // dark by shading) but at grazing sun elevations (+2.6° at
                    // Malapert) a site merged onto the sphere sits exactly in
                    // the shadow map's terminator/acne zone — the whole scene
                    // flipped lit↔dark frame to frame ("still blinking"). Same
                    // treatment as the Sun body mesh.
                    bevy::light::NotShadowCaster,
                    Name::new(format!("Globe tile f{} L{} {},{}", coord.face, coord.level, coord.i, coord.j)),
                    ChildOf(lod.surface_grid),
                ))
                .id();
            tiles.resident.insert(*coord, ent);
        }
    }
}
