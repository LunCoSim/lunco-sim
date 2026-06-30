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
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use big_space::prelude::Grid;
use lunco_core::WorldGrid;
use lunco_obstacle_field::field::HeightGrid;
use lunco_obstacle_field::grid_mesh;
use lunco_materials::{ParamValue, ShaderMaterial, ATTRIBUTE_MORPH_TARGET};

use crate::quadtree::{QuadCoord, Quadtree, Selected, Square};
use crate::tile_mesh::bake_tile_mesh;

/// Vertices per tile side (so each tile is `TILE_RES²` verts). 49 → 48² quads.
/// Higher = finer geometry per tile (smoother crater rims / slopes, fewer visible
/// triangle "lines") at the same tile count — cheap on a modern GPU.
const TILE_RES: usize = 49;
/// Deepest LOD the viz refines to. Bounds the tile count near the camera. 7 gives
/// finer near-field geometry (drivable crater detail) than 6.
const MAX_DEPTH: u8 = 7;
/// `refine_range(d) = RANGE_FACTOR · geometric_error(d)`. Larger → refine from
/// farther (more fine tiles on screen), so mid-distance crater rims stop faceting.
const RANGE_FACTOR: f64 = 4.5;

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

/// One spawned LOD tile + the regen **generation** it was baked at. When a live
/// re-bake (Inspector edit) changes the heights, the terrain's generation bumps; a
/// tile whose `gen` is older is *stale* — it keeps rendering (so the surface never
/// opens a hole) while a fresh replacement bakes near-camera-first, and is despawned
/// the instant that fresh tile spawns. This is what makes regeneration *progressive*
/// instead of a synchronous despawn-everything flash.
#[derive(Clone, Copy)]
struct TileSlot {
    entity: Entity,
    gen: u32,
}

/// The LOD tile entities currently spawned for a terrain, keyed by quadtree node.
/// `mode` is the shader the live tiles were built with (a mode change swaps their
/// materials in place); `gen` bumps on every live height re-bake so tiles refresh
/// progressively (see [`TileSlot`]) rather than all being despawned at once.
#[derive(Component, Default)]
pub struct LodTiles {
    tiles: HashMap<QuadCoord, TileSlot>,
    mode: TerrainShaderMode,
    gen: u32,
}

impl LodTiles {
    /// Bump the generation: every live tile becomes stale and re-bakes from the new
    /// heights, near-camera-first, while still covering the surface until replaced.
    /// Called by the live re-bake instead of despawning the whole tile set.
    pub fn invalidate(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }
}

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
        // Off-thread baking → safe to start several tasks/frame for a fast,
        // non-blocking fill. Raise/lower live in the Inspector.
        TerrainLodConfig { range_factor: RANGE_FACTOR, max_depth: MAX_DEPTH, bakes_per_frame: 4 }
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
/// Max bake tasks in flight per terrain (backpressure so a big move doesn't queue
/// thousands of tasks). New tasks wait for slots to free.
const MAX_INFLIGHT_BAKES: usize = 64;
/// A freshly-spawned LOD tile **refines in** from its parent's coarse lattice over
/// this many seconds (a CDLOD "settle") instead of popping — this both animates LOD
/// refinement as you move and makes live height re-bakes resolve smoothly.
const REVEAL_SECS: f32 = 0.35;

/// A LOD tile currently playing its reveal "settle" (refine from the parent lattice
/// to its own geometry). Carries a transient per-tile material being tweened; when
/// the reveal completes the tile swaps back to the shared (batched) depth material
/// and this is removed. Bounded by the per-frame spawn rate × [`REVEAL_SECS`].
#[derive(Component)]
pub(crate) struct TileReveal {
    elapsed: f32,
    /// The transient clone being tweened (`reveal` 0→1); dropped when done.
    anim: Handle<ShaderMaterial>,
    /// The shared cached material to restore once revealed (recovers batching).
    shared: Handle<ShaderMaterial>,
}

/// Result of an off-thread tile bake: the finished CPU `Mesh` (not yet uploaded)
/// plus the spawn metadata the main thread needs.
struct BakedTile {
    mesh: Mesh,
    center: [f64; 2],
    depth: u32,
    morph_start: f32,
    morph_end: f32,
}

/// In-flight off-thread tile bakes for a terrain, keyed by quadtree node. The CPU
/// bake (`bake_tile_mesh` + grid mesh build) runs on the [`AsyncComputeTaskPool`];
/// the main thread only uploads the finished mesh + spawns the entity — so baking
/// never blocks the frame ("non-blocking, extend outward"). Cancelled by drop when
/// the terrain despawns.
#[derive(Component, Default)]
pub struct PendingTileBakes(HashMap<QuadCoord, (u32, Task<BakedTile>)>);

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

/// Spawn one LOD tile entity (mesh + per-(mode,depth) material, anchored to its own
/// big_space cell). Shared by the cache-hit and finalized-bake paths.
#[allow(clippy::too_many_arguments)]
fn spawn_tile(
    commands: &mut Commands,
    grid: &Grid,
    grid_entity: Entity,
    terrain: Entity,
    coord: QuadCoord,
    mesh: Handle<Mesh>,
    center: [f64; 2],
    depth: u32,
    morph_start: f32,
    morph_end: f32,
    mode: TerrainShaderMode,
    shader: &Handle<Shader>,
    materials: &mut Assets<ShaderMaterial>,
    lod_mats: &mut LodMaterials,
) -> (Entity, Handle<ShaderMaterial>) {
    let (cell, local) = grid.translation_to_grid(DVec3::new(center[0], 0.0, center[1]));
    let mat = if let Some(h) = lod_mats.0.get(&(mode, depth)) {
        h.clone()
    } else {
        let h = build_tile_material(mode, morph_start, morph_end, depth, shader, materials);
        lod_mats.0.insert((mode, depth), h.clone());
        h
    };
    let ent = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(mat.clone()),
            cell,
            Transform::from_translation(local),
            Visibility::Inherited,
            LodTileOf(terrain),
            Name::new(format!("LodTile d{} {},{}", coord.depth, coord.x, coord.z)),
            ChildOf(grid_entity),
        ))
        .id();
    (ent, mat)
}

/// Start a tile's reveal "settle": give it a transient clone of its shared material
/// with `reveal = 0` (vertices ride the parent's coarse lattice) and a [`TileReveal`]
/// so [`animate_tile_reveal`] tweens it up to its own geometry. Only sub-root tiles
/// animate (the root has no coarser parent to grow from).
fn begin_reveal(
    commands: &mut Commands,
    entity: Entity,
    shared: Handle<ShaderMaterial>,
    materials: &mut Assets<ShaderMaterial>,
) {
    let Some(mut anim) = materials.get(&shared).cloned() else { return };
    anim.set("reveal", ParamValue::F32(0.0));
    let anim = materials.add(anim);
    commands
        .entity(entity)
        .insert((MeshMaterial3d(anim.clone()), TileReveal { elapsed: 0.0, anim, shared }));
}

/// Per-frame: advance each revealing tile's `reveal` 0→1, then restore the shared
/// (batched) material + drop the transient clone. See [`TileReveal`] / [`REVEAL_SECS`].
pub(crate) fn animate_tile_reveal(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    mut q: Query<(Entity, &mut TileReveal)>,
) {
    let dt = time.delta_secs();
    for (ent, mut rev) in &mut q {
        rev.elapsed += dt;
        let t = (rev.elapsed / REVEAL_SECS).clamp(0.0, 1.0);
        // smoothstep ease so the settle starts/ends gently.
        let s = t * t * (3.0 - 2.0 * t);
        if let Some(m) = materials.get_mut(&rev.anim) {
            m.set("reveal", ParamValue::F32(s));
        }
        if t >= 1.0 {
            let anim = rev.anim.clone();
            commands
                .entity(ent)
                .insert(MeshMaterial3d(rev.shared.clone()))
                .remove::<TileReveal>();
            materials.remove(&anim);
        }
    }
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
        &mut PendingTileBakes,
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

    for (terrain, t_gt, hf, viz, mut tiles, mut pending, mode_opt) in &mut terrains {
        let mode = mode_opt.copied().unwrap_or_default();
        // The terrain's current height generation: a tile/bake tagged with an older
        // gen is stale (a live re-bake changed the heights) and is replaced near-first.
        let cur_gen = tiles.gen;
        // The mode's shader drives both the @vertex (CDLOD morph) and @fragment
        // stages; load-by-path → cached handle, hot-reloads on edit.
        let shader = asset_server.load(mode.shader_path());
        // Shader mode changed (inspector edit) → SWAP the material on every live
        // tile in place (same geometry, new shader/colour) instead of despawning +
        // rebuilding, which left a one-frame black hole until the tiles re-baked.
        if tiles.mode != mode {
            let old_mode = tiles.mode;
            let swaps: Vec<(Entity, u32)> =
                tiles.tiles.iter().map(|(c, s)| (s.entity, c.depth as u32)).collect();
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
            tiles.mode = mode;
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

        // A coord is *fresh* (no work needed) when it has a resident tile OR an
        // in-flight bake tagged with the current generation. A stale entry (older
        // gen, from before a live re-bake) still renders but no longer counts as
        // satisfied, so a current-gen replacement is queued for it.
        let fresh_tile = |tiles: &LodTiles, c: &QuadCoord| {
            tiles.tiles.get(c).is_some_and(|s| s.gen == cur_gen)
        };

        // ── Finalize completed off-thread bakes ──────────────────────
        // Poll in-flight tasks; for each finished bake, upload its mesh (cheap, main
        // thread) + spawn the tile. The expensive DEM sampling already ran on a
        // worker thread, so the frame never blocks on baking.
        let mut done: Vec<(QuadCoord, u32, BakedTile)> = Vec::new();
        pending.0.retain(|coord, (gen, task)| match block_on(future::poll_once(&mut *task)) {
            Some(baked) => {
                done.push((*coord, *gen, baked));
                false
            }
            None => true,
        });
        for (coord, gen, baked) in done {
            // A bake from a superseded generation (heights changed while it ran) is
            // discarded — its mesh would show the OLD terrain.
            if gen != cur_gen {
                continue;
            }
            let handle = meshes.add(baked.mesh);
            mesh_cache.0.insert(coord, handle.clone());
            // No longer selected while it baked → keep the cached mesh, skip spawning.
            if !wanted.contains(&coord) {
                continue;
            }
            let depth = baked.depth;
            let (ent, shared) = spawn_tile(
                &mut commands, grid, grid_entity, terrain, coord, handle, baked.center,
                depth, baked.morph_start, baked.morph_end, mode, &shader, &mut materials,
                &mut lod_mats,
            );
            // Sub-root tiles settle in from their parent's coarse lattice (no pop).
            if depth > 0 {
                begin_reveal(&mut commands, ent, shared, &mut materials);
            }
            // Replace any stale slot at this coord, despawning the tile it held.
            if let Some(old) = tiles.tiles.insert(coord, TileSlot { entity: ent, gen: cur_gen }) {
                commands.entity(old.entity).try_despawn();
            }
        }

        // ── Queue new work, nearest-first ────────────────────────────
        // Cache hits spawn instantly; misses spawn an off-thread bake task (budgeted:
        // `bakes_per_frame` new tasks/frame, ≤ MAX_INFLIGHT in flight). Tiles anchor
        // to their OWN big_space `CellCoord` (vertices baked relative to the tile
        // centre) so far-from-origin tiles keep f32 precision.
        let pool = AsyncComputeTaskPool::get();
        for s in &sel {
            // Skip coords already satisfied at the current generation (resident tile
            // or in-flight current-gen bake). Stale tiles fall through → re-baked.
            let have_pending = pending.0.get(&s.coord).is_some_and(|(g, _)| *g == cur_gen);
            if fresh_tile(&tiles, &s.coord) || have_pending {
                continue;
            }
            let depth = s.coord.depth as u32;
            // Per-depth morph band: finite for sub-root nodes, "never" for the root.
            let (morph_start, morph_end) = if s.morph_end.is_finite() {
                (s.morph_start as f32, s.morph_end as f32)
            } else {
                (1.0e20, 1.0e21)
            };
            if let Some(cached) = mesh_cache.0.get(&s.coord) {
                let (ent, shared) = spawn_tile(
                    &mut commands, grid, grid_entity, terrain, s.coord, cached.clone(),
                    s.region.center, depth, morph_start, morph_end, mode, &shader,
                    &mut materials, &mut lod_mats,
                );
                if depth > 0 {
                    begin_reveal(&mut commands, ent, shared, &mut materials);
                }
                if let Some(old) =
                    tiles.tiles.insert(s.coord, TileSlot { entity: ent, gen: cur_gen })
                {
                    commands.entity(old.entity).try_despawn();
                }
                continue;
            }
            // Cache miss → needs a bake. Respect the per-frame + in-flight budgets
            // (keep scanning for cheap cache hits regardless of the bake budget).
            if bake_budget == 0 || pending.0.len() >= MAX_INFLIGHT_BAKES {
                continue;
            }
            bake_budget -= 1;
            let dem_arc = hf.0.clone();
            let region = s.region;
            let tile_res = viz.tile_res;
            let half = h;
            let center = s.region.center;
            let task = pool.spawn(async move {
                let tm = bake_tile_mesh(&dem_arc, region, tile_res, half, center);
                let mut mesh = grid_mesh(tm.positions, tm.normals, tm.uvs, tm.indices);
                mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, tm.morph_targets);
                BakedTile { mesh, center, depth, morph_start, morph_end }
            });
            pending.0.insert(s.coord, (cur_gen, task));
        }

        // Despawn no-longer-wanted (or stale, replaced) tiles, but KEEP one while it
        // still covers a wanted region that has no fresh tile yet — otherwise the
        // despawn opens a hole showing black sky ("black squares"). This is also what
        // makes a live re-bake progressive: on a generation bump every tile goes
        // stale at once, all keep covering the surface, and each is reaped only when
        // its current-gen replacement bakes in (near-camera-first).
        let missing: Vec<Square> = sel
            .iter()
            .filter(|s| !fresh_tile(&tiles, &s.coord))
            .map(|s| s.region)
            .collect();
        tiles.tiles.retain(|coord, slot| {
            // A fresh, still-wanted tile always stays.
            if slot.gen == cur_gen && wanted.contains(coord) {
                return true;
            }
            // Otherwise (not wanted, or stale): hold it only while it plugs a hole.
            let region = qt.region(*coord);
            let covers_hole = missing.iter().any(|m| squares_overlap(region, *m));
            if !covers_hole {
                commands.entity(slot.entity).try_despawn();
            }
            covers_hole
        });

        // Bound the mesh cache: when it grows past the cap, keep only meshes for
        // currently-resident tiles (deterministic geometry → dropped ones re-bake on
        // demand). Single-terrain scenes are the norm; with several terrains this may
        // drop another's cached meshes, which is harmless (they re-bake).
        if mesh_cache.0.len() > CACHE_CAP {
            let resident: HashSet<QuadCoord> = tiles.tiles.keys().copied().collect();
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
