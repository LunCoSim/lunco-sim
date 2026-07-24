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
//! band, overlay uniforms, per-depth map weights) and its texture layers — and
//! **nothing in this crate names a material**. `lunco-render-bevy` binds it,
//! caching by `ShaderLook::key()`, so every tile in the same mode and LOD band
//! shares ONE material and ONE bind group. That cache *is* the old hand-rolled
//! `LodMaterials`/`MatKey` table, done generically: the `(mode, depth, band
//! bucket)` that `MatKey` encoded is simply the look's own content now.
//!
//! Keep the key COARSE. It is what lets tiles batch, so any per-tile parameter
//! added here mints a material per tile and costs draw calls — removing the old
//! per-tile reveal step from this key roughly halved frame time on moonbase
//! (33.8 -> 79.2 FPS). See `docs/architecture/render-decoupling.md`.
//!
//! The companion canonical-res collider ring is [`crate::collider_ring`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use big_space::prelude::{CellCoord, Grid};
use lunco_core::{on_command, register_commands, Command, WorldGrid};
use lunco_materials::{
    ParamValue, ShaderLook, TextureLayer, ATTRIBUTE_MORPH_NORMAL, ATTRIBUTE_MORPH_TARGET,
};
use lunco_obstacle_field::grid_mesh;
use lunco_terrain_core::{measure_node_error, HeightSource, REFINE_HYSTERESIS};

use crate::derived_layers::{TerrainAuthoredMaps, TerrainDerivedMaps};
use crate::oracle::SurfaceOracle;
use crate::quadtree::{QuadCoord, Quadtree, Selected, Square};

/// Vertices per tile side (so each tile is `TILE_RES²` verts). 49 → 48² quads.
/// Higher = finer geometry per tile (smoother crater rims / slopes, fewer visible
/// triangle "lines") at the same tile count — cheap on a modern GPU.
const TILE_RES: usize = 49;

/// Mesh resolution of the single tile a [`LodFrozen`] terrain draws — verts per side.
///
/// 2049² ≈ 4.2M verts / 8.4M triangles in ONE draw call: over the moonbase's ~1 km
/// window, ~0.5 m between vertices.
///
/// Sized to the CLOSEST the shot gets, not to the window. At 1025 (~1 m spacing) a
/// wide establishing orbit was fine, but a 90 m pass at 35 m altitude reads that
/// spacing as faceting — one tile has to carry, everywhere, the detail the quadtree
/// used to spend only where the camera was.
///
/// Why not finer: 4097² is 16.8M verts (~1 GB with indices) and a ~20 s bake, for
/// 0.25 m spacing the surface cannot fill — the DEM is ~2 m/px, and below that only
/// the analytic crater/overzoom modifiers have anything to add. 0.5 m is about where
/// sampling stops buying real detail.
const CINEMATIC_TILE_RES: usize = 2049;
/// Deepest LOD the viz refines to. Bounds the tile count near the camera. With
/// error-driven selection only feature tiles (rims, peaks) actually reach it, so
/// 8 (≈0.65 m vertex pitch on a ±4 km DEM) stays cheap while crater rims resolve.
/// Going deeper cannot sharpen craterlets: [`Overzoom`]'s Nyquist gate is already
/// at full amplitude by depth 8 for any authored `lunco:layer:maxFeature` — where
/// detail dies is a property of the layer parameters, not of this cap.
const MAX_DEPTH: u8 = 8;
/// On-screen error (px, at the canonical viewport) at which a node refines —
/// the ONE detail-vs-cost knob of the error-driven metric. Smaller = finer, and
/// refine ranges scale as its inverse, so it also sets how far out craters
/// resolve before the camera reaches them. Cost is bounded by `tile_budget` +
/// nearest-first splits.
const TARGET_PIXEL_ERROR: f64 = 2.0;
/// Canonical viewport for the screen metric (fixed → selection is independent of
/// any client's real resolution/FOV; peers select identically).
const CANON_SCREEN_H_PX: f64 = 1080.0;
const CANON_FOV_Y_RAD: f64 = std::f64::consts::FRAC_PI_4; // 45°
/// Probe mesh resolution for [`measure_node_error`] — coarse on purpose: the
/// measurement senses "is there detail here worth refining toward," it does not
/// need the tile's full 49² fidelity. ~657 oracle samples per (memoized) node.
const NODE_ERROR_PROBE_RES: usize = 9;

/// Marks a perspective camera as a visual terrain-detail authority.
///
/// Every active marked camera contributes one stable, camera-centred detail cover.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct TerrainVisualFocus {
    /// Optional per-camera override for the radius around the camera guaranteed
    /// to use the finest visual geometry. `None` uses
    /// [`lunco_settings::TerrainSettings::visual_detail_radius_m`].
    ///
    /// This covers nearby subjects (for example a third-person rover) without
    /// making physics bodies visual-LOD authorities.
    pub near_detail_radius_m: Option<f64>,
    /// Optional per-camera override for the extra distance a previously refined
    /// near-field tile remains selected after the camera moves away. `None` uses
    /// [`lunco_settings::TerrainSettings::visual_detail_hysteresis_m`].
    pub near_detail_hysteresis_m: Option<f64>,
}

impl Default for TerrainVisualFocus {
    fn default() -> Self {
        Self {
            near_detail_radius_m: None,
            near_detail_hysteresis_m: None,
        }
    }
}

#[derive(Clone, Copy)]
struct VisualDemand {
    position: DVec3,
    forward: DVec3,
    near_detail_radius_m: f64,
    near_detail_hysteresis_m: f64,
}

#[derive(Clone, Copy)]
struct TerrainVisualDemand {
    focus: [f64; 2],
    eye_height: f64,
    heading: Option<bevy::math::DVec2>,
    /// The camera's actual ground position.
    required: bool,
    near_detail_radius_m: f64,
    near_detail_hysteresis_m: f64,
}

/// Frame-local detail demands collected once from explicit ECS focus markers.
#[derive(Resource, Default)]
pub struct TerrainDetailDemands {
    visual: Vec<VisualDemand>,
}

/// Give every perspective scene camera the standard visual-detail policy.
pub(crate) fn mark_terrain_visual_foci(
    mut commands: Commands,
    cameras: Query<(Entity, &Projection), (Added<Camera>, Without<TerrainVisualFocus>)>,
) {
    for (entity, projection) in &cameras {
        if matches!(projection, Projection::Perspective(_)) {
            commands
                .entity(entity)
                .try_insert(TerrainVisualFocus::default());
        }
    }
}

/// Collect active visual cameras for the visual streamer.
pub(crate) fn collect_terrain_detail_demands(
    mut demands: ResMut<TerrainDetailDemands>,
    terrain_settings: Option<Res<lunco_settings::TerrainSettings>>,
    cameras: Query<(
        Entity,
        &Camera,
        &Projection,
        &TerrainVisualFocus,
        Has<Camera3d>,
    )>,
    parents: Query<&ChildOf>,
    grids: Query<&Grid>,
    spatial: Query<(Option<&CellCoord>, &Transform)>,
) {
    let defaults = terrain_settings.as_deref().cloned().unwrap_or_default();
    demands.visual.clear();
    demands.visual.extend(
        cameras
            .iter()
            .filter(|(_, camera, projection, _, has_pipeline)| {
                // `Camera::is_active` alone is insufficient: USD and avatar
                // setup briefly create bare Camera components before their 3D
                // render graph exists. They emit Bevy's "no render graph"
                // warning and cannot affect the image, so letting them steer
                // terrain detail creates an invisible competing focus.
                *has_pipeline
                    && camera.is_active
                    && matches!(projection, Projection::Perspective(_))
            })
            .filter_map(|(entity, _, _, focus, _)| {
                let position =
                    lunco_core::coords::grid_absolute(entity, &parents, &grids, &spatial)?;
                let (_, transform) = spatial.get(entity).ok()?;
                Some(VisualDemand {
                    position,
                    forward: (transform.rotation * Vec3::NEG_Z).as_dvec3(),
                    near_detail_radius_m: focus
                        .near_detail_radius_m
                        .unwrap_or(defaults.visual_detail_radius_m)
                        .max(0.0),
                    near_detail_hysteresis_m: focus
                        .near_detail_hysteresis_m
                        .unwrap_or(defaults.visual_detail_hysteresis_m)
                        .max(0.0),
                })
            }),
    );
    demands.visual.sort_by(|a, b| {
        a.position
            .x
            .total_cmp(&b.position.x)
            .then(a.position.y.total_cmp(&b.position.y))
            .then(a.position.z.total_cmp(&b.position.z))
    });
}

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
        TerrainLodViz {
            max_depth: MAX_DEPTH,
            tile_res: TILE_RES,
        }
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
    /// Whether this tile is currently VISIBLE. A tile can be resident but hidden:
    /// the coarse base is always resident, and only draws where the finer tile
    /// that should cover its area is not ready yet. Tracked so the `Visibility`
    /// command is issued on a flip, not every frame.
    drawn: bool,
    /// A new entity stays hidden for one complete ECS turn so the render binder
    /// can attach its shader material before the draw partition exposes it.
    ready: bool,
}

/// Render extraction can still reference an entity for the frame in which main
/// world visibility changes. Hide retired tiles first, then despawn after two
/// complete updates so Bevy's render-world slab never observes a freed entity.
#[derive(Component)]
pub(crate) struct RetiringTerrainTile {
    updates_left: u8,
}

fn retire_tile(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .try_insert((Visibility::Hidden, RetiringTerrainTile { updates_left: 2 }));
}

pub(crate) fn retire_terrain_tiles(
    mut commands: Commands,
    mut retiring: Query<(Entity, &mut RetiringTerrainTile)>,
) {
    for (entity, mut retirement) in &mut retiring {
        if retirement.updates_left == 0 {
            commands.entity(entity).try_despawn();
        } else {
            retirement.updates_left -= 1;
        }
    }
}

/// Build the DRAW partition: which tiles actually render this frame.
///
/// For each selected node, the deepest node in its ancestor chain (itself first) that is
/// READY — this is Cesium `ForbidHoles` / MSFS "best currently available data": when a fine
/// tile has not baked yet, its parent draws **instead of** it. Then any node an ancestor
/// already covers is dropped, so the result is DISJOINT — exactly one tile per point, which
/// is what keeps a coarse stand-in from z-fighting through the fine surface it replaces.
///
/// Pure and separated from the streaming system on purpose: this is the invariant the whole
/// design rests on ("never a hole, never an overlap"), it was previously three copies of the
/// same ancestor walk inline in a 700-line system, and a bug in it renders as terrain
/// flicker rather than as anything a type error would catch.
///
/// `scratch` is caller-owned to keep this allocation-free on the hot path.
/// The one always-resident fallback. It covers the complete terrain from the
/// first drawable frame; all non-root bakes belong to the actual camera-selected
/// cover. Keeping a second, late-arriving hierarchy of coarse ancestors caused
/// the exact fine → coarse → fine regression this stream is designed to prevent.
fn fallback_coords() -> impl Iterator<Item = QuadCoord> {
    std::iter::once(QuadCoord::ROOT)
}

/// Choose what to draw for each selected node: itself if ready, else its deepest
/// ready ancestor.
///
/// `is_drawable` is **residency, not freshness** — and that distinction is the
/// whole fix for the terrain blanking.
///
/// `invalidate()` bumps the generation and KEEPS every tile (see its docs): the
/// tiles are marked stale, not removed, and each is still geometrically valid —
/// same coord, same region, mesh baked with its own `origin_y`. Only the heights
/// are a generation old. Keying this walk on freshness therefore hid every tile in
/// the same frame on any invalidation, caught live as
/// `uncovered=265 drawn=0 resident=510`: five hundred good tiles in memory, none
/// drawn, terrain to clear colour until the re-bakes landed.
///
/// A stale tile keeps drawing ITSELF until its own replacement is ready, and
/// `tiles.insert` then swaps that replacement in at the same coord and despawns the
/// old entity — an atomic per-coord hand-off. Freshness still drives BAKING (a
/// stale tile is re-baked); it just no longer decides visibility.
///
/// NOT to be confused with the earlier rejected attempt, which fell back to a stale
/// *ancestor* when the exact node was stale: that made tiles alternate between a
/// stale deep tile and a fresh coarse one as re-bakes landed, and looked worse.
/// Substituting a DIFFERENT node is the thing that fails; keeping the SAME node on
/// screen until it is replaced is what works.
/// Cover edits (splits + merges) applied per frame. Bounds churn: the cover is
/// PERSISTENT state now, so a frame changes a handful of nodes instead of
/// re-deriving hundreds. High enough that a fast camera converges in a few frames.
const MAX_COVER_EDITS: usize = 64;

/// The up-to-4 same-depth edge-neighbour coords of `c` (clipped at the root square).
fn edge_neighbours(c: QuadCoord) -> impl Iterator<Item = QuadCoord> {
    let side = 1i64 << c.depth;
    [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)]
        .into_iter()
        .filter_map(move |(dx, dz)| {
            let x = c.x as i64 + dx;
            let z = c.z as i64 + dz;
            (x >= 0 && z >= 0 && x < side && z < side).then(|| QuadCoord {
                depth: c.depth,
                x: x as u32,
                z: z as u32,
            })
        })
}

/// Depth at which the cover covers `q`'s region from ABOVE: `Some(d ≤ q.depth)`
/// if `q` itself or an ancestor is in the cover, `None` if the region is covered
/// by strictly deeper nodes. Exactness (every point under exactly one cover node)
/// is what makes these three cases exhaustive.
fn covering_depth(cover: &HashSet<QuadCoord>, mut q: QuadCoord) -> Option<u8> {
    loop {
        if cover.contains(&q) {
            return Some(q.depth);
        }
        q = q.parent()?;
    }
}

/// Would replacing `p`'s four children by `p` break the 2:1 edge restriction?
/// True iff some region edge-adjacent to `p` is covered at depth ≥ `p.depth + 2`.
/// Tested on the depth-`p.depth+1` rim coords just outside `p`: by exactness,
/// "neither the coord nor any ancestor is in the cover" ⟺ covered strictly
/// deeper than `p.depth + 1`.
fn merge_violates_restriction(cover: &HashSet<QuadCoord>, p: QuadCoord) -> bool {
    let d1 = p.depth + 1;
    let side = 1i64 << d1;
    let (x0, z0) = (2 * p.x as i64, 2 * p.z as i64);
    let rim = [
        (x0 - 1, z0),
        (x0 - 1, z0 + 1), // left
        (x0 + 2, z0),
        (x0 + 2, z0 + 1), // right
        (x0, z0 - 1),
        (x0 + 1, z0 - 1), // below
        (x0, z0 + 2),
        (x0 + 1, z0 + 2), // above
    ];
    rim.into_iter().any(|(x, z)| {
        x >= 0
            && z >= 0
            && x < side
            && z < side
            && covering_depth(
                cover,
                QuadCoord {
                    depth: d1,
                    x: x as u32,
                    z: z as u32,
                },
            )
            .is_none()
    })
}

/// Evolve the PERSISTENT cover one bounded step toward what the metric wants.
///
/// This replaces the global budget fit. That fit re-derived `pixel_error` every
/// frame to hit the tile budget, and because every refine distance is a function of
/// `pixel_error`, ANY change re-selected the entire cover — measured on moonbase as
/// `wanted` alternating 349 <-> 532 every frame. It also had no fixed point (coarsen
/// above 100% of budget, refine below 85%, accept up to 100%, rungs ~1.5x apart), so
/// it oscillated and needed a dwell timer to damp a loop that should not exist.
///
/// Here the budget is enforced by REFUSING FURTHER SPLITS, not by moving a global
/// metric, so there is nothing to re-derive and no mass republish is possible. The
/// metric itself is fixed (the configured `pixel_error`), which also restores
/// view-independent, peer-identical selection.
///
/// Split priority is `dist / refine_range`, so the budget is spent nearest-first.
/// Merges run before splits (they free budget) and only past the same
/// [`REFINE_HYSTERESIS`] band the recursive walk uses, so a camera parked on a
/// threshold cannot flip a quad every frame.
///
/// The cover stays an exact, disjoint REPLACE cover throughout: a split swaps one
/// node for its four children, a merge swaps four siblings for their parent, and
/// nothing else touches it.
///
/// It also stays RESTRICTED (edge-adjacent nodes differ by ≤ 1 depth — the CDLOD
/// morph contract; a 2-level seam has no band wide enough to blend it). The cover
/// starts at `{ROOT}`, which is trivially restricted, so the invariant holds by
/// induction: every merge is guarded ([`merge_violates_restriction`]) and every
/// split first force-splits any coarser edge-neighbour (which, ON a restricted
/// cover, is at most one level up — so the cascade is depth-1 per neighbour,
/// recursing only through the chain it builds). The recursive `select_with_error`
/// walk never enforced this, which is one of the reasons it was replaced.
///
/// Returns the number of splits refused because they would exceed `budget` —
/// zero at any fixed point whose wanted set fits (see
/// [`TerrainStreamStatus::budget_refused`]).
#[cfg(test)]
fn evolve_cover(
    qt: &Quadtree,
    cover: &mut HashSet<QuadCoord>,
    focus: [f64; 2],
    eye_height: f64,
    node_error: &impl Fn(QuadCoord, Square) -> f64,
    budget: usize,
) -> usize {
    evolve_cover_for_foci(qt, cover, &[(focus, eye_height)], node_error, budget)
}

/// Multi-viewport form of [`evolve_cover`]. A node uses its nearest visual focus,
/// so all active marked cameras contribute to one exact cover without one camera
/// evicting another's terrain.
#[cfg(test)]
fn evolve_cover_for_foci(
    qt: &Quadtree,
    cover: &mut HashSet<QuadCoord>,
    foci: &[([f64; 2], f64)],
    node_error: &impl Fn(QuadCoord, Square) -> f64,
    budget: usize,
) -> usize {
    evolve_cover_for_foci_with_retention(qt, cover, foci, node_error, budget, &|_, _| false)
}

/// The persistent-cover evolution with an optional retain band. A retain band
/// blocks voluntary merges of already-refined tiles, but never requests new
/// splits; it is therefore hysteresis rather than a larger detail radius.
fn evolve_cover_for_foci_with_retention(
    qt: &Quadtree,
    cover: &mut HashSet<QuadCoord>,
    foci: &[([f64; 2], f64)],
    node_error: &impl Fn(QuadCoord, Square) -> f64,
    budget: usize,
    retain_refinement: &impl Fn(QuadCoord, Square) -> bool,
) -> usize {
    if cover.is_empty() {
        cover.insert(QuadCoord::ROOT);
    }
    let range = |c: QuadCoord| qt.error_refine_range(node_error(c, qt.region(c)));
    let dist = |c: QuadCoord| {
        foci.iter()
            .map(|(focus, eye_height)| qt.focus_distance(c, *focus, *eye_height))
            .reduce(f64::min)
            .unwrap_or(f64::INFINITY)
    };
    // How far past its refine range a node sits: < 1 wants to be finer, > 1 coarser.
    let slack = |c: QuadCoord| dist(c) / range(c).max(1e-9);

    // Full quads whose parent could take over, ranked by how far past the band.
    let mut parents: HashSet<QuadCoord> = HashSet::new();
    for c in cover.iter() {
        if let Some(p) = c.parent() {
            parents.insert(p);
        }
    }
    let mut merges: Vec<(f64, QuadCoord)> = parents
        .into_iter()
        .filter(|p| p.children().iter().all(|k| cover.contains(k)))
        .map(|p| {
            let slack = if retain_refinement(p, qt.region(p)) {
                0.0
            } else {
                slack(p)
            };
            (slack, p)
        })
        .collect();
    // Total order for peer determinism: slack, then coord (a slack-only sort
    // leaves equal keys in HashSet iteration order, so a budget truncation
    // would edit different nodes on different peers/runs).
    merges.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| (a.1.depth, a.1.x, a.1.z).cmp(&(b.1.depth, b.1.x, b.1.z)))
    });

    let mut edits = 0usize;
    let mut budget_refused = 0usize;
    let merge_one = |cover: &mut HashSet<QuadCoord>, p: QuadCoord| {
        // Re-check: an earlier merge in this pass may have consumed these children.
        if !p.children().iter().all(|k| cover.contains(k)) {
            return false;
        }
        for k in p.children() {
            cover.remove(&k);
        }
        cover.insert(p);
        true
    };

    // 1. Voluntary merges — the node is genuinely past the hysteresis band.
    for &(s, p) in merges.iter() {
        if edits >= MAX_COVER_EDITS {
            break;
        }
        if s < REFINE_HYSTERESIS {
            break; // sorted: nothing further out remains
        }
        // Restriction guard — also what makes cascade splits STABLE: a node
        // force-split for a neighbour's sake drifts past the hysteresis band, but
        // merging it back would re-violate, so the guard refuses and the cover
        // keeps its fixed point instead of flip-flopping.
        if merge_violates_restriction(cover, p) {
            continue;
        }
        if merge_one(cover, p) {
            edits += 1;
        }
    }

    // 2. Forced merges — over budget (the budget shrank, or forced splits pushed us
    //    past it). Give up the LEAST valuable *non-retained* quad first. A retain
    //    band is visual stability policy, not merely a voluntary-merge hint: the
    //    old reverse sort put its zero-slack near-camera entries FIRST here, causing
    //    the precise fine → coarse → fine flicker it was meant to prevent. Only if
    //    no far merge can satisfy the hard budget may this pass touch retained tiles.
    //    Not bounded by `MAX_COVER_EDITS`: being over budget is a frame-rate problem,
    //    and unlike a global metric change this only touches the quads it drops.
    if cover.len() > budget {
        for retained in [false, true] {
            // `merges` is sorted farthest-first (largest slack first). Preserve
            // that order here: the reverse iterator used to drop the nearest
            // non-retained quad before an available far one, producing a fine →
            // coarse → fine pulse as a moving camera crossed successive tiles.
            for &(_, p) in &merges {
                if cover.len() <= budget {
                    break;
                }
                if retain_refinement(p, qt.region(p)) != retained {
                    continue;
                }
                // Restricted merges are skipped, so a budget shrink can stay over
                // budget for a frame or two — the deepest quads always merge freely
                // (their neighbours can be at most one level deeper than THEM, never
                // two below the parent), and each pass of bottom-up merging unlocks
                // the next, so it converges instead of stalling.
                if merge_violates_restriction(cover, p) {
                    continue;
                }
                merge_one(cover, p);
            }
            if cover.len() <= budget {
                break;
            }
        }
    }

    // 3. Splits, nearest-first, until the budget is spent.
    let mut splits: Vec<(f64, QuadCoord)> = cover
        .iter()
        .copied()
        .filter(|c| c.depth < qt.max_depth)
        .map(|c| (slack(c), c))
        .filter(|&(s, _)| s < 1.0)
        .collect();
    splits.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| (a.1.depth, a.1.x, a.1.z).cmp(&(b.1.depth, b.1.x, b.1.z)))
    });
    for &(_, c) in splits.iter() {
        if edits >= MAX_COVER_EDITS {
            break;
        }
        if !cover.contains(&c) {
            continue; // consumed by an earlier chain's cascade
        }
        // Splitting `c` puts depth `c.depth + 1` against every edge-adjacent
        // region, so any neighbour covered COARSER than `c` must split first —
        // the classic restricted-quadtree cascade. On a restricted cover each
        // such neighbour is exactly one level up, and its own cascade needs (the
        // ancestors the loop pushes transitively), so the chain stays short and
        // the walk terminates at the root.
        let mut chain = vec![c];
        let mut i = 0;
        while i < chain.len() {
            let n = chain[i];
            for nb in edge_neighbours(n) {
                if let Some(nd) = covering_depth(cover, nb) {
                    if nd < n.depth {
                        let shift = nb.depth - nd;
                        let a = QuadCoord {
                            depth: nd,
                            x: nb.x >> shift,
                            z: nb.z >> shift,
                        };
                        if !chain.contains(&a) {
                            chain.push(a);
                        }
                    }
                }
            }
            i += 1;
        }
        // The whole chain lands or none of it does — a partial cascade would leave
        // the violation it exists to prevent. `continue`, not `break`: a later,
        // cheaper split (no cascade) may still fit the remaining budget.
        if edits + chain.len() > MAX_COVER_EDITS {
            continue; // transient: retried next pass once the edit budget frees up
        }
        if cover.len() + 3 * chain.len() > budget {
            // Unlike the edit cap this does not resolve by waiting, so it is
            // counted and surfaced.
            budget_refused += 1;
            continue;
        }
        // Coarsest first, so every forced split lands on an already-conforming rim.
        chain.sort_by_key(|q| q.depth);
        for q in chain {
            if !cover.remove(&q) {
                continue;
            }
            for k in q.children() {
                cover.insert(k);
            }
            edits += 1;
        }
    }
    budget_refused
}

fn build_draw_partition(
    selected: impl Iterator<Item = QuadCoord>,
    is_drawable: impl Fn(QuadCoord) -> bool,
    draw: &mut HashSet<QuadCoord>,
    scratch: &mut Vec<QuadCoord>,
) {
    draw.clear();
    for coord in selected {
        let mut c = coord;
        loop {
            if is_drawable(c) {
                draw.insert(c);
                break;
            }
            match c.parent() {
                Some(p) => c = p,
                // Nothing RESIDENT anywhere up the chain — only reachable before the
                // coarse base has baked at all. The area draws nothing for now.
                None => break,
            }
        }
    }
    scratch.clear();
    for c in draw.iter() {
        let mut p = *c;
        while let Some(up) = p.parent() {
            p = up;
            if draw.contains(&p) {
                scratch.push(*c);
                break;
            }
        }
    }
    for c in scratch.drain(..) {
        draw.remove(&c);
    }
}

/// Root depth. The root tile alone is the permanent no-hole fallback.
const ROOT_FALLBACK_DEPTH: u8 = QuadCoord::ROOT.depth;

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
    /// focus + eye height, generation, oracle identity, LOD knobs). When it
    /// matches last frame's and no bake is in flight, the
    /// resident tile set is already correct and the whole reselection is skipped —
    /// the idle-camera fast path (see [`update_lod_tiles`]). `None` = never selected.
    last_sig: Option<u64>,
    /// The PERSISTENT cover: the leaves currently selected. Evolved incrementally by
    /// `evolve_cover` instead of re-derived each frame, which is what removes the
    /// mass re-selection the old global budget fit caused.
    cover: HashSet<QuadCoord>,
    /// Whether the always-resident root fallback is fully baked.
    ///
    /// Load-bearing for correctness, not just speed: while this is false the idle
    /// fast path MUST NOT skip the frame body. The gate gives up when nothing is
    /// in flight and the camera has not moved — but "nothing in flight" also
    /// happens the frame the last queued bakes land, and with a still camera the
    /// remaining coarse tiles would then never be queued again. The base would sit
    /// permanently incomplete and the fallback it exists to provide would silently
    /// not be there. Also lets the enumeration be skipped once complete.
    coarse_ready: bool,
    /// Budget-refused split count from the last live [`evolve_cover`] pass. Kept
    /// on the component because the idle fast path skips selection — a
    /// starved-but-idle terrain must keep reporting, not read as healthy.
    budget_refused: usize,
    /// The first camera cover is immutable until its full near-camera patch is ready.
    /// Camera rigs often settle over several startup frames; re-selecting in that
    /// interval cancelled and replaced the same near tiles repeatedly, producing
    /// coarse → fine → coarse flicker before normal traversal had even begun.
    bootstrap_ready: bool,
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
        self.coarse_ready = false;
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
        self.coarse_ready = false;
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

/// Quantise a morph-band end onto the shared bucket lattice. `u32::MAX` = the
/// "never morphs" root sentinel.
fn band_bucket(morph_end: f32) -> u32 {
    if !morph_end.is_finite() || morph_end >= 1.0e19 {
        return u32::MAX;
    }
    // Quarter-log steps (~±12% band granularity).
    (morph_end.max(1.0).ln() * 4.0).floor() as u32
}

/// The geomorph start/end ratio, read off the core quadtree type so the shader
/// band cannot drift from the `Quadtree::morph_ratio` the CPU selection uses.
fn morph_ratio() -> f32 {
    Quadtree::new(1.0, 0, 1.0, 1.0).morph_ratio as f32
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
    (morph_ratio() * end, end, bucket)
}

/// Runtime-tunable LOD knobs (Inspector → "Terrain LOD"). Global across terrains.
/// Changing these re-selects tiles live so you can dial detail-vs-distance and the
/// load smoothness without a rebuild.
#[derive(Resource, Clone, Copy, Reflect, PartialEq)]
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
        // Must roughly fit the metric's steady-state want at TARGET_PIXEL_ERROR:
        // refused splits leave the far field on coarse parents indefinitely
        // (surfaced as `TerrainStreamStatus::budget_refused`).
        let tile_budget = 768;
        // On wasm32 the `AsyncComputeTaskPool` has NO threads: the "off-thread"
        // bake future runs to completion on the MAIN thread the instant it is
        // polled. A tile bake is ~12k composed-oracle samples (2401 verts ×
        // height_at + eps normals + the parent lattice), each walking the full
        // modifier chain — so `bakes_per_frame` is a direct main-thread frame cost
        // there. Cap it at 1, mirroring `collider_ring`'s wasm `bake_budget = 2`.
        #[cfg(target_arch = "wasm32")]
        let bakes_per_frame = 1;
        #[cfg(not(target_arch = "wasm32"))]
        // Native bakes stay off-thread. Issue enough work to fill the guaranteed
        // camera near field as one burst instead of revealing a few sibling
        // groups per rendered frame (the visible startup "clickering").
        let bakes_per_frame = 24;
        TerrainLodConfig {
            pixel_error: TARGET_PIXEL_ERROR,
            max_depth: MAX_DEPTH,
            bakes_per_frame,
            tile_budget,
        }
    }
}

/// Live-tune [`TerrainLodConfig`] from the API/scripts — the same fields the
/// Inspector's "Terrain LOD" section edits. Omitted fields keep their current
/// values. Written through raw: the selection pass clamps at its use sites, so
/// command and Inspector edits go through the same guards.
#[Command(default)]
pub struct SetTerrainLod {
    pub pixel_error: Option<f64>,
    pub max_depth: Option<u8>,
    pub bakes_per_frame: Option<usize>,
    pub tile_budget: Option<usize>,
}

#[on_command(SetTerrainLod)]
fn on_set_terrain_lod(trigger: On<SetTerrainLod>, mut cfg: ResMut<TerrainLodConfig>) {
    let ev = trigger.event();
    if let Some(v) = ev.pixel_error {
        cfg.pixel_error = v;
    }
    if let Some(v) = ev.max_depth {
        cfg.max_depth = v;
    }
    if let Some(v) = ev.bakes_per_frame {
        cfg.bakes_per_frame = v;
    }
    if let Some(v) = ev.tile_budget {
        cfg.tile_budget = v;
    }
}

register_commands!(on_set_terrain_lod);

/// Memoized per-node measured geometric error for a terrain's current oracle —
/// the cache behind error-driven CDLOD selection. Keyed by quadtree node; wiped
/// whenever the composed surface changes (live re-compose). Errors are measured
/// lazily for nodes the selection walk actually visits (O(visited), a few tens of
/// µs each, then cached for the surface's lifetime).
#[derive(Component, Default)]
pub struct TerrainNodeErrors {
    map: HashMap<QuadCoord, f64>,
    /// `surface_key()` of the oracle the cached errors were measured against —
    /// content identity, never the Arc pointer (allocator address reuse would
    /// keep a stale map alive).
    oracle_key: u64,
}

/// Cache of baked tile meshes keyed by quadtree node and mesh resolution (a 49²
/// streamed tile and a 2049² cinematic tile at the same coord must not collide).
/// A tile's geometry is a pure
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
pub struct LodMeshCache(HashMap<(Entity, QuadCoord, usize), (Handle<Mesh>, f64)>);

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
    pub fn drop_region(
        &mut self,
        terrain: Entity,
        bounds: Option<[f64; 4]>,
        root_half_extent: f64,
    ) {
        match bounds {
            None => self.0.retain(|(e, _, _), _| *e != terrain),
            Some(aabb) => self.0.retain(|(e, coord, _), _| {
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
// REMOVED: the per-tile reveal "settle" (`REVEAL_SECS` / `TileReveal` /
// `animate_tile_reveal`). Tiles are now born fully revealed.
//
// It broke CDLOD's core invariant. The morph factor MUST be a pure function of
// world position — that is precisely why the shader derives `dist` per vertex —
// because it is what makes two independently-built neighbours compute identical
// positions at their shared edge without communicating. Reveal added a per-tile,
// TIME-varying term (`m = max(morph, 1.0 - reveal)`), so two adjacent tiles at the
// same depth and distance that spawned a few frames apart disagreed at that edge:
// a crack opened, the skirt behind it caught the light, and the seam shimmered as
// the reveal animated. Movement staggers spawn times, so the artifact tracked
// movement — which is exactly how it was reported.
//
// Reveal existed to hide BAKE LATENCY, from before there was a fallback: tiles
// used to appear out of nothing, so they were eased in. The always-resident coarse
// base solves that properly (a blurry parent instead of a hole — Cesium's
// `ForbidHoles`, MSFS's "best currently available data"), which is why
// `docs/architecture/terrain-precompute-plan.md` already lists this machinery under
// "What this deletes". Two mechanisms were solving one problem and the older one
// was geometrically wrong.
//
// Do not reintroduce a per-tile fade to smooth LOD changes. Anything that varies
// per tile must not enter the vertex position, or neighbours crack. A legitimate
// cross-fade would have to be a function of position only, or live in shading.

/// Result of an off-thread tile bake: the finished CPU `Mesh` (not yet uploaded)
/// plus the spawn metadata the main thread needs.
struct BakedTile {
    mesh: Mesh,
    center: [f64; 2],
    depth: u32,
    /// Verts per side the mesh was baked at — part of the mesh-cache key.
    res: usize,
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
/// already present so the hot re-write path (the overlay sync)
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
    look.textures
        .insert(TextureLayer::ShadowCache, cache.image.clone());
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
fn tile_map_weights(depth: u32, map_res: usize, tile_res: usize) -> (f32, f32, f32) {
    let r = map_res as f32 / (((1u32 << depth.min(24)) * (tile_res as u32 - 1)) as f32);
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
pub(crate) fn apply_maps_to_look(
    look: &mut ShaderLook,
    maps: &TerrainDerivedMaps,
    depth: u32,
    tile_res: usize,
) {
    // Depth for the LOD-depth analysis overlay. Keyed, not live: it is genuinely
    // per-tile, but it costs no extra material splits because the map weights
    // below already key on `depth` too — `lod_depth` varies with nothing the key
    // did not already vary with. (`maps.res`/`tile_res` are per-TERRAIN, constant
    // across the tiles of one terrain at one depth, so they add no split either.)
    set_param(look, "lod_depth", ParamValue::F32(depth as f32));
    look.textures
        .insert(TextureLayer::Surface, maps.surface.clone());
    look.textures
        .insert(TextureLayer::Normal, maps.normal.clone());
    let (w_normal, w_ao, w_tone) = tile_map_weights(depth, maps.res, tile_res);
    set_param(look, "weight_normal", ParamValue::F32(w_normal));
    set_param(look, "weight_ao", ParamValue::F32(w_ao));
    set_param(look, "weight_tone", ParamValue::F32(w_tone));
}

/// Bind a terrain's AUTHORED layer maps (from its UsdShade Material network)
/// onto one tile look — the streamed-path counterpart of the static mesh's
/// `bind_terrain_layers`.
///
/// Unlike the derived maps, these weights are NOT per-depth. An authored
/// orthophoto is the site's actual appearance at every distance; fading it by
/// LOD would make the ground change colour as the camera moved. The author's
/// weight is the weight.
///
/// A layer the network does not author gets its weight forced to zero as well
/// as its slot left empty — both, so a stale weight surviving on a reused look
/// can never sample Bevy's fallback white as though it were terrain.
pub(crate) fn apply_authored_maps_to_look(look: &mut ShaderLook, authored: &TerrainAuthoredMaps) {
    match &authored.albedo {
        Some(h) => {
            look.textures.insert(TextureLayer::Albedo, h.clone());
            set_param(
                look,
                "weight_albedo",
                ParamValue::F32(authored.weight_albedo),
            );
        }
        None => set_param(look, "weight_albedo", ParamValue::F32(0.0)),
    }
    match &authored.mineral {
        Some(h) => {
            look.textures.insert(TextureLayer::Mineral, h.clone());
            set_param(
                look,
                "weight_mineral",
                ParamValue::F32(authored.weight_mineral),
            );
        }
        None => set_param(look, "weight_mineral", ParamValue::F32(0.0)),
    }
}

/// The appearance INTENT of one LOD tile: the geomorph shader (it drives both the
/// `@vertex` morph and the `@fragment` stage), its morph band, and — in `Lit`
/// mode — the derived maps, the far-shadow cache and the analysis overlay.
///
/// THE SHARING CONTRACT: two tiles in the same mode and band bucket
/// produce an EQUAL `ShaderLook::key()`, so `lunco-render-bevy` hands them the same
/// `ShaderMaterial` handle — one bind group, one batch. This is the property the
/// hand-rolled `MatKey`/`LodMaterials` cache existed for; it is now a consequence of
/// the look's content rather than a table anyone has to remember to consult.
/// Anything that varies per-tile (a raw `morph_end` instead of the snapped band, an
/// un-bucketed value) would mint a material per tile and destroy batching — which is
/// exactly why the band is snapped before it lands here.
fn tile_look(
    mode: TerrainShaderMode,
    depth: u32,
    tile_res: usize,
    morph_start: f32,
    morph_end: f32,
    maps: Option<&TerrainDerivedMaps>,
    authored: Option<&TerrainAuthoredMaps>,
    shadow: Option<&TileShadowCache>,
    overlay: crate::overlay::OverlayUniforms,
) -> ShaderLook {
    let path = mode.shader_path();
    let mut look = ShaderLook::new(path).with_vertex_shader(path);
    set_param(&mut look, "morph_start", ParamValue::F32(morph_start));
    set_param(&mut look, "morph_end", ParamValue::F32(morph_end));
    match mode {
        TerrainShaderMode::DebugLod => {
            set_param(&mut look, "base_color", ParamValue::Vec3(lod_rgb(depth)))
        }
        TerrainShaderMode::Plain => set_param(
            &mut look,
            "base_color",
            ParamValue::Vec3([0.35, 0.34, 0.32]),
        ),
        TerrainShaderMode::Lit => {
            if let Some(maps) = maps {
                apply_maps_to_look(&mut look, maps, depth, tile_res);
            }
            // AFTER the derived maps: both write `weight_*`, and the author's
            // opinion is the one that should survive.
            if let Some(authored) = authored {
                apply_authored_maps_to_look(&mut look, authored);
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
    tile_res: usize,
    morph_end: f32,
    mode: TerrainShaderMode,
    maps: Option<&TerrainDerivedMaps>,
    authored: Option<&TerrainAuthoredMaps>,
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
    // snapped end at the quadtree's morph ratio).
    let (ms, me, _bucket) = snap_band(morph_end);
    commands
        .spawn((
            Mesh3d(mesh),
            tile_look(
                mode, depth, tile_res, ms, me, maps, authored, shadow, overlay,
            ),
            cell,
            Transform::from_translation(local),
            // The render binder reacts to `ShaderLook` after this deferred spawn.
            // Keep the tile hidden for one complete ECS turn; the streamer promotes
            // it on the next update, after its material exists.
            Visibility::Hidden,
            LodTileOf(terrain),
            Name::new(format!("LodTile d{} {},{}", coord.depth, coord.x, coord.z)),
            // Streamed runtime detail, not scene content — hidden from the Entity
            // list unless the user opts in.
            lunco_core::SystemManaged,
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
    /// Obsolete in-flight requests dropped this frame after selection moved on.
    pub stale_cancelled: usize,
    /// Splits the selection wanted but `tile_budget` refused (summed over
    /// terrains). Unlike `pending` this is not transient: non-zero at a still
    /// camera means areas hold on coarser parents until `tile_budget` is raised
    /// or `pixel_error` coarsened.
    pub budget_refused: usize,
    /// Active camera positions inside a streaming terrain footprint.
    pub focus_wanted: usize,
    /// Camera positions whose exact selected tile (not a coarse fallback) is ready.
    pub focus_resident: usize,
}

fn required_focus_depth(
    cover: &HashSet<QuadCoord>,
    demands: &[TerrainVisualDemand],
    quadtree: &Quadtree,
) -> Option<u8> {
    demands
        .iter()
        .filter(|demand| demand.required)
        .filter_map(|demand| {
            cover
                .iter()
                .copied()
                .find(|&coord| quadtree.region(coord).distance_to(demand.focus) <= f64::EPSILON)
                .map(|coord| coord.depth)
        })
        .min()
}

fn near_field_error_floor(region: Square, tile_res: usize, demands: &[TerrainVisualDemand]) -> f64 {
    demands
        .iter()
        .any(|focus| {
            focus.required && region.distance_to(focus.focus) <= focus.near_detail_radius_m
        })
        .then(|| region.side() / (tile_res - 1) as f64)
        .unwrap_or(0.0)
}

fn near_field_retains_refinement(region: Square, demands: &[TerrainVisualDemand]) -> bool {
    demands.iter().any(|focus| {
        focus.required
            && region.distance_to(focus.focus)
                <= focus.near_detail_radius_m + focus.near_detail_hysteresis_m
    })
}

fn cached_node_error(
    cache: &std::cell::RefCell<&mut HashMap<QuadCoord, f64>>,
    coord: QuadCoord,
    measure: impl FnOnce() -> f64,
) -> f64 {
    // Copy the value out in its own scope. Keeping the `Ref` temporary alive
    // through the miss branch and then calling `borrow_mut` is a runtime panic.
    if let Some(error) = { cache.borrow().get(&coord).copied() } {
        return error;
    }
    let error = measure();
    cache.borrow_mut().insert(coord, error);
    error
}

fn add_focus_readiness(
    status: &mut TerrainStreamStatus,
    demands: &[TerrainVisualDemand],
    selected: impl IntoIterator<Item = QuadCoord> + Clone,
    quadtree: &Quadtree,
    is_ready: impl Fn(QuadCoord) -> bool,
) {
    for demand in demands.iter().filter(|demand| demand.required) {
        status.focus_wanted += 1;
        let selected_coord = selected
            .clone()
            .into_iter()
            .find(|&coord| quadtree.region(coord).distance_to(demand.focus) <= f64::EPSILON);
        if selected_coord.is_some_and(&is_ready) {
            status.focus_resident += 1;
        }
    }
}

fn bootstrap_cover_is_ready(
    selected: &[Selected],
    demands: &[TerrainVisualDemand],
    is_ready: impl Fn(QuadCoord) -> bool,
) -> bool {
    demands.iter().filter(|focus| focus.required).all(|focus| {
        selected
            .iter()
            // Unlocking after only the tile under the camera was ready still made
            // its neighbours hand off one at a time. Keep the opening cover fixed
            // until every selected tile touching the guaranteed near field has a
            // mesh. The draw partition then reveals that local patch as one stable
            // cover instead of a succession of coarse/fine substitutions.
            .filter(|selected| {
                selected.region.distance_to(focus.focus) <= focus.near_detail_radius_m
            })
            .all(|selected| is_ready(selected.coord))
    })
}

/// **Run tile streaming in LOCKSTEP with the frame instead of against the wall
/// clock.** Set while an offline recording is capturing; clear otherwise.
///
/// # Why this exists
///
/// Ordinary streaming is real-time paced on purpose: a frame starts at most
/// [`TerrainLodConfig::bakes_per_frame`] bakes, caps in-flight work at
/// [`MAX_INFLIGHT_BAKES`], and *polls* those off-thread tasks with `poll_once` — so
/// a bake lands on whichever frame it happens to finish on. That is exactly right
/// for interactive play (the frame never blocks on baking) and exactly wrong for
/// offline capture, because "whichever frame it happens to finish on" is a function
/// of thread scheduling, not of the shot.
///
/// MEASURED: with the recorder's readiness gate and recorder-owned camera-path
/// release both in place — frame 0 bit-identical across runs — two full runs of
/// `episode_02_rover.usda` still differed on the FROZEN shots (01, 02, 03, 06) in
/// 25-38 separate blocks of frames each, with the final frame matching every time.
/// A transient, not an accumulation: the readiness gate guarantees the wanted tile
/// set is fully resident at frame 0, but as the camera moves through the shot the
/// selection changes, new bakes are queued, and they land a
/// scheduling-dependent number of frames later. Same shot, same camera, different
/// tiles drawn on any given frame.
///
/// # What setting it does
///
/// Two changes in [`update_lod_tiles`], both only while set:
///
/// 1. **No pacing budgets.** `bakes_per_frame` and [`MAX_INFLIGHT_BAKES`] are
///    lifted, so the set of bakes STARTED on a frame is a pure function of that
///    frame's selection rather than of a budget interacting with the previous
///    frame's carry-over.
/// 2. **Drain, don't poll.** Pending bakes are blocked on to completion rather than
///    `poll_once`d, so every bake started on frame N is resident by frame N+1 —
///    always, on every machine, instead of "eventually".
///
/// The result is that what is drawn on frame N is a pure function of N. Note this
/// does NOT make streaming instantaneous: finalize runs before selection in the
/// system body, so a tile is still one frame behind the selection that asked for
/// it. That is fine — reproducibility needs the latency to be CONSTANT, not zero.
///
/// It costs frame time (the frame now blocks on baking), which is the correct
/// trade for an offline render and unacceptable for interactive play. Hence a flag,
/// not a change of default.
///
/// # Who sets it
///
/// `lunco-sandbox`, mirroring `OfflineRecordingState::active` — the same inversion
/// as `report_terrain_status`/`report_scene_spawn_status`, for the same reason.
/// `lunco-workbench` (which owns the recorder) is a UI-shell crate and cannot name
/// terrain; `lunco-terrain-surface` must not know what a recorder is. `lunco-sandbox`
/// is the assembly point that sees both.
///
/// Distinct from [`LodFrozen`], which is an authored per-terrain opt-in that stops
/// re-selection outright. This one keeps selection live (the shot still refines as
/// the camera moves) and only makes its timing deterministic.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct TerrainStreamLockstep(pub bool);

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
    wanted: HashSet<QuadCoord>,
    /// The DRAW partition: exactly one tile per covered area — a ready wanted node,
    /// or the deepest ready ancestor standing in for one that is not.
    draw: HashSet<QuadCoord>,
    /// The always-resident root fallback, as a bake target.
    coarse: Vec<Selected>,
    /// Scratch for the disjointness pass over `draw`.
    drop_covered: Vec<QuadCoord>,
    /// Active cameras projected into the terrain's local frame.
    visual_foci: Vec<TerrainVisualDemand>,
    /// Allocation-free projection of `visual_foci` into the pure CDLOD metric.
    focus_metric: Vec<([f64; 2], f64)>,
}

/// Freeze this terrain's LOD selection once its tiles are up: authored
/// `bool lunco:terrain:lodFrozen = true` on the Terrain prim.
///
/// For a SCRIPTED shot. Streaming exists to adapt an unbounded world to a viewpoint
/// nobody predicted; a cinematic's viewpoint is authored, so there is nothing to
/// adapt to — and adapting anyway is visible, because a camera crossing LOD bands
/// at speed evicts and re-bakes tiles mid-shot (the ground blinking under a moving
/// camera). Frozen, the set that streamed in before the shot started is the set that
/// is drawn, all the way through.
///
/// The first selection still runs — freezing an empty terrain would draw nothing at
/// all. It is the RE-selection that stops.
///
/// Not for a free-flying camera: a frozen terrain does not refine for one, so
/// anything outside the initial set stays coarse or absent.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct LodFrozen;

/// Per-frame: stream the LOD tile set for each streaming terrain against the camera.
pub fn update_lod_tiles(
    mut commands: Commands,
    demands: Res<TerrainDetailDemands>,
    // The big_space world grid each tile anchors into (its own `CellCoord`).
    grids: Query<(Entity, &Grid), With<WorldGrid>>,
    mut terrains: Query<(
        Entity,
        &DemHeightField,
        &TerrainLodViz,
        &mut LodTiles,
        &mut PendingTileBakes,
        &mut TerrainNodeErrors,
        Option<&TerrainShaderMode>,
        Option<&TerrainDerivedMaps>,
        Option<&TerrainAuthoredMaps>,
        Option<&TileShadowCache>,
        Has<LodFrozen>,
    )>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mesh_cache: ResMut<LodMeshCache>,
    cfg: Res<TerrainLodConfig>,
    mut stream_status: ResMut<TerrainStreamStatus>,
    // Set while an offline recording captures — see [`TerrainStreamLockstep`].
    lockstep: Res<TerrainStreamLockstep>,
    settings: Option<Res<lunco_settings::TerrainSettings>>,
    overlay_params: Res<crate::overlay::TerrainOverlayParams>,
    mut scratch: Local<StreamScratch>,
) {
    // Snapshot the analysis-overlay uniforms once; every tile built this frame paints
    // the current params (live re-tuning of resident tiles rides `sync_terrain_overlay`).
    let overlay = overlay_params.uniforms();
    *stream_status = TerrainStreamStatus::default();
    // Split-borrow the per-frame scratch buffers (see [`StreamScratch`]).
    let StreamScratch {
        swaps,
        done,
        keyed,
        wanted,
        draw,
        coarse,
        drop_covered,
        visual_foci,
        focus_metric,
    } = &mut *scratch;
    let enable_shaders = settings.as_ref().map(|s| s.enable_shaders).unwrap_or(true);
    if demands.visual.is_empty() {
        return;
    }
    // No world grid yet → can't anchor tiles; skip this frame.
    let Ok((grid_entity, grid)) = grids.single() else {
        return;
    };

    // Per-frame bake budget shared across all terrains (amortise scale changes).
    //
    // Under lockstep the budgets are LIFTED, not merely raised: a budget makes the
    // bakes started on a frame depend on what the previous frame could not fit,
    // which is precisely the frame-to-frame carry-over that has to go for the shot
    // to be reproducible. See [`TerrainStreamLockstep`].
    let lockstep = lockstep.0;
    let mut bake_budget = if lockstep {
        usize::MAX
    } else {
        cfg.bakes_per_frame.max(1)
    };
    let max_inflight = if lockstep {
        usize::MAX
    } else {
        MAX_INFLIGHT_BAKES
    };
    // Live streaming terrains — the mesh cache is GLOBAL (keyed by `(terrain,
    // coord)`), so its cap must scale with them or two terrains would fight over
    // one terrain's worth of entries and thrash each other every frame.
    let terrain_count = terrains.iter().count().max(1);

    for (
        terrain,
        hf,
        viz,
        mut tiles,
        mut pending,
        mut errs,
        mode_opt,
        maps,
        authored,
        shadow,
        frozen,
    ) in &mut terrains
    {
        // A tile spawned on the previous update has now had one complete ECS turn
        // for `ShaderLook` → render-material binding. Only now may the draw
        // partition expose it; until then its ready ancestor remains visible.
        let mut promoted_tiles = false;
        for slot in tiles.tiles.values_mut() {
            if !slot.ready {
                slot.ready = true;
                promoted_tiles = true;
            }
        }

        // Frozen and already covered ⇒ the drawn set is final. Report it as fully
        // resident (it is — that is the point) so the status bar clears and anything
        // gating on residency, like a camera path waiting to start, is satisfied.
        if frozen && !tiles.tiles.is_empty() && pending.0.is_empty() {
            stream_status.wanted += tiles.tiles.len();
            stream_status.resident += tiles.tiles.len();
            continue;
        }
        let mut mode = mode_opt
            .copied()
            .unwrap_or_else(TerrainShaderMode::platform_default);
        if !enable_shaders {
            mode = TerrainShaderMode::Plain;
        }
        // The frozen cover is ONE tile for the whole terrain, so it carries the
        // detail the whole quadtree used to spread over thousands — mesh it far
        // finer than a streamed tile. `viz.tile_res` (49) over the full window
        // would be ~20 m between vertices: one tile, and no terrain.
        let tile_res = if frozen {
            CINEMATIC_TILE_RES
        } else {
            viz.tile_res
        };
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
                tiles
                    .tiles
                    .iter()
                    .map(|(c, s)| (s.entity, c.depth as u32, s.morph_end)),
            );
            for &(ent, depth, morph_end) in swaps.iter() {
                // Each tile carries its own morph band; restate it under the new mode.
                let (ms, me, _) = snap_band(morph_end);
                let look = tile_look(
                    mode, depth, tile_res, ms, me, maps, authored, shadow, overlay,
                );
                commands.entity(ent).try_insert(look);
            }
            tiles.mode = mode;
        }

        // Project every active visual camera into this terrain's local DEM frame.
        // Direction remains a bake-priority bias, but it is deliberately not a
        // second selection focus: startup camera settling otherwise creates a
        // transient high-detail island that visibly appears and disappears.
        let oracle = &hf.0;
        let h = oracle.half_extent() as f64;
        visual_foci.clear();
        for demand in demands.visual.iter() {
            let local = demand.position;
            let local_forward = demand.forward;
            let heading = bevy::math::DVec2::new(local_forward.x, local_forward.z);
            let heading = (heading.length() > 1e-3).then(|| heading.normalize());
            let here = [local.x, local.z];
            if here[0].abs() > h || here[1].abs() > h {
                continue;
            }
            let ground = oracle.height_at(here[0], here[1]);
            visual_foci.push(TerrainVisualDemand {
                focus: here,
                eye_height: (local.y - ground).max(0.0),
                heading,
                required: true,
                near_detail_radius_m: demand.near_detail_radius_m,
                near_detail_hysteresis_m: demand.near_detail_hysteresis_m,
            });
        }
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
        let pixel_error = base_px;
        let qt = quadtree_for(pixel_error);
        // ── Idle-camera fast path ────────────────────────────────────────────
        // The selection below (quadtree walks + budget-coarsen loop + sort + queue
        // + retain) is a pure function of (focus, eye height, generation, oracle
        // identity, LOD knobs). Re-deriving it EVERY
        // frame with a still camera was the dominant idle terrain CPU cost
        // (obs 23593). Fold those inputs into a signature; when it matches last
        // frame's AND no bake is mid-flight (nothing to finalize/spawn), last
        // frame's resident tiles are already correct — skip the whole body.
        // Quantise focus/eye so sub-tile jitter doesn't defeat the gate; a slow
        // creep re-runs the frame it crosses a quantum (a 1-frame-late reselection
        // is invisible — tiles morph).
        {
            const IDLE_QUANT_M: f64 = 0.5;
            let q = |v: f64| (v / IDLE_QUANT_M).round() as i64 as u64;
            let mut sig = lunco_precompute::Fnv1a::new();
            for visual in visual_foci.iter() {
                sig.write_u64(q(visual.focus[0]));
                sig.write_u64(q(visual.focus[1]));
                sig.write_u64(q(visual.eye_height));
            }
            sig.write_u64(cur_gen as u64);
            // Oracle identity — a live re-compose changes the surface without
            // always bumping gen, and would otherwise be missed by the gate.
            // Content key, not the Arc pointer: a reused allocation address
            // would pass the gate with a stale surface.
            sig.write_u64(hf.0.surface_key());
            // LOD knobs (Inspector) — a live tweak must re-select.
            sig.write_u64(cfg.pixel_error.to_bits());
            sig.write_u64(cfg.tile_budget as u64);
            sig.write_u64(cfg.max_depth as u64);
            let sig = sig.finish();
            if !promoted_tiles
                && pending.0.is_empty()
                && tiles.last_sig == Some(sig)
                && tiles.coarse_ready
            {
                // Idle: resident tiles already match. Contribute this terrain's
                // resident count so the status bar still reads "done", not "0/0".
                stream_status.wanted += tiles.tiles.len();
                stream_status.resident += tiles.tiles.len();
                stream_status.budget_refused += tiles.budget_refused;
                add_focus_readiness(
                    &mut stream_status,
                    visual_foci,
                    tiles.cover.iter().copied(),
                    &qt,
                    |coord| {
                        tiles
                            .tiles
                            .get(&coord)
                            .is_some_and(|slot| slot.ready && slot.drawn)
                    },
                );
                continue;
            }
            tiles.last_sig = Some(sig);
        }
        // Runtime LOD knobs (Inspector) drive detail-vs-cost live; tile_res stays
        // per-terrain (changing it would invalidate the mesh cache). The range
        // factor derives from the CANONICAL screen metric (fixed viewport + the
        // pixel_error knob) so selection stays view-independent + peer-identical.
        // FIXED metric. `pixel_error` is a pure quality knob again — it is never
        // moved to chase the tile budget, so every refine distance (and therefore
        // every tile's `morph_end` and material band bucket) is stable frame to
        // frame. The budget is enforced incrementally instead; see `evolve_cover`.
        // This also restores view-independent, peer-identical selection.
        // ERROR-DRIVEN selection: refine where the MEASURED surface error says
        // there is detail worth refining toward (crater rims, peaks), not on the
        // uniform per-depth schedule. Errors are memoized per node against the
        // current oracle; the cache wipes when the oracle is swapped (live edit).
        let oracle_key = hf.0.surface_key();
        if errs.oracle_key != oracle_key {
            errs.map.clear();
            errs.oracle_key = oracle_key;
        }
        let err_map = std::cell::RefCell::new(&mut errs.map);
        let src: &SurfaceOracle = hf.0.as_ref();
        let node_error = |c: QuadCoord, region: Square| -> f64 {
            let measured = cached_node_error(&err_map, c, || {
                // The probe estimates the resolved parent approximation error.
                // Over-zoom remains Nyquist-gated here so the cached measurement
                // stays camera-independent.
                let probe_step = region.side() / (NODE_ERROR_PROBE_RES - 1) as f64;
                let limited = src.detail_limited(probe_step);
                measure_node_error(&limited, region, NODE_ERROR_PROBE_RES)
            });
            // A sparse coarse-node probe cannot discover a small crater that lies
            // between its samples. Without a conservative near-field floor, that
            // zero error prevents the camera's branch from ever being split, so
            // the deeper probe that *could* see the crater is never reached.
            //
            // Use the tile's own vertex pitch as its conservative geometric error
            // only on branches containing an actual camera. This is standard
            // observer-centred CDLOD: it guarantees fine geometry under the view
            // without globally refining flat/far terrain or turning look-ahead
            // samples into false camera positions.
            measured.max(near_field_error_floor(region, tile_res, visual_foci))
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
        // NOT a full `select_with_error` walk. The cover is persistent state now
        // (`evolve_cover` below); walking the whole tree here and discarding it would
        // reintroduce the per-frame cost this change exists to remove.
        let mut sel: Vec<Selected>;
        if frozen {
            // NO LOD — ONE tile, meshed at `CINEMATIC_TILE_RES`, covering the whole
            // terrain.
            //
            // There is no quadtree here at all, which is the point: `pixel_error`
            // refines by DISTANCE FROM THE CAMERA and `tile_budget` coarsens it
            // until the selection fits, so both re-decide the cover whenever the
            // camera moves — the ground re-loading under a moving shot. A single
            // node cannot be split, merged, evicted or re-baked by anything a
            // camera does.
            //
            // One tile rather than the whole tree at `max_depth`: that is 4^8 =
            // 65_536 tiles (~157M verts), which does not load in any useful time.
            // And it would buy nothing — depth 8 puts vertices 0.08 m apart, far
            // below what the DEM carries, so it is interpolating detail that is not
            // there. One tile at ~1025² samples the surface oracle (DEM + analytic
            // craters) as finely as it has anything to say, in a single draw call.
            sel = vec![Selected {
                coord: QuadCoord::ROOT,
                region: qt.region(QuadCoord::ROOT),
                // Geomorph blends a tile toward its coarser parent; the root has no
                // parent, and there is no LOD transition left to hide.
                morph_start: f64::INFINITY,
                morph_end: f64::INFINITY,
            }];
        } else {
            // A `pixel_error` change re-derives every refine distance, so the WHOLE cover is
            // re-selected in one frame — measured on moonbase as `wanted` alternating
            // 349 ↔ 532 EVERY FRAME, i.e. hundreds of tiles re-picked per frame forever. With
            // unrefinement that reads as detail dipping coarse and snapping back: the jitter.
            //
            // It oscillated because the two thresholds overlapped: coarsen above 100% of
            // budget, refine below 85%, and ACCEPT a refined rung right up to 100%. One rung is
            // ~1.5x the tile count of the next, so refining from 68% landed at ~104%, which
            // coarsened straight back under 85%, which refined again. No fixed point exists.
            //
            // Two changes make it settle:
            //   * a refined rung must land inside the same 85% band the coarsen path exits at,
            //     so the thresholds form a real hysteresis band instead of overlapping;
            //   * after any change the rung is HELD for a dwell, so a camera drifting across a
            //     threshold cannot re-cut the cover every frame. Large overshoot (>150%) skips
            //     the dwell, so frame rate is never hostage to the damping.
            // INCREMENTAL: evolve the persistent cover a bounded step, then read the
            // selection off it. No global metric moves, so no mass re-selection exists to
            // oscillate — see `evolve_cover`.
            if tiles.cover.is_empty() || tiles.bootstrap_ready {
                focus_metric.clear();
                focus_metric.extend(
                    visual_foci
                        .iter()
                        .map(|demand| (demand.focus, demand.eye_height)),
                );
                tiles.budget_refused = 0;
                // One evolve pass can split a branch by one level because candidates
                // are snapshotted before edits. Build the initial cover to its fixed
                // point once, then keep it unchanged until its camera tile is visibly
                // ready. This avoids cancelling/rebuilding startup tiles while the
                // camera rig settles over its first few frames.
                if tiles.cover.is_empty() {
                    for _ in 0..cfg.max_depth.max(1) {
                        let before = required_focus_depth(&tiles.cover, visual_foci, &qt);
                        tiles.budget_refused += evolve_cover_for_foci_with_retention(
                            &qt,
                            &mut tiles.cover,
                            focus_metric,
                            &node_error,
                            budget,
                            &|_, region| near_field_retains_refinement(region, visual_foci),
                        );
                        let after = required_focus_depth(&tiles.cover, visual_foci, &qt);
                        if after == before || after == Some(cfg.max_depth) {
                            break;
                        }
                    }
                } else if tiles.bootstrap_ready {
                    tiles.budget_refused = evolve_cover_for_foci_with_retention(
                        &qt,
                        &mut tiles.cover,
                        focus_metric,
                        &node_error,
                        budget,
                        &|_, region| near_field_retains_refinement(region, visual_foci),
                    );
                }
            }
            sel = tiles
                .cover
                .iter()
                .map(|&c| {
                    let parent_range = c
                        .parent()
                        .map(|p| qt.error_refine_range(node_error(p, qt.region(p))))
                        .unwrap_or(f64::INFINITY);
                    qt.selected(c, parent_range)
                })
                .collect();
        }
        wanted.clear();
        wanted.extend(sel.iter().map(|s| s.coord));

        // Selection is authoritative. Drop superseded generations and camera
        // requests no longer selected before they can occupy every worker slot.
        // While an exact camera-underfoot tile is missing, background coarse-base
        // preload (except the root safety carpet) must yield its worker slots.
        // Otherwise a camera arriving during startup can wait behind an entire
        // breadth-first preload even though its selected tile is first in the new
        // queue.
        let urgent_missing = sel.iter().any(|selected| {
            visual_foci.iter().any(|demand| {
                demand.required
                    && selected.region.distance_to(demand.focus) <= f64::EPSILON
                    && !tiles
                        .tiles
                        .get(&selected.coord)
                        .is_some_and(|slot| slot.gen == cur_gen && slot.ready)
            })
        });
        let pending_before = pending.0.len();
        pending.0.retain(|coord, (gen, _)| {
            *gen == cur_gen
                && (wanted.contains(coord)
                    || *coord == QuadCoord::ROOT
                    || (!urgent_missing && coord.depth <= ROOT_FALLBACK_DEPTH))
        });
        stream_status.stale_cancelled += pending_before - pending.0.len();
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
        // The DISTANCE morph settles children onto the parent lattice, so the
        // out-of-depth-order coarse->fine handoff never pops. It is a pure function
        // of world position, which is what keeps neighbouring tiles agreeing at
        // their shared edge — see the note where the per-tile reveal was removed.
        // NOTE: the old `CARPET_DEPTH` two-tier key is gone. It bucketed depth <= 2 ahead of
        // everything else to bake a "carpet" first — but the selection is a REPLACE cover and
        // a site DEM never selects depths 0-2, so the branch never fired and no carpet ever
        // existed (that absence is what made the terrain go black). The coarse base now does
        // that job properly: it is enumerated, not selected, and queued ahead of `sel`. So
        // this sort only has to order the SELECTION, by screen-space benefit.
        // Minimum screen-space benefit across actual active camera positions.
        // Prefetch samples shape the selected cover, but are not substitute
        // viewpoints: letting an 80 m look-ahead endpoint compete as an equal
        // focus delayed the tile directly beneath the camera.
        let benefit = |s: &Selected| -> f64 {
            let size = s.region.half * 2.0;
            visual_foci
                .iter()
                .filter(|visual| visual.required)
                .map(|visual| {
                    let to = bevy::math::DVec2::new(
                        s.region.center[0] - visual.focus[0],
                        s.region.center[1] - visual.focus[1],
                    );
                    let base = to.length_squared() / (size * size).max(1e-9);
                    let Some(heading) = visual.heading else {
                        return base;
                    };
                    let distance = to.length();
                    if distance < 1e-6 {
                        return base;
                    }
                    // cos 1 (dead ahead) → ×1 … cos −1 (behind) → ×4.
                    base * (2.5 - 1.5 * (to / distance).dot(heading))
                })
                .reduce(f64::min)
                .unwrap_or(f64::INFINITY)
        };
        let camera_underfoot = |s: &Selected| {
            visual_foci
                .iter()
                .any(|visual| visual.required && s.region.distance_to(visual.focus) <= f64::EPSILON)
        };
        // Decorate-sort-undecorate: `dist2`/`benefit` pay a sqrt/dot per call and
        // the comparator re-ran them on BOTH sides of every comparison
        // (O(n log n) evaluations) — compute each tile's key once, sort on the
        // cached key.
        keyed.clear();
        keyed.extend(sel.drain(..).map(|s| {
            let priority = u8::from(!camera_underfoot(&s));
            (priority, 0, benefit(&s), s)
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
        let fresh_tile =
            |tiles: &LodTiles, c: &QuadCoord| tiles.tiles.get(c).is_some_and(|s| s.gen == cur_gen);

        // ── Finalize completed off-thread bakes ──────────────────────
        // Poll in-flight tasks; for each finished bake, upload its mesh (cheap, main
        // thread) + spawn the tile. The expensive DEM sampling already ran on a
        // worker thread, so the frame never blocks on baking.
        //
        // ...unless we are in LOCKSTEP (offline capture), where the frame blocks on
        // EVERY in-flight bake instead. `poll_once` returns whatever happens to be
        // done, which makes the tile set drawn on a given frame a function of thread
        // scheduling; draining to completion makes it a function of the frame index.
        // See [`TerrainStreamLockstep`] for the measurement that motivated this.
        done.clear();
        if lockstep {
            for (coord, (gen, task)) in pending.0.drain() {
                done.push((coord, gen, block_on(task)));
            }
            // `drain()` on a `HashMap` yields in an arbitrary (hash-seed-dependent)
            // order, and downstream `done` handling inserts into `LodTiles`/despawns
            // — so sort to a deterministic order before consuming it. Coordinates are
            // unique per terrain, so this is a total order.
            done.sort_by_key(|(coord, _, _)| (coord.depth, coord.x, coord.z));
        } else {
            pending.0.retain(
                |coord, (gen, task)| match block_on(future::poll_once(&mut *task)) {
                    Some(baked) => {
                        done.push((*coord, *gen, baked));
                        false
                    }
                    None => true,
                },
            );
        }
        for (coord, gen, baked) in done.drain(..) {
            // A bake from a superseded generation (heights changed while it ran) is
            // discarded — its mesh would show the OLD terrain.
            if gen != cur_gen {
                continue;
            }
            let handle = meshes.add(baked.mesh);
            let oy = baked.origin_y;
            mesh_cache
                .0
                .insert((terrain, coord, baked.res), (handle.clone(), oy));
            // No longer selected while it baked → keep the cached mesh, skip spawning.
            if !wanted.contains(&coord) {
                continue;
            }
            let depth = baked.depth;
            // `oy` (baked with the mesh) anchors the tile's cell to its geometry — see
            // `spawn_tile`/`bake_tile_mesh` `origin_y`. Using the baked value (not a
            // spawn-time recompute) keeps mesh and placement in lock-step across gens.
            let ent = spawn_tile(
                &mut commands,
                grid,
                grid_entity,
                terrain,
                coord,
                handle,
                baked.center,
                depth,
                baked.res,
                baked.morph_end,
                mode,
                maps,
                authored,
                shadow,
                overlay,
                oy,
            );
            // Replace any stale slot at this coord, despawning the tile it held.
            if let Some(old) = tiles.tiles.insert(
                coord,
                TileSlot {
                    entity: ent,
                    gen: cur_gen,
                    morph_end: baked.morph_end,
                    drawn: false,
                    ready: false,
                },
            ) {
                retire_tile(&mut commands, old.entity);
            }
        }

        // ── Queue new work, nearest-first ────────────────────────────
        // Cache hits spawn instantly; misses spawn an off-thread bake task (budgeted:
        // `bakes_per_frame` new tasks/frame, ≤ MAX_INFLIGHT in flight). Tiles anchor
        // to their OWN big_space `CellCoord` (vertices baked relative to the tile
        // centre) so far-from-origin tiles keep f32 precision.
        let pool = AsyncComputeTaskPool::get();
        // The one root fallback is queued before the selection. It is a complete
        // visual floor; every other job is selected detail, never a redundant
        // coarse hierarchy that could later replace visible detail.
        coarse.clear();
        if !frozen && !tiles.coarse_ready {
            for c in fallback_coords() {
                coarse.push(Selected {
                    coord: c,
                    region: qt.region(c),
                    // A coarse-base tile is a stand-in, not a member of the selected
                    // cover: it must not morph toward a parent of its own. The
                    // non-finite sentinel maps to the shader's no-morph bucket.
                    morph_start: f64::INFINITY,
                    morph_end: f64::INFINITY,
                });
            }
        }
        // The root fallback is a correctness prerequisite, not just the first item
        // in a priority queue. Starting fine bakes beside it can saturate the worker
        // pool on a cold cache, leaving the only complete visual floor pending while
        // detail work runs. Until the root is actually drawable, admit *only* that
        // bake. Once it is visible, the normal near-camera-first queue resumes.
        let root_ready = tiles
            .tiles
            .get(&QuadCoord::ROOT)
            .is_some_and(|slot| slot.ready);
        // The root fallback precedes urgent work after that gate.
        for s in coarse
            .iter()
            .take(1)
            .chain(sel.iter())
            .chain(coarse.iter().skip(1))
        {
            if !root_ready && s.coord != QuadCoord::ROOT {
                continue;
            }
            // Skip coords already satisfied at the current generation (resident tile
            // or in-flight current-gen bake). Stale tiles fall through → re-baked.
            let have_pending = pending.0.get(&s.coord).is_some_and(|(g, _)| *g == cur_gen);
            if fresh_tile(&tiles, &s.coord) || have_pending {
                continue;
            }
            let depth = s.coord.depth as u32;
            // Per-node morph band: finite for sub-root nodes, "never" for the root
            // (the sentinel `snap_band` maps to the no-morph bucket).
            let morph_end = if s.morph_end.is_finite() {
                s.morph_end as f32
            } else {
                1.0e21
            };
            if let Some((cached, oy)) = mesh_cache.0.get(&(terrain, s.coord, tile_res)) {
                // Placed at the mesh's OWN baked `origin_y` (stored beside it), never a
                // recompute — otherwise a cache hit against a since-composed oracle jumps.
                let ent = spawn_tile(
                    &mut commands,
                    grid,
                    grid_entity,
                    terrain,
                    s.coord,
                    cached.clone(),
                    s.region.center,
                    depth,
                    tile_res,
                    morph_end,
                    mode,
                    maps,
                    authored,
                    shadow,
                    overlay,
                    *oy,
                );
                if let Some(old) = tiles.tiles.insert(
                    s.coord,
                    TileSlot {
                        entity: ent,
                        gen: cur_gen,
                        morph_end,
                        drawn: false,
                        ready: false,
                    },
                ) {
                    retire_tile(&mut commands, old.entity);
                }
                continue;
            }
            // Cache miss → needs a bake. Respect the per-frame + in-flight budgets
            // (keep scanning for cheap cache hits regardless of the bake budget).
            if bake_budget == 0 || pending.0.len() >= max_inflight {
                continue;
            }
            bake_budget -= 1;
            let oracle_arc = hf.0.clone();
            let coord = s.coord;
            let region = s.region;
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
                    oracle_arc.as_ref(),
                    coord,
                    region,
                    tile_res,
                    half,
                    center,
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
                mesh.insert_attribute(ATTRIBUTE_MORPH_NORMAL, tm.morph_normals);
                // The SAME anchor `bake_tile_mesh_cached` rebased the mesh Y by (full
                // oracle at the tile centre). Carried on `BakedTile` so the main thread
                // places the tile at exactly the height its mesh was baked for.
                let origin_y = oracle_arc.height_at(center[0], center[1]);
                BakedTile {
                    mesh,
                    center,
                    depth,
                    res: tile_res,
                    morph_end,
                    origin_y,
                }
            });
            pending.0.insert(s.coord, (cur_gen, task));
        }

        // The base is complete once every enumerated coarse node is resident at this
        // generation. Latching it stops the per-frame enumeration AND re-arms the idle
        // fast path (which is held off until then — see `LodTiles::coarse_ready`).
        if !tiles.coarse_ready && !coarse.is_empty() {
            tiles.coarse_ready = coarse.iter().all(|s| fresh_tile(&tiles, &s.coord));
        }

        // ── Unrefinement: draw the best READY data, never a hole ─────────────
        // Cesium's `ForbidHoles`: "unrefine back to a parent tile when a child isn't done
        // loading… never rendered with holes, though the tile rendered instead may have low
        // resolution". MSFS states the same rule as "draw tiles using the best currently
        // available data ● tiles can use data from a parent". Both draw the parent INSTEAD
        // of its children — never underneath them, which would punch a coarse shell through
        // the fine surface wherever the terrain is concave.
        //
        // So: each selected node draws itself if ready, else its deepest ready ancestor.
        // The coarse base guarantees that walk terminates, which is what the old design
        // lacked — it could only hold a trailing tile that happened to overlap, so panning
        // somewhere new had nothing to fall back to and showed clear colour.
        build_draw_partition(
            sel.iter().map(|s| s.coord),
            // RESIDENCY. A stale tile keeps drawing itself until its own re-bake
            // replaces it in place — see this function's docs.
            |c| tiles.tiles.get(&c).is_some_and(|slot| slot.ready),
            draw,
            drop_covered,
        );

        // Retain + visibility. The coarse base is never despawned; everything else lives
        // while it is wanted or actively drawn as a stand-in.
        tiles.tiles.retain(|coord, slot| {
            let keep = coord.depth <= ROOT_FALLBACK_DEPTH
                || wanted.contains(coord)
                || draw.contains(coord);
            if !keep {
                retire_tile(&mut commands, slot.entity);
                return false;
            }
            let vis = draw.contains(coord);
            if slot.drawn != vis {
                slot.drawn = vis;
                commands.entity(slot.entity).try_insert(if vis {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                });
            }
            true
        });

        // The first cover may now begin adapting normally. Until its complete
        // camera-near patch is prepared it is intentionally immutable: changing
        // that cover while the rig settles only creates startup churn.
        if !tiles.bootstrap_ready
            && bootstrap_cover_is_ready(&sel, visual_foci, |coord| {
                tiles.tiles.get(&coord).is_some_and(|slot| slot.ready)
            })
        {
            tiles.bootstrap_ready = true;
            // The cover was intentionally held while the camera rig settled;
            // force one clean signature pass now that normal adaptation resumes.
            tiles.last_sig = None;
        }

        // How many selected areas have NO cover at all — the metric that must stay 0.
        // Non-zero means something rendered as clear colour, and it can only happen
        // before the coarse base has finished baking.
        let uncovered = sel
            .iter()
            .filter(|s| {
                let mut c = s.coord;
                loop {
                    if draw.contains(&c) {
                        return false;
                    }
                    match c.parent() {
                        Some(p) => c = p,
                        None => return true,
                    }
                }
            })
            .count();
        if uncovered > 0 {
            debug!(
                target: "terrain_stream",
                uncovered,
                wanted = wanted.len(),
                drawn = draw.len(),
                resident = tiles.tiles.len(),
                backlog = pending.0.len(),
                "terrain has uncovered area (coarse base still baking?)"
            );
        }

        // Streaming progress for the UI indicator (accumulated across terrains).
        stream_status.wanted += wanted.len();
        stream_status.resident += wanted
            .iter()
            .filter(|c| tiles.tiles.get(c).is_some_and(|slot| slot.ready))
            .count();
        stream_status.pending += pending.0.len();
        stream_status.budget_refused += tiles.budget_refused;
        add_focus_readiness(
            &mut stream_status,
            visual_foci,
            sel.iter().map(|selected| selected.coord),
            &qt,
            |coord| {
                tiles
                    .tiles
                    .get(&coord)
                    .is_some_and(|slot| slot.ready && slot.drawn)
            },
        );

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
            mesh_cache
                .0
                .retain(|(e, c, _), _| *e != terrain || resident.contains(c));
        }
    }
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
    mesh_cache.0.retain(|(t, _, _), _| !dead(t));
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
        (
            &TerrainDerivedMaps,
            &LodTiles,
            &TerrainLodViz,
            Has<LodFrozen>,
        ),
        Changed<TerrainDerivedMaps>,
    >,
    mut looks: Query<&mut ShaderLook>,
) {
    for (maps, tiles, viz, frozen) in &changed {
        if tiles.mode != TerrainShaderMode::Lit {
            continue;
        }
        let tile_res = if frozen {
            CINEMATIC_TILE_RES
        } else {
            viz.tile_res
        };
        for (depth, entity) in tiles.tiles_with_depth() {
            if let Ok(mut look) = looks.get_mut(entity) {
                apply_maps_to_look(&mut look, maps, depth, tile_res);
            }
        }
    }
}

/// The same late-bind for AUTHORED maps. The layer binder needs the composed
/// stage AND the twin's asset root, so it publishes
/// [`TerrainAuthoredMaps`] well after the first tiles are already resident —
/// and it re-publishes whenever the author drags a weight or repoints a map
/// through the Material network, since those are journaled attribute edits.
/// Restating in place keeps that live without tile churn or a re-bake.
///
/// Lit tiles only, for the same reason as the derived maps: the flat/debug
/// shader declares no map bindings, so writing them there would only mint
/// pointless material variants.
pub(crate) fn bind_authored_maps_to_tiles(
    changed: Query<(&TerrainAuthoredMaps, &LodTiles), Changed<TerrainAuthoredMaps>>,
    mut looks: Query<&mut ShaderLook>,
) {
    for (authored, tiles) in &changed {
        if tiles.mode != TerrainShaderMode::Lit {
            continue;
        }
        for (_depth, entity) in tiles.tiles_with_depth() {
            if let Ok(mut look) = looks.get_mut(entity) {
                apply_authored_maps_to_look(&mut look, authored);
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

#[cfg(test)]
mod draw_partition_tests {
    use super::*;

    fn c(depth: u8, x: u32, z: u32) -> QuadCoord {
        QuadCoord { depth, x, z }
    }

    #[test]
    fn fallback_is_exactly_one_root_tile() {
        assert_eq!(fallback_coords().collect::<Vec<_>>(), vec![QuadCoord::ROOT]);
    }

    /// Every selected area must end up covered by exactly ONE drawn node — itself or an
    /// ancestor. This is the invariant whose absence rendered as black terrain.
    fn assert_covered_exactly_once(sel: &[QuadCoord], draw: &HashSet<QuadCoord>) {
        for s in sel {
            let mut covers = 0;
            let mut cur = Some(*s);
            while let Some(n) = cur {
                if draw.contains(&n) {
                    covers += 1;
                }
                cur = n.parent();
            }
            assert_eq!(
                covers, 1,
                "{s:?} covered {covers}× (want exactly 1); draw={draw:?}"
            );
        }
    }

    #[test]
    fn all_ready_draws_the_leaves_themselves() {
        let sel = c(1, 0, 0).children().to_vec();
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(sel.iter().copied(), |_| true, &mut draw, &mut scratch);
        assert_eq!(draw.len(), 4);
        assert_covered_exactly_once(&sel, &draw);
    }

    #[test]
    fn unready_leaf_falls_back_to_its_ready_ancestor() {
        // Nothing fine is ready; only the root is. Every leaf must fall back to it, and the
        // root must be drawn ONCE — not once per leaf.
        let sel = c(1, 0, 0).children().to_vec();
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            |n| n.depth == 0,
            &mut draw,
            &mut scratch,
        );
        assert_eq!(
            draw.len(),
            1,
            "one ancestor stands in for the whole subtree"
        );
        assert!(draw.contains(&QuadCoord::ROOT));
        assert_covered_exactly_once(&sel, &draw);
    }

    #[test]
    fn ready_sibling_is_dropped_when_an_ancestor_stands_in() {
        // THE overlap case: one child ready, its sibling not. The parent must stand in for
        // the quad, and the ready child must NOT also draw — drawing both is the coarse
        // surface punching through the fine one (z-fighting), which is why ForbidHoles
        // unrefines instead of underlaying.
        let kids = c(1, 0, 0).children();
        let ready = kids[0];
        let sel = kids.to_vec();
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            |n| n.depth == 0 || n == ready,
            &mut draw,
            &mut scratch,
        );
        assert!(draw.contains(&QuadCoord::ROOT));
        assert!(
            !draw.contains(&ready),
            "ready child must not draw under a drawn ancestor"
        );
        assert_covered_exactly_once(&sel, &draw);
    }

    #[test]
    fn nothing_ready_leaves_the_area_uncovered_rather_than_wrong() {
        // Before the coarse base lands there is genuinely nothing to draw. It must degrade
        // to "no tile" — never to a bogus one — and must not panic walking off the root.
        let sel = vec![c(3, 5, 2)];
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(sel.iter().copied(), |_| false, &mut draw, &mut scratch);
        assert!(draw.is_empty());
    }

    /// THE REGRESSION TEST for the terrain blanking on a generation bump.
    ///
    /// `invalidate()` keeps every tile and only marks it stale. Keying visibility on
    /// freshness hid all of them at once — caught live as
    /// `uncovered=265 drawn=0 resident=510`. Keyed on residency, each tile keeps
    /// drawing ITSELF until its own re-bake replaces it in place.
    ///
    /// The check that matters is `draw == sel`: every node draws itself, so nothing
    /// is substituted for anything. The earlier rejected fix satisfied "not empty"
    /// while swapping in coarse ancestors, which alternated and looked worse.
    #[test]
    fn a_generation_bump_keeps_each_tile_drawing_itself() {
        let sel = vec![c(3, 0, 0), c(3, 1, 0), c(2, 1, 0), c(2, 0, 1)];
        // Everything resident, nothing fresh — exactly the post-invalidate state.
        let resident: HashSet<QuadCoord> = sel.iter().copied().collect();

        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            |n| resident.contains(&n),
            &mut draw,
            &mut scratch,
        );

        let want: HashSet<QuadCoord> = sel.iter().copied().collect();
        assert_eq!(
            draw, want,
            "each stale-but-resident tile must keep drawing ITSELF; substituting a \
             different node is the rejected fix that looked worse"
        );
        assert_covered_exactly_once(&sel, &draw);
    }

    // ── incremental cover (`evolve_cover`) ───────────────────────────────────

    fn test_qt() -> Quadtree {
        // range_factor 1.0 → refine_range == node error, so the tests state
        // distances directly in metres.
        Quadtree::new(1000.0, 6, 1.0, 100.0)
    }

    #[test]
    fn camera_near_field_reaches_finest_cover_without_a_transient_forward_focus() {
        let qt = Quadtree::from_screen_metric(
            1000.0,
            6,
            1000.0,
            CANON_SCREEN_H_PX,
            CANON_FOV_Y_RAD,
            TARGET_PIXEL_ERROR,
        );
        let actual = TerrainVisualDemand {
            focus: [0.0, 0.0],
            eye_height: 3.0,
            heading: None,
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        let demands = [actual];
        let foci = demands
            .iter()
            .map(|demand| (demand.focus, demand.eye_height))
            .collect::<Vec<_>>();
        let error =
            |_coord: QuadCoord, region: Square| near_field_error_floor(region, TILE_RES, &demands);
        let mut cover = HashSet::new();

        for _ in 0..qt.max_depth {
            evolve_cover_for_foci(&qt, &mut cover, &foci, &error, 768);
        }

        assert_eq!(
            required_focus_depth(&cover, &demands, &qt),
            Some(qt.max_depth),
            "the exact camera branch must reach finest detail in the opening update"
        );
        let nearby_depth = cover
            .iter()
            .find(|&&coord| qt.region(coord).distance_to([20.0, 0.0]) <= f64::EPSILON)
            .map(|coord| coord.depth)
            .expect("nearby subject point must remain covered");
        assert_eq!(
            nearby_depth, qt.max_depth,
            "the guaranteed camera near field must cover nearby visible subjects"
        );
        let forward_depth = cover
            .iter()
            .find(|&&coord| qt.region(coord).distance_to([80.0, 0.0]) <= f64::EPSILON)
            .map(|coord| coord.depth)
            .expect("forward point must remain covered");
        assert!(
            forward_depth < qt.max_depth,
            "forward direction must not create a separate finest-detail island"
        );
    }

    #[test]
    fn bootstrap_waits_for_the_entire_camera_near_field() {
        let qt = test_qt();
        let selected = QuadCoord::ROOT
            .children()
            .into_iter()
            .map(|coord| Selected {
                coord,
                region: qt.region(coord),
                morph_start: f64::INFINITY,
                morph_end: f64::INFINITY,
            })
            .collect::<Vec<_>>();
        let demand = TerrainVisualDemand {
            focus: [0.0, 0.0],
            eye_height: 3.0,
            heading: None,
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        let missing = selected[0].coord;

        assert!(
            !bootstrap_cover_is_ready(&selected, &[demand], |coord| coord != missing),
            "one unprepared neighbouring tile must keep the opening cover stable"
        );
        assert!(
            bootstrap_cover_is_ready(&selected, &[demand], |_| true),
            "the cover may adapt once every near-camera tile is prepared"
        );
    }

    #[test]
    fn repeated_same_frame_node_error_cache_miss_then_hits_do_not_overlap_borrows() {
        let coord = QuadCoord::ROOT;
        let mut storage = HashMap::new();
        let cache = std::cell::RefCell::new(&mut storage);
        let measurements = std::cell::Cell::new(0usize);

        for _ in 0..16 {
            let error = cached_node_error(&cache, coord, || {
                measurements.set(measurements.get() + 1);
                42.0
            });
            assert_eq!(error, 42.0);
        }

        assert_eq!(
            measurements.get(),
            1,
            "one miss must insert once; repeated refinement passes must be cache hits"
        );
    }

    /// The cover must remain an exact, disjoint REPLACE cover of the root after any
    /// number of evolution steps: every point covered by exactly one node.
    fn assert_exact_cover(cover: &HashSet<QuadCoord>) {
        assert!(!cover.is_empty(), "cover went empty");
        // No node may be an ancestor of another (disjointness).
        for a in cover.iter() {
            let mut p = a.parent();
            while let Some(n) = p {
                assert!(
                    !cover.contains(&n),
                    "{a:?} lies under {n:?} — cover overlaps"
                );
                p = n.parent();
            }
        }
        // Total area must equal the root's: sum of 4^-depth == 1.
        let area: f64 = cover.iter().map(|c| 0.25f64.powi(c.depth as i32)).sum();
        assert!(
            (area - 1.0).abs() < 1e-9,
            "cover area {area} != 1 — holes or overlaps"
        );
    }

    /// Driving the camera around must never break the cover, and must never exceed
    /// the tile budget. This is the invariant the old global budget fit enforced by
    /// re-deriving everything; here it has to survive incremental edits.
    #[test]
    fn evolving_cover_stays_exact_and_within_budget() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let budget = 40;
        let mut cover = HashSet::new();

        // Sweep the focus across the terrain, then back out to a distance.
        for step in 0..60 {
            let x = -900.0 + (step as f64) * 30.0;
            evolve_cover(&qt, &mut cover, [x, 0.0], 2.0, &err, budget);
            assert_exact_cover(&cover);
            assert!(
                cover.len() <= budget,
                "cover {} exceeded budget {budget}",
                cover.len()
            );
        }
        for step in 0..30 {
            let h = 100.0 + (step as f64) * 400.0;
            evolve_cover(&qt, &mut cover, [0.0, 0.0], h, &err, budget);
            assert_exact_cover(&cover);
            assert!(cover.len() <= budget);
        }
    }

    /// Do two cover nodes share a boundary SEGMENT (not merely a corner)? Normalised
    /// `[0,1]` extents are exact powers of two, so the touch/overlap tests are exact
    /// — no epsilon.
    fn edge_adjacent(a: QuadCoord, b: QuadCoord) -> bool {
        let ext = |c: QuadCoord| {
            let s = 0.5f64.powi(c.depth as i32);
            (
                c.x as f64 * s,
                c.x as f64 * s + s,
                c.z as f64 * s,
                c.z as f64 * s + s,
            )
        };
        let (ax0, ax1, az0, az1) = ext(a);
        let (bx0, bx1, bz0, bz1) = ext(b);
        let touch_x = ax1 == bx0 || bx1 == ax0;
        let touch_z = az1 == bz0 || bz1 == az0;
        let over_x = ax0 < bx1 && bx0 < ax1;
        let over_z = az0 < bz1 && bz0 < az1;
        (touch_x && over_z) || (touch_z && over_x)
    }

    /// Worst neighbour depth gap in a cover, with the offending pair.
    fn worst_depth_gap(cover: &HashSet<QuadCoord>) -> (u32, Option<(QuadCoord, QuadCoord)>) {
        let v: Vec<QuadCoord> = cover.iter().copied().collect();
        let mut worst = 0u32;
        let mut pair = None;
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                if !edge_adjacent(v[i], v[j]) {
                    continue;
                }
                let gap = (v[i].depth as i32 - v[j].depth as i32).unsigned_abs();
                if gap > worst {
                    worst = gap;
                    pair = Some((v[i], v[j]));
                }
            }
        }
        (worst, pair)
    }

    /// CONTROL for [`evolving_cover_stays_restricted`]: the checker itself must
    /// accept a hand-built restricted cover and flag a hand-built 2-level seam,
    /// or a pass over `evolve_cover` output proves nothing.
    #[test]
    fn the_restriction_checker_detects_seams() {
        let d1 = |x, z| QuadCoord { depth: 1, x, z };
        // (1,0,0) replaced by its four children: every edge crosses ≤ 1 level.
        let mut restricted: HashSet<QuadCoord> =
            [d1(1, 0), d1(0, 1), d1(1, 1)].into_iter().collect();
        restricted.extend((0..2).flat_map(|x| (0..2).map(move |z| QuadCoord { depth: 2, x, z })));
        let (gap, pair) = worst_depth_gap(&restricted);
        assert!(
            gap <= 1,
            "checker rejects a restricted cover: gap {gap} at {pair:?}"
        );
        // (1,0,0) replaced by its SIXTEEN depth-3 descendants: they touch the
        // depth-1 siblings directly — a 2-level seam the checker must flag.
        let mut seamed: HashSet<QuadCoord> = [d1(1, 0), d1(0, 1), d1(1, 1)].into_iter().collect();
        seamed.extend((0..4).flat_map(|x| (0..4).map(move |z| QuadCoord { depth: 3, x, z })));
        let (gap, _) = worst_depth_gap(&seamed);
        assert_eq!(gap, 2, "checker missed a 2-level seam");
    }

    /// THE CDLOD RESTRICTION: edge-adjacent selected nodes must differ by at most one
    /// level, because a single morph band is all that blends them — a 2-level jump has
    /// no band wide enough and the seam becomes a hard edge (visible as a staircase
    /// where sub-DEM detail switches on).
    ///
    /// `evolve_cover` enforces this by construction — guarded merges + cascade
    /// splits, from a trivially-restricted `{ROOT}` start (see its docs). This test
    /// is the proof it stays that way under a camera sweep; it FAILED (gap 2 at
    /// step 4) before the guards existed.
    #[test]
    fn evolving_cover_stays_restricted() {
        let qt = test_qt();
        // Depth-decaying error — see the control test for why a constant one is
        // degenerate here.
        let err = |c: QuadCoord, _r: Square| 120.0f64 * 0.5f64.powi(c.depth as i32);
        let budget = 40;
        let mut cover = HashSet::new();

        for step in 0..60 {
            let x = -900.0 + (step as f64) * 30.0;
            evolve_cover(&qt, &mut cover, [x, 0.0], 2.0, &err, budget);
            let (gap, pair) = worst_depth_gap(&cover);
            assert!(
                gap <= 1,
                "step {step}: neighbour depth gap {gap} > 1 at {pair:?}"
            );
        }
        for step in 0..30 {
            let h = 100.0 + (step as f64) * 400.0;
            evolve_cover(&qt, &mut cover, [0.0, 0.0], h, &err, budget);
            let (gap, pair) = worst_depth_gap(&cover);
            assert!(
                gap <= 1,
                "height step {step}: neighbour depth gap {gap} > 1 at {pair:?}"
            );
        }
    }

    /// A STATIONARY camera must reach a fixed point and then stop editing. The old
    /// fit could not: it alternated `wanted` 349 <-> 532 every frame forever, which
    /// is what re-cut hundreds of tiles per frame and read as jitter.
    #[test]
    fn a_still_camera_reaches_a_fixed_point() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut cover = HashSet::new();
        for _ in 0..200 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 64);
        }
        let settled = cover.clone();
        for _ in 0..50 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 64);
            assert_eq!(
                cover, settled,
                "cover still churning on a stationary camera — no fixed point"
            );
        }
    }

    #[test]
    fn multiple_visual_foci_share_one_stable_cover() {
        let qt = test_qt();
        let err = |c: QuadCoord, _r: Square| 120.0f64 * 0.5f64.powi(c.depth as i32);
        let foci = [([-700.0, 0.0], 5.0), ([700.0, 0.0], 5.0)];
        let mut cover = HashSet::new();
        for _ in 0..200 {
            evolve_cover_for_foci(&qt, &mut cover, &foci, &err, 256);
        }
        let settled = cover.clone();
        evolve_cover_for_foci(&qt, &mut cover, &foci, &err, 256);
        assert_eq!(
            cover, settled,
            "multiple cameras must reach one fixed point"
        );
        assert_exact_cover(&cover);

        for focus in foci.map(|(focus, _)| focus) {
            let leaf = qt.node_containing(qt.max_depth, focus);
            assert!(
                covering_depth(&cover, leaf).is_none_or(|depth| depth >= 3),
                "each active camera must retain local detail"
            );
        }
    }

    #[test]
    fn ecs_focus_marker_collects_only_active_perspective_cameras() {
        let mut app = App::new();
        app.init_resource::<TerrainDetailDemands>().add_systems(
            Update,
            (mark_terrain_visual_foci, collect_terrain_detail_demands).chain(),
        );
        let grid = app.world_mut().spawn(Grid::new(1_000.0, 100.0)).id();
        let camera = app
            .world_mut()
            .spawn((
                Camera {
                    is_active: true,
                    ..default()
                },
                Camera3d::default(),
                Projection::Perspective(default()),
                CellCoord::default(),
                Transform::from_xyz(123.0, 10.0, -45.0),
                ChildOf(grid),
            ))
            .id();
        let inactive_camera = app
            .world_mut()
            .spawn((
                Camera {
                    is_active: false,
                    ..default()
                },
                Camera3d::default(),
                Projection::Perspective(default()),
                CellCoord::default(),
                Transform::IDENTITY,
                ChildOf(grid),
            ))
            .id();

        app.update();

        assert!(app.world().get::<TerrainVisualFocus>(camera).is_some());
        assert!(
            app.world()
                .get::<TerrainVisualFocus>(inactive_camera)
                .is_some(),
            "activation controls demand, not marker ownership"
        );
        let demands = app.world().resource::<TerrainDetailDemands>();
        assert_eq!(demands.visual.len(), 1);
        assert_eq!(demands.visual[0].position, DVec3::new(123.0, 10.0, -45.0));

        // A bare active Camera has no render graph; it must not add a hidden
        // terrain-detail focus that can churn the visible camera's LOD cover.
        app.world_mut().spawn((
            Camera {
                is_active: true,
                ..default()
            },
            Projection::Perspective(default()),
            CellCoord::default(),
            Transform::from_xyz(-300.0, 10.0, 0.0),
            ChildOf(grid),
            TerrainVisualFocus::default(),
        ));
        app.update();
        assert_eq!(
            app.world().resource::<TerrainDetailDemands>().visual.len(),
            1
        );
    }

    #[test]
    fn retired_tile_survives_two_render_extractions_before_despawn() {
        let mut app = App::new();
        app.add_systems(Update, retire_terrain_tiles);
        let tile = app
            .world_mut()
            .spawn((Visibility::Hidden, RetiringTerrainTile { updates_left: 2 }))
            .id();

        app.update();
        app.update();
        assert!(app.world().get_entity(tile).is_ok());

        app.update();
        assert!(app.world().get_entity(tile).is_err());
    }

    /// The refusal counter is starvation, not fill progress: any budget that
    /// covers the metric's want reports zero at the fixed point, any budget
    /// below it reports the refused splits.
    #[test]
    fn budget_starvation_is_reported() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let settle = |budget: usize| {
            let mut cover = HashSet::new();
            for _ in 0..200 {
                evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, budget);
            }
            (
                evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, budget),
                cover,
            )
        };
        let (refused, unconstrained) = settle(usize::MAX);
        assert_eq!(
            refused, 0,
            "fixed point within budget must not report refusals"
        );
        let want = unconstrained.len();
        let (refused, _) = settle(want);
        assert_eq!(refused, 0, "budget == want must not report refusals");
        for budget in [want / 2, want / 8, 16] {
            let (refused, cover) = settle(budget);
            assert!(
                refused > 0,
                "budget {budget} < want {want} (cover {}) reached a fixed point \
                 without reporting refusals",
                cover.len()
            );
        }
    }

    /// Lowering the budget must be absorbed by merging, not by blowing past it.
    #[test]
    fn shrinking_the_budget_merges_down() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut cover = HashSet::new();
        for _ in 0..200 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 256);
        }
        assert!(cover.len() > 16, "expected a refined cover to shrink from");
        for _ in 0..50 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 16);
        }
        assert_exact_cover(&cover);
        assert!(
            cover.len() <= 16,
            "cover {} did not shrink to budget 16",
            cover.len()
        );
    }

    #[test]
    fn budget_pressure_sheds_far_cover_before_a_retained_quad() {
        let qt = test_qt();
        let retained = c(1, 0, 0);
        let farthest = c(1, 1, 1);
        let mut cover = (0..4)
            .flat_map(|z| (0..4).map(move |x| c(2, x, z)))
            .collect::<HashSet<_>>();
        // Keep all quads inside the refine band so this exercises the forced,
        // not voluntary, merge path.
        let err = |_c: QuadCoord, _r: Square| 1_000_000.0f64;

        evolve_cover_for_foci_with_retention(
            &qt,
            &mut cover,
            &[([-700.0, -700.0], 5.0)],
            &err,
            13,
            &|coord, _| coord == retained,
        );

        assert!(
            retained
                .children()
                .iter()
                .all(|child| cover.contains(child)),
            "a retained near-camera quad must survive while another merge can satisfy the budget"
        );
        assert!(
            cover.len() <= 13,
            "non-retained quads should absorb the budget cut"
        );
        assert!(cover.contains(&farthest));
        assert!(
            farthest
                .children()
                .iter()
                .all(|child| !cover.contains(child)),
            "the farthest eligible quad must merge before nearer non-retained cover"
        );
    }

    #[test]
    fn mixed_depths_stay_disjoint() {
        // A realistic cover: deep near-field, shallower further out, one deep node unready.
        let deep = c(3, 0, 0);
        let sel = vec![deep, c(3, 1, 0), c(2, 1, 0), c(2, 0, 1)];
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            |n| n != deep && n.depth >= 1,
            &mut draw,
            &mut scratch,
        );
        assert_covered_exactly_once(&sel, &draw);
        // Disjointness: no drawn node may be an ancestor of another drawn node.
        for a in draw.iter() {
            let mut p = a.parent();
            while let Some(n) = p {
                assert!(!draw.contains(&n), "{a:?} drawn under drawn ancestor {n:?}");
                p = n.parent();
            }
        }
    }
}
