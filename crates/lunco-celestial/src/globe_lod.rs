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
//! [`GlobeTiles`] tracks the resident tile set; [`update_globe_lod`] diffs the
//! desired set against it each frame. Tile placement replicates the proven static
//! pattern verbatim (mesh body-local, entity anchored at the tile centre via the
//! surface grid's `translation_to_grid`, `set_parent_in_place`) so correctness is
//! preserved — only *which* tiles exist becomes dynamic.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use lunco_materials::ShaderLook;
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
struct HandoffSource(CompositeHeightSource<BoundarySiteSource, MeanSphereSource>);

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
        // The DEM is in the body's absolute vertical datum. The bridge must not
        // turn a kilometre-scale datum offset into a near-vertical wall at the
        // crop edge. Choose the first tangent distance where the mean sphere's
        // sagitta reaches that measured border datum, then use the authored
        // footprint as the minimum. This is the body geometry's scale, not a
        // visual fudge factor.
        let border_datum = oracle.grid().border_datum();
        let sagitta_distance = (2.0 * radius_m * border_datum.abs()).sqrt();
        let blend_m = half_extent.max(sagitta_distance).max(0.0);
        let region = Square {
            center: [0.0, 0.0],
            half: half_extent,
        };
        let source = HandoffSource(CompositeHeightSource::new(
            BoundarySiteSource {
                oracle: oracle.clone(),
                region,
                half_extent,
                datum_m: border_datum,
                boundary_m: oracle.spacing() as f64,
            },
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
    /// Live tiles (in the desired LOD set).
    pub resident: HashMap<TileCoord, Entity>,
    /// Tiles that left the desired set, kept alive for a few frames while
    /// their replacements' meshes reach the GPU. Despawning old and spawning
    /// new in the SAME frame opened a one-frame hole per swap (a fresh
    /// `Mesh3d` renders only after render-world extraction/prepare) — with a
    /// moving camera the LOD churns continuously and the whole sphere
    /// flickered ("still blinking"). The brief overlap of coplanar identical
    /// surfaces is invisible; a hole is not.
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

/// Whether a direction lies in the exact DEM square.
fn tile_fully_in_handoff(face: u8, level: u32, i: i32, j: i32, handoff: &GlobeHandoff) -> bool {
    let geometry = handoff.geometry();
    [
        (
            -1.0 + i as f64 * 2.0 / (1i64 << level) as f64,
            -1.0 + j as f64 * 2.0 / (1i64 << level) as f64,
        ),
        (
            -1.0 + (i + 1) as f64 * 2.0 / (1i64 << level) as f64,
            -1.0 + j as f64 * 2.0 / (1i64 << level) as f64,
        ),
        (
            -1.0 + i as f64 * 2.0 / (1i64 << level) as f64,
            -1.0 + (j + 1) as f64 * 2.0 / (1i64 << level) as f64,
        ),
        (
            -1.0 + (i + 1) as f64 * 2.0 / (1i64 << level) as f64,
            -1.0 + (j + 1) as f64 * 2.0 / (1i64 << level) as f64,
        ),
        (
            -1.0 + (i as f64 + 0.5) * 2.0 / (1i64 << level) as f64,
            -1.0 + (j as f64 + 0.5) * 2.0 / (1i64 << level) as f64,
        ),
    ]
    .iter()
    .map(|&(u, v)| cube_to_sphere(face, u, v))
    .all(|direction| geometry.contains(direction))
}

/// Whether two tiles overlap on the sphere: same body face and one is the
/// other's quadtree ancestor (or the same node).
fn tiles_overlap(a: &TileCoord, b: &TileCoord) -> bool {
    if a.body != b.body || a.face != b.face {
        return false;
    }
    let (deep, shallow) = if a.level >= b.level { (a, b) } else { (b, a) };
    let d = deep.level - shallow.level;
    (deep.i >> d) == shallow.i && (deep.j >> d) == shallow.j
}

fn branch_has_gap(
    desired: &HashSet<TileCoord>,
    resident: &HashMap<TileCoord, Entity>,
    face: u8,
) -> bool {
    desired.iter().filter(|tile| tile.face == face).any(|leaf| {
        !resident
            .keys()
            .any(|resident| tiles_overlap(leaf, resident))
    })
}

/// Per-frame: stream each body's cube-sphere tile set against the camera.
pub(crate) fn update_globe_lod(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    budget: Res<GlobeLodBudget>,
    // `With<SceneCamera>`, NOT `With<Camera3d>`: "which entity is the scene camera?"
    // is a render-FREE question, and asking it with `Camera3d` was what made this
    // crate link bevy_core_pipeline → wgpu. See `lunco_render::camera`.
    cameras: Query<(&Camera, &GlobalTransform, &bevy::camera::RenderTarget), With<SceneCamera>>,
    transforms: Query<&GlobalTransform>,
    grids: Query<&Grid>,
    mut bodies: Query<(Entity, &GlobeLod, &mut GlobeTiles, Option<&GlobeHandoff>)>,
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

    for (body_ent, lod, mut tiles, handoff) in &mut bodies {
        // Camera relative to the body centre (= the surface grid origin, inertial),
        // in the frame the tiles live in. f32 render-space is plenty for choosing
        // the LOD; tile PLACEMENT below stays f64-precise via `translation_to_grid`.
        let Ok(sg_gt) = transforms.get(lod.surface_grid) else {
            continue;
        };
        let Ok(sg_grid) = grids.get(lod.surface_grid) else {
            continue;
        };
        let camera_body_local = cam_pos - sg_gt.translation().as_dvec3();

        // A handoff changes the geometry of every resident tile that crosses its
        // boundary. Retire the old meshes before solving the new cover so a
        // cached uncut globe tile cannot survive under the local DEM.
        let handoff_changed = tiles.last_solve_handoff.as_ref() != handoff;
        if handoff_changed {
            debug!(
            "globe LOD handoff solve: body={body_ent:?} camera={cam_pos:?} surface_grid={:?} surface_grid_world={:?} body_local={camera_body_local:?} radius={:.0} handoff={}",
                lod.surface_grid,
                sg_gt.translation(),
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
        }

        // CAMERA-MOTION GATE. Everything below — two `HashSet`s, a `Vec`, a sort,
        // and six recursive quadtree descents — is a pure function of
        // (camera_body_local, handoff, the resident set). With the resident set
        // settled and both inputs unmoved the answer is bit-identical to last
        // frame's, so a parked view rebuilt ~600 `TileCoord`s per body per frame
        // to conclude that nothing should change.
        //
        // This is the same shape as the cadence gate the ephemeris cluster uses
        // (`cadence::tracked_needs_solve`) — an error budget rather than a rate —
        // but it cannot BE that gate: the tile set depends on the CAMERA, which
        // no epoch tolerance can see. A body's LOD must react to a camera that
        // moves while the clock is paused.
        if let Some(prev_cam) = tiles.last_solve_cam {
            let altitude = (camera_body_local.length() - lod.radius_m).abs().max(1.0);
            let slack = LOD_CAMERA_MOTION_FRACTION * altitude;
            if tiles.last_solve_handoff.as_ref() == handoff
                && (camera_body_local - prev_cam).length_squared() < slack * slack
            {
                continue;
            }
        }

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

        // Site DEM handoff: tiles fully under the curved terrain patch are
        // never desired (see `GlobeHandoff`). Retirement below then lets any
        // resident tile there go once its surviving siblings are up.
        if let Some(handoff) = handoff {
            desired.retain(|c| !tile_fully_in_handoff(c.face, c.level, c.i, c.j, handoff));
        }

        // Spawn newly-desired tiles FIRST (so this frame's spawns count as
        // coverage for retirement below), BUDGETED per frame by
        // `GlobeLodBudget`. Coarse-and-near first: a coarse tile covers the
        // most area (unblocks the most retirements), a near tile is what the
        // viewer is looking at. Placement verbatim from the proven static
        // path: mesh in body-local (tile_center = ZERO), entity anchored at the
        // tile centre via the surface grid, reparented in place.
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
            if branch_has_gap(&desired, &tiles.resident, face) {
                missing.insert(root);
            }
        }
        let mut missing: Vec<TileCoord> = missing.into_iter().collect();
        missing.sort_by(|a, b| {
            a.level.cmp(&b.level).then_with(|| {
                tile_dist2(a, lod.radius_m, camera_body_local).total_cmp(&tile_dist2(
                    b,
                    lod.radius_m,
                    camera_body_local,
                ))
            })
        });
        // Initial fill is budgeted too. A scene load must not synchronously
        // allocate the whole finest visible shell before the render thread can
        // upload anything; the same backpressure applies at every camera range.
        let tile_bytes = tile_mesh_bytes(lod.res);
        let mut fresh_bytes = 0usize;
        for coord in missing.into_iter().take(budget.spawn_tiles_per_frame) {
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
                    Mesh3d(mesh_handle),
                    lod.look.clone(),
                    coord,
                    TerrainTile,
                    tile_cell,
                    Transform::from_translation(tile_local_pos),
                    GlobalTransform::default(),
                    Visibility::Visible,
                    InheritedVisibility::default(),
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
        }

        // Retire resident tiles that left the desired set — but ONLY once
        // every desired tile overlapping their footprint is itself resident.
        // With budgeted spawning the replacements arrive before retirement:
        // this check requires every desired overlapping tile to be resident.
        // Do not keep the old tile for an additional grace period: parent/child
        // globe tiles occupy the same surface and depth-fight during close/far
        // camera jumps, which presents as Earth blinking.
        let resident_now: HashSet<TileCoord> = tiles.resident.keys().copied().collect();
        let mut newly_retired: Vec<(Entity, u8, TileCoord)> = Vec::new();
        tiles.resident.retain(|coord, ent| {
            if desired.contains(coord) {
                return true;
            }
            let covered = desired
                .iter()
                .filter(|d| tiles_overlap(d, coord))
                .all(|d| resident_now.contains(d));
            if covered {
                newly_retired.push((*ent, 0, *coord));
                false
            } else {
                true
            }
        });
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

        // Arm the camera-motion gate ONLY if this pass left nothing outstanding:
        // every desired tile is resident (the spawn budget may have deferred
        // some) and nothing is still retiring (those carry a per-frame
        // countdown). Otherwise the gate stays open and the next frame continues
        // the work — the budget's whole point is to finish over several frames.
        let settled =
            tiles.retiring.is_empty() && desired.iter().all(|c| tiles.resident.contains_key(c));
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
                handoff: Option<&GlobeHandoff>,
                body: Entity,
                face: u8,
                level: u32,
                i: i32,
                j: i32,
            ) -> bool {
                // The exact DEM square is an intentional globe omission because
                // the local terrain projection owns those triangles.
                if handoff.is_some_and(|h| tile_fully_in_handoff(face, level, i, j, h)) {
                    return true;
                }
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
                    (0..2).all(|dj| {
                        covered(set, handoff, body, face, level + 1, i * 2 + di, j * 2 + dj)
                    })
                })
            }
            for face in 0..6u8 {
                if !covered(&resident, handoff, body_ent, face, 0, 0, 0) {
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
    use lunco_terrain_globe::quad_sphere::tile_center_uv;

    const MOON_RADIUS_M: f64 = 1_737_400.0;

    /// The source-backed handoff `placement.rs` builds for a given footprint.
    fn handoff_for(half_extent_m: f64, dir: DVec3) -> GlobeHandoff {
        use lunco_obstacle_field::field::HeightGrid;

        GlobeHandoff::new(
            dir,
            DVec3::Z,
            DVec3::Y,
            MOON_RADIUS_M,
            Arc::new(SurfaceOracle::bare(Arc::new(HeightGrid::new_flat(3, 1.0)))),
            half_extent_m,
        )
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

    /// The handoff keeps the exact DEM square out of the globe LOD set.
    ///
    /// A tile may straddle the exact DEM square, so the tile remains resident and
    /// the mesh builder clips only its outside triangles. A fully covered tile is
    /// removed from the LOD set altogether.
    #[test]
    fn a_dem_sized_handoff_drops_only_fully_covered_tiles() {
        let (face, level, i, j) = (0u8, 8u32, 128, 128);
        let (u, v) = tile_center_uv(face, level, i, j);
        let dir = cube_to_sphere(face, u, v).normalize();

        assert!(
            tile_fully_in_handoff(face, level, i, j, &handoff_for(20_000.0, dir)),
            "a tile wholly inside the DEM square is removed"
        );
        assert!(!tile_fully_in_handoff(
            face,
            level,
            i,
            j,
            &handoff_for(1.0, dir)
        ));
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
        assert!(branch_has_gap(&desired, &resident, 3));
        resident.insert(root, Entity::PLACEHOLDER);
        assert!(!branch_has_gap(&desired, &resident, 3));
    }
}
