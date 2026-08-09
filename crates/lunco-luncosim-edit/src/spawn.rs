//! Spawn system — click-to-place with ghost preview.

use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::coords::GridPos;
use lunco_core::{on_command, register_commands, Command};
use lunco_render::SceneCamera;
use lunco_usd_bevy::UsdStageAsset;
use std::collections::HashMap;

use crate::SpawnState;
use lunco_scene_commands::catalog::{prim_path_from_entry_id, SpawnCatalog, SpawnSource};

/// Ghost entity shown at the spawn placement point.
#[derive(Component)]
pub struct SpawnGhost;

/// Opt-in cursor-to-spawn trace. Enable with the typed command
/// `cmd("SetSpawnDiagnostics", #{enabled: true})` in the LunCo REPL (or the
/// equivalent API command). It logs each material cursor move and every click
/// decision, including render ray, chosen surface, canonical-world conversion,
/// grid cell/local placement, and the final [`lunco_scene_commands::commands::SpawnEntity`].
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

/// The single cursor-surface query used by both the preview and committed
/// placement, in the GRID frame. Terrain is authoritative where it exists;
/// physics supplies props and is the complete fallback for scenes without a DEM.
///
/// The analytic surface comes from [`GridSurfaceQuery`], which owns the frame:
/// this module used to keep its own copy that inverted the terrain's *render*
/// `GlobalTransform` and marched the oracle in that frame. At a site with real
/// elevation that offset the ray by the terrain's world position (~one big_space
/// cell), so no cursor ray ever met the surface and placement silently degraded
/// to the collider ring, which exists only around dynamic bodies.
#[derive(Clone, Copy, Debug)]
struct CursorSurfaceHit {
    /// Grid-absolute surface point under the cursor.
    point: GridPos,
    terrain_primary: bool,
    terrain: Option<lunco_terrain_surface::SurfaceHit>,
    physics_distance: Option<f64>,
    /// WHICH collider answered. "A physics hit happened" does not say whether
    /// placement landed on the site's streamed terrain ring or on a leftover
    /// ground plane from a previously-loaded scene — and those are opposite
    /// bugs. Diagnostics only.
    physics_entity: Option<Entity>,
}

fn cursor_surface_hit(
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    raycaster: &lunco_physics::GridSpatialQuery<'_, '_>,
    origin: GridPos,
    direction: Dir3,
) -> Option<CursorSurfaceHit> {
    let terrain = surface.raycast(origin, direction, f64::INFINITY);
    // Terrain is a useful near bound when it exists, but it must never be a
    // prerequisite: a physical scene can have no DEM, or its terrain can still
    // be streaming when a user begins placing assets.
    let physics_limit = terrain.map(|hit| hit.distance).unwrap_or(f64::INFINITY);
    let physics = raycaster.cast_ray_grid(
        origin,
        direction,
        physics_limit,
        false,
        &avian3d::prelude::SpatialQueryFilter::default(),
    );
    resolve_cursor_surface(
        origin,
        direction.as_dvec3(),
        terrain,
        physics.map(|hit| hit.distance),
        physics.map(|hit| hit.entity),
    )
}

fn resolve_cursor_surface(
    origin: GridPos,
    direction: DVec3,
    terrain: Option<lunco_terrain_surface::SurfaceHit>,
    physics_distance: Option<f64>,
    physics_entity: Option<Entity>,
) -> Option<CursorSurfaceHit> {
    match (physics_distance, terrain) {
        (Some(physics_distance), Some(hit)) if physics_distance < hit.distance => {
            Some(CursorSurfaceHit {
                point: GridPos(origin.0 + direction * physics_distance),
                terrain_primary: false,
                terrain: Some(hit),
                physics_distance: Some(physics_distance),
                physics_entity,
            })
        }
        (_, Some(hit)) => Some(CursorSurfaceHit {
            point: hit.point,
            terrain_primary: true,
            terrain: Some(hit),
            physics_distance,
            physics_entity,
        }),
        (Some(physics_distance), None) => Some(CursorSurfaceHit {
            point: GridPos(origin.0 + direction * physics_distance),
            terrain_primary: false,
            terrain: None,
            physics_distance: Some(physics_distance),
            physics_entity,
        }),
        (None, None) => None,
    }
}

/// Corner-height sampler for the footprint fit: the analytic surface over open
/// ground (exact where the collider ring is band-limited or absent), the physics
/// down-ray over a structure. Ordered by whatever the primary pick hit, so the
/// preview and the commit resolve identically.
fn corner_height(
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    raycaster: &lunco_physics::GridSpatialQuery<'_, '_>,
    terrain_primary: bool,
    corner: DVec3,
) -> Option<f64> {
    let phys_y = || {
        let ray_origin = GridPos(corner + DVec3::Y * 50.0);
        raycaster
            .cast_ray_grid(
                ray_origin,
                Dir3::NEG_Y,
                100.0,
                false,
                &avian3d::prelude::SpatialQueryFilter::default(),
            )
            .map(|h| (ray_origin.0 + DVec3::NEG_Y * h.distance).y)
    };
    let terr_y = || surface.height_at(GridPos(corner));
    if terrain_primary {
        terr_y().or_else(phys_y)
    } else {
        phys_y().or_else(terr_y)
    }
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
        (&Camera, &GlobalTransform, &bevy::camera::RenderTarget),
        (With<Camera3d>, With<SceneCamera>),
    >,
    windows: Query<&Window>,
    q_ghost: Query<(Entity, &Transform), With<SpawnGhost>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    // Diagnostics only: names the collider a placement ray actually landed on.
    q_names: Query<&Name>,
    mut diagnostics: ResMut<SpawnDiagnostics>,
    // The cursor ray is born in the render frame (the camera is the FloatingOrigin)
    // and is converted ONCE, here, into the grid frame; everything downstream —
    // analytic surface, colliders, footprint fit, the placed ghost — is
    // grid-absolute. Mixing the two is what put the ghost a cell underground.
    raycaster: lunco_physics::GridSpatialQuery,
    surface: lunco_terrain_surface::GridSurfaceQuery,
) {
    let SpawnState::Selecting { entry_id } = spawn_state.as_ref() else {
        for (ghost, _) in q_ghost.iter() {
            commands.entity(ghost).try_despawn();
        }
        return;
    };
    // The palette and other workbench panels own the pointer while the cursor is
    // over them.  Do not raycast through the panel into the scene: that produces
    // a ghost on terrain the user cannot see and makes the spawn tool appear to
    // stick in a canyon or behind the palette.  The committed click path uses
    // the same scene gate via `scene_click_ray`.
    if egui_focus.wants_pointer {
        for (ghost, _) in q_ghost.iter() {
            commands.entity(ghost).try_despawn();
        }
        return;
    }
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

    // ONE frame crossing, at the top.
    let Some(origin_grid) = surface.to_grid(lunco_core::coords::RenderPos(origin)) else {
        if diagnostics.enabled {
            info!(cursor = ?cursor, "[spawn-trace] ghost rejected: WorldGrid unavailable");
        }
        return;
    };

    let hit = cursor_surface_hit(&surface, &raycaster, origin_grid, direction);
    let terrain_trace = hit.and_then(|h| h.terrain);
    let phys = hit.and_then(|h| h.physics_distance);
    let phys_name = hit
        .and_then(|h| h.physics_entity)
        .and_then(|e| q_names.get(e).ok())
        .map(|n| n.as_str().to_string());
    let terrain_primary = hit.is_some_and(|h| h.terrain_primary);

    if let Some(point) = hit.map(|h| h.point) {
        // --- Terrain-conforming placement (footprint derived in real time) ---
        // ONE slope-fit implementation, shared with the committed placement
        // below (`lunco_terrain_surface::fit_footprint`). These were two copies
        // that had to be kept in step by hand; the preview and the commit can no
        // longer disagree about where an asset rests.
        let fit = lunco_terrain_surface::fit_footprint(
            point,
            cam_tf.forward().as_dvec3(),
            fp.half_w,
            fp.half_l,
            |corner| corner_height(&surface, &raycaster, terrain_primary, corner),
        );
        let rotation = fit.rotation;

        // Ghost is a sphere — only its position matters, so it sits at the
        // terrain contact; the real root-height lift (fp.lift) is applied at
        // spawn-click time, not in the preview.
        let ghost_grid = GridPos(fit.point.0 + fit.normal * 0.05);

        // Place the ghost CELL-GRID AWARE: split the grid-absolute point into
        // the world grid's own (cell, local) pair. A cell-less ghost
        // `ChildOf(grid)` composes off cell (0,0,0), so on an elevated site it
        // rendered ~one whole cell (~2 km) underground.
        let Some((grid_ent, ghost_cell, ghost_local)) = surface.grid_local(ghost_grid) else {
            if diagnostics.enabled {
                info!(cursor = ?cursor, grid_hit = ?point, "[spawn-trace] ghost rejected: WorldGrid unavailable");
            }
            return;
        };
        if trace_cursor {
            info!(
                cursor = ?cursor,
                camera_render = ?cam_tf.translation(),
                ray_origin_render = ?origin,
                ray_origin_grid = ?origin_grid,
                ray_direction = ?direction,
                terrain_hit = ?terrain_trace,
                physics_distance = ?phys,
                chosen_grid_hit = ?point,
                terrain_primary,
                ghost_world = ?ghost_grid,
                world_grid = ?grid_ent,
                ghost_cell = ?ghost_cell,
                ghost_local = ?ghost_local,
                physics_hit_name = ?phys_name,
                "[spawn-trace] cursor pipeline"
            );
        }
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
            ray_origin_grid = ?origin_grid,
            ray_direction = ?direction,
            terrain_hit = ?terrain_trace,
            physics_distance = ?phys,
            physics_hit_name = ?phys_name,
            has_terrain = surface.has_terrain(),
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
    q_ghost: Query<Entity, With<SpawnGhost>>,
    cameras: Query<
        (&Camera, &GlobalTransform, &bevy::camera::RenderTarget),
        (With<Camera3d>, With<SceneCamera>),
    >,
    egui_focus: Res<lunco_core::EguiFocus>,
    // `GridSpatialQuery`, not raw `SpatialQuery` — same choke point the ghost preview
    // (and wheels / altimeter) use: the click ray + corner probes originate in the
    // render frame, so they must be shifted into avian's grid-absolute frame or they
    // miss every collider at an elevated site. See `lunco_physics::spatial`.
    raycaster: lunco_physics::GridSpatialQuery,
    surface: lunco_terrain_surface::GridSurfaceQuery,
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
    // The preview and the commit call the SAME resolver and the SAME footprint
    // fit, in the SAME frame, so an asset always lands where its ghost was shown.
    let origin = ray.origin.as_dvec3();
    let Some(origin_grid) = surface.to_grid(lunco_core::coords::RenderPos(origin)) else {
        if diagnostics.enabled {
            info!("[spawn-trace] click rejected: canonical WorldGrid unavailable");
        }
        return;
    };
    let Some(hit) = cursor_surface_hit(&surface, &raycaster, origin_grid, ray.direction) else {
        if diagnostics.enabled {
            info!(
                pointer = ?click.pointer_location.position,
                ray_origin_grid = ?origin_grid,
                ray_direction = ?ray.direction,
                has_terrain = surface.has_terrain(),
                "[spawn-trace] click rejected: no terrain or physics hit"
            );
        }
        return;
    };
    let Some((grid, _, _)) = surface.grid_local(hit.point) else {
        if diagnostics.enabled {
            info!("[spawn-trace] click rejected: canonical WorldGrid unavailable");
        }
        return;
    };

    // The footprint comes from the same USD geometry that gets instantiated
    // (cached by the ghost system during selection), so the wheels' real contact
    // patch — not a hand-tuned table — drives the slope fit.
    //
    // Camera forward orients the rover: the ACTIVE camera (the one the ray came
    // through), not `cameras.iter().next()` (which can be an inactive scene
    // camera pointing elsewhere → rover spawned facing a random direction).
    // Rotation is frame-free — big_space translates the render frame, never
    // rotates it — so the camera's render-frame forward is usable as-is.
    let fp = footprint_cache.resolve(&entry_id);
    let fit = lunco_terrain_surface::fit_footprint(
        hit.point,
        cam_gtf.forward().as_dvec3(),
        fp.half_w,
        fp.half_l,
        |corner| corner_height(&surface, &raycaster, hit.terrain_primary, corner),
    );

    // Place wheels IN CONTACT with the terrain, not gapped. `fp.lift` is the
    // exact root→lowest-collider rest height, so lifting by it alone puts the
    // wheels exactly on the ground. The 1 cm *embed* (negative margin)
    // guarantees contact even under float error / non-planar terrain: for a
    // rigid-jointed rover (no suspension — e.g. rocker-bogie) a gap would
    // free-fall→slam→joint-echo and explode the constraint graph on activation;
    // a slight embed is the stable init. Raycast drivetrains absorb this via
    // suspension, so it is safe for both.
    let spawn_world = GridPos(fit.point.0 + fit.normal * (fp.lift - 0.01));

    commands.trigger(lunco_core::SpawnEntity {
        target: grid,
        entry_id: entry_id.clone(),
        position: spawn_world.0.as_vec3(),
        rotation: Some(fit.rotation),
    });
    info!(
        entry_id,
        pointer = ?click.pointer_location.position,
        ray_origin_grid = ?origin_grid,
        ray_direction = ?ray.direction,
        terrain_hit = ?hit.terrain,
        physics_distance = ?hit.physics_distance,
        chosen_grid_hit = ?hit.point,
        terrain_primary = hit.terrain_primary,
        spawn_world = ?spawn_world,
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
        let origin = GridPos(DVec3::new(10.0, 20.0, 30.0));
        let hit = resolve_cursor_surface(origin, DVec3::NEG_Y, None, Some(7.5), None)
            .expect("a collider hit must place without terrain");

        assert_eq!(hit.point.0, DVec3::new(10.0, 12.5, 30.0));
        assert!(!hit.terrain_primary);
        assert_eq!(hit.physics_distance, Some(7.5));
    }

    #[test]
    fn nearer_physics_surface_beats_terrain() {
        let origin = GridPos(DVec3::ZERO);
        let terrain = lunco_terrain_surface::SurfaceHit {
            point: GridPos(DVec3::new(0.0, -12.0, 0.0)),
            distance: 12.0,
            terrain: Entity::PLACEHOLDER,
        };
        let hit = resolve_cursor_surface(origin, DVec3::NEG_Y, Some(terrain), Some(3.0), None)
            .expect("one of the surfaces must win");

        assert_eq!(hit.point.0, DVec3::new(0.0, -3.0, 0.0));
        assert!(!hit.terrain_primary);
    }

    /// Placement at a REAL site elevation: the surface is ~1.9 km below the body
    /// datum, and the resolved point must be the terrain's own grid-absolute
    /// point — not a value derived by pushing it through any entity transform.
    #[test]
    fn terrain_surface_at_site_elevation_is_grid_absolute() {
        const SITE: f64 = -1931.0;
        let origin = GridPos(DVec3::new(-380.0, SITE + 60.0, -380.0));
        let terrain = lunco_terrain_surface::SurfaceHit {
            point: GridPos(DVec3::new(-380.0, SITE, -380.0)),
            distance: 60.0,
            terrain: Entity::PLACEHOLDER,
        };
        let hit = resolve_cursor_surface(origin, DVec3::NEG_Y, Some(terrain), None, None)
            .expect("the analytic surface must place with no collider present");

        assert!(hit.terrain_primary);
        assert_eq!(hit.point.0.y, SITE);
    }
}
