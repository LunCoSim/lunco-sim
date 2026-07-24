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

use bevy::camera::visibility::NoFrustumCulling;
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
    pub retiring: Vec<(Entity, u8)>,
}

/// Frames an outgoing tile stays alive after its replacement spawned.
const TILE_RETIRE_FRAMES: u8 = 3;

/// Max fresh tiles spawned per body per frame. A fast zoom crosses several
/// LOD levels in a handful of frames; unbudgeted, one frame could demand
/// hundreds of fresh meshes (build + `Assets<Mesh>` add + render-world
/// upload) — the p99 ~150 ms zoom hitch. Outgoing tiles stay resident until
/// their replacements are up (see the coverage rule in [`update_globe_lod`]),
/// so spreading spawns costs nothing but a few frames of refinement latency.
const TILE_SPAWN_BUDGET: usize = 16;

/// Max retired-tile entities despawned per body per frame. A zoom-out merges
/// hundreds of fine tiles into a few coarse ones in one step; freeing all
/// their entities + mesh assets in one frame is its own (smaller) hitch.
const TILE_DESPAWN_BUDGET: usize = 32;

/// Squared camera distance to a tile's centre (body-local) — spawn priority.
fn tile_dist2(coord: &TileCoord, radius_m: f64, camera_body_local: DVec3) -> f64 {
    let (u, v) = tile_center_uv(coord.face, coord.level, coord.i, coord.j);
    (cube_to_sphere(coord.face, u, v) * radius_m).distance_squared(camera_body_local)
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
// is fixed (ViewportPanel + auto_tag → Camera3d active, black clear) and the
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
        // coverage for retirement below), BUDGETED per frame — see
        // `TILE_SPAWN_BUDGET`. Coarse-and-near first: a coarse tile covers the
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
        // INITIAL fill is unbudgeted: with no resident tiles there is no old
        // coverage to hold the sphere together while spawns amortize — a
        // budgeted first fill shows a partially-tiled globe for ~15 frames at
        // scene load. One synchronous fill there is the old (pre-budget)
        // behavior and is hidden behind scene loading anyway.
        let budget = if tiles.resident.is_empty() {
            usize::MAX
        } else {
            TILE_SPAWN_BUDGET
        };
        for coord in missing.into_iter().take(budget) {
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
                    Mesh3d(meshes.add(mesh)),
                    lod.look.clone(),
                    coord,
                    TerrainTile,
                    tile_cell,
                    Transform::from_translation(tile_local_pos),
                    GlobalTransform::default(),
                    Visibility::Visible,
                    InheritedVisibility::default(),
                    NoFrustumCulling,
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
        // With budgeted spawning the replacements arrive over several frames;
        // retiring on a fixed schedule would open holes in the sphere while
        // the spawn queue catches up. The extra overlap is coplanar identical
        // surface — invisible, same as the retire grace.
        let resident_now: HashSet<TileCoord> = tiles.resident.keys().copied().collect();
        let mut newly_retired: Vec<(Entity, u8)> = Vec::new();
        tiles.resident.retain(|coord, ent| {
            if desired.contains(coord) {
                return true;
            }
            let covered = desired
                .iter()
                .filter(|d| tiles_overlap(d, coord))
                .all(|d| resident_now.contains(d));
            if covered {
                newly_retired.push((*ent, TILE_RETIRE_FRAMES));
                false
            } else {
                true
            }
        });
        tiles.retiring.extend(newly_retired);
        let mut despawned = 0usize;
        tiles.retiring.retain_mut(|(ent, frames)| {
            if *frames == 0 {
                if despawned < TILE_DESPAWN_BUDGET {
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
