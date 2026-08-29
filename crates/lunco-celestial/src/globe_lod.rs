//! Camera-driven cube-sphere **live LOD** for celestial bodies (globe scale).
//!
//! Replaces the old fixed 24-tile shell (6 faces × 2×2 at level 1) with a
//! recursive quadtree subdivision per face: tiles refine near the camera and
//! coarsen far away, so a body shows planetary curvature from orbit and finer
//! relief as you approach. The selection is the globe crate's sphere-correct
//! `subdivide_face` (camera distance vs tile arc-size) — kept there as the pure
//! spine; this module is the scene integration (spawn/despawn + appearance intent),
//! which lives in `lunco-celestial` because that's what owns the bodies, textures,
//! grids and the blueprint look.
//!
//! Per body, [`GlobeLod`] carries the params + the surface grid + look;
//! [`GlobeTiles`] tracks residency, the bounded mesh cache, and the cached
//! selection inputs; [`update_globe_lod`] reconciles that state with the camera.
//! Tile placement uses the grid's `translation_to_grid` together with a
//! centre-relative mesh, so the authoritative BigSpace pose is established at
//! spawn and remains stable while only tile residency changes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use lunco_core::SceneViewport;
use lunco_materials::{ShaderLook, ShaderLookReady};
use lunco_render::SceneCamera;
use lunco_terrain_core::{CompositeHeightSource, HeightSource, Square};
use lunco_terrain_globe::quad_sphere::{cube_to_sphere, subdivide_face, tile_center_uv};
use lunco_terrain_globe::{
    create_quadsphere_tile_mesh, GlobeHandoff as GlobeHandoffGeometry, GlobeSurfacePatch,
    TerrainTile, TileCoord,
};
use lunco_terrain_surface::SurfaceOracle;

/// Per-body live-LOD context. Inserted on a celestial body entity in place of the
/// old fixed tile loop; [`update_globe_lod`] reads it to stream cube-sphere tiles.
#[derive(Component)]
pub struct GlobeLod {
    /// Body radius (m) — tile vertices ride this sphere.
    pub radius_m: f64,
    /// The surface grid the tiles anchor into (its own `CellCoord` per tile).
    pub surface_grid: Entity,
    /// Appearance intent applied to every tile (the body's blueprint look). Cloned
    /// onto each tile; the binder's content-keyed cache collapses them back to ONE
    /// `ShaderMaterial` per body — the same single-handle batching the old
    /// `Handle<ShaderMaterial>` field guaranteed by hand.
    pub look: ShaderLook,
    /// Vertices per tile side.
    pub res: u32,
    /// Deepest subdivision level near the camera.
    pub max_lod: u32,
    /// `refine when dist < tile_arc · factor` — larger = refine from farther.
    pub lod_distance_factor: f64,
}

/// A finite DEM's continuation at the globe boundary.
///
/// The DEM owns exactly its authored square. Outside that square it has no
/// measured samples, so extending the nearest edge sample through the whole
/// globe collar would turn an edge crater/rim into a many-kilometre artificial
/// apron. The only valid continuation is the measured border datum, reached
/// over one raster posting so the handoff remains continuous at the boundary.
#[derive(Clone)]
struct BoundarySiteSource {
    oracle: Arc<SurfaceOracle>,
    region: Square,
    half_extent: f64,
    datum_m: f64,
    boundary_m: f64,
}

impl HeightSource for BoundarySiteSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        if self.region.distance_to([x, z]) <= 0.0 {
            return self.oracle.height_at(x, z);
        }

        let edge_height = self.oracle.height_at(
            x.clamp(-self.half_extent, self.half_extent),
            z.clamp(-self.half_extent, self.half_extent),
        );
        if self.boundary_m <= 0.0 {
            return self.datum_m;
        }
        let t = (self.region.distance_to([x, z]) / self.boundary_m).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        edge_height + (self.datum_m - edge_height) * t
    }
}

/// Mean-body sphere expressed as a tangent-plane height graph. This is the
/// globe side of the composed source, not a second terrain datum.
#[derive(Clone, Copy)]
struct MeanSphereSource {
    radius_m: f64,
}

/// An explicitly authored flat site surface. Its top face is the local terrain
/// datum; the existing composite source transitions from that finite footprint
/// to the mean-body sphere outside the footprint collar.
#[derive(Clone, Copy)]
struct FlatSurfaceSource {
    height_m: f64,
}

impl HeightSource for FlatSurfaceSource {
    fn height_at(&self, _x: f64, _z: f64) -> f64 {
        self.height_m
    }
}

impl HeightSource for MeanSphereSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        // The handoff coordinates are gnomonic (`x = R·tan(theta)`), not
        // orthographic tangent-plane coordinates. Convert that ray to its
        // exact radial intersection so the composed source and globe mesh use
        // the same sphere geometry at the collar's outer edge.
        let q = (1.0 + (x * x + z * z) / (self.radius_m * self.radius_m)).sqrt();
        self.radius_m / q - self.radius_m
    }
}

#[derive(Clone)]
enum SiteSurfaceSource {
    Dem(BoundarySiteSource),
    Flat(FlatSurfaceSource),
}

impl HeightSource for SiteSurfaceSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        match self {
            Self::Dem(source) => source.height_at(x, z),
            Self::Flat(source) => source.height_at(x, z),
        }
    }
}

#[derive(Clone)]
struct HandoffSource(CompositeHeightSource<SiteSurfaceSource, MeanSphereSource>);

impl HeightSource for HandoffSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        self.0.height_at(x, z)
    }
}

/// The one continuous site/globe ownership record for a body.
///
/// Inside the exact DEM square, the local terrain remains authoritative. Outside
/// it, the same composed source supplies a footprint-derived collar and then the
/// mean-body sphere. The component owns the source so globe tile generation never
/// reaches into terrain assets or invents a fallback height.
#[derive(Component, Clone)]
pub struct GlobeHandoff {
    pub dir: DVec3,
    pub east: DVec3,
    pub north: DVec3,
    pub half_extent: f64,
    pub radius_m: f64,
    pub blend_m: f64,
    source: HandoffSource,
    source_key: u64,
}

impl PartialEq for GlobeHandoff {
    fn eq(&self, other: &Self) -> bool {
        self.dir == other.dir
            && self.east == other.east
            && self.north == other.north
            && self.half_extent == other.half_extent
            && self.radius_m == other.radius_m
            && self.blend_m == other.blend_m
            && self.source_key == other.source_key
    }
}

impl GlobeHandoff {
    pub fn new(
        dir: DVec3,
        east: DVec3,
        north: DVec3,
        radius_m: f64,
        oracle: Arc<SurfaceOracle>,
        half_extent: f64,
    ) -> Self {
        // The DEM is in the body's absolute vertical datum. The body-scale
        // sagitta determines the source-to-sphere collar below; the finite DEM
        // source itself must only carry its measured edge relief through one
        // raster posting. Extending the edge sample across the whole collar
        // turns a real edge feature into an invented apron.
        let border_datum = oracle.grid().border_datum();
        let sagitta_distance = (2.0 * radius_m * border_datum.abs()).sqrt();
        let blend_m = half_extent.max(sagitta_distance).max(0.0);
        let region = Square {
            center: [0.0, 0.0],
            half: half_extent,
        };
        let source = HandoffSource(CompositeHeightSource::new(
            SiteSurfaceSource::Dem(BoundarySiteSource {
                oracle: oracle.clone(),
                region,
                half_extent,
                datum_m: border_datum,
                boundary_m: oracle.spacing() as f64,
            }),
            MeanSphereSource { radius_m },
            region,
            blend_m,
        ));
        Self {
            dir,
            east,
            north,
            half_extent,
            radius_m,
            blend_m,
            source,
            source_key: oracle.surface_key(),
        }
    }

    /// Compose an authored flat site cube with the mean-body sphere. The
    /// footprint is deliberately square because the globe clip contract is a
    /// square tangent-plane cutout; non-square authored geometry is rejected by
    /// the USD terrain projection before it reaches this constructor.
    pub fn new_flat(
        dir: DVec3,
        east: DVec3,
        north: DVec3,
        radius_m: f64,
        height_m: f64,
        half_extent: f64,
    ) -> Self {
        let sagitta_distance = (2.0 * radius_m * height_m.abs()).sqrt();
        let blend_m = half_extent.max(sagitta_distance).max(0.0);
        let region = Square {
            center: [0.0, 0.0],
            half: half_extent,
        };
        let source = HandoffSource(CompositeHeightSource::new(
            SiteSurfaceSource::Flat(FlatSurfaceSource { height_m }),
            MeanSphereSource { radius_m },
            region,
            blend_m,
        ));
        Self {
            dir,
            east,
            north,
            half_extent,
            radius_m,
            blend_m,
            source,
            source_key: height_m.to_bits() ^ half_extent.to_bits().rotate_left(17),
        }
    }

    fn geometry(&self) -> GlobeHandoffGeometry {
        GlobeHandoffGeometry {
            dir: self.dir,
            east: self.east,
            north: self.north,
            radius_m: self.radius_m,
            half_extent: self.half_extent,
            blend_m: self.blend_m,
        }
    }

    fn patch(&self) -> GlobeSurfacePatch<'_> {
        GlobeSurfacePatch {
            handoff: self.geometry(),
            source: &self.source,
        }
    }
}

/// The cube-sphere tiles currently resident for a body, keyed by quadtree node.
#[derive(Component, Default)]
pub struct GlobeTiles {
    /// Live tiles, including temporary coarse cover while desired replacements
    /// stream in.
    pub resident: HashMap<TileCoord, Entity>,
    /// Hidden tiles retained briefly after an atomic draw-cover handoff.
    ///
    /// Material-ready parent/child coverage is exchanged through visibility;
    /// the old entity is then kept hidden for two render extraction turns so
    /// removal cannot race the extracted render world. Parent and child are
    /// never drawable together.
    pub retiring: Vec<(Entity, u8, TileCoord)>,
    /// Reusable mesh handles for recently streamed tiles. The cache is bounded
    /// and only keeps handles that are not currently needed when it evicts; live
    /// entities keep their own handles, so releasing a cache entry cannot remove
    /// an in-use asset.
    mesh_cache: HashMap<TileCoord, CachedTileMesh>,
    mesh_cache_bytes: usize,
    cache_clock: u64,
    /// Camera position (body-local) the desired set was last solved AND fully
    /// realised at — the camera-motion gate for [`update_globe_lod`].
    ///
    /// `None` means "the last pass left work outstanding" (spawns still queued
    /// under the budget, or tiles still retiring), so the next frame must run
    /// regardless of camera motion. It is set to `Some` only when the resident
    /// set exactly covers the desired set with nothing retiring, i.e. when there
    /// is provably nothing for another pass to do.
    ///
    /// Entity-scoped (a field on the body's own component) rather than a
    /// `Local<HashMap<Entity, _>>` in the system, for the same reason
    /// `MissionSpawned` is (missions.rs): a `Local` outlives scene teardown and
    /// would keep stale keys for despawned bodies, while this dies with the body.
    pub last_solve_cam: Option<DVec3>,
    /// The [`GlobeHandoff`] in force at that solve. The handoff is an INPUT to
    /// the desired set, so a site appearing/moving must re-open the gate even if
    /// the camera has not moved a millimetre.
    pub last_solve_handoff: Option<GlobeHandoff>,
    /// Cached desired leaf set from the last selection pass. Selection depends
    /// on the camera, LOD parameters, handoff, and resident cover; it does not
    /// depend on material readiness or the retirement countdown. Keeping the
    /// result on the owning body lets streaming finish without recursively
    /// walking the whole globe again every frame.
    desired: HashSet<TileCoord>,
    last_selection_cam: Option<DVec3>,
    last_selection_handoff: Option<GlobeHandoff>,
    last_selection_resident_revision: u64,
    resident_revision: u64,
    last_selection_lod_key: Option<(u64, u32, u64)>,
}

#[derive(Clone)]
struct CachedTileMesh {
    handle: Handle<Mesh>,
    bytes: usize,
    last_used: u64,
}

/// Resource limits for live globe streaming.
///
/// These are resource values rather than hidden constants so a host can tune
/// them for a known adapter without changing the scene or the LOD algorithm.
/// The resident limit is a backpressure boundary: once reached, refinement
/// waits for old tiles to retire instead of allocating an unbounded replacement
/// set.
#[derive(Resource, Clone, Copy, Debug)]
pub struct GlobeLodBudget {
    /// Maximum fresh tile entities created for one body in one frame.
    pub spawn_tiles_per_frame: usize,
    /// Maximum retired tile entities released for one body in one frame.
    pub despawn_tiles_per_frame: usize,
    /// Approximate mesh bytes allowed for resident and retiring tile entities.
    pub max_resident_mesh_bytes: usize,
    /// Approximate bytes retained by the reusable mesh-handle cache.
    pub max_cached_mesh_bytes: usize,
    /// Fresh mesh bytes allowed in one frame, independent of entity count.
    pub max_fresh_mesh_bytes_per_frame: usize,
}

impl Default for GlobeLodBudget {
    fn default() -> Self {
        Self {
            spawn_tiles_per_frame: 16,
            despawn_tiles_per_frame: 32,
            max_resident_mesh_bytes: 64 * 1024 * 1024,
            max_cached_mesh_bytes: 16 * 1024 * 1024,
            max_fresh_mesh_bytes_per_frame: 4 * 1024 * 1024,
        }
    }
}

/// Camera motion, as a fraction of its ALTITUDE above the body, below which the
/// desired tile set cannot have changed enough to be worth recomputing.
///
/// Altitude and not distance-to-centre because altitude is what drives
/// refinement: `subdivide_face` splits when the camera is nearer than the tile's
/// arc-size times `lod_distance_factor`, and near the surface that distance IS
/// the altitude. A 1% change in it can only flip a tile already within 1% of its
/// split threshold — and those are exactly the tiles the resident-set dead band
/// already holds steady, so no tile changes state that would not have flapped
/// anyway. The threshold collapses to zero as the camera approaches the surface,
/// where the gate matters least and precision matters most.
const LOD_CAMERA_MOTION_FRACTION: f64 = 0.01;

/// Squared camera distance to a tile's centre (body-local) — spawn priority.
fn tile_dist2(coord: &TileCoord, radius_m: f64, camera_body_local: DVec3) -> f64 {
    let (u, v) = tile_center_uv(coord.face, coord.level, coord.i, coord.j);
    (cube_to_sphere(coord.face, u, v) * radius_m).distance_squared(camera_body_local)
}

/// Conservative CPU/GPU accounting for the mesh layout produced by
/// `create_quadsphere_tile_mesh` (position, normal, UV and u32 indices).
fn tile_mesh_bytes(res: u32) -> usize {
    let side = res as usize + 1;
    let vertices = side.saturating_mul(side);
    let indices = (res as usize)
        .saturating_mul(res as usize)
        .saturating_mul(6);
    vertices
        .saturating_mul((3 + 3 + 2) * std::mem::size_of::<f32>())
        .saturating_add(indices.saturating_mul(std::mem::size_of::<u32>()))
}

fn evict_unused_mesh_cache(tiles: &mut GlobeTiles, budget: &GlobeLodBudget) {
    if tiles.mesh_cache_bytes <= budget.max_cached_mesh_bytes {
        return;
    }

    let in_use: HashSet<TileCoord> = tiles
        .resident
        .keys()
        .copied()
        .chain(tiles.retiring.iter().map(|(_, _, coord)| *coord))
        .collect();
    let mut candidates: Vec<(TileCoord, u64)> = tiles
        .mesh_cache
        .iter()
        .filter(|(coord, _)| !in_use.contains(coord))
        .map(|(coord, cached)| (*coord, cached.last_used))
        .collect();
    candidates.sort_unstable_by_key(|(_, last_used)| *last_used);

    for (coord, _) in candidates {
        if tiles.mesh_cache_bytes <= budget.max_cached_mesh_bytes {
            break;
        }
        if let Some(cached) = tiles.mesh_cache.remove(&coord) {
            tiles.mesh_cache_bytes = tiles.mesh_cache_bytes.saturating_sub(cached.bytes);
        }
    }
}

fn branch_has_gap(
    desired: &HashSet<TileCoord>,
    resident: &HashMap<TileCoord, Entity>,
    resident_coverage: &HashSet<TileCoord>,
    face: u8,
) -> bool {
    desired.iter().filter(|tile| tile.face == face).any(|leaf| {
        let mut ancestor = Some(*leaf);
        let covered_by_ancestor = std::iter::from_fn(|| {
            let current = ancestor?;
            ancestor = tile_parent(current);
            Some(current)
        })
        .any(|ancestor| resident.contains_key(&ancestor));
        !covered_by_ancestor && !resident_coverage.contains(leaf)
    })
}

/// Index every resident tile and its ancestors once, so coverage checks do not
/// scan every resident tile for every desired leaf.
fn resident_coverage(resident: &HashMap<TileCoord, Entity>) -> HashSet<TileCoord> {
    let mut coverage = HashSet::new();
    for coord in resident.keys().copied() {
        let mut current = Some(coord);
        while let Some(tile) = current {
            coverage.insert(tile);
            current = tile_parent(tile);
        }
    }
    coverage
}

fn tile_parent(coord: TileCoord) -> Option<TileCoord> {
    (coord.level > 0).then(|| TileCoord {
        body: coord.body,
        face: coord.face,
        level: coord.level - 1,
        i: coord.i >> 1,
        j: coord.j >> 1,
    })
}

fn tile_children(coord: TileCoord) -> [TileCoord; 4] {
    let level = coord.level + 1;
    [
        TileCoord {
            body: coord.body,
            face: coord.face,
            level,
            i: coord.i * 2,
            j: coord.j * 2,
        },
        TileCoord {
            body: coord.body,
            face: coord.face,
            level,
            i: coord.i * 2 + 1,
            j: coord.j * 2,
        },
        TileCoord {
            body: coord.body,
            face: coord.face,
            level,
            i: coord.i * 2,
            j: coord.j * 2 + 1,
        },
        TileCoord {
            body: coord.body,
            face: coord.face,
            level,
            i: coord.i * 2 + 1,
            j: coord.j * 2 + 1,
        },
    ]
}

/// Collect an exact ready cover at or below `coord`.
///
/// Prefer the node itself. If it is not drawable, all four child branches must
/// be drawable; a partial child set is not a cover and must fall back to an
/// ancestor instead. `max_level` bounds recursion to the deepest resident tile.
fn collect_ready_subtree(
    coord: TileCoord,
    resident: &HashMap<TileCoord, Entity>,
    ready: &HashSet<TileCoord>,
    max_level: u32,
    out: &mut Vec<TileCoord>,
) -> bool {
    if resident.contains_key(&coord) && ready.contains(&coord) {
        out.push(coord);
        return true;
    }
    if coord.level >= max_level {
        return false;
    }
    let start = out.len();
    for child in tile_children(coord) {
        if !collect_ready_subtree(child, resident, ready, max_level, out) {
            out.truncate(start);
            return false;
        }
    }
    true
}

/// Build the one disjoint drawable cover for a desired quadtree leaf set.
///
/// A resident entity is only a resource allocation. It becomes drawable after
/// [`ShaderLookReady`] proves its complete shader/texture state. Refinement keeps
/// the nearest ready ancestor until all replacement branches are ready;
/// coarsening keeps the complete ready child cover until the parent is ready.
/// Parent and child are therefore never visible together.
fn build_draw_cover(
    desired: &HashSet<TileCoord>,
    resident: &HashMap<TileCoord, Entity>,
    ready: &HashSet<TileCoord>,
) -> HashSet<TileCoord> {
    let max_level = resident.keys().map(|coord| coord.level).max().unwrap_or(0);
    let mut draw = HashSet::new();
    let mut subtree = Vec::new();
    for leaf in desired {
        subtree.clear();
        if collect_ready_subtree(*leaf, resident, ready, max_level, &mut subtree) {
            draw.extend(subtree.iter().copied());
            continue;
        }
        let mut ancestor = tile_parent(*leaf);
        while let Some(coord) = ancestor {
            if resident.contains_key(&coord) && ready.contains(&coord) {
                draw.insert(coord);
                break;
            }
            ancestor = tile_parent(coord);
        }
    }

    // Desired leaves can share a fallback ancestor. If one branch inserted an
    // ancestor after another inserted descendants, retain the ancestor only.
    let snapshot: Vec<TileCoord> = draw.iter().copied().collect();
    for coord in snapshot {
        let mut ancestor = tile_parent(coord);
        while let Some(parent) = ancestor {
            if draw.contains(&parent) {
                draw.remove(&coord);
                break;
            }
            ancestor = tile_parent(parent);
        }
    }
    draw
}

/// Resolve the active camera in a body's rotating surface Grid.
///
/// Globe selection is simulation-space work: it chooses persistent tile
/// identities and therefore must use BigSpace's authoritative
/// `(CellCoord, Transform)` hierarchy. [`GlobalTransform`] is a camera-relative
/// f32 render product; reconstructing a cross-body position from two of them
/// quantizes the Earth-Moon baseline and can make the selected quadtree branch
/// alternate as the floating origin moves.
fn camera_position_in_surface_grid(
    camera: Entity,
    surface_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<DVec3> {
    lunco_core::coords::pose_in_grid(camera, surface_grid, q_parents, q_grids, q_spatial)
        .map(|(position, _)| position)
}

/// Per-frame: stream each body's cube-sphere tile set against the camera.
pub(crate) fn update_globe_lod(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    budget: Res<GlobeLodBudget>,
    // `SceneViewport` is the presentation owner. Reading `Camera::is_active`
    // here would make simulation-space LOD depend on a render-side actuation
    // that occurs later in PostUpdate, and would reintroduce a startup race.
    viewport: Res<SceneViewport>,
    // `With<SceneCamera>`, NOT `With<Camera3d>`: "which entity is the scene camera?"
    // is a render-FREE question, and asking it with `Camera3d` was what made this
    // crate link bevy_core_pipeline → wgpu. See `lunco_render::camera`.
    cameras: Query<(Entity, &bevy::camera::RenderTarget), With<SceneCamera>>,
    q_parents: Query<&ChildOf>,
    grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    material_ready: Query<(), With<ShaderLookReady>>,
    visibility: Query<&Visibility>,
    mut bodies: Query<(Entity, &GlobeLod, &mut GlobeTiles, Option<&GlobeHandoff>)>,
) {
    // ONLY the explicitly bound window camera may steer the LOD. The binding is
    // intentionally allowed to be absent while a scene is projecting; an empty
    // cover during that lifecycle interval is correct and must not panic the app.
    let Some(camera_entity) = viewport.active_camera.filter(|entity| {
        cameras
            .get(*entity)
            .is_ok_and(|(_, target)| matches!(target, bevy::camera::RenderTarget::Window(_)))
    }) else {
        return;
    };

    for (body_ent, lod, mut tiles, handoff) in &mut bodies {
        // Camera relative to the body centre in the rotating frame the tiles
        // live in. This is an f64 cross-grid conversion through BigSpace's
        // authoritative cells. LOD identity must never be inferred from the
        // lossy, floating-origin-relative render `GlobalTransform` projection.
        let camera_body_local = camera_position_in_surface_grid(
            camera_entity,
            lod.surface_grid,
            &q_parents,
            &grids,
            &q_spatial,
        )
        .unwrap_or_else(|| {
            panic!(
                "globe LOD camera {camera_entity:?} and surface Grid {:?} are not connected through one BigSpace hierarchy",
                lod.surface_grid
            )
        });
        let sg_grid = grids.get(lod.surface_grid).unwrap_or_else(|_| {
            panic!(
                "GlobeLod on body {body_ent:?} names {:?} as its surface Grid, but that entity has no Grid component",
                lod.surface_grid
            )
        });

        // A handoff changes the geometry of every resident tile that crosses its
        // boundary. Retire the old meshes before solving the new cover so a
        // cached uncut globe tile cannot survive under the local DEM.
        let handoff_changed = tiles.last_solve_handoff.as_ref() != handoff;
        if handoff_changed {
            debug!(
                "globe LOD handoff solve: body={body_ent:?} camera={camera_entity:?} surface_grid={:?} body_local={camera_body_local:?} radius={:.0} handoff={}",
                lod.surface_grid,
                lod.radius_m,
                handoff.is_some()
            );
            for (_, entity) in tiles.resident.drain() {
                commands.entity(entity).try_despawn();
            }
            for (entity, _, _) in tiles.retiring.drain(..) {
                commands.entity(entity).try_despawn();
            }
            tiles.mesh_cache.clear();
            tiles.mesh_cache_bytes = 0;
            tiles.last_solve_cam = None;
            tiles.desired.clear();
            tiles.last_selection_cam = None;
            tiles.last_selection_handoff = None;
            tiles.last_selection_lod_key = None;
            tiles.resident_revision = tiles.resident_revision.wrapping_add(1);
        }

        // CAMERA-MOTION GATE. Resident reconciliation still runs while a
        // replacement is streaming or retiring. Once it settles, the recursive
        // selection result is cached and a parked view only performs the cheap
        // readiness/cover checks below. The selection itself is a pure function
        // of (camera_body_local, handoff, resident set, and LOD parameters).
        //
        // This is the same shape as the cadence gate the ephemeris cluster uses
        // (`cadence::tracked_needs_solve`) — an error budget rather than a rate —
        // but it cannot BE that gate: the tile set depends on the CAMERA, which
        // no epoch tolerance can see. A body's LOD must react to a camera that
        // moves while the clock is paused.
        let all_resident_ready = tiles
            .resident
            .values()
            .all(|entity| material_ready.contains(*entity));
        if let Some(prev_cam) = tiles.last_solve_cam {
            let altitude = (camera_body_local.length() - lod.radius_m).abs().max(1.0);
            let slack = LOD_CAMERA_MOTION_FRACTION * altitude;
            if tiles.last_solve_handoff.as_ref() == handoff
                && all_resident_ready
                && tiles.retiring.is_empty()
                && (camera_body_local - prev_cam).length_squared() < slack * slack
            {
                continue;
            }
        }

        // Desired leaf set: recurse all six faces from the root. The resident
        // set feeds the split/merge dead band (no per-frame flapping when the
        // camera parks exactly on a threshold — e.g. the 3.0-radii focus snap).
        // Once a selection is computed, keep it on the owning body while the
        // resident set catches up. Material readiness and retirement do not
        // change the mathematical selection, so they must not force another
        // full quadtree walk.
        let lod_key = (
            lod.radius_m.to_bits(),
            lod.max_lod,
            lod.lod_distance_factor.to_bits(),
        );
        let selection_needs_rebuild = tiles.last_selection_cam.is_none_or(|previous| {
            let altitude = (camera_body_local.length() - lod.radius_m).abs().max(1.0);
            let slack = LOD_CAMERA_MOTION_FRACTION * altitude;
            (camera_body_local - previous).length_squared() >= slack * slack
        }) || tiles.last_selection_handoff.as_ref() != handoff
            || tiles.last_selection_resident_revision != tiles.resident_revision
            || tiles.last_selection_lod_key != Some(lod_key);
        let resident: HashSet<TileCoord> = tiles.resident.keys().copied().collect();
        let resident_coverage = resident_coverage(&tiles.resident);
        let desired = if selection_needs_rebuild {
            let mut desired = HashSet::new();
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
            tiles.desired = desired.clone();
            tiles.last_selection_cam = Some(camera_body_local);
            tiles.last_selection_handoff = handoff.cloned();
            tiles.last_selection_resident_revision = tiles.resident_revision;
            tiles.last_selection_lod_key = Some(lod_key);
            desired
        } else {
            tiles.desired.clone()
        };

        // Site DEM handoff is resolved by exact per-triangle clipping in the
        // globe mesh. Keep the quadtree cover intact: a tile's spherical
        // triangles are not the local terrain square, and retiring a tile from
        // a few direction samples can leave an uncovered outside sliver.

        // Spawn newly-desired tiles FIRST (so this frame's spawns count as
        // coverage for retirement below), BUDGETED per frame by
        // `GlobeLodBudget`. Coarse-and-near first: a coarse tile covers the
        // most area (unblocks the most retirements), a near tile is what the
        // viewer is looking at. Meshes are centre-relative and entities are
        // anchored at their tile centre through the surface grid.
        let mut missing: HashSet<TileCoord> = desired
            .iter()
            .filter(|c| !tiles.resident.contains_key(c))
            .copied()
            .collect();
        // A budgeted globe must never bootstrap with only refined leaves: until
        // every leaf in a branch is resident, its root is the exact coarse cover
        // for that branch. Without this fallback the first 16 child meshes can
        // leave the other faces visibly black for several frames. The root is
        // intentionally not added to `desired`; normal retirement below keeps it
        // until all overlapping desired leaves are present, then removes it.
        for face in 0..6u8 {
            let root = TileCoord {
                body: body_ent,
                face,
                level: 0,
                i: 0,
                j: 0,
            };
            if desired.contains(&root) || tiles.resident.contains_key(&root) {
                continue;
            }
            if branch_has_gap(&desired, &tiles.resident, &resident_coverage, face) {
                missing.insert(root);
            }
        }
        let mut prioritized: Vec<(TileCoord, f64)> = missing
            .into_iter()
            .map(|coord| {
                let distance = tile_dist2(&coord, lod.radius_m, camera_body_local);
                (coord, distance)
            })
            .collect();
        prioritized.sort_unstable_by(|(a, a_distance), (b, b_distance)| {
            a.level
                .cmp(&b.level)
                .then_with(|| a_distance.total_cmp(b_distance))
                .then_with(|| a.face.cmp(&b.face))
                .then_with(|| a.i.cmp(&b.i))
                .then_with(|| a.j.cmp(&b.j))
        });
        // Initial fill is budgeted too. A scene load must not synchronously
        // allocate the whole finest visible shell before the render thread can
        // upload anything; the same backpressure applies at every camera range.
        let tile_bytes = tile_mesh_bytes(lod.res);
        let mut fresh_bytes = 0usize;
        for (coord, _) in prioritized.into_iter().take(budget.spawn_tiles_per_frame) {
            let needs_fresh_mesh = !tiles.mesh_cache.contains_key(&coord);
            if needs_fresh_mesh
                && (fresh_bytes.saturating_add(tile_bytes) > budget.max_fresh_mesh_bytes_per_frame
                    || (tiles.resident.len() + tiles.retiring.len())
                        .saturating_mul(tile_bytes)
                        .saturating_add(fresh_bytes)
                        .saturating_add(tile_bytes)
                        > budget.max_resident_mesh_bytes)
            {
                break;
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
            tiles.cache_clock = tiles.cache_clock.wrapping_add(1);
            let cache_clock = tiles.cache_clock;
            let mesh_handle = if let Some(cached) = tiles.mesh_cache.get_mut(&coord) {
                cached.last_used = cache_clock;
                cached.handle.clone()
            } else {
                let mesh = create_quadsphere_tile_mesh(
                    body_ent,
                    coord.face,
                    coord.level,
                    coord.i,
                    coord.j,
                    lod.radius_m,
                    lod.res,
                    tile_body_local,
                    handoff.map(GlobeHandoff::patch),
                );
                let handle = meshes.add(mesh);
                tiles.mesh_cache.insert(
                    coord,
                    CachedTileMesh {
                        handle: handle.clone(),
                        bytes: tile_bytes,
                        last_used: cache_clock,
                    },
                );
                tiles.mesh_cache_bytes = tiles.mesh_cache_bytes.saturating_add(tile_bytes);
                fresh_bytes = fresh_bytes.saturating_add(tile_bytes);
                handle
            };
            // Atomic (ChildOf, CellCoord, Transform): the grid-local pose is
            // authored at spawn, so no render-derived GlobalTransform is needed
            // to establish the tile's BigSpace placement.
            let ent = commands
                .spawn((
                    Mesh3d(mesh_handle),
                    lod.look.clone(),
                    coord,
                    TerrainTile,
                    tile_cell,
                    Transform::from_translation(tile_local_pos),
                    GlobalTransform::default(),
                    // Residency and drawability are separate. The render binder
                    // promotes this entity with `ShaderLookReady`; the disjoint
                    // cover below then swaps visibility atomically with its
                    // parent/children.
                    Visibility::Hidden,
                    InheritedVisibility::default(),
                    // The tile's BigSpace placement is immutable for its entire
                    // residency. LOD changes replace tiles by despawning/spawning
                    // them; they never move an existing tile. Let BigSpace's
                    // built-in stationary path skip this high-precision leaf
                    // while still allowing floating-origin updates.
                    Stationary,
                    // NO `NoFrustumCulling`. It was here from the era when tile
                    // meshes were built at full body-local magnitude (vertices
                    // ~radius from the entity origin) — an AABB that big and that
                    // badly centred culls wrongly, and switching it off hid the
                    // symptom. Meshes are CENTRE-RELATIVE now (see the note at
                    // `create_quadsphere_tile_mesh` below), so each tile's AABB is
                    // a tight box about its own origin and ordinary culling is
                    // correct — which is how `lunco-terrain-surface`'s CDLOD tiles,
                    // grid-direct children with their own `CellCoord` and the same
                    // cell-local mesh convention, have always rendered. With ~600
                    // resident tiles per body and most of them on the far side of
                    // the sphere or off-screen, submitting the whole set every
                    // frame was pure draw-call overhead.
                    //
                    // The globe is a FEATURELESS sphere of planetary size; as a
                    // shadow caster it contributes nothing (its night side is
                    // dark by shading) but at grazing sun elevations (+2.6° at
                    // Malapert) a site merged onto the sphere sits exactly in
                    // the shadow map's terminator/acne zone — the whole scene
                    // flipped lit↔dark frame to frame ("still blinking"). Same
                    // treatment as the Sun body mesh.
                    bevy::light::NotShadowCaster,
                    Name::new(format!(
                        "Globe tile f{} L{} {},{}",
                        coord.face, coord.level, coord.i, coord.j
                    )),
                    // Streamed runtime detail — hidden from author-facing lists.
                    lunco_core::SystemManaged,
                    ChildOf(lod.surface_grid),
                ))
                .id();
            tiles.resident.insert(coord, ent);
            tiles.resident_revision = tiles.resident_revision.wrapping_add(1);
        }

        let ready: HashSet<TileCoord> = tiles
            .resident
            .iter()
            .filter_map(|(coord, entity)| material_ready.contains(*entity).then_some(*coord))
            .collect();
        let draw = build_draw_cover(&desired, &tiles.resident, &ready);

        // Visibility is one exact quadtree partition. This is the critical LOD
        // invariant: no coplanar parent/child overlap (z-fighting/brightness
        // squares), and no newly-created but materially-unready replacement.
        for (coord, entity) in &tiles.resident {
            let target = if draw.contains(coord) {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if visibility
                .get(*entity)
                .is_ok_and(|current| *current == target)
            {
                continue;
            }
            commands.entity(*entity).try_insert(target);
        }

        // A non-desired tile remains resident only while it is part of the
        // drawable fallback cover. Once a complete replacement is ready, hide
        // it in the same command batch that reveals the replacement, then keep
        // the hidden entity alive for two extraction turns before despawning.
        let mut newly_retired: Vec<(Entity, u8, TileCoord)> = Vec::new();
        let resident_count = tiles.resident.len();
        tiles.resident.retain(|coord, ent| {
            if desired.contains(coord) || draw.contains(coord) {
                return true;
            }
            if !visibility
                .get(*ent)
                .is_ok_and(|current| *current == Visibility::Hidden)
            {
                commands.entity(*ent).try_insert(Visibility::Hidden);
            }
            newly_retired.push((*ent, 2, *coord));
            false
        });
        if tiles.resident.len() != resident_count {
            tiles.resident_revision = tiles.resident_revision.wrapping_add(1);
        }
        tiles.retiring.extend(newly_retired);
        let mut despawned = 0usize;
        tiles.retiring.retain_mut(|(ent, frames, _coord)| {
            if *frames == 0 {
                if despawned < budget.despawn_tiles_per_frame {
                    commands.entity(*ent).try_despawn();
                    despawned += 1;
                    return false;
                }
                // Over budget — despawn on a later frame.
                return true;
            }
            *frames -= 1;
            true
        });
        evict_unused_mesh_cache(&mut tiles, &budget);

        // Arm the camera-motion gate only when the desired set itself is the
        // complete ready draw cover. Residency without material readiness is not
        // settled, and a fallback ancestor/descendant must keep reconciliation
        // running until its exact replacement can take ownership.
        let settled = tiles.retiring.is_empty()
            && draw == desired
            && desired.iter().all(|coord| ready.contains(coord));
        tiles.last_solve_cam = settled.then_some(camera_body_local);
        tiles.last_solve_handoff = handoff.cloned();

        // `LUNCO_LOD_VALIDATE=1`: assert the resident set still covers the
        // whole sphere after this frame's spawn/retire pass (the invariant
        // the budgeted streaming must never break). Ground truth for hole
        // reports — API-side entity censuses are ambiguous (registry lag,
        // retiring-tile overlap, cross-body name collisions).
        if std::env::var("LUNCO_LOD_VALIDATE").is_ok() {
            let resident: HashSet<TileCoord> = tiles.resident.keys().copied().collect();
            fn covered(
                set: &HashSet<TileCoord>,
                body: Entity,
                face: u8,
                level: u32,
                i: i32,
                j: i32,
            ) -> bool {
                if set.contains(&TileCoord {
                    body,
                    face,
                    level,
                    i,
                    j,
                }) {
                    return true;
                }
                if level > 12 {
                    return false;
                }
                (0..2).all(|di| {
                    (0..2).all(|dj| covered(set, body, face, level + 1, i * 2 + di, j * 2 + dj))
                })
            }
            for face in 0..6u8 {
                if !covered(&resident, body_ent, face, 0, 0, 0) {
                    warn!(
                        "globe LOD hole: body {body_ent} face {face} uncovered ({} resident, {} retiring)",
                        resident.len(),
                        tiles.retiring.len()
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemState;

    #[test]
    fn cross_body_lod_camera_uses_the_authoritative_big_space_pose() {
        let mut world = World::new();
        let grid = lunco_core::WorldGridConfig::default().grid();
        let root = world.spawn(grid.clone()).id();

        let earth_center = DVec3::new(-4_671_234.375, 81_234.625, -19_876.125);
        let earth_rotation = Quat::from_rotation_y(1.234_567);
        let (earth_cell, earth_local) = grid.translation_to_grid(earth_center);
        let earth_fixed = world
            .spawn((
                grid.clone(),
                earth_cell,
                Transform::from_translation(earth_local).with_rotation(earth_rotation),
                ChildOf(root),
            ))
            .id();
        let earth_surface = world
            .spawn((
                grid.clone(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(earth_fixed),
            ))
            .id();

        let moon_center = DVec3::new(379_728_918.812_5, -43_210.437_5, 77_777.312_5);
        let moon_rotation = Quat::from_rotation_y(-0.456_789);
        let (moon_cell, moon_local) = grid.translation_to_grid(moon_center);
        let moon_fixed = world
            .spawn((
                grid.clone(),
                moon_cell,
                Transform::from_translation(moon_local).with_rotation(moon_rotation),
                ChildOf(root),
            ))
            .id();
        let moon_surface = world
            .spawn((
                grid.clone(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(moon_fixed),
            ))
            .id();

        let camera_moon_local = DVec3::new(123.456_789, 1_737_412.345_678, -987.654_321);
        let (camera_cell, camera_local) = grid.translation_to_grid(camera_moon_local);
        let camera = world
            .spawn((
                camera_cell,
                Transform::from_translation(camera_local),
                ChildOf(moon_surface),
            ))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let actual = {
            let (q_parents, q_grids, q_spatial) = state.get(&world).unwrap();
            camera_position_in_surface_grid(camera, earth_surface, &q_parents, &q_grids, &q_spatial)
                .expect("the Moon camera and Earth surface share the celestial BigSpace")
        };
        let expected_root = moon_center
            + moon_rotation.as_dquat()
                * grid
                    .grid_position_double(&camera_cell, &Transform::from_translation(camera_local));
        let expected =
            earth_rotation.as_dquat().normalize().inverse() * (expected_root - earth_center);

        assert!(
            actual.distance(expected) < 1.0e-3,
            "cross-body camera pose lost BigSpace precision: actual={actual:?}, expected={expected:?}, error={} m",
            actual.distance(expected)
        );

        // Re-express the exact same camera pose across a cell boundary. The
        // selected globe position is physical state, so BigSpace recentering
        // must not perturb it or trigger a different LOD branch.
        world.entity_mut(camera).insert((
            CellCoord::new(camera_cell.x + 1, camera_cell.y, camera_cell.z),
            Transform::from_translation(camera_local - Vec3::X * grid.cell_edge_length()),
        ));
        let after_recenter = {
            let (q_parents, q_grids, q_spatial) = state.get(&world).unwrap();
            camera_position_in_surface_grid(camera, earth_surface, &q_parents, &q_grids, &q_spatial)
                .unwrap()
        };
        assert!(
            after_recenter.distance(actual) < 1.0e-3,
            "camera LOD pose changed across an equivalent BigSpace cell expression: before={actual:?}, after={after_recenter:?}"
        );
    }

    #[test]
    fn finite_dem_edge_relief_is_not_extruded_into_the_globe_collar() {
        use lunco_obstacle_field::field::HeightGrid;

        let mut grid = HeightGrid::new_flat(3, 10.0);
        grid.heights.fill(100.0);
        grid.heights[2 * 3 + 2] = 200.0;
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(grid)));
        let region = Square {
            center: [0.0, 0.0],
            half: 10.0,
        };
        let source = BoundarySiteSource {
            oracle,
            region,
            half_extent: 10.0,
            datum_m: 100.0,
            boundary_m: 10.0,
        };

        assert_eq!(source.height_at(10.0, 10.0), 200.0);
        assert_eq!(source.height_at(20.0, 20.0), 100.0);
        assert!(source.height_at(15.0, 15.0) < 200.0);
    }

    #[test]
    fn flat_site_handoff_owns_the_authored_datum_inside_its_square() {
        let handoff = GlobeHandoff::new_flat(DVec3::X, DVec3::Z, DVec3::Y, 1_737_400.0, 0.0, 100.0);
        assert!(handoff
            .geometry()
            .contains(DVec3::new(1.0, 0.00001, 0.00001).normalize()));
        assert!(!handoff
            .geometry()
            .contains(DVec3::new(1.0, 0.001, 0.0).normalize()));
        let patch = handoff.patch();
        assert_eq!(patch.source.height_at(0.0, 0.0), 0.0);
        assert_eq!(patch.source.height_at(50.0, -50.0), 0.0);
    }

    #[test]
    fn a_missing_refined_branch_requires_a_coarse_cover() {
        let body = Entity::PLACEHOLDER;
        let leaf = TileCoord {
            body,
            face: 3,
            level: 2,
            i: 1,
            j: 2,
        };
        let root = TileCoord {
            body,
            face: 3,
            level: 0,
            i: 0,
            j: 0,
        };
        let desired = HashSet::from([leaf]);
        let mut resident = HashMap::new();
        let coverage = resident_coverage(&resident);
        assert!(branch_has_gap(&desired, &resident, &coverage, 3));
        resident.insert(root, Entity::PLACEHOLDER);
        let coverage = resident_coverage(&resident);
        assert!(!branch_has_gap(&desired, &resident, &coverage, 3));

        resident.clear();
        resident.insert(
            TileCoord {
                body,
                face: 3,
                level: 3,
                i: 2,
                j: 4,
            },
            Entity::PLACEHOLDER,
        );
        let coverage = resident_coverage(&resident);
        assert!(!branch_has_gap(&desired, &resident, &coverage, 3));
    }

    fn face_root(body: Entity) -> TileCoord {
        TileCoord {
            body,
            face: 0,
            level: 0,
            i: 0,
            j: 0,
        }
    }

    #[test]
    fn refinement_draws_one_disjoint_cover_only_after_all_children_are_ready() {
        let body = Entity::PLACEHOLDER;
        let root = face_root(body);
        let children = tile_children(root);
        let desired = HashSet::from(children);
        let mut resident = HashMap::from([(root, Entity::PLACEHOLDER)]);
        let mut ready = HashSet::from([root]);

        assert!(build_draw_cover(&desired, &resident, &ready) == HashSet::from([root]));

        for child in children.into_iter().take(3) {
            resident.insert(child, Entity::PLACEHOLDER);
            ready.insert(child);
        }
        assert!(
            build_draw_cover(&desired, &resident, &ready) == HashSet::from([root]),
            "a partial child set must not overlap its drawable parent"
        );

        let last = children[3];
        resident.insert(last, Entity::PLACEHOLDER);
        ready.insert(last);
        assert!(
            build_draw_cover(&desired, &resident, &ready) == desired,
            "the complete ready child cover must atomically replace its parent"
        );
    }

    #[test]
    fn coarsening_keeps_complete_ready_children_until_parent_is_ready() {
        let body = Entity::PLACEHOLDER;
        let root = face_root(body);
        let children = tile_children(root);
        let desired = HashSet::from([root]);
        let mut resident = HashMap::new();
        let mut ready = HashSet::new();
        for child in children {
            resident.insert(child, Entity::PLACEHOLDER);
            ready.insert(child);
        }

        assert!(
            build_draw_cover(&desired, &resident, &ready) == HashSet::from(children),
            "coarsening must retain the previous exact cover, not expose a hole"
        );

        resident.insert(root, Entity::PLACEHOLDER);
        ready.insert(root);
        assert!(
            build_draw_cover(&desired, &resident, &ready) == HashSet::from([root]),
            "a ready parent must atomically replace all children"
        );
    }
}
