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
//! # Appearance is INTENT here, not a material
//!
//! A tile carries a [`ShaderLook`] — the shader path, its named parameters (morph
//! band, reveal step, overlay uniforms, per-depth map weights) and its texture
//! layers — and **nothing in this crate names a material**. `lunco-render-bevy`
//! binds it, caching by `ShaderLook::key()`, so tiles in the same LOD band and
//! reveal step share ONE material and ONE bind group. That cache *is* the old
//! hand-rolled `LodMaterials`/`MatKey` table, done generically: the `(mode, depth,
//! band bucket, reveal step)` that `MatKey` encoded are simply the look's own
//! content now. See `docs/architecture/render-decoupling.md`.
//!
//! The companion canonical-res collider ring is [`crate::collider_ring`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use big_space::prelude::Grid;
use lunco_core::WorldGrid;
use lunco_obstacle_field::grid_mesh;
use lunco_materials::{ParamValue, ShaderLook, TextureLayer, ATTRIBUTE_MORPH_TARGET};
use lunco_terrain_core::{measure_node_error, HeightSource};

use crate::derived_layers::TerrainDerivedMaps;
use crate::oracle::SurfaceOracle;
use crate::quadtree::{QuadCoord, Quadtree, Selected, Square};

/// Vertices per tile side (so each tile is `TILE_RES²` verts). 49 → 48² quads.
/// Higher = finer geometry per tile (smoother crater rims / slopes, fewer visible
/// triangle "lines") at the same tile count — cheap on a modern GPU.
const TILE_RES: usize = 49;
/// Deepest LOD the viz refines to. Bounds the tile count near the camera. With
/// error-driven selection only feature tiles (rims, peaks) actually reach it, so
/// 8 (≈0.65 m vertex pitch on a ±4 km DEM) stays cheap while crater rims resolve.
const MAX_DEPTH: u8 = 8;
/// On-screen error (px, at the canonical viewport) at which a node refines —
/// the ONE detail-vs-cost knob of the error-driven metric. Smaller = finer.
const TARGET_PIXEL_ERROR: f64 = 3.0;
/// Canonical viewport for the screen metric (fixed → selection is independent of
/// any client's real resolution/FOV; peers select identically).
const CANON_SCREEN_H_PX: f64 = 1080.0;
const CANON_FOV_Y_RAD: f64 = std::f64::consts::FRAC_PI_4; // 45°
/// Probe mesh resolution for [`measure_node_error`] — coarse on purpose: the
/// measurement senses "is there detail here worth refining toward," it does not
/// need the tile's full 49² fidelity. ~657 oracle samples per (memoized) node.
const NODE_ERROR_PROBE_RES: usize = 9;

/// The composed surface oracle retained on a terrain entity — the ONE height
/// truth every consumer samples (LOD tile baker, collider ring, derived-layer
/// texture bakes, rock scatter, `TerrainHeight` query). `Arc` so off-thread bakes
/// share it without a copy.
#[derive(Component)]
pub struct DemHeightField(pub Arc<SurfaceOracle>);

/// Analytic DEM ground height at world `(x, z)` — reads the retained height grid
/// directly (no avian collider), so it answers **before** a collider tile streams
/// in. Returns the world-space `Y` of the terrain surface, or `None` when no DEM
/// terrain covers the point.
///
/// This is the *placement* twin of [`crate::query::TerrainHeightProvider`] (the
/// `query("TerrainHeight")` API): spawn placement uses it so a rover dropped over
/// un-streamed terrain lands on the surface instead of free-falling through the
/// not-yet-baked collider. Mirror its coordinate convention (query `(x,z)` in the
/// terrain's `GlobalTransform` frame; DEM anchors at the origin cell).
pub fn dem_ground_height<'a>(
    terrains: impl IntoIterator<Item = (&'a GlobalTransform, &'a DemHeightField)>,
    x: f64,
    z: f64,
) -> Option<f64> {
    use lunco_terrain_core::HeightSource;
    // GRID-ABSOLUTE in, GRID-ABSOLUTE out. The DEM owner is anchored at the grid
    // ORIGIN cell with an identity transform (`terrain.rs`), so terrain-local ==
    // grid-absolute and the oracle is sampled directly. This helper feeds the
    // spawn path, which plants the returned Y as a grid-absolute `Transform` (cell
    // 0 + avian recenter). Round-tripping through the terrain's *render*
    // `GlobalTransform` (as this used to) returned an origin-relative Y (and shifted
    // x,z by the floating-origin offset), so at elevation spawned bodies dropped
    // ~2 km below the surface and free-fell. `_gt` intentionally unused.
    for (_gt, hf) in terrains {
        let grid = hf.0.as_ref();
        if x.abs() > grid.half_extent() as f64 || z.abs() > grid.half_extent() as f64 {
            continue;
        }
        return Some(HeightSource::height_at(grid, x, z));
    }
    None
}

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
    /// Platform default when a terrain authors no explicit mode: [`Lit`](Self::Lit)
    /// everywhere. Web used to default to [`Plain`](Self::Plain) when the Lit
    /// far field was carried by ~5 unconditional per-fragment `fbm` calls
    /// (~100 ms on a WebGL iGPU); the far field is texture-driven now (baked
    /// surface/normal maps) and `terrain_geomorph_web.wgsl` gates every bump
    /// layer behind its distance fade, so distant fragments cost two texture
    /// samples. Switchable live in the Inspector either way.
    pub fn platform_default() -> Self {
        TerrainShaderMode::Lit
    }

    /// The `.wgsl` this mode draws with (all carry the CDLOD vertex morph).
    fn shader_path(self) -> &'static str {
        match self {
            TerrainShaderMode::Lit => {
                #[cfg(target_arch = "wasm32")]
                {
                    "shaders/terrain_geomorph_web.wgsl"
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    "shaders/terrain_geomorph.wgsl"
                }
            }
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
    /// The tile's selected morph-band end (parent refine range) — kept so a live
    /// shader-mode swap can rebuild the right band material without re-selecting.
    morph_end: f32,
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
    /// Signature of the inputs the tile SELECTION is a pure function of (camera
    /// focus + eye height, dynamic-body footprints, generation, oracle identity,
    /// LOD knobs). When it matches last frame's and no bake is in flight, the
    /// resident tile set is already correct and the whole reselection is skipped —
    /// the idle-camera fast path (see [`update_lod_tiles`]). `None` = never selected.
    last_sig: Option<u64>,
    /// `pixel_error` that satisfied the tile budget on the last selection — the
    /// budget-fit warm start. The coarsening loop re-runs a full
    /// `select_with_error` walk per ×1.6 step, and always starting from the
    /// configured base re-paid every failing rung (~9 walks/frame on a loaded
    /// scene). The next selection starts ONE step below this (clamped to the
    /// base) so steady state costs ~1-2 walks and quality still recovers, a
    /// rung per selection, when the load drops. `None` = never fitted.
    last_fit_px: Option<f64>,
    /// Last selection's cover — the LOD **hysteresis** memory
    /// ([`lunco_terrain_core::REFINE_HYSTERESIS`]). A node already refined last frame
    /// keeps its children until the camera backs out past `1.15 ×` its refine range,
    /// so a camera hovering ON a refine boundary no longer re-splits and re-merges
    /// that node every frame (a despawn + spawn + 0.35 s reveal per flip on a tile
    /// whose LOD never changed). Empty = first selection → the bare metric.
    prev_sel: HashSet<QuadCoord>,
}

impl LodTiles {
    /// The shader mode the resident tiles were built with — the D8 gate: only `Lit`
    /// tiles carry the map/overlay params, so only they are re-stated when those
    /// change.
    pub(crate) fn shader_mode(&self) -> TerrainShaderMode {
        self.mode
    }

    /// Every resident tile entity (the late-bind / live-tune targets).
    pub(crate) fn tile_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.tiles.values().map(|s| s.entity)
    }

    /// Every resident tile as `(quadtree depth, entity)` — the depth drives the
    /// derived-map blend weights (`tile_map_weights`).
    pub(crate) fn tiles_with_depth(&self) -> impl Iterator<Item = (u32, Entity)> + '_ {
        self.tiles.iter().map(|(c, s)| (c.depth as u32, s.entity))
    }

    /// Bump the generation: every live tile becomes stale and re-bakes from the new
    /// heights, near-camera-first, while still covering the surface until replaced.
    /// Called by the live re-bake instead of despawning the whole tile set.
    pub fn invalidate(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    /// Invalidate only the tiles whose world footprint overlaps `bounds`
    /// (`[min_x, min_z, max_x, max_z]`, terrain-local metres) — the incremental
    /// re-bake for a **bounded** edit (a brush/flatten touches a small patch, not
    /// the whole terrain). Tiles outside the region are re-stamped to the new
    /// generation so they read as fresh and are never re-selected or re-baked; only
    /// overlapping tiles fall through to a re-bake. `None` bounds = whole terrain →
    /// same as [`invalidate`]. `root_half_extent` is the DEM half-extent (the
    /// quadtree root region), so each tile's square derives from its `QuadCoord`.
    pub fn invalidate_region(&mut self, bounds: Option<[f64; 4]>, root_half_extent: f64) {
        let new_gen = self.gen.wrapping_add(1);
        if let Some(aabb) = bounds {
            for (coord, slot) in self.tiles.iter_mut() {
                // Non-overlapping tiles keep the NEW gen → stay fresh (skipped).
                // Overlapping tiles keep their OLD gen → stale → re-baked.
                if !node_overlaps_aabb(*coord, root_half_extent, aabb) {
                    slot.gen = new_gen;
                }
            }
        }
        self.gen = new_gen;
    }

    /// Remove (and return for despawn) every tile already stale from a PRIOR
    /// invalidation — i.e. older than the current generation. Called right before a
    /// new `invalidate()` so that rapid successive re-bakes keep at most ONE
    /// generation of hole-cover instead of piling up generations of dead tiles (which
    /// made the per-frame tile bookkeeping go O(n²) and tanked the frame rate).
    pub fn reap_stale(&mut self) -> Vec<Entity> {
        let cur = self.gen;
        let mut dead = Vec::new();
        self.tiles.retain(|_, slot| {
            if slot.gen == cur {
                true
            } else {
                dead.push(slot.entity);
                false
            }
        });
        dead
    }
}

/// Whether the world square of quadtree node `coord` (derived from the DEM
/// `root_half_extent`, origin-centred — matching [`lunco_terrain_core::Quadtree::region`])
/// overlaps the axis-aligned `[min_x, min_z, max_x, max_z]` box. The shared
/// spatial test behind the incremental region re-bake.
fn node_overlaps_aabb(coord: QuadCoord, root_half_extent: f64, aabb: [f64; 4]) -> bool {
    let [min_x, min_z, max_x, max_z] = aabb;
    let nodes_per_side = (1u64 << coord.depth) as f64;
    let side = (2.0 * root_half_extent) / nodes_per_side;
    let half = 0.5 * side;
    let cx = -root_half_extent + (coord.x as f64 + 0.5) * side;
    let cz = -root_half_extent + (coord.z as f64 + 0.5) * side;
    cx - half <= max_x && cx + half >= min_x && cz - half <= max_z && cz + half >= min_z
}

/// Back-pointer from a spawned LOD tile to its owning terrain. Tiles are parented
/// to the big_space **grid** (so each can carry its own `CellCoord`), not to the
/// terrain entity — so this tag lets [`despawn_orphaned_lod_tiles`] reap them when
/// the terrain is gone (e.g. on twin reload) instead of leaking under the grid.
#[derive(Component)]
pub struct LodTileOf(pub Entity);

/// Quantisation steps of the reveal tween (`reveal` 0 → 1). 8 steps at
/// [`REVEAL_SECS`] = 0.35 s is a step every ~44 ms — below the perceptual
/// threshold for this soft lattice settle.
///
/// Quantising matters as much as it ever did, it just no longer needs a bespoke
/// table: the reveal step is a PARAMETER VALUE in the tile's [`ShaderLook`], and
/// the binder's content cache turns the 8 steps of a band into 8 shared materials
/// rather than one per tile. (Reveal used to clone the shared material per tile and
/// `get_mut` it every frame for 0.35 s: the uniform buffer + bind group were
/// re-prepared every frame, batching died — ≈84 unique materials alive at 4
/// bakes/frame × 60 fps — and the continuous `AssetEvent::Modified` defeated the
/// `mat_changed` early-out in `lunco-materials`. A tile now changes its look ONLY on
/// a step boundary, and the binder swaps a cached handle. Nothing mutates an asset.)
const REVEAL_STEPS: u8 = 8;
/// The `reveal == 1` (steady-state, batched) look — the one a tile keeps once its
/// settle finishes, and the one every non-revealing tile draws with.
const REVEAL_FULL: u8 = REVEAL_STEPS;

/// Quantise a morph-band end onto the shared bucket lattice. `u32::MAX` = the
/// "never morphs" root sentinel.
fn band_bucket(morph_end: f32) -> u32 {
    if !morph_end.is_finite() || morph_end >= 1.0e19 {
        return u32::MAX;
    }
    // Quarter-log steps (~±12% band granularity).
    (morph_end.max(1.0).ln() * 4.0).floor() as u32
}

/// Snap a selected morph band to its bucket's representative values, so the tile
/// and its cached material agree exactly. Snapping DOWN (floor bucket) means a
/// tile always finishes morphing *before* the selection swaps in its parent — a
/// slightly early morph, never a pop.
fn snap_band(morph_end: f32) -> (f32, f32, u32) {
    let bucket = band_bucket(morph_end);
    if bucket == u32::MAX {
        return (1.0e20, 1.0e21, bucket);
    }
    let end = (bucket as f32 * 0.25).exp();
    (0.7 * end, end, bucket)
}

/// Runtime-tunable LOD knobs (Inspector → "Terrain LOD"). Global across terrains.
/// Changing these re-selects tiles live so you can dial detail-vs-distance and the
/// load smoothness without a rebuild.
#[derive(Resource, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct TerrainLodConfig {
    /// Screen-metric refinement threshold (px at the canonical viewport): a node
    /// refines while its MEASURED surface error subtends more than this. Smaller
    /// = finer everywhere; detail lands where the surface earns it (rims, peaks),
    /// not uniformly by distance.
    pub pixel_error: f64,
    /// Deepest quadtree level the streamer refines to (caps closest-up detail).
    pub max_depth: u8,
    /// Tiles BAKED per frame across all terrains. 1 = smoothest frame-time but
    /// slowest fill; raise for a faster initial load at the cost of bigger spikes.
    pub bakes_per_frame: usize,
    /// Cap on SELECTED tiles per terrain. The error-driven walk's tile count is
    /// otherwise unbounded in the terrain: at realistic crater densities every
    /// mid-distance node measures metres of error, and a 3 px target refined a
    /// ~1 km disc to max depth (~6.8k live tiles ≈ 33M triangles on moonbase —
    /// the 49 FPS). Enforced by COARSENING `pixel_error` until the selection
    /// fits (so geomorph bands stay consistent with the actual transition
    /// distances), not by capping the walk. ~500 tiles ≈ 2.3M terrain triangles.
    pub tile_budget: usize,
}

impl Default for TerrainLodConfig {
    fn default() -> Self {
        // Off-thread baking → safe to start several tasks/frame for a fast,
        // non-blocking fill. Raise/lower live in the Inspector.
        //
        // `tile_budget` caps SELECTED tiles per terrain — the dominant terrain GPU
        // cost (~512 tiles ≈ 2.3M tris re-rendered every frame). On the wasm/WebGL
        // target (single render thread, no CPU-side preprocessing) that throughput
        // is the biggest steady-state cost, so the browser starts at a lighter
        // budget — the terrain EXTENT is unchanged (coarsening `pixel_error` keeps
        // the same footprint with fewer, larger far-field tiles); only distant
        // detail softens. Native keeps the full budget. Tune live in the Inspector.
        #[cfg(target_arch = "wasm32")]
        let tile_budget = 64;
        #[cfg(not(target_arch = "wasm32"))]
        let tile_budget = 512;
        // On wasm32 the `AsyncComputeTaskPool` has NO threads: the "off-thread"
        // bake future runs to completion on the MAIN thread the instant it is
        // polled. A tile bake is ~12k composed-oracle samples (2401 verts ×
        // height_at + eps normals + the parent lattice), each walking the full
        // modifier chain — so `bakes_per_frame` is a direct main-thread frame cost
        // there. Cap it at 1, mirroring `collider_ring`'s wasm `bake_budget = 2`.
        #[cfg(target_arch = "wasm32")]
        let bakes_per_frame = 1;
        #[cfg(not(target_arch = "wasm32"))]
        let bakes_per_frame = 4;
        TerrainLodConfig {
            pixel_error: TARGET_PIXEL_ERROR,
            max_depth: MAX_DEPTH,
            bakes_per_frame,
            tile_budget,
        }
    }
}

/// Memoized per-node measured geometric error for a terrain's current oracle —
/// the cache behind error-driven CDLOD selection. Keyed by quadtree node; wiped
/// whenever the oracle Arc is swapped (live re-compose). Errors are measured
/// lazily for nodes the selection walk actually visits (O(visited), a few tens of
/// µs each, then cached for the oracle's lifetime).
#[derive(Component, Default)]
pub struct TerrainNodeErrors {
    map: HashMap<QuadCoord, f64>,
    /// Identity of the oracle the cached errors were measured against.
    oracle_ptr: usize,
}

/// Cache of baked tile meshes keyed by quadtree node. A tile's geometry is a pure
/// function of its `QuadCoord` (deterministic DEM sampling), so a despawned tile
/// re-selected later (LOD-boundary oscillation, revisiting an area) reuses its mesh
/// handle instead of re-baking + re-uploading — the "tile caching" that was missing.
/// Bounded: trimmed to currently-resident tiles when it grows past `CACHE_CAP`.
/// Value: the cached mesh handle AND the `origin_y` its vertices were rebased by at
/// bake time. `origin_y` MUST travel with the mesh — the tile is placed at it, so a
/// recompute at spawn (against a since-changed oracle, e.g. a crater layer composed
/// mid-load) would seat the mesh at a different height than it was baked for and the
/// tile would jump/jitter. Stored together, mesh and placement can never disagree.
#[derive(Resource, Default)]
pub struct LodMeshCache(HashMap<(Entity, QuadCoord), (Handle<Mesh>, f64)>);

impl LodMeshCache {
    /// Drop cached meshes a live height edit invalidated, scoped to one `terrain`.
    /// Geometry is a pure function of `(terrain, coord)` only while that terrain's
    /// oracle is fixed; a brush/flatten changes its surface, so a re-selected tile
    /// in the edited area would otherwise reuse its pre-edit mesh. `Some(bounds)`
    /// drops just the overlapping nodes (the incremental patch); `None` (a whole-
    /// terrain spec change) clears the terrain's entries. Other terrains' entries —
    /// and this terrain's non-overlapping ones — survive, so revisiting unedited
    /// ground still hits the cache. Keying on the terrain `Entity` (not the coord
    /// alone) is what stops one terrain reusing another's mesh for a shared coord.
    pub fn drop_region(&mut self, terrain: Entity, bounds: Option<[f64; 4]>, root_half_extent: f64) {
        match bounds {
            None => self.0.retain(|(e, _), _| *e != terrain),
            Some(aabb) => self.0.retain(|(e, coord), _| {
                *e != terrain || !node_overlaps_aabb(*coord, root_half_extent, aabb)
            }),
        }
    }
}

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
/// to its own geometry). The tile walks the [`REVEAL_STEPS`] quantised reveal values
/// of its band — written into its own [`ShaderLook`], so the binder resolves each
/// step to one SHARED material — and settles on the fully-revealed look when done.
/// No per-tile material clone, no per-frame `Assets::get_mut`. Bounded by the
/// per-frame spawn rate × [`REVEAL_SECS`].
#[derive(Component)]
pub(crate) struct TileReveal {
    elapsed: f32,
    /// The reveal step currently written into the tile's look.
    step: u8,
}

/// Result of an off-thread tile bake: the finished CPU `Mesh` (not yet uploaded)
/// plus the spawn metadata the main thread needs.
struct BakedTile {
    mesh: Mesh,
    center: [f64; 2],
    depth: u32,
    morph_end: f32,
    /// Surface height at the tile centre the mesh Y was rebased by (see `LodMeshCache`).
    origin_y: f64,
}

/// In-flight off-thread tile bakes for a terrain, keyed by quadtree node. The CPU
/// bake (`bake_tile_mesh` + grid mesh build) runs on the [`AsyncComputeTaskPool`];
/// the main thread only uploads the finished mesh + spawns the entity — so baking
/// never blocks the frame ("non-blocking, extend outward"). Cancelled by drop when
/// the terrain despawns.
#[derive(Component, Default)]
pub struct PendingTileBakes(HashMap<QuadCoord, (u32, Task<BakedTile>)>);

/// Far-field sun-shadow wiring for a STREAMED terrain's tiles: the pre-baked
/// R8 sun-visibility texture (lunco-environment's horizon shadow cache) plus
/// the CSM far bound the shader blends in beyond. Written by the app glue
/// (which can see both the environment's `HorizonShadowCache` and this crate);
/// consumed by the tile materials. `on == 0` disables sampling (params written
/// so a cache that goes stale can be switched off without touching handles).
#[derive(Component, Clone)]
pub struct TileShadowCache {
    pub image: Handle<Image>,
    pub on: f32,
    pub csm_far: f32,
}

/// Write one named parameter into a look, reusing the existing slot when the key is
/// already present so the hot re-write path (the reveal step, the overlay sync)
/// doesn't allocate a `String` per call.
pub(crate) fn set_param(look: &mut ShaderLook, name: &str, v: ParamValue) {
    if let Some(slot) = look.values.get_mut(name) {
        *slot = v;
    } else {
        look.values.insert(name.to_string(), v);
    }
}

/// Bind a terrain's far-shadow cache onto one tile look (Lit only).
pub(crate) fn apply_shadow_cache_to_look(look: &mut ShaderLook, cache: &TileShadowCache) {
    look.textures.insert(TextureLayer::ShadowCache, cache.image.clone());
    set_param(look, "shadow_cache_on", ParamValue::F32(cache.on));
    set_param(look, "csm_far", ParamValue::F32(cache.csm_far));
}

/// Per-depth weights for the baked derived maps, from the ratio of the tile's
/// vertex pitch to the map's texel pitch (`r = map_res / (2^depth · quads)`,
/// window-size independent):
///
/// - `weight_normal` fades IN where the tile geometry is COARSER than the map
///   (far tiles — the map carries the crater rims the mesh LOD'd away) and OFF
///   where fine near geometry out-resolves the map (blending the coarser map
///   there would only blur real relief).
/// - `weight_ao` / `weight_tone` stay partially on everywhere (bowls genuinely
///   receive less sky light at any range) and saturate on coarse tiles.
fn tile_map_weights(depth: u32, map_res: usize) -> (f32, f32, f32) {
    let r = map_res as f32 / (((1u32 << depth.min(24)) * (TILE_RES as u32 - 1)) as f32);
    let w_normal = ((r - 0.75) / 1.5).clamp(0.0, 1.0);
    let w_ao = (0.35 + (r - 0.5) * 0.4).clamp(0.35, 1.0);
    let w_tone = (0.5 + (r - 0.5) * 0.35).clamp(0.5, 1.0);
    (w_normal, w_ao, w_tone)
}

/// Bind a terrain's baked derived maps + per-depth weights onto one tile
/// look (Lit mode only — the flat/debug shader declares no map bindings).
///
/// The map handles are part of `ShaderLook::key()`, so two terrains with different
/// baked maps correctly get different materials, and every tile of ONE terrain at
/// one depth still shares a single one.
pub(crate) fn apply_maps_to_look(look: &mut ShaderLook, maps: &TerrainDerivedMaps, depth: u32) {
    look.textures.insert(TextureLayer::Surface, maps.surface.clone());
    look.textures.insert(TextureLayer::Normal, maps.normal.clone());
    let (w_normal, w_ao, w_tone) = tile_map_weights(depth, maps.res);
    set_param(look, "weight_normal", ParamValue::F32(w_normal));
    set_param(look, "weight_ao", ParamValue::F32(w_ao));
    set_param(look, "weight_tone", ParamValue::F32(w_tone));
}

/// The `reveal` uniform a quantised reveal step stands for (`REVEAL_FULL` → 1.0).
fn reveal_value(step: u8) -> f32 {
    if step >= REVEAL_FULL {
        1.0
    } else {
        step as f32 / REVEAL_STEPS as f32
    }
}

/// The appearance INTENT of one LOD tile: the geomorph shader (it drives both the
/// `@vertex` morph and the `@fragment` stage), its morph band, its reveal step, and
/// — in `Lit` mode — the derived maps, the far-shadow cache and the analysis
/// overlay.
///
/// THE SHARING CONTRACT: two tiles in the same mode, band bucket and reveal step
/// produce an EQUAL `ShaderLook::key()`, so `lunco-render-bevy` hands them the same
/// `ShaderMaterial` handle — one bind group, one batch. This is the property the
/// hand-rolled `MatKey`/`LodMaterials` cache existed for; it is now a consequence of
/// the look's content rather than a table anyone has to remember to consult.
/// Anything that varies per-tile (a raw `morph_end` instead of the snapped band, an
/// un-bucketed value) would mint a material per tile and destroy batching — which is
/// exactly why the band is snapped and the reveal is quantised before it lands here.
fn tile_look(
    mode: TerrainShaderMode,
    depth: u32,
    morph_start: f32,
    morph_end: f32,
    reveal: u8,
    maps: Option<&TerrainDerivedMaps>,
    shadow: Option<&TileShadowCache>,
    overlay: crate::overlay::OverlayUniforms,
) -> ShaderLook {
    let path = mode.shader_path();
    let mut look = ShaderLook::new(path).with_vertex_shader(path);
    set_param(&mut look, "morph_start", ParamValue::F32(morph_start));
    set_param(&mut look, "morph_end", ParamValue::F32(morph_end));
    set_param(&mut look, "reveal", ParamValue::F32(reveal_value(reveal)));
    match mode {
        TerrainShaderMode::DebugLod => {
            set_param(&mut look, "base_color", ParamValue::Vec3(lod_rgb(depth)))
        }
        TerrainShaderMode::Plain => {
            set_param(&mut look, "base_color", ParamValue::Vec3([0.35, 0.34, 0.32]))
        }
        TerrainShaderMode::Lit => {
            if let Some(maps) = maps {
                apply_maps_to_look(&mut look, maps, depth);
            }
            if let Some(shadow) = shadow {
                apply_shadow_cache_to_look(&mut look, shadow);
            }
            // Analysis overlay params (slope hazard). Only the Lit shaders declare
            // them; a fresh tile thus paints the current overlay with no extra pass.
            overlay.apply(&mut look);
        }
    }
    look
}

/// Spawn one LOD tile entity (mesh + its `ShaderLook` intent, anchored to its own
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
    morph_end: f32,
    reveal: u8,
    mode: TerrainShaderMode,
    maps: Option<&TerrainDerivedMaps>,
    shadow: Option<&TileShadowCache>,
    overlay: crate::overlay::OverlayUniforms,
    // Surface height at the tile centre — the SAME value the mesh was rebased by in
    // `bake_tile_mesh` (`origin_y`). Anchoring the tile's `CellCoord` here (not Y=0)
    // keeps the tile in the SAME big_space cell as the content standing on it, and its
    // rebased geometry local to that origin. On flat terrain this is ≈0 (unchanged).
    origin_y: f64,
) -> Entity {
    let (cell, local) = grid.translation_to_grid(DVec3::new(center[0], origin_y, center[1]));
    // Snap the selected band onto the bucket lattice so tiles with near-identical
    // parent ranges share one batched material (`morph_start` is derived from the
    // snapped end at the fixed 0.7 ratio).
    let (ms, me, _bucket) = snap_band(morph_end);
    commands
        .spawn((
            Mesh3d(mesh),
            tile_look(mode, depth, ms, me, reveal, maps, shadow, overlay),
            cell,
            Transform::from_translation(local),
            Visibility::Inherited,
            LodTileOf(terrain),
            Name::new(format!("LodTile d{} {},{}", coord.depth, coord.x, coord.z)),
            ChildOf(grid_entity),
            // Terrain tiles RECEIVE shadows (rovers/objects cast onto them) but must
            // NOT be shadow casters: the ~150-400 live tiles would otherwise be
            // re-rendered into all 4 sun cascades every frame (the dominant terrain
            // frame cost — ~16ms; the flat scene has no such geometry). Crater-rim
            // self-shadowing rides the sun horizon ray-march, not the cascade pass.
            // (`bevy::light` is render-FREE — this is not a `bevy_pbr` name.)
            bevy::light::NotShadowCaster,
            #[cfg(target_arch = "wasm32")]
            bevy::light::NotShadowReceiver,
        ))
        .id()
}

/// Per-frame: step each revealing tile through the quantised reveal lattice, then
/// settle it on the fully-revealed value.
///
/// The tile's [`ShaderLook`] is touched **only when it crosses a step boundary** —
/// so no asset is mutated, no uniform is re-uploaded, and the binder simply swaps in
/// the cached material for that step. (Getting this wrong is the R5 regression: an
/// unconditional `look.values` write would mark `Changed<ShaderLook>` every frame
/// for every revealing tile and drag the binder along with it.) See [`TileReveal`] /
/// [`REVEAL_SECS`].
pub(crate) fn animate_tile_reveal(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut TileReveal, &mut ShaderLook)>,
) {
    let dt = time.delta_secs();
    for (ent, mut rev, mut look) in &mut q {
        rev.elapsed += dt;
        let t = (rev.elapsed / REVEAL_SECS).clamp(0.0, 1.0);
        if t >= 1.0 {
            set_param(&mut look, "reveal", ParamValue::F32(reveal_value(REVEAL_FULL)));
            // A live re-bake (e.g. a brush/flatten edit or an obstacle-field
            // regen) can despawn this tile in the same frame, before these
            // deferred commands apply — `try_*` no-ops on a stale entity
            // instead of panicking the command buffer.
            commands.entity(ent).try_remove::<TileReveal>();
            continue;
        }
        // smoothstep ease so the settle starts/ends gently, quantised to the
        // shared step lattice.
        let s = t * t * (3.0 - 2.0 * t);
        let step = ((s * REVEAL_STEPS as f32) as u8).min(REVEAL_STEPS - 1);
        if step == rev.step {
            // NOT a step boundary: leave `ShaderLook` untouched so change detection
            // stays quiet (touching it here is the per-frame-mutation trap).
            continue;
        }
        rev.step = step;
        set_param(&mut look, "reveal", ParamValue::F32(reveal_value(step)));
    }
}

/// Cross-terrain tile-streaming progress, derived fresh each frame by
/// [`update_lod_tiles`]: how much of the WANTED tile set is actually on
/// screen. UI layers read this to show a "terrain streaming…" indicator —
/// without one, a freshly opened scene is a black void until the first bakes
/// land, which reads as a hang. Kept here (engine-side, UI-free) so the UI is
/// a pure derived read.
#[derive(Resource, Default, Clone, Copy)]
pub struct TerrainStreamStatus {
    /// Tiles the current selection wants on screen (all streaming terrains).
    pub wanted: usize,
    /// Wanted tiles with a resident mesh entity (stale-but-covering counts —
    /// the ground is visible, just not current).
    pub resident: usize,
    /// Off-thread bakes in flight.
    pub pending: usize,
}

/// Per-frame scratch for [`update_lod_tiles`] — the five collections the streaming
/// pass used to heap-allocate EVERY frame per terrain (material swaps, finished
/// bakes, the sort keys, the hole-cover set, the wanted set). Hoisted into a
/// `Local` so a moving camera reuses the capacity instead of re-allocating; the
/// idle-signature gate already skips the whole pass when nothing moved.
#[derive(Default)]
pub struct StreamScratch {
    swaps: Vec<(Entity, u32, f32)>,
    done: Vec<(QuadCoord, u32, BakedTile)>,
    keyed: Vec<(u8, u8, f64, Selected)>,
    missing: Vec<Square>,
    wanted: HashSet<QuadCoord>,
}

/// Per-frame: stream the LOD tile set for each streaming terrain against the camera.
pub fn update_lod_tiles(
    mut commands: Commands,
    // `Camera3d` lives in `bevy_core_pipeline` (→ bevy_render → wgpu). The
    // render-FREE `bevy_camera` equivalent for "a 3D scene camera" is a `Camera`
    // with a PERSPECTIVE `Projection` — which is also what excludes the egui host's
    // orthographic `Camera2d`. Same set of cameras as before, no GPU stack.
    cameras: Query<(&GlobalTransform, &Projection), With<Camera>>,
    // Dynamic bodies (rovers, payloads) are FORCED refinement foci: the
    // physics collider ring under them is fixed-resolution, so the ground they
    // stand on must be drawn at matching detail even when the camera-driven
    // selection would keep it coarse (far chase cam, budget-coarsened metric)
    // — otherwise wheels visibly hover on collider bumps the mesh doesn't show.
    bodies: Query<(&avian3d::prelude::RigidBody, &GlobalTransform)>,
    // The big_space world grid each tile anchors into (its own `CellCoord`).
    grids: Query<(Entity, &Grid), With<WorldGrid>>,
    mut terrains: Query<(
        Entity,
        &GlobalTransform,
        &DemHeightField,
        &TerrainLodViz,
        &mut LodTiles,
        &mut PendingTileBakes,
        &mut TerrainNodeErrors,
        Option<&TerrainShaderMode>,
        Option<&TerrainDerivedMaps>,
        Option<&TileShadowCache>,
    )>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mesh_cache: ResMut<LodMeshCache>,
    cfg: Res<TerrainLodConfig>,
    mut stream_status: ResMut<TerrainStreamStatus>,
    settings: Option<Res<lunco_settings::TerrainSettings>>,
    overlay_params: Res<crate::overlay::TerrainOverlayParams>,
    mut scratch: Local<StreamScratch>,
) {
    // Snapshot the analysis-overlay uniforms once; every tile built this frame paints
    // the current params (live re-tuning of resident tiles rides `sync_terrain_overlay`).
    let overlay = overlay_params.uniforms();
    *stream_status = TerrainStreamStatus::default();
    // Split-borrow the per-frame scratch buffers (see [`StreamScratch`]).
    let StreamScratch { swaps, done, keyed, missing, wanted } = &mut *scratch;
    let enable_shaders = settings.as_ref().map(|s| s.enable_shaders).unwrap_or(true);
    // Use the first 3D camera as the focus. (Multiple cameras → the viz follows
    // whichever; fine for a debug view.)
    let Some(cam) = cameras
        .iter()
        .find(|(_, p)| matches!(p, Projection::Perspective(_)))
        .map(|(gt, _)| gt)
    else {
        return;
    };
    let cam_pos = cam.translation();
    // No world grid yet → can't anchor tiles; skip this frame.
    let Ok((grid_entity, grid)) = grids.single() else { return };

    // Per-frame bake budget shared across all terrains (amortise scale changes).
    let mut bake_budget = cfg.bakes_per_frame.max(1);
    // Live streaming terrains — the mesh cache is GLOBAL (keyed by `(terrain,
    // coord)`), so its cap must scale with them or two terrains would fight over
    // one terrain's worth of entries and thrash each other every frame.
    let terrain_count = terrains.iter().count().max(1);

    for (terrain, t_gt, hf, viz, mut tiles, mut pending, mut errs, mode_opt, maps, shadow) in
        &mut terrains
    {
        let mut mode = mode_opt.copied().unwrap_or_else(TerrainShaderMode::platform_default);
        if !enable_shaders {
            mode = TerrainShaderMode::Plain;
        }
        // The terrain's current height generation: a tile/bake tagged with an older
        // gen is stale (a live re-bake changed the heights) and is replaced near-first.
        let cur_gen = tiles.gen;
        // Shader mode changed (inspector edit) → RESTATE the look of every live tile
        // in place (same geometry, new shader/colour) instead of despawning +
        // rebuilding, which left a one-frame black hole until the tiles re-baked.
        // The binder resolves each new look through its cache, so the swap costs one
        // material per (mode, band) — not one per tile.
        if tiles.mode != mode {
            swaps.clear();
            swaps.extend(
                tiles.tiles.iter().map(|(c, s)| (s.entity, c.depth as u32, s.morph_end)),
            );
            for &(ent, depth, morph_end) in swaps.iter() {
                // Each tile carries its own morph band; restate it under the new mode.
                let (ms, me, _) = snap_band(morph_end);
                let look = tile_look(mode, depth, ms, me, REVEAL_FULL, maps, shadow, overlay);
                // A tile mid-settle would otherwise keep stepping the OLD mode's
                // reveal values — end the settle instead; the mode swap is a rare,
                // explicit Inspector action.
                commands.entity(ent).try_insert(look).try_remove::<TileReveal>();
            }
            tiles.mode = mode;
        }

        // Camera in the terrain's local frame (the DEM frame, origin-centred).
        let inv_t = t_gt.affine().inverse();
        let local = inv_t.transform_point3(cam_pos);
        let focus = [local.x as f64, local.z as f64];

        let oracle = &hf.0;
        let h = oracle.half_extent() as f64;
        // Eye height = camera height ABOVE the terrain surface directly below it.
        // Feeding this to the 3D metric means looking down from altitude coarsens
        // the ground below (true distance), instead of refining it as XZ-only would.
        let ground = oracle.height_at(local.x as f64, local.z as f64);
        let eye_height = (local.y as f64 - ground).max(0.0);

        // ── Idle-camera fast path ────────────────────────────────────────────
        // The selection below (quadtree walks + budget-coarsen loop + sort + queue
        // + retain) is a pure function of (focus, eye height, dynamic-body
        // footprints, generation, oracle identity, LOD knobs). Re-deriving it EVERY
        // frame with a still camera was the dominant idle terrain CPU cost
        // (obs 23593). Fold those inputs into a signature; when it matches last
        // frame's AND no bake is mid-flight (nothing to finalize/spawn), last
        // frame's resident tiles are already correct — skip the whole body.
        // Quantise focus/eye so sub-tile jitter doesn't defeat the gate; a slow
        // creep re-runs the frame it crosses a quantum (a 1-frame-late reselection
        // is invisible — tiles morph). Rovers moving continuously keep their tiles
        // refining because their footprint enters the signature.
        {
            const IDLE_QUANT_M: f64 = 0.5;
            let q = |v: f64| (v / IDLE_QUANT_M).round() as i64 as u64;
            let mut sig = lunco_precompute::Fnv1a::new();
            sig.write_u64(q(focus[0]));
            sig.write_u64(q(focus[1]));
            sig.write_u64(q(eye_height));
            sig.write_u64(cur_gen as u64);
            // Oracle identity — a live re-compose swaps the Arc without always
            // bumping gen, and would otherwise be missed by the gate.
            sig.write_u64(Arc::as_ptr(&hf.0) as *const () as usize as u64);
            // LOD knobs (Inspector) — a live tweak must re-select.
            sig.write_u64(cfg.pixel_error.to_bits());
            sig.write_u64(cfg.tile_budget as u64);
            sig.write_u64(cfg.max_depth as u64);
            // Dynamic-body footprints (rovers): their forced max-depth refinement
            // follows them, so a moving body must re-select.
            for (rb, gt) in &bodies {
                if !matches!(rb, avian3d::prelude::RigidBody::Dynamic) {
                    continue;
                }
                let lb = inv_t.transform_point3(gt.translation());
                if (lb.x as f64).abs() > h || (lb.z as f64).abs() > h {
                    continue; // off this DEM — doesn't affect its selection
                }
                sig.write_u64(q(lb.x as f64));
                sig.write_u64(q(lb.z as f64));
            }
            let sig = sig.finish();
            if pending.0.is_empty() && tiles.last_sig == Some(sig) {
                // Idle: resident tiles already match. Contribute this terrain's
                // resident count so the status bar still reads "done", not "0/0".
                stream_status.wanted += tiles.tiles.len();
                stream_status.resident += tiles.tiles.len();
                continue;
            }
            tiles.last_sig = Some(sig);
        }
        // Runtime LOD knobs (Inspector) drive detail-vs-cost live; tile_res stays
        // per-terrain (changing it would invalidate the mesh cache). The range
        // factor derives from the CANONICAL screen metric (fixed viewport + the
        // pixel_error knob) so selection stays view-independent + peer-identical.
        let quadtree_for = |px: f64| {
            Quadtree::from_screen_metric(
                h,
                cfg.max_depth.max(1),
                h,
                CANON_SCREEN_H_PX,
                CANON_FOV_Y_RAD,
                px,
            )
        };
        let base_px = cfg.pixel_error.clamp(0.5, 32.0);
        // Warm-start the budget fit (see [`LodTiles::last_fit_px`]): resume at the
        // value that satisfied the budget last time, clamped to the configured base
        // so quality can climb back when the load drops.
        //
        // HYSTERESIS: the old warm start began one rung FINER (`px / 1.6`) and then
        // coarsened back, so near the budget boundary it alternated between two
        // `pixel_error` rungs every frame — which moves `morph_end` → moves
        // `band_bucket` → swapped EVERY tile's material on alternating frames. Now
        // we only COARSEN when over budget, and only try one refine step when the
        // selection is comfortably (15%) under it — and keep the refinement only if
        // it still fits. Both ends are one-way, so the fit settles.
        let mut pixel_error = tiles.last_fit_px.unwrap_or(base_px).max(base_px);
        let mut qt = quadtree_for(pixel_error);
        // ERROR-DRIVEN selection: refine where the MEASURED surface error says
        // there is detail worth refining toward (crater rims, peaks), not on the
        // uniform per-depth schedule. Errors are memoized per node against the
        // current oracle; the cache wipes when the oracle is swapped (live edit).
        let oracle_ptr = Arc::as_ptr(&hf.0) as usize;
        if errs.oracle_ptr != oracle_ptr {
            errs.map.clear();
            errs.oracle_ptr = oracle_ptr;
        }
        let err_map = std::cell::RefCell::new(&mut errs.map);
        let src: &SurfaceOracle = hf.0.as_ref();
        let node_error = |c: QuadCoord, region: Square| -> f64 {
            if let Some(&e) = err_map.borrow().get(&c) {
                return e;
            }
            // Gate over-zoom synthesis at the probe's own spacing: sub-probe
            // detail can't inform THIS node's refinement (it surfaces at deeper
            // nodes, whose finer probes see it) — and it keeps coarse-node
            // probes cheap.
            let probe_step = region.side() / (NODE_ERROR_PROBE_RES - 1) as f64;
            let limited = src.detail_limited(probe_step);
            let e = measure_node_error(&limited, region, NODE_ERROR_PROBE_RES);
            err_map.borrow_mut().insert(c, e);
            e
        };
        // Fit the tile budget by COARSENING THE METRIC, not by capping the walk.
        // A hard cap (`select_with_error_budgeted`) stops refinement at a
        // budget-determined radius while every tile's geomorph band still assumes
        // the UNBUDGETED refine distances — so detail ended in a hard line (the
        // morph blend never ran) instead of fading. Raising pixel_error re-derives
        // the range factor, so the transition distance and the morph band move
        // TOGETHER and the LOD edge stays a blend. Node errors are memoized, so
        // the re-walks are cheap; the loop is bounded by the 32 px clamp.
        let budget = cfg.tile_budget.max(16);
        // The previous cover drives the refine HYSTERESIS band (see `LodTiles::prev_sel`).
        let prev_sel = std::mem::take(&mut tiles.prev_sel);
        let mut sel = qt.select_with_error(focus, eye_height, &node_error, &prev_sel);
        if sel.len() > budget {
            // Over budget → coarsen until it fits.
            while sel.len() > budget && pixel_error < 32.0 {
                pixel_error = (pixel_error * 1.6).min(32.0);
                qt = quadtree_for(pixel_error);
                sel = qt.select_with_error(focus, eye_height, &node_error, &prev_sel);
            }
        } else if pixel_error > base_px && sel.len() * 100 < budget * 85 {
            // Comfortably under budget → try ONE refine step back toward the
            // configured quality; keep it only if it still fits.
            let finer = (pixel_error / 1.6).max(base_px);
            let qt_finer = quadtree_for(finer);
            let sel_finer = qt_finer.select_with_error(focus, eye_height, &node_error, &prev_sel);
            if sel_finer.len() <= budget {
                pixel_error = finer;
                qt = qt_finer;
                sel = sel_finer;
            }
        }
        tiles.last_fit_px = Some(pixel_error);
        // Ground under dynamic bodies is ALWAYS drawn at max depth — the
        // fixed-resolution collider ring under a rover carries small-crater
        // relief a coarse camera-driven tile doesn't, and the rover visibly
        // hovers on the undrawn bumps. A handful of forced splits per body,
        // outside the budget (the budget bounds the broad view, not this).
        for (rb, gt) in &bodies {
            if !matches!(rb, avian3d::prelude::RigidBody::Dynamic) {
                continue;
            }
            let local_b = inv_t.transform_point3(gt.translation());
            let (bx, bz) = (local_b.x as f64, local_b.z as f64);
            if bx.abs() > h || bz.abs() > h {
                continue; // off this DEM
            }
            qt.refine_selection_at(&mut sel, [bx, bz], &node_error);
        }
        wanted.clear();
        wanted.extend(sel.iter().map(|s| s.coord));
        // Remember this cover as the next selection's hysteresis memory. Stored AFTER
        // the body-forced splits so a node force-refined under a rover is also held
        // through the band instead of flipping back the moment the rover drifts.
        tiles.prev_sel.clear();
        tiles.prev_sel.extend(wanted.iter().copied());

        // Intelligent baking, two phases:
        //
        // 1. CARPET — the selection's coarsest tiles first (a depth-N tile costs
        //    the same 49² samples as a leaf but covers 4^(maxdepth−N)× the
        //    area), so the whole view is covered by a low-res carpet within the
        //    first few frames instead of staying BLACK.
        // 2. BENEFIT — everything finer ordered by distance measured in units
        //    of the tile's own size (≈ inverse screen-space error): the leaf at
        //    the camera's feet outranks a mid-depth ring 800 m out. Strict
        //    depth-major order here was wrong on open: every mid-depth tile
        //    across the whole 1.5 km view baked before the leaves under the
        //    camera — the user watched FAR detail land while the near ground
        //    lagged coarse.
        //
        // The reveal morph settles children onto the parent lattice, so the
        // out-of-depth-order coarse→fine handoff never pops.
        const CARPET_DEPTH: u8 = 2;
        let dist2 = |s: &Selected| -> f64 {
            (s.region.center[0] - focus[0]).powi(2) + (s.region.center[1] - focus[1]).powi(2)
        };
        // Camera heading in the terrain's local XZ, for view-direction weighting:
        // of two tiles with equal screen-space benefit, the one AHEAD of the
        // camera bakes first — the one behind isn't on screen. `None` when
        // looking straight down (no meaningful heading; weight disabled).
        let heading = {
            let f = t_gt.affine().inverse().transform_vector3(cam.forward().as_vec3());
            let v = bevy::math::DVec2::new(f.x as f64, f.z as f64);
            (v.length() > 1e-3).then(|| v.normalize())
        };
        let benefit = |s: &Selected| -> f64 {
            let size = s.region.half * 2.0;
            let base = dist2(s) / (size * size).max(1e-9);
            let Some(hd) = heading else { return base };
            let to = bevy::math::DVec2::new(
                s.region.center[0] - focus[0],
                s.region.center[1] - focus[1],
            );
            let d = to.length();
            if d < 1e-6 {
                return base;
            }
            // cos 1 (dead ahead) → ×1 … cos −1 (behind) → ×4.
            base * (2.5 - 1.5 * (to / d).dot(hd))
        };
        // Decorate-sort-undecorate: `dist2`/`benefit` pay a sqrt/dot per call and
        // the comparator re-ran them on BOTH sides of every comparison
        // (O(n log n) evaluations) — compute each tile's key once, sort on the
        // cached key. Carpet keys order by (0, depth, dist²); benefit keys by
        // (1, 0, benefit) — the same total order the comparator produced.
        keyed.clear();
        keyed.extend(sel.drain(..).map(|s| {
            if s.coord.depth <= CARPET_DEPTH {
                (0, s.coord.depth, dist2(&s), s)
            } else {
                (1, 0, benefit(&s), s)
            }
        }));
        keyed.sort_by(|a, b| {
            (a.0, a.1)
                .cmp(&(b.0, b.1))
                .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
        });
        sel.extend(keyed.drain(..).map(|(_, _, _, s)| s));

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
        done.clear();
        pending.0.retain(|coord, (gen, task)| match block_on(future::poll_once(&mut *task)) {
            Some(baked) => {
                done.push((*coord, *gen, baked));
                false
            }
            None => true,
        });
        for (coord, gen, baked) in done.drain(..) {
            // A bake from a superseded generation (heights changed while it ran) is
            // discarded — its mesh would show the OLD terrain.
            if gen != cur_gen {
                continue;
            }
            let handle = meshes.add(baked.mesh);
            let oy = baked.origin_y;
            mesh_cache.0.insert((terrain, coord), (handle.clone(), oy));
            // No longer selected while it baked → keep the cached mesh, skip spawning.
            if !wanted.contains(&coord) {
                continue;
            }
            let depth = baked.depth;
            // Sub-root tiles settle in from their parent's coarse lattice (no pop):
            // they are BORN at reveal step 0 and `animate_tile_reveal` walks them up.
            // The root has no coarser parent to grow from → born fully revealed.
            let reveal = if depth > 0 { 0 } else { REVEAL_FULL };
            // `oy` (baked with the mesh) anchors the tile's cell to its geometry — see
            // `spawn_tile`/`bake_tile_mesh` `origin_y`. Using the baked value (not a
            // spawn-time recompute) keeps mesh and placement in lock-step across gens.
            let ent = spawn_tile(
                &mut commands, grid, grid_entity, terrain, coord, handle, baked.center, depth,
                baked.morph_end, reveal, mode, maps, shadow, overlay, oy,
            );
            if depth > 0 {
                commands.entity(ent).try_insert(TileReveal { elapsed: 0.0, step: 0 });
            }
            // Replace any stale slot at this coord, despawning the tile it held.
            if let Some(old) = tiles.tiles.insert(
                coord,
                TileSlot { entity: ent, gen: cur_gen, morph_end: baked.morph_end },
            ) {
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
            // Per-node morph band: finite for sub-root nodes, "never" for the root
            // (the sentinel `snap_band` maps to the no-morph bucket).
            let morph_end = if s.morph_end.is_finite() { s.morph_end as f32 } else { 1.0e21 };
            if let Some((cached, oy)) = mesh_cache.0.get(&(terrain, s.coord)) {
                let reveal = if depth > 0 { 0 } else { REVEAL_FULL };
                // Placed at the mesh's OWN baked `origin_y` (stored beside it), never a
                // recompute — otherwise a cache hit against a since-composed oracle jumps.
                let ent = spawn_tile(
                    &mut commands, grid, grid_entity, terrain, s.coord, cached.clone(),
                    s.region.center, depth, morph_end, reveal, mode, maps, shadow, overlay, *oy,
                );
                if depth > 0 {
                    commands.entity(ent).try_insert(TileReveal { elapsed: 0.0, step: 0 });
                }
                if let Some(old) = tiles
                    .tiles
                    .insert(s.coord, TileSlot { entity: ent, gen: cur_gen, morph_end })
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
            let oracle_arc = hf.0.clone();
            let coord = s.coord;
            let region = s.region;
            let tile_res = viz.tile_res;
            let half = h;
            let center = s.region.center;
            let task = pool.spawn(async move {
                // Off-thread body → invisible to Bevy's per-system spans; give
                // Tracy (`--features tracy`) its own zone.
                let _span = bevy::log::info_span!("terrain_tile_bake").entered();
                // Content-addressed bake: a warm reload of the same composed
                // surface streams this tile from the `cache://` dir; a miss
                // samples the oracle (over-zoom Nyquist-gated at this tile's
                // vertex spacing inside the bake) and persists for next time.
                //
                // TODO(R1): on wasm this cache ALWAYS misses — every tile, every
                // session. `bake_tile_mesh_cached` → `lunco_precompute::bake_or_load`
                // → `lunco_precompute::{load_blob, store_blob}`, both hard no-ops on
                // `target_arch = "wasm32"` (lunco-precompute/src/lib.rs). The seam to
                // wire is `lunco_storage::opfs_blob::{read, write}` (already used for
                // the DEM grid blob in `terrain.rs`), which is async — so
                // `bake_or_load` needs an async twin. Owner: lunco-precompute.
                let tm = crate::tile_cache::bake_tile_mesh_cached(
                    oracle_arc.as_ref(), coord, region, tile_res, half, center,
                );
                // RENDER_WORLD only: nothing reads a tile mesh's CPU vertex data back
                // (physics rides the collider ring, picking rides the oracle), so the
                // ~160 KB CPU copy per tile — ~164 MB across a full cache, doubled
                // against VRAM — was pure waste. (The STATIC terrain mesh keeps
                // `default()`: the horizon bake reads it back.)
                let mut mesh = grid_mesh(
                    tm.positions,
                    tm.normals,
                    tm.uvs,
                    tm.indices,
                    bevy::asset::RenderAssetUsages::RENDER_WORLD,
                );
                mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, tm.morph_targets);
                // The SAME anchor `bake_tile_mesh_cached` rebased the mesh Y by (full
                // oracle at the tile centre). Carried on `BakedTile` so the main thread
                // places the tile at exactly the height its mesh was baked for.
                let origin_y = oracle_arc.height_at(center[0], center[1]);
                BakedTile { mesh, center, depth, morph_end, origin_y }
            });
            pending.0.insert(s.coord, (cur_gen, task));
        }

        // Despawn no-longer-wanted (or stale, replaced) tiles, but KEEP one while it
        // still covers a wanted region that has no fresh tile yet — otherwise the
        // despawn opens a hole showing black sky ("black squares"). This is also what
        // makes a live re-bake progressive: on a generation bump every tile goes
        // stale at once, all keep covering the surface, and each is reaped only when
        // its current-gen replacement bakes in (near-camera-first).
        missing.clear();
        missing.extend(sel.iter().filter(|s| !fresh_tile(&tiles, &s.coord)).map(|s| s.region));
        tiles.tiles.retain(|coord, slot| {
            // Keep ANY tile whose coord is still wanted — O(1). A fresh one is final;
            // a stale one covers the surface until its same-coord replacement bakes in
            // (the spawn paths despawn the slot they replace). This O(1) keep is what
            // stops a full-generation invalidation from going O(stale × missing).
            if wanted.contains(coord) {
                return true;
            }
            // Not wanted (camera moved off it): hold only while it plugs a not-yet-
            // baked hole. This overlap runs ONLY for the small trailing-edge set.
            let region = qt.region(*coord);
            let covers_hole = missing.iter().any(|m| squares_overlap(region, *m));
            if !covers_hole {
                commands.entity(slot.entity).try_despawn();
            }
            covers_hole
        });

        // Streaming progress for the UI indicator (accumulated across terrains).
        stream_status.wanted += wanted.len();
        stream_status.resident += wanted.iter().filter(|c| tiles.tiles.contains_key(c)).count();
        stream_status.pending += pending.0.len();

        // Bound the mesh cache: when it grows past the cap, drop THIS terrain's
        // non-resident meshes (deterministic geometry → they re-bake on demand).
        // Other terrains' entries are left untouched — the cap is a soft memory
        // bound, and dropping a terrain we're not currently processing would just
        // force it to re-bake next frame.
        //
        // The cap is GLOBAL but the cache is keyed by `(terrain, coord)`, so it
        // scales with the live terrain count: a flat `CACHE_CAP` meant that with two
        // terrains (or with entries left behind by a DEAD one — now evicted in
        // `despawn_orphaned_lod_tiles`) `len() > CACHE_CAP` was true EVERY frame, and
        // the live terrain's non-resident meshes were trimmed every frame — the tile
        // cache permanently defeated, every trailing-edge tile re-baking on demand.
        if mesh_cache.0.len() > CACHE_CAP * terrain_count {
            let resident: HashSet<QuadCoord> = tiles.tiles.keys().copied().collect();
            mesh_cache.0.retain(|(e, c), _| *e != terrain || resident.contains(c));
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
///
/// Change-driven: a tile only orphans when its owner loses [`TerrainLodViz`]
/// (component removal or terrain despawn — both emit the removal event), so the
/// per-frame every-tile ownership poll is skipped until one fires. The liveness
/// re-check keeps the exact old semantics for a remove-and-re-add in one frame.
pub fn despawn_orphaned_lod_tiles(
    mut commands: Commands,
    mut removed: RemovedComponents<TerrainLodViz>,
    tiles: Query<(Entity, &LodTileOf)>,
    streaming: Query<(), With<TerrainLodViz>>,
    mut mesh_cache: ResMut<LodMeshCache>,
) {
    let orphaned: HashSet<Entity> = removed.read().collect();
    if orphaned.is_empty() {
        return;
    }
    for (ent, owner) in &tiles {
        if orphaned.contains(&owner.0) && streaming.get(owner.0).is_err() {
            commands.entity(ent).try_despawn();
        }
    }
    let dead = |t: &Entity| orphaned.contains(t) && streaming.get(*t).is_err();
    // (The dead terrain's cached MATERIALS — which pin its derived-map images,
    // megabytes of GPU texture — are no longer this crate's problem: the tiles hold
    // the only `ShaderLook`s that key them, so `lunco-render-bevy`'s binder cache
    // sweep drops them once they are unreferenced. This crate owns no material.)
    //
    // Its cached MESHES still are. Nothing else evicts them: the cap-trim in
    // `update_lod_tiles` only ever touches the terrain it is currently processing,
    // so up to `CACHE_CAP` strong `Handle<Mesh>` per dead terrain (≈160 KB each ⇒
    // ~164 MB) leaked FOREVER across every twin reload / scene swap — and once the
    // dead entries alone exceeded the cap, they also defeated the live terrain's
    // cache every frame.
    mesh_cache.0.retain(|(t, _), _| !dead(t));
}

/// When a terrain's derived maps finish baking AFTER its tiles exist (the
/// common case — the AO march takes seconds while the first tiles stream in),
/// restate the maps + per-depth weights on every resident Lit tile's look — no tile
/// churn, no re-bake, and the binder collapses them back onto one material per
/// depth. `Changed` also covers the re-bake that follows a live edit.
///
/// D8: **Lit tiles only.** The flat/debug shader declares no map bindings, so
/// writing them there would only mint pointless material variants.
pub(crate) fn bind_derived_maps_to_tiles(
    changed: Query<
        (&TerrainDerivedMaps, &LodTiles),
        (Changed<TerrainDerivedMaps>, With<TerrainLodViz>),
    >,
    mut looks: Query<&mut ShaderLook>,
) {
    for (maps, tiles) in &changed {
        if tiles.mode != TerrainShaderMode::Lit {
            continue;
        }
        for (depth, entity) in tiles.tiles_with_depth() {
            if let Ok(mut look) = looks.get_mut(entity) {
                apply_maps_to_look(&mut look, maps, depth);
            }
        }
    }
}

/// Same late-bind for the far-shadow cache: the horizon/shadow-cache bake (and
/// every sun-driven re-bake) lands long after tiles exist — restate it on the
/// resident Lit tiles' looks.
pub(crate) fn bind_shadow_cache_to_tiles(
    changed: Query<(&TileShadowCache, &LodTiles), (Changed<TileShadowCache>, With<TerrainLodViz>)>,
    mut looks: Query<&mut ShaderLook>,
) {
    for (cache, tiles) in &changed {
        if tiles.mode != TerrainShaderMode::Lit {
            continue;
        }
        for entity in tiles.tile_entities() {
            if let Ok(mut look) = looks.get_mut(entity) {
                apply_shadow_cache_to_look(&mut look, cache);
            }
        }
    }
}
