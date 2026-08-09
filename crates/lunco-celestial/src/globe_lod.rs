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

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use lunco_materials::ShaderLook;
use lunco_render::SceneCamera;
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
    /// The [`GlobePunch`] in force at that solve. The punch is an INPUT to the
    /// desired set, so a site appearing/moving must re-open the gate even if the
    /// camera has not moved a millimetre.
    pub last_solve_punch: Option<GlobePunch>,
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

/// Hole-punch under a site's DEM terrain patch: globe tiles that lie FULLY
/// inside this cone around the site direction are dropped from the desired
/// set. The DEM curves onto the sphere (`TerrainBodyCurvature`) and covers
/// this region completely, so the globe underneath is pure overdraw — and
/// worse, it pokes through crater floors that dip below the datum sphere.
/// Tiles merely overlapping the cone's edge still render (the DEM's feathered
/// edge sits `edge_lift_m` above them, so no z-fight). Inserted/updated by
/// `placement::sync_terrain_body_curvature`.
#[derive(Component, Clone, Copy, PartialEq)]
pub struct GlobePunch {
    /// Site direction (unit) in the tile frame (body-fixed).
    pub dir: DVec3,
    /// Cosine of the punch cone's angular radius.
    pub cos_theta: f64,
}

/// Whether the tile's spherical footprint lies entirely inside the punch cone
/// (all four corners + centre — sufficient for any tile small enough to fit a
/// sub-degree cone; a level-0 face's 90°-spread corners can never all pass).
fn tile_fully_in_punch(face: u8, level: u32, i: i32, j: i32, punch: &GlobePunch) -> bool {
    let step = 2.0 / (1i64 << level) as f64;
    let u0 = -1.0 + i as f64 * step;
    let v0 = -1.0 + j as f64 * step;
    [
        (u0, v0),
        (u0 + step, v0),
        (u0, v0 + step),
        (u0 + step, v0 + step),
        (u0 + step * 0.5, v0 + step * 0.5),
    ]
    .iter()
    .all(|&(u, v)| cube_to_sphere(face, u, v).dot(punch.dir) >= punch.cos_theta)
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

// TODO(globe-invisible): In luncosim's dev `cargo run`, the globe is NOT
// visible — the viewport renders black even though this system spawns the
// correct tile entities (verified via list_entities: f0-f5 L0 + L1 refinements
// for Earth & Moon, camera auto-focused Earth at 3x radius). The viewport CHROME
// is fixed (ViewportPanel + canonical SceneCamera binder → Camera3d active,
// black clear) and the
// 2x-radius tile placement bug is fixed (tiles now built centre-relative). But
// nothing renders. Prior notes say spacecraft glTFs were also invisible, so the
// remaining cause is likely GLOBAL, not tile-specific. Suspects to investigate:
//   - avatar camera clip planes: `update_avatar_clip_planes_system`
//     (lunco-avatar) only adapts near/far for cameras WITH AdaptiveNearPlane +
//     CellCoord + ChildOf(Grid). If the Observer Camera misses one, projection
//     stays default (far≈1000 m) → everything at orbital distance is clipped.
//   - blueprint.wgsl ShaderMaterial actually producing visible output for the
//     globe tiles (backface winding / cull mode / `transition` mode).
//   - big_space GlobalTransform propagation for tiles under the surface grid.
// NOTE: luncosim screenshots (MCP + HTTP CaptureScreenshot) render the viewport
// WHITE — they do not composite the Camera3d pass — so this must be verified in
// the real window, not via screenshot. See memory
// project_luncosim_viewport_and_globe_fix.
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
    mut bodies: Query<(Entity, &GlobeLod, &mut GlobeTiles, Option<&GlobePunch>)>,
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

    for (body_ent, lod, mut tiles, punch) in &mut bodies {
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

        // CAMERA-MOTION GATE. Everything below — two `HashSet`s, a `Vec`, a sort,
        // and six recursive quadtree descents — is a pure function of
        // (camera_body_local, punch, the resident set). With the resident set
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
            if tiles.last_solve_punch.as_ref() == punch
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

        // Site DEM hole-punch: tiles fully under the curved terrain patch are
        // never desired (see `GlobePunch`). Retirement below then lets any
        // resident tile there go once its surviving siblings are up.
        if let Some(p) = punch {
            desired.retain(|c| !tile_fully_in_punch(c.face, c.level, c.i, c.j, p));
        }

        // Spawn newly-desired tiles FIRST (so this frame's spawns count as
        // coverage for retirement below), BUDGETED per frame by
        // `GlobeLodBudget`. Coarse-and-near first: a coarse tile covers the
        // most area (unblocks the most retirements), a near tile is what the
        // viewer is looking at. Placement verbatim from the proven static
        // path: mesh in body-local (tile_center = ZERO), entity anchored at the
        // tile centre via the surface grid, reparented in place.
        let mut missing: Vec<TileCoord> = desired
            .iter()
            .filter(|c| !tiles.resident.contains_key(c))
            .copied()
            .collect();
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
        tiles.last_solve_punch = punch.copied();

        // `LUNCO_LOD_VALIDATE=1`: assert the resident set still covers the
        // whole sphere after this frame's spawn/retire pass (the invariant
        // the budgeted streaming must never break). Ground truth for hole
        // reports — API-side entity censuses are ambiguous (registry lag,
        // retiring-tile overlap, cross-body name collisions).
        if std::env::var("LUNCO_LOD_VALIDATE").is_ok() {
            let resident: HashSet<TileCoord> = tiles.resident.keys().copied().collect();
            fn covered(
                set: &HashSet<TileCoord>,
                punch: Option<&GlobePunch>,
                body: Entity,
                face: u8,
                level: u32,
                i: i32,
                j: i32,
            ) -> bool {
                // The site hole-punch is an INTENTIONAL hole (the DEM covers it).
                if punch.is_some_and(|p| tile_fully_in_punch(face, level, i, j, p)) {
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
                        covered(set, punch, body, face, level + 1, i * 2 + di, j * 2 + dj)
                    })
                })
            }
            for face in 0..6u8 {
                if !covered(&resident, punch, body_ent, face, 0, 0, 0) {
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

    /// The cone `placement.rs` builds for a given ground footprint.
    fn punch_for(half_extent_m: f64, dir: DVec3) -> GlobePunch {
        let sin_theta = (half_extent_m * 0.999) / MOON_RADIUS_M;
        GlobePunch {
            dir,
            cos_theta: (1.0 - sin_theta * sin_theta).sqrt(),
        }
    }

    /// The punch cannot be what keeps the globe off a surface site.
    ///
    /// `tile_fully_in_punch` needs a whole tile inside the cone, so a cone sized at the
    /// DEM's own half extent drops NOTHING. That is why an opaque datum-radius globe
    /// hung over sites authored at negative elevations as a flat grey lid. The fix is
    /// `GLOBE_SINK_M` (lunco-terrain-globe), NOT a bigger cone — a cone large enough to
    /// bite would void ~60 km around a 2 km site. This pins the geometry so nobody
    /// "fixes" the lid by growing the punch again.
    #[test]
    fn a_dem_sized_punch_cannot_drop_a_tile_but_the_site_floor_can() {
        let (face, level, i, j) = (0u8, 8u32, 128, 128);
        let (u, v) = tile_center_uv(face, level, i, j);
        let dir = cube_to_sphere(face, u, v).normalize();

        // A 1950 m footprint is a 0.064° cone; these tiles subtend ~0.35°.
        assert!(
            !tile_fully_in_punch(face, level, i, j, &punch_for(1950.0, dir)),
            "a DEM-sized cone cannot contain a tile — hence the floor"
        );

        // SITE_PUNCH_DEG = 2.0 in `placement.rs`.
        let floor = MOON_RADIUS_M * 2.0_f64.to_radians().sin();
        assert!(
            tile_fully_in_punch(face, level, i, j, &punch_for(floor, dir)),
            "the site-scale floor must actually drop tiles"
        );
    }
}
