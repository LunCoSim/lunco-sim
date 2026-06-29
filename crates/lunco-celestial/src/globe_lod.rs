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

use bevy::math::DVec3;
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
pub struct GlobeTiles(pub HashMap<TileCoord, Entity>);

/// Per-frame: stream each body's cube-sphere tile set against the camera.
pub fn update_globe_lod(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    transforms: Query<&GlobalTransform>,
    grids: Query<&Grid>,
    mut bodies: Query<(Entity, &GlobeLod, &mut GlobeTiles)>,
) {
    let Some(cam) = cameras.iter().next() else { return };
    let cam_pos = cam.translation().as_dvec3();

    for (body_ent, lod, mut tiles) in &mut bodies {
        // Camera relative to the body centre (= the surface grid origin, inertial),
        // in the frame the tiles live in. f32 render-space is plenty for choosing
        // the LOD; tile PLACEMENT below stays f64-precise via `translation_to_grid`.
        let Ok(sg_gt) = transforms.get(lod.surface_grid) else { continue };
        let Ok(sg_grid) = grids.get(lod.surface_grid) else { continue };
        let camera_body_local = cam_pos - sg_gt.translation().as_dvec3();

        // Desired leaf set: recurse all six faces from the root.
        let mut desired: HashSet<TileCoord> = HashSet::new();
        for face in 0..6u8 {
            subdivide_face(
                &mut desired,
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

        // Despawn tiles no longer desired.
        tiles.0.retain(|coord, ent| {
            let keep = desired.contains(coord);
            if !keep {
                commands.entity(*ent).despawn();
            }
            keep
        });

        // Spawn newly-desired tiles — placement verbatim from the proven static
        // path: mesh in body-local (tile_center = ZERO), entity anchored at the
        // tile centre via the surface grid, reparented in place.
        for coord in &desired {
            if tiles.0.contains_key(coord) {
                continue;
            }
            let (u, v) = tile_center_uv(coord.face, coord.level, coord.i, coord.j);
            let tile_center_dir = cube_to_sphere(coord.face, u, v);
            let tile_body_local = tile_center_dir * lod.radius_m;
            let (tile_cell, tile_local_pos) = sg_grid.translation_to_grid(tile_body_local);
            let mesh = create_quadsphere_tile_mesh(
                body_ent, coord.face, coord.level, coord.i, coord.j, lod.radius_m, lod.res, DVec3::ZERO,
            );
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
                    Name::new(format!("Globe tile f{} L{} {},{}", coord.face, coord.level, coord.i, coord.j)),
                ))
                .set_parent_in_place(lod.surface_grid)
                .id();
            tiles.0.insert(*coord, ent);
        }
    }
}
