//! S3 (visual-only): camera-driven CDLOD **tile streaming for seeing LODs**.
//!
//! This is the first live consumer of the dormant quadtree/tile spine. It is an
//! opt-in DEBUG visualisation: when a DEM terrain is built with `lod_viz`, the
//! static visual mesh is suppressed (the heightfield COLLIDER still spawns, so
//! physics is unchanged) and instead a set of LOD tiles is streamed every frame:
//!
//! 1. read the camera position in the terrain's local XZ frame → `focus`,
//! 2. [`Quadtree::select`] the node set for that focus (fine under the camera,
//!    coarse far away),
//! 3. diff against the currently-spawned tiles ([`LodTiles`]): bake + spawn the
//!    new nodes ([`bake_tile_mesh`], real DEM-sampled geometry), despawn the gone,
//! 4. tint each tile by its quadtree **depth** so the LOD structure is visible and
//!    you watch tiles refine as you move.
//!
//! Geomorph (the `MORPH_TARGET` lerp) is deliberately NOT wired here — a flat
//! per-depth colour + real per-tile geometry is the clearest first view of the
//! LOD machinery. The morph shader + a native-res collider ring are the next step.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::Grid;
use lunco_core::WorldGrid;
use lunco_obstacle_field::field::HeightGrid;
use lunco_obstacle_field::grid_mesh;

use crate::quadtree::{QuadCoord, Quadtree};
use crate::tile_mesh::bake_tile_mesh;

/// Vertices per tile side (so each tile is `TILE_RES²` verts). 33 → 32² quads.
const TILE_RES: usize = 33;
/// Deepest LOD the viz refines to. Bounds the tile count near the camera.
const MAX_DEPTH: u8 = 6;
/// `refine_range(d) = RANGE_FACTOR · geometric_error(d)`. Larger → refine from
/// farther (more fine tiles on screen).
const RANGE_FACTOR: f64 = 3.0;

/// The DEM grid retained on a terrain entity so LOD tiles can sample heights.
/// `Arc` so a future off-thread bake can share it without a copy.
#[derive(Component)]
pub struct DemHeightField(pub Arc<HeightGrid>);

/// Marker + params: this terrain streams visual LOD tiles. Inserted by the build
/// when the request set `lod_viz`. Physics stays on the static heightfield collider.
#[derive(Component)]
pub struct TerrainLodViz {
    pub max_depth: u8,
    pub tile_res: usize,
}

impl Default for TerrainLodViz {
    fn default() -> Self {
        TerrainLodViz { max_depth: MAX_DEPTH, tile_res: TILE_RES }
    }
}

/// The LOD tile entities currently spawned for a terrain, keyed by quadtree node.
#[derive(Component, Default)]
pub struct LodTiles(pub HashMap<QuadCoord, Entity>);

/// Back-pointer from a spawned LOD tile to its owning terrain. Tiles are parented
/// to the big_space **grid** (so each can carry its own `CellCoord`), not to the
/// terrain entity — so this tag lets [`despawn_orphaned_lod_tiles`] reap them when
/// the terrain is gone (e.g. on twin reload) instead of leaking under the grid.
#[derive(Component)]
pub struct LodTileOf(pub Entity);

/// Cached one-material-per-depth so tile churn at LOD boundaries doesn't allocate
/// a new `StandardMaterial` every spawn.
#[derive(Resource, Default)]
pub struct LodMaterials(HashMap<u32, Handle<StandardMaterial>>);

/// Flat, distinct colour per LOD depth (mirrors `terrain_debug.wgsl`'s palette
/// intent): coarse→fine sweeps blue→cyan→green→yellow→orange→red→magenta.
fn lod_color(depth: u32) -> Color {
    const P: [(f32, f32, f32); 7] = [
        (0.20, 0.35, 0.85), // 0 coarse — blue
        (0.20, 0.75, 0.85), // 1 — cyan
        (0.25, 0.80, 0.35), // 2 — green
        (0.85, 0.85, 0.25), // 3 — yellow
        (0.90, 0.55, 0.20), // 4 — orange
        (0.85, 0.25, 0.25), // 5 — red
        (0.80, 0.30, 0.80), // 6+ fine — magenta
    ];
    let (r, g, b) = P[(depth as usize).min(P.len() - 1)];
    Color::srgb(r, g, b)
}

/// Per-frame: stream the LOD tile set for each `lod_viz` terrain against the camera.
pub fn update_lod_tiles(
    mut commands: Commands,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    // The big_space world grid each tile anchors into (its own `CellCoord`).
    grids: Query<(Entity, &Grid), With<WorldGrid>>,
    mut terrains: Query<(
        Entity,
        &GlobalTransform,
        &DemHeightField,
        &TerrainLodViz,
        &mut LodTiles,
    )>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut lod_mats: ResMut<LodMaterials>,
) {
    // Use the first 3D camera as the focus. (Multiple cameras → the viz follows
    // whichever; fine for a debug view.)
    let Some(cam) = cameras.iter().next() else { return };
    let cam_pos = cam.translation();
    // No world grid yet → can't anchor tiles; skip this frame.
    let Ok((grid_entity, grid)) = grids.single() else { return };

    for (terrain, t_gt, hf, viz, mut tiles) in &mut terrains {
        // Camera in the terrain's local frame (the DEM frame, origin-centred).
        let local = t_gt.affine().inverse().transform_point3(cam_pos);
        let focus = [local.x as f64, local.z as f64];

        let dem = &hf.0;
        let h = dem.half_extent as f64;
        // Eye height = camera height ABOVE the terrain surface directly below it.
        // Feeding this to the 3D metric means looking down from altitude coarsens
        // the ground below (true distance), instead of refining it as XZ-only would.
        let ground = dem.height_at(local.x, local.z) as f64;
        let eye_height = (local.y as f64 - ground).max(0.0);
        let qt = Quadtree::new(h, viz.max_depth, RANGE_FACTOR, h);
        let sel = qt.select_3d(focus, eye_height);
        let wanted: HashSet<QuadCoord> = sel.iter().map(|s| s.coord).collect();

        // Despawn tiles no longer selected.
        tiles.0.retain(|coord, ent| {
            let keep = wanted.contains(coord);
            if !keep {
                commands.entity(*ent).despawn();
            }
            keep
        });

        // Spawn newly-selected tiles (real DEM-sampled geometry, tinted by depth).
        // Each tile anchors to its OWN big_space `CellCoord` (computed from its
        // world centre) with vertices baked relative to that centre — so a tile far
        // from the origin keeps f32 precision instead of riding one shared cell.
        // The DEM sits at the grid origin (terrain anchored at `CellCoord::default`),
        // so the tile's DEM-local centre equals its absolute grid position.
        for s in &sel {
            if tiles.0.contains_key(&s.coord) {
                continue;
            }
            let center = s.region.center;
            let tm = bake_tile_mesh(dem, s.region, viz.tile_res, h, center);
            let mesh = meshes.add(grid_mesh(tm.positions, tm.normals, tm.uvs, tm.indices));
            let (cell, local) = grid.translation_to_grid(DVec3::new(center[0], 0.0, center[1]));
            let depth = s.coord.depth as u32;
            let mat = lod_mats
                .0
                .entry(depth)
                .or_insert_with(|| {
                    materials.add(StandardMaterial {
                        base_color: lod_color(depth),
                        perceptual_roughness: 1.0,
                        ..default()
                    })
                })
                .clone();
            let ent = commands
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(mat),
                    cell,
                    Transform::from_translation(local),
                    Visibility::Inherited,
                    LodTileOf(terrain),
                    Name::new(format!("LodTile d{} {},{}", s.coord.depth, s.coord.x, s.coord.z)),
                    ChildOf(grid_entity),
                ))
                .id();
            tiles.0.insert(s.coord, ent);
        }
    }
}

/// Reap LOD tiles whose owning terrain no longer exists (or no longer streams) —
/// e.g. after a twin reload. Tiles are children of the big_space grid (so each can
/// carry its own `CellCoord`), so they don't die with the terrain entity; this is
/// their lifecycle tether.
pub fn despawn_orphaned_lod_tiles(
    mut commands: Commands,
    tiles: Query<(Entity, &LodTileOf)>,
    streaming: Query<(), With<TerrainLodViz>>,
) {
    for (ent, owner) in &tiles {
        if streaming.get(owner.0).is_err() {
            commands.entity(ent).despawn();
        }
    }
}
