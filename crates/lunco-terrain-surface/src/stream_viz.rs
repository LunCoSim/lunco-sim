//! S3: camera-driven CDLOD **visual tile streaming** — the production surface.
//!
//! When a DEM terrain streams (default; opt out per build), the single static
//! visual mesh is suppressed (the heightfield COLLIDER still spawns, so physics
//! is unchanged) and a set of LOD tiles is streamed every frame:
//!
//! 1. read the camera position in the terrain's local XZ frame → `focus`,
//! 2. [`Quadtree::select_3d`] the node set for that focus + eye height (fine under
//!    the camera, coarse far away),
//! 3. diff against the currently-spawned tiles ([`LodTiles`]): bake + spawn the
//!    new nodes ([`bake_tile_mesh`], real DEM-sampled geometry), despawn the gone,
//! 4. draw each tile with the `terrain_geomorph` shader: a CDLOD **vertex morph**
//!    (`POSITION → MORPH_TARGET` by camera distance, so no LOD pop) + the
//!    procedural **regolith** fragment (FBM bump + PBR sun + CSM shadows).
//!
//! Materials are cached per LOD depth — they share the regolith look and differ
//! only in the per-depth morph band. The companion canonical-res collider ring is
//! [`crate::collider_ring`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::Grid;
use lunco_core::WorldGrid;
use lunco_obstacle_field::field::HeightGrid;
use lunco_obstacle_field::grid_mesh;
use lunco_materials::{ParamValue, ShaderMaterial, ATTRIBUTE_MORPH_TARGET};

use crate::quadtree::{QuadCoord, Quadtree};
use crate::tile_mesh::bake_tile_mesh;

/// Vertices per tile side (so each tile is `TILE_RES²` verts). 33 → 32² quads.
const TILE_RES: usize = 33;
/// Deepest LOD the viz refines to. Bounds the tile count near the camera.
const MAX_DEPTH: u8 = 6;
/// `refine_range(d) = RANGE_FACTOR · geometric_error(d)`. Larger → refine from
/// farther (more fine tiles on screen).
const RANGE_FACTOR: f64 = 3.0;
/// Max tiles BAKED per frame (across all terrains). A big scale/zoom change can
/// select hundreds of new tiles at once; baking them all in one frame on the main
/// thread is the "stuck on scale change" hitch. Capping it amortises the work over
/// frames — the coarser parent stays visible until each tile refines in. Despawns
/// are unbounded (cheap). One tile/frame is the smoothest (no per-frame spike); at
/// 140+ FPS a full ring still fills in well under a second.
const MAX_TILE_BAKES_PER_FRAME: usize = 1;

/// The DEM grid retained on a terrain entity so LOD tiles can sample heights.
/// `Arc` so a future off-thread bake can share it without a copy.
#[derive(Component)]
pub struct DemHeightField(pub Arc<HeightGrid>);

/// Which shader the streamed LOD tiles draw with — switchable live in the
/// Inspector (per terrain). Default [`Lit`](TerrainShaderMode::Lit).
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[reflect(Component)]
pub enum TerrainShaderMode {
    /// Procedural regolith (FBM bump + lunar PBR) — the production look.
    #[default]
    Lit,
    /// Flat per-LOD-depth colour (blue→red) to SEE the quadtree refine.
    DebugLod,
    /// Flat lunar-grey, no FBM — the lightweight "no fancy shader" look.
    Plain,
}

impl TerrainShaderMode {
    /// The `.wgsl` this mode draws with (all carry the CDLOD vertex morph).
    fn shader_path(self) -> &'static str {
        match self {
            TerrainShaderMode::Lit => "shaders/terrain_geomorph.wgsl",
            TerrainShaderMode::DebugLod | TerrainShaderMode::Plain => {
                "shaders/terrain_geomorph_flat.wgsl"
            }
        }
    }
}

/// Flat per-LOD-depth colour for [`TerrainShaderMode::DebugLod`]: coarse→fine
/// sweeps blue→cyan→green→yellow→orange→red→magenta.
fn lod_rgb(depth: u32) -> [f32; 3] {
    const P: [[f32; 3]; 7] = [
        [0.20, 0.35, 0.85],
        [0.20, 0.75, 0.85],
        [0.25, 0.80, 0.35],
        [0.85, 0.85, 0.25],
        [0.90, 0.55, 0.20],
        [0.85, 0.25, 0.25],
        [0.80, 0.30, 0.80],
    ];
    P[(depth as usize).min(P.len() - 1)]
}

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
/// The second field is the shader mode the live tiles were built with — when it
/// changes (inspector edit) the tiles are despawned and rebuilt with the new shader.
#[derive(Component, Default)]
pub struct LodTiles(pub HashMap<QuadCoord, Entity>, TerrainShaderMode);

/// Back-pointer from a spawned LOD tile to its owning terrain. Tiles are parented
/// to the big_space **grid** (so each can carry its own `CellCoord`), not to the
/// terrain entity — so this tag lets [`despawn_orphaned_lod_tiles`] reap them when
/// the terrain is gone (e.g. on twin reload) instead of leaking under the grid.
#[derive(Component)]
pub struct LodTileOf(pub Entity);

/// Cached one geomorph material per LOD depth so tile churn at LOD boundaries
/// doesn't allocate a new `ShaderMaterial` every spawn. Each carries that depth's
/// morph band + colour; the `terrain_geomorph` shader morphs vertices on the GPU.
#[derive(Resource, Default)]
pub struct LodMaterials(HashMap<(TerrainShaderMode, u32), Handle<ShaderMaterial>>);

/// Build a tile `ShaderMaterial` for a `(mode, depth)` with its morph band. The
/// geomorph vertex stage is shared; the fragment + colour come from the mode.
fn build_tile_material(
    mode: TerrainShaderMode,
    morph_start: f32,
    morph_end: f32,
    depth: u32,
    shader: &Handle<Shader>,
    materials: &mut Assets<ShaderMaterial>,
) -> Handle<ShaderMaterial> {
    let mut m = ShaderMaterial::default();
    m.shader = shader.clone();
    m.vertex_shader = Some(shader.clone());
    m.set_many([
        ("morph_start", ParamValue::F32(morph_start)),
        ("morph_end", ParamValue::F32(morph_end)),
    ]);
    match mode {
        TerrainShaderMode::DebugLod => m.set("base_color", ParamValue::Vec3(lod_rgb(depth))),
        TerrainShaderMode::Plain => m.set("base_color", ParamValue::Vec3([0.35, 0.34, 0.32])),
        TerrainShaderMode::Lit => {}
    }
    materials.add(m)
}

/// Read a scalar param off a material (for carrying a tile's morph band across a
/// shader-mode swap).
fn mat_f32(m: &ShaderMaterial, name: &str) -> Option<f32> {
    match m.get(name)? {
        ParamValue::F32(v) => Some(v),
        _ => None,
    }
}

/// Per-frame: stream the LOD tile set for each streaming terrain against the camera.
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
        Option<&TerrainShaderMode>,
    )>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    mut lod_mats: ResMut<LodMaterials>,
    asset_server: Res<AssetServer>,
) {
    // Use the first 3D camera as the focus. (Multiple cameras → the viz follows
    // whichever; fine for a debug view.)
    let Some(cam) = cameras.iter().next() else { return };
    let cam_pos = cam.translation();
    // No world grid yet → can't anchor tiles; skip this frame.
    let Ok((grid_entity, grid)) = grids.single() else { return };

    // Per-frame bake budget shared across all terrains (amortise scale changes).
    let mut bake_budget = MAX_TILE_BAKES_PER_FRAME;

    for (terrain, t_gt, hf, viz, mut tiles, mode_opt) in &mut terrains {
        let mode = mode_opt.copied().unwrap_or_default();
        // The mode's shader drives both the @vertex (CDLOD morph) and @fragment
        // stages; load-by-path → cached handle, hot-reloads on edit.
        let shader = asset_server.load(mode.shader_path());
        // Shader mode changed (inspector edit) → SWAP the material on every live
        // tile in place (same geometry, new shader/colour) instead of despawning +
        // rebuilding, which left a one-frame black hole until the tiles re-baked.
        if tiles.1 != mode {
            let old_mode = tiles.1;
            let swaps: Vec<(Entity, u32)> =
                tiles.0.iter().map(|(c, &e)| (e, c.depth as u32)).collect();
            for (ent, depth) in swaps {
                let handle = if let Some(h) = lod_mats.0.get(&(mode, depth)) {
                    h.clone()
                } else {
                    // Carry this depth's morph band over from the old material.
                    let (ms, me) = lod_mats
                        .0
                        .get(&(old_mode, depth))
                        .and_then(|oh| materials.get(oh))
                        .map(|m| {
                            (
                                mat_f32(m, "morph_start").unwrap_or(1.0e20),
                                mat_f32(m, "morph_end").unwrap_or(1.0e21),
                            )
                        })
                        .unwrap_or((1.0e20, 1.0e21));
                    let h = build_tile_material(mode, ms, me, depth, &shader, &mut materials);
                    lod_mats.0.insert((mode, depth), h.clone());
                    h
                };
                commands.entity(ent).insert(MeshMaterial3d(handle));
            }
            tiles.1 = mode;
        }

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
            // Amortise: stop baking once this frame's budget is spent; the missing
            // tiles are re-selected next frame and bake then (no hitch).
            if bake_budget == 0 {
                break;
            }
            bake_budget -= 1;
            let center = s.region.center;
            let tm = bake_tile_mesh(dem, s.region, viz.tile_res, h, center);
            // Build the grid mesh and attach the CDLOD parent-lattice positions as
            // the morph-target attribute the geomorph vertex shader lerps toward.
            let mut mesh = grid_mesh(tm.positions, tm.normals, tm.uvs, tm.indices);
            mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, tm.morph_targets);
            let mesh = meshes.add(mesh);
            let (cell, local) = grid.translation_to_grid(DVec3::new(center[0], 0.0, center[1]));
            let depth = s.coord.depth as u32;
            // Per-depth morph band (distances): finite for sub-root nodes, "never"
            // for the root (no coarser parent to morph toward).
            let (morph_start, morph_end) = if s.morph_end.is_finite() {
                (s.morph_start as f32, s.morph_end as f32)
            } else {
                (1.0e20, 1.0e21)
            };
            let mat = if let Some(h) = lod_mats.0.get(&(mode, depth)) {
                h.clone()
            } else {
                let h = build_tile_material(mode, morph_start, morph_end, depth, &shader, &mut materials);
                lod_mats.0.insert((mode, depth), h.clone());
                h
            };
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
