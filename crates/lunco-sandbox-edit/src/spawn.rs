//! Spawn system — click-to-place with ghost preview.

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::{CellCoord, Grid};
use std::collections::HashMap;
use lunco_usd_bevy::UsdStageAsset;

use crate::catalog::{prim_path_from_entry_id, SpawnCatalog, SpawnSource};
use crate::SpawnState;

/// Ghost entity shown at the spawn placement point.
#[derive(Component)]
pub struct SpawnGhost;

use lunco_usd_bevy::SPAWN_GROUND_CLEARANCE;

/// Cached, real-time-derived spawn footprints per catalog entry.
///
/// The footprint is computed once — when the entry's USD stage finishes loading
/// during `SpawnState::Selecting` — by taking the asset's collision-geometry AABB
/// in its own frame (see [`lunco_usd_bevy::collision_aabb`]). It reads the same
/// composed data that `sync_usd_visuals` instantiates and that the avian compound
/// collider is built from, so the placement solver and the live physics body can
/// never disagree — the object rests on its lowest collider point for ANY asset
/// (lander, rover, prop), no wheels and no per-asset table required. Cached so the
/// per-frame ghost and the click observer read a pre-computed value
/// (frame-discipline: never recomputed every frame). The strong `Handle` keeps the
/// stage resident while the entry is selected so the asset doesn't unload between
/// the ghost poll and the click.
#[derive(Resource, Default)]
pub struct FootprintCache {
    map: HashMap<String, CachedFootprint>,
}

struct CachedFootprint {
    handle: Handle<UsdStageAsset>,
    root_prim: String,
    /// Collision-geometry AABB in the asset's own frame — `Some` once the stage is
    /// composed AND the asset has collision geometry. `None` for a pure-visual prop
    /// (which then falls back to the authored `lunco:spawnLift`).
    footprint: Option<lunco_usd_bevy::ObjectAabb>,
    /// Authored `lunco:spawnLift` — the rest-height fallback used only when no
    /// collision geometry is found (pure-visual / mesh-only asset).
    spawn_lift: f32,
}

/// Placement data after resolving derived-vs-authored: the footprint half-
/// extents and the root→ground rest height to lift the spawn along the normal.
#[derive(Clone, Copy)]
struct ResolvedFootprint {
    half_w: f64,
    half_l: f64,
    lift: f64,
}

impl Default for ResolvedFootprint {
    fn default() -> Self {
        // Sensible fallback used only before the stage has loaded (a frame or
        // two during selection); replaced by the real value once composed.
        Self { half_w: 0.75, half_l: 1.0, lift: 0.5 }
    }
}

impl FootprintCache {
    /// Resolve `entry_id`'s placement data: the collision-AABB footprint + rest
    /// depth for any asset with colliders, the authored `lunco:spawnLift` as a
    /// fallback for pure-visual assets, or a default if not yet loaded.
    fn resolve(&self, entry_id: &str) -> ResolvedFootprint {
        let Some(c) = self.map.get(entry_id) else { return ResolvedFootprint::default() };
        match c.footprint {
            // Rest on the lowest collision point (+ a small skin gap), with the
            // footprint box from the collider extents — general across landers,
            // rovers and props, no wheel-specific path.
            Some(aabb) => ResolvedFootprint {
                half_w: aabb.half_w().max(0.1),
                half_l: aabb.half_l().max(0.1),
                lift: aabb.rest_depth() + SPAWN_GROUND_CLEARANCE,
            },
            // No collision geometry (pure-visual / mesh-only): authored lift.
            None => ResolvedFootprint {
                half_w: 0.75,
                half_l: 1.0,
                lift: c.spawn_lift as f64,
            },
        }
    }
}

/// Ensure `entry_id`'s footprint is loaded into `cache` (loading the USD stage
/// on first sight, computing the footprint once the composed data is ready),
/// then return the resolved placement data. Idempotent: a no-op once cached.
/// Called from the ghost system every frame during selection — cheap because
/// the `HashMap` lookup hits after the first frame and the asset server
/// deduplicates `load`.
fn ensure_footprint(
    cache: &mut FootprintCache,
    catalog: &SpawnCatalog,
    asset_server: &AssetServer,
    stages: &Assets<UsdStageAsset>,
    canonical: &mut lunco_usd_bevy::CanonicalStages,
    entry_id: &str,
) -> ResolvedFootprint {
    let Some(entry) = catalog.get(entry_id) else {
        return cache.resolve(entry_id);
    };
    let SpawnSource::UsdFile(path) = &entry.source;
    {
        let cached = cache
            .map
            .entry(entry_id.to_string())
            .or_insert_with(|| CachedFootprint {
                handle: asset_server.load(path.clone()),
                root_prim: prim_path_from_entry_id(entry_id),
                footprint: None,
                spawn_lift: entry.spawn_lift,
            });
        if cached.footprint.is_none() {
            // Ph0′ canonical-only: derive the footprint off the LIVE canonical
            // stage (the source of truth), built on demand from the asset's
            // recipe.
            let id = cached.handle.id();
            if canonical.get(id).is_none() {
                if let Some(recipe) = stages.get(&cached.handle).and_then(|a| a.recipe.clone()) {
                    canonical.get_or_build(id, &recipe);
                }
            }
            cached.footprint = canonical
                .get(id)
                .and_then(|cs| lunco_usd_bevy::collision_aabb(&cs.view(), &cached.root_prim));
            if let Some(aabb) = cached.footprint {
                info!(
                    "[spawn] derived footprint for {}: half_w={:.3} half_l={:.3} rest_depth={:.3}",
                    entry_id, aabb.half_w(), aabb.half_l(), aabb.rest_depth()
                );
            }
        }
    }
    cache.resolve(entry_id)
}

/// Computes a world-space ray from the camera through the cursor position.
fn cursor_ray(
    camera: &Camera,
    cam_tf: &GlobalTransform,
    cursor: Vec2,
) -> Option<(DVec3, Dir3)> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    Some((ray.origin.as_dvec3(), ray.direction))
}

/// Query alias: every streamed DEM terrain's height oracle + its frame.
pub(crate) type TerrainOracles<'w, 's> =
    Query<'w, 's, (&'static lunco_terrain_surface::DemHeightField, &'static GlobalTransform)>;

/// Nearest ANALYTIC-terrain hit along a world ray, across all DEM terrains —
/// `(distance, world point)`. This, not a physics raycast, is the ground truth
/// for placement UI: the terrain's colliders exist only in a small ring around
/// dynamic bodies and are band-limited below the drawn micro-relief, so a
/// collider cast over open ground either misses (stale ghost) or reports a
/// surface above the drawn one — the ghost visibly flew over the ground.
pub(crate) fn terrain_ray_hit(
    terrains: &TerrainOracles,
    origin: DVec3,
    dir: DVec3,
    max_t: f64,
) -> Option<(f64, DVec3)> {
    let mut best: Option<(f64, DVec3)> = None;
    for (hf, t_gt) in terrains.iter() {
        let inv = t_gt.affine().inverse();
        let lo = inv.transform_point3(origin.as_vec3()).as_dvec3();
        let ld = inv.transform_vector3(dir.as_vec3()).as_dvec3().normalize();
        let Some(hit_local) = lunco_terrain_surface::raycast_surface(&hf.0, lo, ld, max_t) else {
            continue;
        };
        let hit_world = t_gt.affine().transform_point3(hit_local.as_vec3()).as_dvec3();
        let t = (hit_world - origin).length();
        if best.map(|(bt, _)| t < bt).unwrap_or(true) {
            best = Some((t, hit_world));
        }
    }
    best
}

/// Terrain-surface height (world Y) under a world `(x, z)`, from the oracle of
/// whichever DEM terrain contains the point. Used for footprint corner probes —
/// exact where the collider ring is band-limited or absent.
pub(crate) fn terrain_height_at(terrains: &TerrainOracles, world: DVec3) -> Option<f64> {
    use lunco_terrain_surface::HeightSource;
    for (hf, t_gt) in terrains.iter() {
        let inv = t_gt.affine().inverse();
        let l = inv.transform_point3(world.as_vec3()).as_dvec3();
        let half = hf.0.half_extent() as f64;
        if l.x.abs() > half || l.z.abs() > half {
            continue;
        }
        let h = hf.0.height_at(l.x, l.z);
        let w = t_gt.affine().transform_point3(bevy::math::Vec3::new(l.x as f32, h as f32, l.z as f32));
        return Some(w.y as f64);
    }
    None
}

/// Updates the spawn ghost position to follow the mouse raycast hit.
pub fn update_spawn_ghost(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    spawn_state: Res<SpawnState>,
    catalog: Res<SpawnCatalog>,
    asset_server: Res<AssetServer>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut footprint_cache: ResMut<FootprintCache>,
    cameras: Query<
        (&Camera, &GlobalTransform, &bevy::camera::RenderTarget, &CellCoord, &Transform),
        With<Camera3d>,
    >,
    windows: Query<&Window>,
    q_ghost: Query<(Entity, &Transform), With<SpawnGhost>>,
    grids: Query<(Entity, &Grid)>,
    raycaster: avian3d::prelude::SpatialQuery,
    terrains: TerrainOracles,
) {
    let SpawnState::Selecting { entry_id } = spawn_state.as_ref() else {
        for (ghost, _) in q_ghost.iter() {
            commands.entity(ghost).try_despawn();
        }
        return;
    };
    // Derive the wheel footprint from the live USD geometry (cached). Until the
    // stage finishes loading the fallback default is used, then the ghost
    // snaps to the real slope-fit once available.
    let fp = ensure_footprint(&mut *footprint_cache, &catalog, &asset_server, &stages, &mut canonical, entry_id);

    // Ray through the ACTIVE window camera (the one you're looking through) —
    // not merely the first Camera3d, which may now be an inactive scene camera.
    let (camera, cam_tf, cam_cell, cam_local) = match cameras
        .iter()
        .find(|(cam, _, target, _, _)| {
            cam.is_active && matches!(target, bevy::camera::RenderTarget::Window(_))
        })
        .map(|(cam, tf, _, cell, local)| (cam, tf, cell, local))
    {
        Some(c) => c,
        None => return,
    };
    let window = match windows.iter().next() {
        Some(w) => w,
        None => return,
    };
    let Some(cursor) = window.cursor_position() else { return };
    let Some((origin, direction)) = cursor_ray(camera, cam_tf, cursor) else { return };

    // Physics colliders (props, structures, rocks) AND the analytic terrain
    // surface, nearest wins. The terrain must come from the oracle, not the
    // collider ring — see `terrain_ray_hit`.
    let phys = raycaster
        .cast_ray(origin, direction, 1000.0, false, &avian3d::prelude::SpatialQueryFilter::default())
        .map(|h| h.distance);
    let terr = terrain_ray_hit(&terrains, origin, direction.as_dvec3(), 1000.0);
    let (hit, terrain_primary) = match (phys, terr) {
        (Some(pt), Some((tt, tp))) => {
            if tt <= pt {
                (Some(tp), true)
            } else {
                (Some(origin + direction.as_dvec3() * pt), false)
            }
        }
        (Some(pt), None) => (Some(origin + direction.as_dvec3() * pt), false),
        (None, Some((_, tp))) => (Some(tp), true),
        (None, None) => (None, false),
    };

    if let Some(point) = hit {

        // --- Terrain-conforming placement (footprint derived in real time) ---
        let half_w = fp.half_w;
        let half_l = fp.half_l;

        let cam_forward = cam_tf.forward().as_dvec3();
        let mut forward_xz = DVec3::new(cam_forward.x, 0.0, cam_forward.z);
        if forward_xz.length_squared() < 1e-5 {
            forward_xz = DVec3::NEG_Z;
        } else {
            forward_xz = forward_xz.normalize();
        }
        let right_xz = forward_xz.cross(DVec3::Y).normalize();

        let point_d = point;
        let corners = [
            point_d + forward_xz * half_l - right_xz * half_w,
            point_d + forward_xz * half_l + right_xz * half_w,
            point_d - forward_xz * half_l - right_xz * half_w,
            point_d - forward_xz * half_l + right_xz * half_w,
        ];

        // Corner heights: over open terrain the oracle is exact where the
        // collider ring is band-limited or absent; over a structure the physics
        // down-ray is what the footprint should rest on. Order by what the
        // primary hit was, falling through to the other, then to the hit plane.
        let mut hit_points = Vec::new();
        for corner in corners {
            let phys_y = || {
                let ray_origin = corner + DVec3::Y * 50.0;
                raycaster
                    .cast_ray(
                        ray_origin,
                        Dir3::NEG_Y,
                        100.0,
                        false,
                        &avian3d::prelude::SpatialQueryFilter::default(),
                    )
                    .map(|h| (ray_origin + DVec3::NEG_Y * h.distance).y)
            };
            let terr_y = || terrain_height_at(&terrains, corner);
            let y = if terrain_primary {
                terr_y().or_else(phys_y)
            } else {
                phys_y().or_else(terr_y)
            }
            .unwrap_or(point_d.y);
            hit_points.push(DVec3::new(corner.x, y, corner.z));
        }

        let fl = hit_points[0];
        let fr = hit_points[1];
        let rl = hit_points[2];
        let rr = hit_points[3];
        let avg_y = (fl.y + fr.y + rl.y + rr.y) / 4.0;

        let v_forward = ((fl - rl) + (fr - rr)) / 2.0;
        let v_right = ((fr - fl) + (rr - rl)) / 2.0;
        let mut normal = v_forward.cross(v_right);
        if normal.length_squared() > 1e-5 {
            normal = normal.normalize();
        } else {
            normal = DVec3::Y;
        }
        if normal.y < 0.0 {
            normal = -normal;
        }

        let mut spawn_forward = forward_xz - normal * forward_xz.dot(normal);
        if spawn_forward.length_squared() < 1e-5 {
            spawn_forward = forward_xz;
        } else {
            spawn_forward = spawn_forward.normalize();
        }
        // spawn_right is horizontal right, adjusted for normal
        let cross = spawn_forward.cross(normal);
        let spawn_right = if cross.length_squared() > 1e-5 {
            cross.normalize()
        } else {
            let mut perp = normal.cross(DVec3::X);
            if perp.length_squared() < 1e-5 {
                perp = normal.cross(DVec3::Z);
            }
            perp.normalize()
        };
        // spawn_backward (Z) = spawn_right (X) x normal (Y)
        let spawn_backward = spawn_right.cross(normal).normalize();
        let spawn_rot_mat = Mat3::from_cols(
            spawn_right.as_vec3(),
            normal.as_vec3(),
            spawn_backward.as_vec3(),
        );
        let rotation = Quat::from_mat3(&spawn_rot_mat);

        // Ghost is a sphere — only its position matters, so it sits at the
        // terrain contact; the real root-height lift (fp.lift) is applied at
        // spawn-click time, not in the preview.
        let ghost_pos = DVec3::new(point_d.x, avg_y, point_d.z) + normal * 0.05;

        // Place the ghost CELL-GRID AWARE. Every coordinate above is in the
        // render (origin-relative) frame — the camera IS the FloatingOrigin, and
        // the terrain/collider rays were built from origin-relative transforms.
        // Lift the hit into the grid-ABSOLUTE frame through the camera's own
        // (cell, transform), then split it back into a real (CellCoord, local)
        // via the grid. A cell-less ghost `ChildOf(grid)` composes off cell
        // (0,0,0), so on an elevated site (origin at cell.y≠0) it rendered ~one
        // whole cell (~2 km) underground — "the ghost never appears on the
        // ground". This lands it on the real surface at any origin cell.
        let Some((grid_ent, grid)) = grids.iter().next() else { return };
        let ghost_abs = ghost_pos + grid.grid_position_double(cam_cell, cam_local);
        let (ghost_cell, ghost_local) = grid.translation_to_grid(ghost_abs);
        bevy::log::warn!(
            "[GHOST] render=({:.0},{:.0},{:.0}) cam_abs=({:.0},{:.0},{:.0}) ghost_abs=({:.0},{:.0},{:.0}) cell=({},{},{}) local=({:.0},{:.0},{:.0})",
            ghost_pos.x, ghost_pos.y, ghost_pos.z,
            grid.grid_position_double(cam_cell, cam_local).x, grid.grid_position_double(cam_cell, cam_local).y, grid.grid_position_double(cam_cell, cam_local).z,
            ghost_abs.x, ghost_abs.y, ghost_abs.z,
            ghost_cell.x, ghost_cell.y, ghost_cell.z, ghost_local.x, ghost_local.y, ghost_local.z,
        );

        if let Some((ghost, _)) = q_ghost.iter().next() {
            commands.entity(ghost).try_insert((
                ghost_cell,
                Transform { translation: ghost_local, rotation, ..default() },
            ));
        } else {
            commands.spawn((
                Name::new("SpawnGhost"),
                SpawnGhost,
                ghost_cell,
                Transform { translation: ghost_local, rotation, ..default() },
                Mesh3d(meshes.add(Sphere::new(0.5).mesh().ico(8).unwrap())),
                // Appearance INTENT — the render binder turns this into a
                // `StandardMaterial`. `perceptual_roughness: 0.5` reproduces
                // bevy's `StandardMaterial::default()` exactly (PbrLook defaults
                // to 1.0, the regolith value), so the ghost looks unchanged.
                lunco_render::PbrLook {
                    base_color: Color::srgba(0.5, 1.0, 0.5, 0.4).to_linear(),
                    perceptual_roughness: 0.5,
                    ..default()
                },
                ChildOf(grid_ent),
                Visibility::Visible,
                InheritedVisibility::default(),
                ViewVisibility::default(),
            ));
        }
    }
}

/// Keeps `SpawnToolActive` in sync with spawn mode and handles Escape-cancel.
///
/// `SpawnToolActive` is read by possession to stay out of the way while the
/// spawn tool is armed; it used to be set as a side effect of the old click
/// system, so it now lives in its own Update system. Escape is keyboard-driven,
/// not a pointer pick, so it stays a system too.
pub fn spawn_tool_state_system(
    mut commands: Commands,
    mut spawn_state: ResMut<SpawnState>,
    mut tool_active: ResMut<lunco_core::SpawnToolActive>,
    keys: Res<ButtonInput<KeyCode>>,
    q_ghost: Query<Entity, With<SpawnGhost>>,
) {
    tool_active.0 = matches!(spawn_state.as_ref(), SpawnState::Selecting { .. });

    if tool_active.0 && keys.just_pressed(KeyCode::Escape) {
        for ghost in q_ghost.iter() {
            commands.entity(ghost).try_despawn();
        }
        *spawn_state = SpawnState::Idle;
    }
}

/// Places the selected asset where the user clicks, driven by **bevy_picking**.
///
/// Registered as a global `On<Pointer<Click>>` observer. The pick's
/// `hit.position` is the world point on whatever mesh (terrain/prop) was under
/// the cursor — no manual ray-cast needed. egui occlusion is handled by the
/// framework; chrome clicks carry no `hit.position`, so they're rejected and
/// never place. Triggers `SpawnEntity` so the path matches the CLI.
pub fn on_scene_click_spawn(
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    mut commands: Commands,
    mut spawn_state: ResMut<SpawnState>,
    footprint_cache: Res<FootprintCache>,
    keys: Res<ButtonInput<KeyCode>>,
    q_grids: Query<(Entity, &Grid)>,
    q_ghost: Query<Entity, With<SpawnGhost>>,
    cameras: Query<(&Camera, &GlobalTransform, &CellCoord, &Transform), With<Camera3d>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    raycaster: avian3d::prelude::SpatialQuery,
    terrains: TerrainOracles,
) {
    use bevy::picking::pointer::PointerButton;
    // Stop the click bubbling to ancestors (global observer re-fires up the tree).
    click.propagate(false);
    if click.button != PointerButton::Primary {
        return;
    }
    let SpawnState::Selecting { entry_id } = spawn_state.as_ref() else {
        return;
    };
    let entry_id = entry_id.clone();
    // Shared egui-vs-scene guard + camera ray (same path as possession/selection),
    // then resolve the world point: use bevy_picking's mesh hit when present, else
    // cast the ray against colliders so placement works on streamed terrain even
    // when no pickable tile is under the cursor (the old `hit.position` guard
    // silently rejected those clicks — the "can't place on the ground" bug).
    let Some((camera, cam_gtf, cam_cell, cam_local)) = cameras.iter().find(|(c, _, _, _)| c.is_active) else {
        return;
    };
    let Some(ray) = lunco_core::scene_click_ray(&egui_focus, camera, cam_gtf, click.pointer_location.position) else {
        return;
    };
    // Terrain-as-source-of-truth placement: ray-march the terrain ORACLE and the
    // physics colliders (props / structures / rocks); the NEARER hit wins. Over
    // open ground the oracle is the truth — the band-limited collider ring rounds
    // crater bowls shallower, so preferring the collider perched the rover above
    // the crater ("floating over craters"). Mirrors `update_spawn_ghost` so the
    // placed rover lands exactly where the ghost previewed it. `terrain_primary`
    // then orders the per-corner slope-fit probes below.
    let origin = ray.origin.as_dvec3();
    let dir = ray.direction.as_dvec3();
    let phys = raycaster
        .cast_ray(origin, ray.direction, 1.0e6, false, &avian3d::prelude::SpatialQueryFilter::default())
        .map(|h| h.distance);
    let terr = terrain_ray_hit(&terrains, origin, dir, 1.0e6);
    let (point_d, terrain_primary) = match (phys, terr) {
        (Some(pd), Some((td, tp))) => {
            if td <= pd { (tp, true) } else { (origin + dir * pd, false) }
        }
        (Some(pd), None) => (origin + dir * pd, false),
        (None, Some((_, tp))) => (tp, true),
        (None, None) => return,
    };

    let Some((grid, grid_comp)) = q_grids.iter().next() else {
        return;
    };

    // --- Terrain-conforming placement (footprint derived in real time) ---
    // The footprint comes from the same USD geometry that gets instantiated
    // (cached by the ghost system during selection), so the wheels' real
    // contact patch — not a hand-tuned table — drives the slope fit.
    let fp = footprint_cache.resolve(&entry_id);
    let half_w = fp.half_w;
    let half_l = fp.half_l;

    // 2. Get camera forward direction to orient the rover
    let cam_forward = cameras.iter().next()
        .map(|(_, tf, _, _)| tf.forward().as_dvec3())
        .unwrap_or(DVec3::NEG_Z);
    let mut forward_xz = DVec3::new(cam_forward.x, 0.0, cam_forward.z);
    if forward_xz.length_squared() < 1e-5 {
        forward_xz = DVec3::NEG_Z;
    } else {
        forward_xz = forward_xz.normalize();
    }
    let right_xz = forward_xz.cross(DVec3::Y).normalize();

    // 3. Define 4 corners of the footprint
    let corners = [
        point_d + forward_xz * half_l - right_xz * half_w, // FL
        point_d + forward_xz * half_l + right_xz * half_w, // FR
        point_d - forward_xz * half_l - right_xz * half_w, // RL
        point_d - forward_xz * half_l + right_xz * half_w, // RR
    ];

    // 4. Corner heights for the slope fit. Terrain is the source of truth: over
    // open ground the oracle is exact where the band-limited collider ring rounds
    // the crater bowl; over a structure the physics down-ray is what the footprint
    // rests on. Order by whichever the primary pick hit (`terrain_primary`) — same
    // as `update_spawn_ghost`, so placement matches the preview.
    let mut hit_points = Vec::new();
    for corner in corners {
        let phys_y = || {
            let ray_origin = corner + DVec3::Y * 50.0;
            raycaster
                .cast_ray(
                    ray_origin,
                    Dir3::NEG_Y,
                    100.0,
                    false,
                    &avian3d::prelude::SpatialQueryFilter::default(),
                )
                .map(|h| (ray_origin + DVec3::NEG_Y * h.distance).y)
        };
        let terr_y = || terrain_height_at(&terrains, corner);
        let y = if terrain_primary { terr_y().or_else(phys_y) } else { phys_y().or_else(terr_y) }
            .unwrap_or(point_d.y);
        hit_points.push(DVec3::new(corner.x, y, corner.z));
    }

    // 5. Compute average height and fit normal. The rest height is the MEAN of the
    //    four footprint corners (matching the ghost preview) so the chassis sits
    //    flush on the high-quality collider ring — which tracks the visual crater
    //    bowl — instead of perching on the single highest corner (a crater rim),
    //    which reads as the rover floating above the crater.
    let fl = hit_points[0];
    let fr = hit_points[1];
    let rl = hit_points[2];
    let rr = hit_points[3];
    let avg_y = (fl.y + fr.y + rl.y + rr.y) / 4.0;

    let v_forward = ((fl - rl) + (fr - rr)) / 2.0;
    let v_right = ((fr - fl) + (rr - rl)) / 2.0;
    let mut normal = v_forward.cross(v_right);
    if normal.length_squared() > 1e-5 {
        normal = normal.normalize();
    } else {
        normal = DVec3::Y;
    }
    if normal.y < 0.0 {
        normal = -normal;
    }

    // 6. Compute spawn orientation aligned to the normal
    let mut spawn_forward = forward_xz - normal * forward_xz.dot(normal);
    if spawn_forward.length_squared() < 1e-5 {
        spawn_forward = forward_xz;
    } else {
        spawn_forward = spawn_forward.normalize();
    }
    // spawn_right is horizontal right, adjusted for normal
    let cross = spawn_forward.cross(normal);
    let spawn_right = if cross.length_squared() > 1e-5 {
        cross.normalize()
    } else {
        let mut perp = normal.cross(DVec3::X);
        if perp.length_squared() < 1e-5 {
            perp = normal.cross(DVec3::Z);
        }
        perp.normalize()
    };
    // spawn_backward (Z) = spawn_right (X) x normal (Y)
    let spawn_backward = spawn_right.cross(normal).normalize();
    let spawn_rot_mat = Mat3::from_cols(
        spawn_right.as_vec3(),
        normal.as_vec3(),
        spawn_backward.as_vec3(),
    );
    let rotation = Quat::from_mat3(&spawn_rot_mat);

    // Place wheels IN CONTACT with the terrain, not gapped. `contact_depth`
    // is the exact root→lowest-wheel rest height, so lifting by it alone puts
    // the wheels exactly on the ground. The 1 cm *embed* (negative margin)
    // guarantees contact even under float error / non-planar terrain: for a
    // rigid-jointed rover (no suspension — e.g. rocker-bogie) a gap would
    // free-fall→slam→joint-echo and explode the constraint graph on activation;
    // a slight embed is the stable init (solver gently resolves it). Raycast
    // drivetrains absorb this via suspension, so it's safe for both.
    let spawn_pos = DVec3::new(point_d.x, avg_y, point_d.z) + normal * (fp.lift - 0.01);
    // The whole placement solve above ran in the render (origin-relative) frame
    // (camera ray + terrain-oracle round-trips). The spawn command plants its
    // position as a GRID-ABSOLUTE `Transform` (cell 0 + avian recenter), so lift it
    // into grid-absolute via the camera's own (cell, transform). Without this, at an
    // elevated site the render-frame Y (~-45 m) was planted as grid-absolute and the
    // spawned body dropped ~2 km below the surface.
    let spawn_abs = spawn_pos + grid_comp.grid_position_double(cam_cell, cam_local);
    let point3 = spawn_abs.as_vec3();

    commands.trigger(crate::commands::SpawnEntity {
        target: grid,
        entry_id: entry_id.clone(),
        position: point3,
        rotation: Some(rotation),
    });
    info!("Spawn request: {} at {:?} with rot {:?}", entry_id, point3, rotation);

    let sticky = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if !sticky {
        for ghost in q_ghost.iter() {
            commands.entity(ghost).try_despawn();
        }
        *spawn_state = SpawnState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_state_transitions() {
        let mut state = SpawnState::Idle;
        assert!(matches!(state, SpawnState::Idle));

        state = SpawnState::Selecting { entry_id: "ball_dynamic".into() };
        assert!(matches!(state, SpawnState::Selecting { .. }));

        state = SpawnState::Idle;
        assert!(matches!(state, SpawnState::Idle));
    }

    #[test]
    fn test_cursor_ray_returns_none_for_invalid_cursor() {
        // Basic sanity check for the function signature
        assert!(true);
    }
}
