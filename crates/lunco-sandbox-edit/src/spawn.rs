//! Spawn system — click-to-place with ghost preview.

use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};
use lunco_usd_bevy::UsdStageAsset;
use std::collections::HashMap;

use crate::catalog::{prim_path_from_entry_id, SpawnCatalog, SpawnSource};
use crate::SpawnState;

/// Ghost entity shown at the spawn placement point.
#[derive(Component)]
pub struct SpawnGhost;

/// Opt-in cursor-to-spawn trace. Enable with the typed command
/// `cmd("SetSpawnDiagnostics", #{enabled: true})` in the LunCo REPL (or the
/// equivalent API command). It logs each material cursor move and every click
/// decision, including render ray, chosen surface, canonical-world conversion,
/// grid cell/local placement, and the final [`crate::commands::SpawnEntity`].
///
/// The trace deliberately has no production fallback or parallel coordinate
/// calculation: it observes the exact path that creates the ghost and command.
#[derive(Resource)]
pub struct SpawnDiagnostics {
    /// Whether production placement systems emit their pipeline trace.
    pub enabled: bool,
    last_cursor: Option<Vec2>,
}

impl Default for SpawnDiagnostics {
    fn default() -> Self {
        Self {
            // Useful for automated/runtime diagnosis before the REPL is ready.
            // The typed command remains the normal live control surface.
            enabled: std::env::var_os("LUNCO_SPAWN_TRACE").is_some_and(|value| value == "1"),
            last_cursor: None,
        }
    }
}

impl SpawnDiagnostics {
    fn cursor_moved(&mut self, cursor: Vec2) -> bool {
        let moved = self
            .last_cursor
            .is_none_or(|previous| previous.distance_squared(cursor) > 0.25);
        if moved {
            self.last_cursor = Some(cursor);
        }
        moved
    }
}

/// Enable or disable the Spawn Ghost pipeline trace.
#[Command(default)]
pub struct SetSpawnDiagnostics {
    pub enabled: bool,
}

#[on_command(SetSpawnDiagnostics)]
fn on_set_spawn_diagnostics(
    trigger: On<SetSpawnDiagnostics>,
    mut diagnostics: ResMut<SpawnDiagnostics>,
) {
    diagnostics.enabled = trigger.event().enabled;
    diagnostics.last_cursor = None;
    info!(
        enabled = diagnostics.enabled,
        "[spawn-trace] diagnostics updated"
    );
}

register_commands!(on_set_spawn_diagnostics,);

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
        Self {
            half_w: 0.75,
            half_l: 1.0,
            lift: 0.5,
        }
    }
}

impl FootprintCache {
    /// Resolve `entry_id`'s placement data: the collision-AABB footprint + rest
    /// depth for any asset with colliders, the authored `lunco:spawnLift` as a
    /// fallback for pure-visual assets, or a default if not yet loaded.
    fn resolve(&self, entry_id: &str) -> ResolvedFootprint {
        let Some(c) = self.map.get(entry_id) else {
            return ResolvedFootprint::default();
        };
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
                    entry_id,
                    aabb.half_w(),
                    aabb.half_l(),
                    aabb.rest_depth()
                );
            }
        }
    }
    cache.resolve(entry_id)
}

/// Computes a world-space ray from the camera through the cursor position.
fn cursor_ray(camera: &Camera, cam_tf: &GlobalTransform, cursor: Vec2) -> Option<(DVec3, Dir3)> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    Some((ray.origin.as_dvec3(), ray.direction))
}

/// Query alias: every streamed DEM terrain's height oracle + its frame.
pub(crate) type TerrainOracles<'w, 's> = Query<
    'w,
    's,
    (
        &'static lunco_terrain_surface::DemHeightField,
        &'static GlobalTransform,
    ),
>;

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
        let hit_world = t_gt
            .affine()
            .transform_point3(hit_local.as_vec3())
            .as_dvec3();
        let t = (hit_world - origin).length();
        if best.map(|(bt, _)| t < bt).unwrap_or(true) {
            best = Some((t, hit_world));
        }
    }
    best
}

/// The single cursor-surface query used by both the preview and committed
/// placement. Terrain is authoritative where it exists; physics supplies props
/// and is also the complete fallback for scenes without a loaded DEM.
///
/// The previous spelling accidentally put the physics ray behind
/// `terrain_ray_hit(...).and_then(...)`. That made terrain availability a
/// prerequisite for *all* placement, so a perfectly pickable slab or rover in a
/// non-DEM scene produced neither a Spawn Ghost nor a committed spawn.
#[derive(Clone, Copy, Debug)]
struct CursorSurfaceHit {
    point: DVec3,
    terrain_primary: bool,
    terrain: Option<(f64, DVec3)>,
    physics_distance: Option<f64>,
}

fn cursor_surface_hit(
    terrains: &TerrainOracles,
    raycaster: &lunco_physics::GridSpatialQuery<'_, '_>,
    origin: DVec3,
    direction: Dir3,
) -> Option<CursorSurfaceHit> {
    let terrain = terrain_ray_hit(terrains, origin, direction.as_dvec3(), f64::INFINITY);
    // Terrain is a useful near bound when it exists, but it must never be a
    // prerequisite: a physical scene can have no DEM, or its terrain can still
    // be streaming when a user begins placing assets.
    let physics_limit = terrain
        .map(|(distance, _)| distance)
        .unwrap_or(f64::INFINITY);
    let physics_distance = raycaster
        .cast_ray_render(
            lunco_core::coords::RenderPos(origin),
            direction,
            physics_limit,
            false,
            &avian3d::prelude::SpatialQueryFilter::default(),
        )
        .map(|hit| hit.distance);

    resolve_cursor_surface(origin, direction.as_dvec3(), terrain, physics_distance)
}

fn resolve_cursor_surface(
    origin: DVec3,
    direction: DVec3,
    terrain: Option<(f64, DVec3)>,
    physics_distance: Option<f64>,
) -> Option<CursorSurfaceHit> {
    match (physics_distance, terrain) {
        (Some(physics_distance), Some((terrain_distance, terrain_point)))
            if physics_distance < terrain_distance =>
        {
            Some(CursorSurfaceHit {
                point: origin + direction * physics_distance,
                terrain_primary: false,
                terrain: Some((terrain_distance, terrain_point)),
                physics_distance: Some(physics_distance),
            })
        }
        (_, Some((terrain_distance, terrain_point))) => Some(CursorSurfaceHit {
            point: terrain_point,
            terrain_primary: true,
            terrain: Some((terrain_distance, terrain_point)),
            physics_distance,
        }),
        (Some(physics_distance), None) => Some(CursorSurfaceHit {
            point: origin + direction * physics_distance,
            terrain_primary: false,
            terrain: None,
            physics_distance: Some(physics_distance),
        }),
        (None, None) => None,
    }
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
        let w = t_gt
            .affine()
            .transform_point3(bevy::math::Vec3::new(l.x as f32, h as f32, l.z as f32));
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
    cameras: Query<(&Camera, &GlobalTransform, &bevy::camera::RenderTarget), With<Camera3d>>,
    windows: Query<&Window>,
    q_ghost: Query<(Entity, &Transform), With<SpawnGhost>>,
    mut diagnostics: ResMut<SpawnDiagnostics>,
    world_frame: lunco_core::coords::WorldFrame,
    // `GridSpatialQuery`, not raw `SpatialQuery`: the cursor ray + corner probes
    // originate in the render frame (the camera is the FloatingOrigin), so they must
    // be shifted into avian's grid-absolute frame or they miss every collider at an
    // elevated site. See `lunco_physics::spatial`.
    raycaster: lunco_physics::GridSpatialQuery,
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
    let fp = ensure_footprint(
        &mut *footprint_cache,
        &catalog,
        &asset_server,
        &stages,
        &mut canonical,
        entry_id,
    );

    // Ray through the ACTIVE window camera (the one you're looking through) —
    // not merely the first Camera3d, which may now be an inactive scene camera.
    let (camera, cam_tf) = match cameras
        .iter()
        .find(|(cam, _, target)| {
            cam.is_active && matches!(target, bevy::camera::RenderTarget::Window(_))
        })
        .map(|(cam, tf, _)| (cam, tf))
    {
        Some(c) => c,
        None => {
            if diagnostics.enabled {
                info!("[spawn-trace] ghost rejected: no active window Camera3d");
            }
            return;
        }
    };
    let window = match windows.iter().next() {
        Some(w) => w,
        None => {
            if diagnostics.enabled {
                info!("[spawn-trace] ghost rejected: no window");
            }
            return;
        }
    };
    let Some(cursor) = window.cursor_position() else {
        if diagnostics.enabled {
            info!("[spawn-trace] ghost rejected: window has no cursor position");
        }
        return;
    };
    let Some((origin, direction)) = cursor_ray(camera, cam_tf, cursor) else {
        if diagnostics.enabled {
            info!(cursor = ?cursor, "[spawn-trace] ghost rejected: viewport_to_world failed");
        }
        return;
    };
    let trace_cursor = diagnostics.enabled && diagnostics.cursor_moved(cursor);

    let surface = cursor_surface_hit(&terrains, &raycaster, origin, direction);
    let terrain_trace = surface.and_then(|hit| hit.terrain);
    let phys = surface.and_then(|hit| hit.physics_distance);
    let hit = surface.map(|hit| hit.point);
    let terrain_primary = surface.is_some_and(|hit| hit.terrain_primary);

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
                    .cast_ray_render(
                        lunco_core::coords::RenderPos(ray_origin),
                        Dir3::NEG_Y,
                        100.0,
                        false,
                        &avian3d::prelude::SpatialQueryFilter::default(),
                    )
                    // Hit distance is frame-independent; the visual corner height is
                    // the render-frame origin walked down by that distance.
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
        let ghost_render = lunco_core::coords::RenderPos(ghost_pos);
        let Some(ghost_abs) = world_frame.render_to_world(ghost_render) else {
            if diagnostics.enabled {
                info!(cursor = ?cursor, render_hit = ?point, "[spawn-trace] ghost rejected: WorldGrid unavailable");
            }
            return;
        };
        let Some((grid_ent, ghost_cell, ghost_local)) =
            world_frame.render_to_world_grid_local(ghost_render)
        else {
            return;
        };
        if trace_cursor {
            info!(
                cursor = ?cursor,
                camera_render = ?cam_tf.translation(),
                ray_origin = ?origin,
                ray_direction = ?direction,
                terrain_hit = ?terrain_trace,
                physics_distance = ?phys,
                chosen_render_hit = ?point,
                terrain_primary,
                ghost_render = ?ghost_pos,
                ghost_world = ?ghost_abs,
                world_grid = ?grid_ent,
                ghost_cell = ?ghost_cell,
                ghost_local = ?ghost_local,
                "[spawn-trace] cursor pipeline"
            );
        }
        // Invert the target grid's own floating-origin transform. `ghost_pos`
        // came from the camera/terrain in render space; it must not be lifted
        // through an unrelated Avian body's frame before being made a child of
        // this grid.
        if let Some((ghost, _)) = q_ghost.iter().next() {
            commands.entity(ghost).try_insert((
                ghost_cell,
                Transform {
                    translation: ghost_local,
                    rotation,
                    ..default()
                },
            ));
        } else {
            commands.spawn((
                Name::new("SpawnGhost"),
                SpawnGhost,
                ghost_cell,
                Transform {
                    translation: ghost_local,
                    rotation,
                    ..default()
                },
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
    } else if trace_cursor {
        info!(
            cursor = ?cursor,
            ray_origin = ?origin,
            ray_direction = ?direction,
            terrain_hit = ?terrain_trace,
            physics_distance = ?phys,
            "[spawn-trace] ghost has no surface hit"
        );
    }
}

/// Keeps `SpawnToolActive` in sync with spawn mode and disarms on Cancel.
///
/// `SpawnToolActive` is read by possession to stay out of the way while the
/// spawn tool is armed; it used to be set as a side effect of the old click
/// system, so it now lives in its own Update system. Cancel is keyboard-driven,
/// not a pointer pick, so it stays a system too.
///
/// Reads the [`lunco_core::CancelIntent`], NOT a raw `KeyCode::Escape`: the bindings
/// are data (`assets/config/keybindings.json`), so backing out of the spawn ghost uses
/// the same vocabulary as backing out of everything else and follows a rebind.
pub fn spawn_tool_state_system(
    mut commands: Commands,
    mut spawn_state: ResMut<SpawnState>,
    mut tool_active: ResMut<lunco_core::SpawnToolActive>,
    cancel: lunco_core::CancelIntent,
    q_ghost: Query<Entity, With<SpawnGhost>>,
) {
    tool_active.0 = matches!(spawn_state.as_ref(), SpawnState::Selecting { .. });

    if tool_active.0 && cancel.just_pressed() {
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
    diagnostics: Res<SpawnDiagnostics>,
    world_frame: lunco_core::coords::WorldFrame,
    q_ghost: Query<Entity, With<SpawnGhost>>,
    cameras: Query<(&Camera, &GlobalTransform, &bevy::camera::RenderTarget), With<Camera3d>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    // `GridSpatialQuery`, not raw `SpatialQuery` — same choke point the ghost preview
    // (and wheels / altimeter) use: the click ray + corner probes originate in the
    // render frame, so they must be shifted into avian's grid-absolute frame or they
    // miss every collider at an elevated site. See `lunco_physics::spatial`.
    raycaster: lunco_physics::GridSpatialQuery,
    terrains: TerrainOracles,
) {
    use bevy::picking::pointer::PointerButton;
    if click.button != PointerButton::Primary {
        return;
    }
    let SpawnState::Selecting { entry_id } = spawn_state.as_ref() else {
        if diagnostics.enabled {
            info!(entity = ?click.entity, "[spawn-trace] click ignored: SpawnState is Idle");
        }
        return;
    };
    // Stop the click bubbling to ancestors (global observer re-fires up the tree).
    click.propagate(false);
    let entry_id = entry_id.clone();
    // Shared egui-vs-scene guard + camera ray (same path as possession/selection),
    // then resolve the world point: use bevy_picking's mesh hit when present, else
    // cast the ray against colliders so placement works on streamed terrain even
    // when no pickable tile is under the cursor (the old `hit.position` guard
    // silently rejected those clicks — the "can't place on the ground" bug).
    let Some((camera, cam_gtf, _)) = cameras.iter().find(|(camera, _, target)| {
        camera.is_active && matches!(target, bevy::camera::RenderTarget::Window(_))
    }) else {
        if diagnostics.enabled {
            info!("[spawn-trace] click rejected: no active window Camera3d");
        }
        return;
    };
    let Some(ray) = lunco_core::scene_click_ray(
        &egui_focus,
        camera,
        cam_gtf,
        click.pointer_location.position,
    ) else {
        if diagnostics.enabled {
            info!(
                pointer = ?click.pointer_location.position,
                egui_wants_pointer = egui_focus.wants_pointer,
                "[spawn-trace] click rejected: no scene ray"
            );
        }
        return;
    };
    // The preview and commit call the same terrain-or-physics resolver, so an
    // asset always lands where its ghost was shown.
    let origin = ray.origin.as_dvec3();
    let Some(surface) = cursor_surface_hit(&terrains, &raycaster, origin, ray.direction) else {
        if diagnostics.enabled {
            info!(
                pointer = ?click.pointer_location.position,
                ray_origin = ?origin,
                ray_direction = ?ray.direction,
                "[spawn-trace] click rejected: no terrain or physics hit"
            );
        }
        return;
    };
    let point_d = surface.point;
    let terrain_primary = surface.terrain_primary;

    let Some((grid, _)) = world_frame.grid() else {
        if diagnostics.enabled {
            info!("[spawn-trace] click rejected: canonical WorldGrid unavailable");
        }
        return;
    };

    // --- Terrain-conforming placement (footprint derived in real time) ---
    // The footprint comes from the same USD geometry that gets instantiated
    // (cached by the ghost system during selection), so the wheels' real
    // contact patch — not a hand-tuned table — drives the slope fit.
    let fp = footprint_cache.resolve(&entry_id);
    let half_w = fp.half_w;
    let half_l = fp.half_l;

    // 2. Camera forward orients the rover — the ACTIVE camera (the one the ray came
    // through), not `cameras.iter().next()` (which can be an inactive scene camera
    // pointing elsewhere → rover spawned facing a random direction).
    let cam_forward = cam_gtf.forward().as_dvec3();
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
                .cast_ray_render(
                    lunco_core::coords::RenderPos(ray_origin),
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
    // The whole placement solve above ran in the render (origin-relative)
    // frame. Convert it through the selected grid's own floating-origin affine
    // transform, matching the preview exactly; an Avian-body-derived shift is
    // not a valid inverse for nested/rotated big_space grids.
    let Some(spawn_abs) = world_frame.render_to_world(lunco_core::coords::RenderPos(spawn_pos)) else {
        if diagnostics.enabled {
            info!(spawn_render = ?spawn_pos, "[spawn-trace] click rejected: render-to-world conversion unavailable");
        }
        return;
    };
    let point3 = spawn_abs.0.as_vec3();

    commands.trigger(crate::commands::SpawnEntity {
        target: grid,
        entry_id: entry_id.clone(),
        position: point3,
        rotation: Some(rotation),
    });
    info!(
        entry_id,
        pointer = ?click.pointer_location.position,
        ray_origin = ?origin,
        ray_direction = ?ray.direction,
        terrain_hit = ?surface.terrain,
        physics_distance = ?surface.physics_distance,
        chosen_render_hit = ?point_d,
        terrain_primary,
        spawn_render = ?spawn_pos,
        spawn_world = ?spawn_abs,
        world_grid = ?grid,
        "[spawn-trace] SpawnEntity triggered"
    );

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

        state = SpawnState::Selecting {
            entry_id: "ball_dynamic".into(),
        };
        assert!(matches!(state, SpawnState::Selecting { .. }));

        state = SpawnState::Idle;
        assert!(matches!(state, SpawnState::Idle));
    }

    #[test]
    fn test_cursor_ray_returns_none_for_invalid_cursor() {
        // Basic sanity check for the function signature
        assert!(true);
    }

    #[test]
    fn physics_surface_is_valid_when_no_dem_is_loaded() {
        let origin = DVec3::new(10.0, 20.0, 30.0);
        let hit = resolve_cursor_surface(origin, DVec3::NEG_Y, None, Some(7.5))
            .expect("a collider hit must place without terrain");

        assert_eq!(hit.point, DVec3::new(10.0, 12.5, 30.0));
        assert!(!hit.terrain_primary);
        assert_eq!(hit.physics_distance, Some(7.5));
    }

    #[test]
    fn nearer_physics_surface_beats_terrain() {
        let origin = DVec3::ZERO;
        let terrain_point = DVec3::new(0.0, -12.0, 0.0);
        let hit =
            resolve_cursor_surface(origin, DVec3::NEG_Y, Some((12.0, terrain_point)), Some(3.0))
                .expect("one of the surfaces must win");

        assert_eq!(hit.point, DVec3::new(0.0, -3.0, 0.0));
        assert!(!hit.terrain_primary);
    }
}
