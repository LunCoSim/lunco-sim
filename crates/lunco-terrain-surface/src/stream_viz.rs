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

use crate::quadtree::{QuadCoord, Quadtree, Selected, Square};
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

/// Runtime-tunable LOD knobs (Inspector → "Terrain LOD"). Global across terrains.
/// Changing these re-selects tiles live so you can dial detail-vs-distance and the
/// load smoothness without a rebuild.
#[derive(Resource, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct TerrainLodConfig {
    /// `refine_range(d) = range_factor · geometric_error(d)`. Larger → finer tiles
    /// persist farther from the camera (more detail at distance, more tiles).
    pub range_factor: f64,
    /// Deepest quadtree level the streamer refines to (caps closest-up detail).
    pub max_depth: u8,
    /// Tiles BAKED per frame across all terrains. 1 = smoothest frame-time but
    /// slowest fill; raise for a faster initial load at the cost of bigger spikes.
    pub bakes_per_frame: usize,
}

impl Default for TerrainLodConfig {
    fn default() -> Self {
        TerrainLodConfig { range_factor: RANGE_FACTOR, max_depth: MAX_DEPTH, bakes_per_frame: MAX_TILE_BAKES_PER_FRAME }
    }
}

/// Cache of baked tile meshes keyed by quadtree node. A tile's geometry is a pure
/// function of its `QuadCoord` (deterministic DEM sampling), so a despawned tile
/// re-selected later (LOD-boundary oscillation, revisiting an area) reuses its mesh
/// handle instead of re-baking + re-uploading — the "tile caching" that was missing.
/// Bounded: trimmed to currently-resident tiles when it grows past `CACHE_CAP`.
#[derive(Resource, Default)]
pub struct LodMeshCache(HashMap<QuadCoord, Handle<Mesh>>);

/// Soft cap on cached tile meshes before non-resident entries are trimmed.
const CACHE_CAP: usize = 1024;

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
    mut mesh_cache: ResMut<LodMeshCache>,
    cfg: Res<TerrainLodConfig>,
    asset_server: Res<AssetServer>,
) {
    // Use the first 3D camera as the focus. (Multiple cameras → the viz follows
    // whichever; fine for a debug view.)
    let Some(cam) = cameras.iter().next() else { return };
    let cam_pos = cam.translation();
    // No world grid yet → can't anchor tiles; skip this frame.
    let Ok((grid_entity, grid)) = grids.single() else { return };

    // Per-frame bake budget shared across all terrains (amortise scale changes).
    let mut bake_budget = cfg.bakes_per_frame.max(1);

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
        // Runtime LOD knobs (Inspector) drive detail-vs-distance live; tile_res stays
        // per-terrain (changing it would invalidate the mesh cache).
        let qt = Quadtree::new(h, cfg.max_depth.max(1), cfg.range_factor.max(0.1), h);
        let mut sel = qt.select_3d(focus, eye_height);
        let wanted: HashSet<QuadCoord> = sel.iter().map(|s| s.coord).collect();

        // Intelligent baking: nearest tiles first, so the per-frame budget fills the
        // surface in from under the camera outward ("start closer, then extend").
        let dist2 = |s: &Selected| -> f64 {
            (s.region.center[0] - focus[0]).powi(2) + (s.region.center[1] - focus[1]).powi(2)
        };
        sel.sort_by(|a, b| dist2(a).partial_cmp(&dist2(b)).unwrap_or(std::cmp::Ordering::Equal));

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
            // Reuse the cached mesh if this node was baked before; only a true bake
            // (cache miss) spends the per-frame budget. Amortise: once spent, stop —
            // the remaining tiles re-select next frame (no hitch).
            let mesh = if let Some(cached) = mesh_cache.0.get(&s.coord) {
                cached.clone()
            } else {
                if bake_budget == 0 {
                    break;
                }
                bake_budget -= 1;
                let tm = bake_tile_mesh(dem, s.region, viz.tile_res, h, center);
                // Build the grid mesh and attach the CDLOD parent-lattice positions as
                // the morph-target attribute the geomorph vertex shader lerps toward.
                let mut mesh = grid_mesh(tm.positions, tm.normals, tm.uvs, tm.indices);
                mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, tm.morph_targets);
                let handle = meshes.add(mesh);
                mesh_cache.0.insert(s.coord, handle.clone());
                handle
            };
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

        // Despawn no-longer-wanted tiles, but KEEP an old tile while it still covers
        // a wanted tile that hasn't baked yet — otherwise the despawn opens a hole
        // that shows the black sky through ("black squares", worse at 1 bake/frame).
        // Once a region's replacement is in, the old tile no longer overlaps any
        // missing wanted region and is reaped — so this also bounds growth while the
        // camera moves continuously (tiles left behind stop covering holes).
        let missing: Vec<Square> = sel
            .iter()
            .filter(|s| !tiles.0.contains_key(&s.coord))
            .map(|s| s.region)
            .collect();
        tiles.0.retain(|coord, ent| {
            if wanted.contains(coord) {
                return true;
            }
            let region = qt.region(*coord);
            let covers_hole = missing.iter().any(|m| squares_overlap(region, *m));
            if !covers_hole {
                commands.entity(*ent).despawn();
            }
            covers_hole
        });

        // Bound the mesh cache: when it grows past the cap, keep only meshes for
        // currently-resident tiles (deterministic geometry → dropped ones re-bake on
        // demand). Single-terrain scenes are the norm; with several terrains this may
        // drop another's cached meshes, which is harmless (they re-bake).
        if mesh_cache.0.len() > CACHE_CAP {
            let resident: HashSet<QuadCoord> = tiles.0.keys().copied().collect();
            mesh_cache.0.retain(|c, _| resident.contains(c));
        }
    }
}

/// AABB overlap of two axis-aligned [`Square`] regions (XZ).
fn squares_overlap(a: Square, b: Square) -> bool {
    (a.center[0] - b.center[0]).abs() < a.half + b.half
        && (a.center[1] - b.center[1]).abs() < a.half + b.half
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
