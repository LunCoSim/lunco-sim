//! Camera-driven cube-sphere **live LOD** for celestial bodies (globe scale).
//!
//! Recursive quadtree subdivision per face refines near the camera and coarsens
//! far away, so a body shows planetary curvature from orbit and finer relief as
//! you approach. The selection is the globe crate's sphere-correct
//! `subdivide_face` (camera distance vs tile arc-size); this module integrates
//! the selection with body-owned textures, grids, and appearance intent.
//!
//! Per body, [`GlobeLod`] carries the params + the physical surface grid + look;
//! [`GlobeTiles`] tracks residency, the bounded mesh cache, and the cached
//! selection inputs; [`update_globe_lod`] reconciles that state with the camera.
//! Tile placement uses the grid's `translation_to_grid` together with a
//! centre-relative mesh, so the authoritative BigSpace pose is established at
//! spawn and remains stable while only tile residency changes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use big_space::prelude::*;
use lunco_materials::{ShaderLook, ShaderLookReady};
use lunco_render::SceneCamera;
use lunco_terrain_core::{
    HeightSource, Square, normal_at_bounded, square_boundary_height_at,
    square_boundary_posting_spacing,
};
use lunco_terrain_globe::quad_sphere::{
    balance_cube_sphere_lod, cube_to_sphere, subdivide_face, tile_center_uv,
};
use lunco_terrain_globe::{
    GlobeCutout, GlobeHandoff as GlobeHandoffGeometry, TerrainTile, TileCoord,
    create_quadsphere_tile_mesh, create_square_handoff_collar_mesh,
};
use lunco_terrain_surface::SurfaceOracle;
use lunco_viewport_core::SceneViewport;

/// Per-body live-LOD context read by the runtime LOD system to stream
/// cube-sphere tiles.
#[derive(Component)]
pub struct GlobeLod {
    /// Canonical body radius (m); a site handoff may supply a render-only datum radius.
    pub radius_m: f64,
    /// Body-fixed physical grid for sites, terrain, cameras, and vehicles.
    ///
    /// This grid is intentionally outside the render interpolation branch.
    /// It is the authoritative local frame for authored surface content and
    /// remains at the authoritative physical tick.
    pub surface_grid: Entity,
    /// Appearance intent applied to every tile (the body's blueprint look). Cloned
    /// onto each tile; the binder's content-keyed cache shares one
    /// `ShaderMaterial` per body.
    pub look: ShaderLook,
    /// Vertices per tile side.
    pub res: u32,
    /// Deepest subdivision level near the camera.
    pub max_lod: u32,
    /// `refine when dist < tile_arc · factor` — larger = refine from farther.
    pub lod_distance_factor: f64,
}

/// The site's surface values relative to its render datum.
#[derive(Clone)]
enum SiteSurfaceSource {
    Dem {
        oracle: Arc<SurfaceOracle>,
        datum_m: f64,
    },
    Flat {
        height_m: f64,
        datum_m: f64,
    },
}

impl HeightSource for SiteSurfaceSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        match self {
            Self::Dem { oracle, datum_m } => oracle.height_at(x, z) - datum_m,
            Self::Flat { height_m, datum_m } => height_m - datum_m,
        }
    }
}

impl SiteSurfaceSource {
    fn boundary_height(&self, x: f64, z: f64, half_extent: f64, resolution: usize) -> f64 {
        match self {
            Self::Dem { oracle, datum_m } => {
                square_boundary_height_at(oracle.as_ref(), x, z, half_extent, resolution)
                    .unwrap_or_else(|| oracle.height_at(x, z))
                    - datum_m
            }
            Self::Flat { height_m, datum_m } => height_m - datum_m,
        }
    }

    fn boundary_gradient(&self, x: f64, z: f64, half_extent: f64) -> [f64; 2] {
        match self {
            Self::Dem { oracle, .. } => {
                let normal =
                    normal_at_bounded(oracle.as_ref(), x, z, oracle.spacing() as f64, half_extent);
                [-normal[0] / normal[1], -normal[2] / normal[1]]
            }
            Self::Flat { .. } => [0.0, 0.0],
        }
    }
}

/// The active crop's datum-aligned render shell in its orthographic tangent chart.
#[derive(Clone, Copy)]
struct MeanSphereSource {
    radius_m: f64,
}

impl MeanSphereSource {
    fn gradient_at(&self, x: f64, z: f64) -> [f64; 2] {
        let radial = (self.radius_m.powi(2) - x * x - z * z)
            .max(f64::MIN_POSITIVE)
            .sqrt();
        [-x / radial, -z / radial]
    }
}

impl HeightSource for MeanSphereSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        (self.radius_m.powi(2) - x * x - z * z).max(0.0).sqrt() - self.radius_m
    }
}

/// A finite local source joined to a render-only sphere at this crop's border
/// datum. Canonical celestial and physics radii remain unchanged.
#[derive(Clone)]
struct BoundaryBlendSource {
    site: SiteSurfaceSource,
    globe: MeanSphereSource,
    region: Square,
    collar_m: f64,
    boundary_grid_resolution: usize,
    boundary_posting_m: f64,
}

impl HeightSource for BoundaryBlendSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        if self.region.distance_to([x, z]) <= 0.0 {
            return self.site.height_at(x, z);
        }

        let edge_x = x.clamp(
            self.region.center[0] - self.region.half,
            self.region.center[0] + self.region.half,
        );
        let edge_z = z.clamp(
            self.region.center[1] - self.region.half,
            self.region.center[1] + self.region.half,
        );
        let distance_x = x - edge_x;
        let distance_z = z - edge_z;
        // The prepared collar and its globe cutout are square. Use the same
        // max-norm distance so the fade reaches zero on every side and corner.
        let distance = distance_x.abs().max(distance_z.abs());
        if distance >= self.collar_m {
            return self.globe.height_at(x, z);
        }

        let edge_relief = self.site.boundary_height(
            edge_x,
            edge_z,
            self.region.half,
            self.boundary_grid_resolution,
        ) - self.globe.height_at(edge_x, edge_z);
        let site_gradient = self
            .site
            .boundary_gradient(edge_x, edge_z, self.region.half);
        let globe_gradient = self.globe.gradient_at(edge_x, edge_z);
        // `distance` is the square collar's max-norm distance. Project the
        // relative edge gradient onto that same parameter so corners and side
        // edges continue with the derivative of the actual collar coordinate.
        let edge_slope = if distance > 0.0 {
            ((site_gradient[0] - globe_gradient[0]) * distance_x
                + (site_gradient[1] - globe_gradient[1]) * distance_z)
                / distance
        } else {
            0.0
        };
        let posting_m = self.boundary_posting_m.min(self.collar_m);
        let relief = if posting_m > 0.0 && distance < posting_m {
            edge_relief
                + edge_slope * posting_m * integrated_inverse_smootherstep(distance / posting_m)
        } else {
            let continued = edge_relief + edge_slope * posting_m * 0.5;
            let fade_start = posting_m;
            let fade_width = (self.collar_m - fade_start).max(f64::EPSILON);
            continued * (1.0 - smoothstep((distance - fade_start) / fade_width))
        };
        self.globe.height_at(x, z) + relief
    }
}

fn integrated_inverse_smootherstep(t: f64) -> f64 {
    let t2 = t * t;
    let t4 = t2 * t2;
    let t5 = t4 * t;
    let t6 = t5 * t;
    t - t6 + 3.0 * t5 - 2.5 * t4
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct GlobeHandoffInputKey {
    dir: DVec3,
    east: DVec3,
    north: DVec3,
    body_radius_m: f64,
    half_extent: f64,
    surface_key: u64,
}

/// The one continuous site/globe ownership record for a body.
///
/// Inside the exact DEM square, the local terrain remains authoritative. The
/// render shell uses the crop's border datum; canonical body state is unchanged.
#[derive(Component, Clone)]
pub(crate) struct GlobeHandoff {
    pub dir: DVec3,
    pub east: DVec3,
    pub north: DVec3,
    pub half_extent: f64,
    /// Render-only shell radius, equal to canonical body radius plus crop datum.
    pub radius_m: f64,
    pub site_radius_m: f64,
    /// Width of the geometric stitch between the crop edge and sphere.
    pub collar_m: f64,
    collar_mesh: Arc<Mesh>,
    source_key: u64,
    input_key: Option<GlobeHandoffInputKey>,
}

impl PartialEq for GlobeHandoff {
    fn eq(&self, other: &Self) -> bool {
        self.dir == other.dir
            && self.east == other.east
            && self.north == other.north
            && self.half_extent == other.half_extent
            && self.radius_m == other.radius_m
            && self.site_radius_m == other.site_radius_m
            && self.source_key == other.source_key
            && self.input_key == other.input_key
    }
}

impl GlobeHandoff {
    pub(crate) fn dem_input_key(
        dir: DVec3,
        east: DVec3,
        north: DVec3,
        body_radius_m: f64,
        oracle: &SurfaceOracle,
        half_extent: f64,
    ) -> GlobeHandoffInputKey {
        GlobeHandoffInputKey {
            dir,
            east,
            north,
            body_radius_m,
            half_extent,
            surface_key: oracle.surface_key(),
        }
    }

    pub(crate) fn new(
        dir: DVec3,
        east: DVec3,
        north: DVec3,
        radius_m: f64,
        oracle: Arc<SurfaceOracle>,
        half_extent: f64,
    ) -> Result<Self, &'static str> {
        let input_key =
            Self::dem_input_key(dir, east, north, radius_m, oracle.as_ref(), half_extent);
        let border_datum = oracle.grid().border_datum();
        if !radius_m.is_finite() || radius_m <= 0.0 || !border_datum.is_finite() {
            return Err("body radius or DEM border datum is invalid");
        }
        let render_radius_m = radius_m + border_datum;
        if !render_radius_m.is_finite() || render_radius_m <= 0.0 {
            return Err("DEM border datum does not define a positive local body radius");
        }
        if !half_extent.is_finite() || half_extent <= 0.0 {
            return Err("DEM half extent must be finite and positive");
        }
        let posting_m = square_boundary_posting_spacing(half_extent, oracle.grid().res)
            .ok_or("DEM has no finite square boundary posting grid")?;
        // Keep the measured crop intact and move the visual transition wholly
        // outside it. One crop half-width gives a bounded, scale-relative
        // shoulder without letting an edge spike choose an unbounded skirt.
        let collar_m = half_extent.max(posting_m * 2.0).max(1.0);
        if half_extent + collar_m >= render_radius_m {
            return Err("DEM footprint and edge stitch exceed the orthographic tangent chart");
        }
        let globe = MeanSphereSource {
            radius_m: render_radius_m,
        };
        let region = Square {
            center: [0.0, 0.0],
            half: half_extent,
        };
        let source = BoundaryBlendSource {
            site: SiteSurfaceSource::Dem {
                oracle: oracle.clone(),
                datum_m: border_datum,
            },
            globe,
            region,
            collar_m,
            boundary_grid_resolution: oracle.grid().res,
            boundary_posting_m: posting_m,
        };
        let geometry = GlobeHandoffGeometry {
            dir,
            east,
            north,
            radius_m: render_radius_m,
            site_radius_m: render_radius_m,
            half_extent,
            collar_m,
        };
        let collar_mesh = Arc::new(create_square_handoff_collar_mesh(
            geometry,
            &source,
            oracle.grid().res,
            16,
        )?);
        Ok(Self {
            dir,
            east,
            north,
            half_extent,
            radius_m: render_radius_m,
            site_radius_m: render_radius_m,
            collar_m,
            collar_mesh,
            source_key: oracle.surface_key()
                ^ border_datum.to_bits().rotate_left(13)
                ^ render_radius_m.to_bits().rotate_left(29),
            input_key: Some(input_key),
        })
    }

    pub(crate) fn matches_dem_input(&self, input_key: GlobeHandoffInputKey) -> bool {
        self.input_key == Some(input_key)
    }

    /// Compose an authored flat site plane with a render shell at its datum.
    /// The footprint is deliberately square because the globe clip contract is a
    /// square tangent-plane cutout; non-square authored geometry is rejected by
    /// the USD terrain projection before it reaches this constructor.
    pub(crate) fn new_flat(
        dir: DVec3,
        east: DVec3,
        north: DVec3,
        radius_m: f64,
        height_m: f64,
        half_extent: f64,
    ) -> Result<Self, &'static str> {
        let render_radius_m = radius_m + height_m;
        let globe = MeanSphereSource {
            radius_m: render_radius_m,
        };
        let region = Square {
            center: [0.0, 0.0],
            half: half_extent,
        };
        let edge_relief = globe.height_at(half_extent, half_extent).abs();
        let collar_m = (edge_relief * 20.0).max(half_extent / 32.0).max(1.0);
        let source = BoundaryBlendSource {
            site: SiteSurfaceSource::Flat {
                height_m,
                datum_m: height_m,
            },
            globe,
            region,
            collar_m,
            boundary_grid_resolution: 0,
            boundary_posting_m: 0.0,
        };
        let geometry = GlobeHandoffGeometry {
            dir,
            east,
            north,
            radius_m: render_radius_m,
            site_radius_m: render_radius_m,
            half_extent,
            collar_m,
        };
        let collar_mesh = Arc::new(create_square_handoff_collar_mesh(geometry, &source, 33, 8)?);
        Ok(Self {
            dir,
            east,
            north,
            half_extent,
            radius_m: render_radius_m,
            site_radius_m: render_radius_m,
            collar_m,
            collar_mesh,
            source_key: render_radius_m.to_bits() ^ half_extent.to_bits().rotate_left(17),
            input_key: None,
        })
    }

    fn cutout(&self) -> GlobeCutout {
        GlobeCutout {
            dir: self.dir,
            east: self.east,
            north: self.north,
            site_radius_m: self.site_radius_m,
            half_extent: self.half_extent + self.collar_m,
        }
    }

    fn collar_mesh(&self) -> &Mesh {
        self.collar_mesh.as_ref()
    }
}

/// One bounded, body-owned preparation for the DEM boundary collar.
///
/// The collar mesh samples the crop boundary and builds the exterior stitch, so
/// preparation runs on the compute pool. Keeping the task on the globe entity
/// bounds work to one in-flight preparation per body and lets despawning the
/// body release its task handle with the rest of its presentation state.
#[derive(Component)]
pub(crate) struct GlobeHandoffPreparation {
    input_key: GlobeHandoffInputKey,
    task: Option<Task<Result<GlobeHandoff, &'static str>>>,
    result: Option<Result<GlobeHandoff, &'static str>>,
}

impl GlobeHandoffPreparation {
    pub(crate) fn spawn_dem(
        input_key: GlobeHandoffInputKey,
        dir: DVec3,
        east: DVec3,
        north: DVec3,
        body_radius_m: f64,
        oracle: Arc<SurfaceOracle>,
        half_extent: f64,
    ) -> Self {
        let task = AsyncComputeTaskPool::get().spawn(async move {
            GlobeHandoff::new(dir, east, north, body_radius_m, oracle, half_extent)
        });
        Self {
            input_key,
            task: Some(task),
            result: None,
        }
    }

    pub(crate) fn input_key(&self) -> GlobeHandoffInputKey {
        self.input_key
    }

    pub(crate) fn poll(&mut self) {
        let completed = self
            .task
            .as_mut()
            .and_then(|task| block_on(future::poll_once(task)));
        if let Some(result) = completed {
            self.task = None;
            self.result = Some(result);
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.task.is_none()
    }

    pub(crate) fn take_result(&mut self) -> Option<Result<GlobeHandoff, &'static str>> {
        self.result.take()
    }
}

/// The cube-sphere tiles currently resident for a body, keyed by quadtree node.
#[derive(Component, Default)]
pub(crate) struct GlobeTiles {
    /// Live tiles, including temporary coarse cover while desired replacements
    /// stream in.
    pub resident: HashMap<TileCoord, Entity>,
    /// Hidden tiles retained briefly after an atomic draw-cover handoff.
    ///
    /// Material-ready parent/child coverage is exchanged through visibility;
    /// retiring entities remain hidden for two render extraction turns so
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
    /// One prepared local DEM-to-globe collar, independent of globe tile LOD.
    collar_entity: Option<Entity>,
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
    /// `Local<HashMap<Entity, _>>` in the system, because a `Local` outlives
    /// teardown and would keep stale keys for despawned bodies.
    pub last_solve_cam: Option<DVec3>,
    /// Presentation camera that produced [`last_solve_cam`]. The camera entity
    /// is part of the solve input even when two cameras currently share a pose.
    last_solve_camera: Option<Entity>,
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
    last_selection_lod_key: Option<(u64, u32, u64, u32)>,
    /// CPU mesh builds queued away from the frame thread. The task result is
    /// installed into `Assets<Mesh>` only after it is ready; tile selection and
    /// visibility remain owned by this reconciler.
    pending_meshes: HashMap<TileCoord, Task<Mesh>>,
    /// Completed worker meshes waiting for the bounded main-thread upload
    /// budget. The queue is retained across reconciliations so a worker burst
    /// cannot become an upload burst in one frame.
    completed_meshes: BTreeMap<(u32, u8, i32, i32), (TileCoord, Mesh)>,
}

#[derive(Clone)]
struct CachedTileMesh {
    handle: Handle<Mesh>,
    bytes: usize,
    last_used: u64,
}

fn hierarchy_changed(
    entity: Entity,
    parents: &Query<&ChildOf>,
    changed_transforms: &Query<(), Changed<Transform>>,
    changed_cells: &Query<(), Changed<CellCoord>>,
    changed_parents: &Query<(), Changed<ChildOf>>,
) -> bool {
    let mut current = entity;
    loop {
        if changed_transforms.contains(current)
            || changed_cells.contains(current)
            || changed_parents.contains(current)
        {
            return true;
        }
        let Ok(parent) = parents.get(current) else {
            return false;
        };
        current = parent.parent();
    }
}

/// Wake globe reconciliation only for an unresolved cover or a changed input.
///
/// The reconciler retains its authoritative state on [`GlobeTiles`], so the
/// condition does not maintain a second cache. Walking the active camera's
/// existing hierarchy catches movement from a parent grid without scanning all
/// scene transforms; the body/grid queries cover authored LOD and handoff edits.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GlobeLodUpdateSignals<'w, 's> {
    parents: Query<'w, 's, &'static ChildOf>,
    changed_transforms: Query<'w, 's, (), Changed<Transform>>,
    changed_cells: Query<'w, 's, (), Changed<CellCoord>>,
    changed_parents: Query<'w, 's, (), Changed<ChildOf>>,
    changed_grids: Query<'w, 's, (), Changed<Grid>>,
    changed_lod: Query<'w, 's, (), Changed<GlobeLod>>,
    changed_handoff: Query<'w, 's, (), Changed<GlobeHandoff>>,
    lods: Query<'w, 's, &'static GlobeLod>,
    tiles: Query<'w, 's, &'static GlobeTiles>,
    ready: Query<'w, 's, (), Added<ShaderLookReady>>,
    removed_ready: RemovedComponents<'w, 's, ShaderLookReady>,
    removed_handoff: RemovedComponents<'w, 's, GlobeHandoff>,
}

pub(crate) fn globe_lod_update_due(
    budget: Res<GlobeLodBudget>,
    viewport: Res<SceneViewport>,
    mut signals: GlobeLodUpdateSignals<'_, '_>,
) -> bool {
    // A Twin setting can hold globe reconciliation when its resident-mesh
    // budget is invalid. Valid budget edits are change-driven invalidations.
    if !budget.resident_mesh_budget_valid {
        return false;
    }
    if budget.is_changed() {
        return true;
    }

    // `SceneViewport` is mutably borrowed by the camera reconciler every frame,
    // so its resource change tick is not an invalidation signal. The active
    // camera value is compared with the camera entity recorded on each body's
    // authoritative solve state below.
    if !signals.changed_lod.is_empty()
        || !signals.changed_handoff.is_empty()
        || !signals.ready.is_empty()
        || signals.removed_ready.read().next().is_some()
        || signals.removed_handoff.read().next().is_some()
    {
        return true;
    }

    let Some(camera) = viewport.active_camera else {
        return false;
    };

    if signals.tiles.iter().any(|tiles| {
        tiles.last_solve_cam.is_none()
            || !tiles.retiring.is_empty()
            || tiles.last_solve_camera != Some(camera)
    }) {
        return true;
    }

    if hierarchy_changed(
        camera,
        &signals.parents,
        &signals.changed_transforms,
        &signals.changed_cells,
        &signals.changed_parents,
    ) {
        return true;
    }

    signals
        .lods
        .iter()
        .any(|lod| signals.changed_grids.contains(lod.surface_grid))
}

/// Resource limits for live globe streaming.
///
/// These are resource values rather than hidden constants so a host can tune
/// them for a known adapter without changing the scene or the LOD algorithm.
/// The byte limit is a backpressure boundary for resident, retiring, in-flight,
/// and completed tiles; refinement waits for uploads and retirements instead of
/// accumulating an unbounded mesh queue.
#[derive(Resource, Clone, Copy, Debug)]
pub struct GlobeLodBudget {
    /// Maximum fresh tile entities created for one body in one frame.
    pub spawn_tiles_per_frame: usize,
    /// Maximum completed CPU meshes uploaded to `Assets<Mesh>` for one body in
    /// one frame. Mesh generation is off-thread, but the Bevy asset insertion
    /// still belongs to the frame thread and must be bounded separately from
    /// task admission.
    pub mesh_uploads_per_frame: usize,
    /// Maximum retired tile entities released for one body in one frame.
    pub despawn_tiles_per_frame: usize,
    /// Approximate mesh bytes reserved by resident, retiring, in-flight, and
    /// completed tiles.
    pub max_resident_mesh_bytes: usize,
    /// Whether the active Twin's resident mesh budget was valid.
    ///
    /// The application adapter clears this when a Twin supplies a malformed
    /// override, holding globe reconciliation until the setting is corrected.
    pub resident_mesh_budget_valid: bool,
    /// Approximate bytes retained by the reusable mesh-handle cache.
    pub max_cached_mesh_bytes: usize,
    /// Fresh mesh bytes admitted to workers and committed to render assets per
    /// body and visualization frame, independent of entity count.
    pub max_fresh_mesh_bytes_per_frame: usize,
}

impl Default for GlobeLodBudget {
    fn default() -> Self {
        Self {
            spawn_tiles_per_frame: 16,
            mesh_uploads_per_frame: 4,
            despawn_tiles_per_frame: 32,
            max_resident_mesh_bytes: 72 * 1024 * 1024,
            resident_mesh_budget_valid: true,
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
/// `create_quadsphere_tile_mesh` (position, normal, globe direction and indices).
fn tile_mesh_bytes(res: u32) -> usize {
    let side = res as usize + 1;
    let vertices = side.saturating_mul(side);
    let indices = (res as usize)
        .saturating_mul(res as usize)
        .saturating_mul(6);
    vertices
        .saturating_mul((3 + 3 + 3) * std::mem::size_of::<f32>())
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
    let max_resident_level = resident.keys().map(|tile| tile.level).max().unwrap_or(0);
    desired
        .iter()
        .filter(|tile| tile.face == face)
        .any(|leaf| !resident_covers(*leaf, resident, resident_coverage, max_resident_level))
}

/// Return whether resident tiles cover the complete area of `coord`.
///
/// An ancestor covers the whole node, while descendants cover only their own
/// quadrants. A node without a resident ancestor is complete only when its
/// coverage entry exists and every recursive child is covered, keeping the
/// coarse fallback visible while refinement is incomplete.
fn resident_covers(
    coord: TileCoord,
    resident: &HashMap<TileCoord, Entity>,
    resident_coverage: &HashSet<TileCoord>,
    max_resident_level: u32,
) -> bool {
    let mut ancestor = Some(coord);
    while let Some(current) = ancestor {
        if resident.contains_key(&current) {
            return true;
        }
        ancestor = tile_parent(current);
    }
    if coord.level >= max_resident_level || !resident_coverage.contains(&coord) {
        return false;
    }
    tile_children(coord)
        .into_iter()
        .all(|child| resident_covers(child, resident, resident_coverage, max_resident_level))
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
    lunco_spatial::coords::pose_in_grid(camera, surface_grid, q_parents, q_grids, q_spatial)
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
        let tile_bytes = tile_mesh_bytes(lod.res);
        let render_radius_m = handoff.map_or(lod.radius_m, |value| value.radius_m);

        // A handoff changes the geometry of every resident tile that crosses its
        // boundary. Retire the old meshes before solving the new cover so a
        // cached uncut globe tile cannot survive under the local DEM.
        let handoff_changed = tiles.last_solve_handoff.as_ref() != handoff;
        if handoff_changed {
            debug!(
                "globe LOD handoff solve: body={body_ent:?} camera={camera_entity:?} surface_grid={:?} body_local={camera_body_local:?} radius={:.0} handoff={}",
                lod.surface_grid,
                render_radius_m,
                handoff.is_some()
            );
            if let Some(entity) = tiles.collar_entity.take() {
                commands.entity(entity).try_despawn();
            }
            for (_, entity) in tiles.resident.drain() {
                commands.entity(entity).try_despawn();
            }
            for (entity, _, _) in tiles.retiring.drain(..) {
                commands.entity(entity).try_despawn();
            }
            tiles.mesh_cache.clear();
            tiles.mesh_cache_bytes = 0;
            tiles.pending_meshes.clear();
            tiles.completed_meshes.clear();
            tiles.last_solve_cam = None;
            tiles.desired.clear();
            tiles.last_selection_cam = None;
            tiles.last_selection_handoff = None;
            tiles.last_selection_lod_key = None;
            tiles.resident_revision = tiles.resident_revision.wrapping_add(1);
            if let Some(handoff) = handoff {
                let center = handoff.dir * handoff.radius_m;
                let (cell, local) = sg_grid.translation_to_grid(center);
                let mesh = meshes.add(handoff.collar_mesh().clone());
                let entity = commands
                    .spawn((
                        Mesh3d(mesh),
                        lod.look.clone(),
                        cell,
                        Transform::from_translation(local),
                        GlobalTransform::default(),
                        Visibility::Hidden,
                        InheritedVisibility::default(),
                        Stationary,
                        bevy::light::NotShadowCaster,
                        Name::new("Globe DEM handoff collar"),
                        lunco_core::SystemManaged,
                        ChildOf(lod.surface_grid),
                    ))
                    .id();
                tiles.collar_entity = Some(entity);
            }
        }

        // Poll completed CPU builds without ever waiting for one. The old
        // synchronous path spent tens of milliseconds in
        // `create_quadsphere_tile_mesh` while the frame thread was trying to
        // bootstrap the globe. Installing the finished Mesh on the main thread
        // is required by Bevy, but the expensive vertex generation is not.
        {
            let GlobeTiles {
                pending_meshes,
                completed_meshes,
                ..
            } = &mut *tiles;
            pending_meshes.retain(
                |coord, task| match block_on(future::poll_once(&mut *task)) {
                    Some(mesh) => {
                        completed_meshes
                            .insert((coord.level, coord.face, coord.i, coord.j), (*coord, mesh));
                        false
                    }
                    None => true,
                },
            );
        }
        let mesh_completion_pending = !tiles.completed_meshes.is_empty();
        // A worker can finish many meshes between two render frames. Keep the
        // completed queue across reconciliations and bound only the main-thread
        // asset uploads; completed tiles are committed in stable coarse-to-fine
        // coordinate order so worker completion order cannot reorder the view.
        let mut uploaded_mesh_bytes = 0usize;
        for _ in 0..budget.mesh_uploads_per_frame {
            if tiles.completed_meshes.is_empty() {
                break;
            }
            if uploaded_mesh_bytes.saturating_add(tile_bytes)
                > budget.max_fresh_mesh_bytes_per_frame
            {
                break;
            }
            let (coord, mesh) = tiles
                .completed_meshes
                .pop_first()
                .expect("the completed mesh selected above remains queued")
                .1;
            tiles.cache_clock = tiles.cache_clock.wrapping_add(1);
            let last_used = tiles.cache_clock;
            let handle = meshes.add(mesh);
            tiles.mesh_cache.insert(
                coord,
                CachedTileMesh {
                    handle,
                    bytes: tile_bytes,
                    last_used,
                },
            );
            tiles.mesh_cache_bytes = tiles.mesh_cache_bytes.saturating_add(tile_bytes);
            uploaded_mesh_bytes = uploaded_mesh_bytes.saturating_add(tile_bytes);
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
            let altitude = (camera_body_local.length() - render_radius_m)
                .abs()
                .max(1.0);
            let slack = LOD_CAMERA_MOTION_FRACTION * altitude;
            if tiles.last_solve_handoff.as_ref() == handoff
                && tiles.last_solve_camera == Some(camera_entity)
                && all_resident_ready
                && tiles.retiring.is_empty()
                && !mesh_completion_pending
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
            render_radius_m.to_bits(),
            lod.max_lod,
            lod.lod_distance_factor.to_bits(),
            lod.res,
        );
        let selection_needs_rebuild = tiles.last_selection_cam.is_none_or(|previous| {
            let altitude = (camera_body_local.length() - render_radius_m)
                .abs()
                .max(1.0);
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
                    render_radius_m,
                    lod.max_lod,
                    lod.lod_distance_factor,
                    &[],
                );
            }
            let max_level = desired.iter().map(|tile| tile.level).max().unwrap_or(0);
            balance_cube_sphere_lod(&mut desired, body_ent, max_level);
            if tiles.last_selection_handoff.as_ref() != handoff {
                let deepest_level = desired.iter().map(|tile| tile.level).max().unwrap_or(0);
                debug!(
                    "globe LOD handoff cover: body={body_ent:?} collar_m={:.0} leaves={} deepest_level={deepest_level}",
                    handoff.map(|value| value.collar_m).unwrap_or(0.0),
                    desired.len()
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
        // A camera move can invalidate a queued mesh before its worker has
        // started. Drop that work at the authoritative selection boundary so
        // stale globe detail cannot consume the worker pool or later enter the
        // mesh cache.
        tiles
            .pending_meshes
            .retain(|coord, _| missing.contains(coord));
        let mut prioritized: Vec<(TileCoord, f64)> = missing
            .into_iter()
            .map(|coord| {
                let distance = tile_dist2(&coord, render_radius_m, camera_body_local);
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
        let mut fresh_bytes = 0usize;
        for (coord, _) in prioritized.into_iter().take(budget.spawn_tiles_per_frame) {
            if tiles.pending_meshes.contains_key(&coord) {
                continue;
            }
            if tiles
                .completed_meshes
                .values()
                .any(|(completed, _)| *completed == coord)
            {
                continue;
            }
            let needs_fresh_mesh = !tiles.mesh_cache.contains_key(&coord);
            let reserved_meshes = tiles
                .resident
                .len()
                .saturating_add(tiles.retiring.len())
                .saturating_add(tiles.pending_meshes.len())
                .saturating_add(tiles.completed_meshes.len());
            if needs_fresh_mesh
                && (fresh_bytes.saturating_add(tile_bytes) > budget.max_fresh_mesh_bytes_per_frame
                    || reserved_meshes
                        .saturating_mul(tile_bytes)
                        .saturating_add(fresh_bytes)
                        .saturating_add(tile_bytes)
                        > budget.max_resident_mesh_bytes)
            {
                break;
            }
            let (u, v) = tile_center_uv(coord.face, coord.level, coord.i, coord.j);
            let tile_center_dir = cube_to_sphere(coord.face, u, v);
            let tile_body_local = tile_center_dir * render_radius_m;
            let (tile_cell, tile_local_pos) = sg_grid.translation_to_grid(tile_body_local);
            // Build the mesh relative to the tile centre: the entity is placed at
            // that centre via the grid, so the mesh carries only each vertex's
            // small offset from it. Keeping vertex magnitudes small avoids f32
            // precision loss at planetary radii.
            tiles.cache_clock = tiles.cache_clock.wrapping_add(1);
            let cache_clock = tiles.cache_clock;
            let mesh_handle = if let Some(cached) = tiles.mesh_cache.get_mut(&coord) {
                cached.last_used = cache_clock;
                cached.handle.clone()
            } else {
                let handoff = handoff.cloned();
                let radius = render_radius_m;
                let res = lod.res;
                fresh_bytes = fresh_bytes.saturating_add(tile_bytes);
                let task = AsyncComputeTaskPool::get().spawn(async move {
                    let _span = bevy::log::info_span!("globe_tile_mesh").entered();
                    let cutout = handoff.as_ref().map(GlobeHandoff::cutout);
                    create_quadsphere_tile_mesh(
                        body_ent,
                        coord.face,
                        coord.level,
                        coord.i,
                        coord.j,
                        radius,
                        res,
                        tile_body_local,
                        cutout,
                    )
                });
                tiles.pending_meshes.insert(coord, task);
                continue;
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
        if let Some(entity) = tiles.collar_entity {
            let target = if material_ready.contains(entity) {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if !visibility
                .get(entity)
                .is_ok_and(|current| *current == target)
            {
                commands.entity(entity).try_insert(target);
            }
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
        tiles.last_solve_camera = settled.then_some(camera_entity);
        tiles.last_solve_handoff = handoff.cloned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemState;

    #[test]
    fn cross_body_lod_camera_uses_the_authoritative_big_space_pose() {
        let mut world = World::new();
        let grid = lunco_spatial::WorldGridConfig::default().grid();
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
    fn flat_site_uses_its_authored_datum_and_expanded_globe_cutout() {
        let cutout = GlobeCutout {
            dir: DVec3::X,
            east: DVec3::Z,
            north: DVec3::Y,
            site_radius_m: 1_737_400.0,
            half_extent: 8_100.0,
        };
        let projected = |direction: DVec3| {
            [
                direction.normalize().dot(cutout.east) * cutout.site_radius_m,
                direction.normalize().dot(cutout.north) * cutout.site_radius_m,
            ]
        };
        let [inside_x, inside_z] = projected(DVec3::new(1.0, 0.00001, 0.00001));
        assert!(inside_x.abs() <= cutout.half_extent && inside_z.abs() <= cutout.half_extent);
        let [outside_x, outside_z] = projected(DVec3::new(1.0, 0.01, 0.0));
        assert!(outside_x.abs() > cutout.half_extent || outside_z.abs() > cutout.half_extent);
        let flat = SiteSurfaceSource::Flat {
            height_m: 0.0,
            datum_m: 0.0,
        };
        assert_eq!(flat.height_at(0.0, 0.0), 0.0);
        assert_eq!(flat.height_at(50.0, -50.0), 0.0);
    }

    #[test]
    fn square_collar_reaches_the_globe_at_its_full_expanded_boundary() {
        let globe = MeanSphereSource {
            radius_m: 100_000.0,
        };
        let collar = BoundaryBlendSource {
            site: SiteSurfaceSource::Flat {
                height_m: -2.0,
                datum_m: 0.0,
            },
            globe,
            region: Square {
                center: [0.0, 0.0],
                half: 100.0,
            },
            collar_m: 1_000.0,
            boundary_grid_resolution: 0,
            boundary_posting_m: 0.0,
        };

        // At this expanded-square corner, Euclidean distance exceeds the
        // collar width even though the point lies on its authored boundary.
        // The square metric keeps the relief fade continuous up to that edge.
        let x = 100.0 + 500.0;
        let z = 100.0 + 1_000.0;
        assert_eq!(collar.height_at(x, z), globe.height_at(x, z));

        let just_inside = collar.height_at(x, z - 100.0);
        let globe_inside = globe.height_at(x, z - 100.0);
        assert!(just_inside < globe_inside);
        assert!((just_inside - globe_inside).abs() < 0.1);
    }

    #[test]
    fn dem_handoff_preserves_crop_and_transitions_only_in_visual_collar() {
        let mut grid = lunco_terrain_surface::HeightGrid::new_flat(9, 100.0);
        grid.heights.fill(-1_918.0);
        grid.heights[4 * grid.res + 4] = -1_888.0;
        grid.heights[4 * grid.res + 8] = -1_906.0;
        let raw_heights = grid.heights.clone();
        let datum = grid.border_datum();
        let body_radius_m = 1_737_400.0;
        let oracle = Arc::new(SurfaceOracle::new(
            Arc::new(grid),
            vec![lunco_terrain_surface::oracle::curvature_contribution(
                body_radius_m,
                datum,
            )],
        ));
        let globe = MeanSphereSource {
            radius_m: body_radius_m + datum,
        };
        let collar = BoundaryBlendSource {
            site: SiteSurfaceSource::Dem {
                oracle: oracle.clone(),
                datum_m: datum,
            },
            globe,
            region: Square {
                center: [0.0, 0.0],
                half: 100.0,
            },
            collar_m: 100.0,
            boundary_grid_resolution: oracle.grid().res,
            boundary_posting_m: 25.0,
        };

        assert_eq!(
            collar.height_at(0.0, 0.0),
            oracle.height_at(0.0, 0.0) - datum
        );
        assert_eq!(
            collar.height_at(100.0, 0.0),
            oracle.height_at(100.0, 0.0) - datum
        );
        assert_eq!(oracle.grid().heights, raw_heights);

        let edge = collar.height_at(100.0, 0.0);
        let just_outside = collar.height_at(100.001, 0.0);
        assert!((just_outside - edge).abs() < 0.001);
        assert!(collar.height_at(125.0, 0.0) > globe.height_at(125.0, 0.0));
        assert_eq!(collar.height_at(200.0, 0.0), globe.height_at(200.0, 0.0));
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
        resident.insert(tile_children(leaf)[0], Entity::PLACEHOLDER);
        let coverage = resident_coverage(&resident);
        assert!(branch_has_gap(&desired, &resident, &coverage, 3));

        for child in tile_children(leaf).into_iter().skip(1) {
            resident.insert(child, Entity::PLACEHOLDER);
        }
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
