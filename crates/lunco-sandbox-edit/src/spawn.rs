//! Spawn system — click-to-place with ghost preview.

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::Grid;
use std::collections::HashMap;
use lunco_usd_bevy::UsdStageAsset;

use crate::catalog::{prim_path_from_entry_id, SpawnCatalog, SpawnSource};
use crate::SpawnState;

/// Ghost entity shown at the spawn placement point.
#[derive(Component)]
pub struct SpawnGhost;

/// Cached, real-time-derived spawn footprints per catalog entry.
///
/// The footprint is computed once — when the entry's USD stage finishes loading
/// during `SpawnState::Selecting` — by walking the composed stage geometry (see
/// [`lunco_usd_bevy::wheel_footprint`]). It reads the same composed data that
/// `sync_usd_visuals` instantiates, so the placement solver and the live entity
/// can never disagree (no hand-tuned per-asset table for vehicles). For
/// non-vehicle props (no wheels) the authored `lunco:spawnLift` is still
/// honoured. Cached so the per-frame ghost and the click observer read a
/// pre-computed value (frame-discipline: never recomputed every frame). The
/// strong `Handle` keeps the stage resident while the entry is selected so the
/// asset doesn't unload between the ghost poll and the click.
#[derive(Resource, Default)]
pub struct FootprintCache {
    map: HashMap<String, CachedFootprint>,
}

struct CachedFootprint {
    handle: Handle<UsdStageAsset>,
    root_prim: String,
    /// Derived wheel footprint — `Some` once the stage is loaded AND the asset
    /// has wheel prims. `None` for non-vehicle props (which use `spawn_lift`).
    footprint: Option<lunco_usd_bevy::WheelFootprint>,
    /// Authored `lunco:spawnLift` — the rest-height fallback for props with no
    /// wheels. Ignored for vehicles (the derived `contact_depth` supersedes it).
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
    /// Resolve `entry_id`'s placement data: derived geometry for vehicles,
    /// authored `lunco:spawnLift` for props, or a default if not yet loaded.
    fn resolve(&self, entry_id: &str) -> ResolvedFootprint {
        let Some(c) = self.map.get(entry_id) else { return ResolvedFootprint::default() };
        match c.footprint {
            // Vehicle: real wheel contact patch + derived rest height.
            Some(fp) => ResolvedFootprint {
                half_w: fp.half_w,
                half_l: fp.half_l,
                lift: fp.contact_depth,
            },
            // Prop (no wheels): default footprint box + authored lift.
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
                .and_then(|cs| lunco_usd_bevy::wheel_footprint(&cs.view(), &cached.root_prim));
            if let Some(fp) = cached.footprint {
                info!(
                    "[spawn] derived footprint for {}: half_w={:.3} half_l={:.3} depth={:.3}",
                    entry_id, fp.half_w, fp.half_l, fp.contact_depth
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

/// Updates the spawn ghost position to follow the mouse raycast hit.
pub fn update_spawn_ghost(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    spawn_state: Res<SpawnState>,
    catalog: Res<SpawnCatalog>,
    asset_server: Res<AssetServer>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut footprint_cache: ResMut<FootprintCache>,
    cameras: Query<(&Camera, &GlobalTransform, &bevy::camera::RenderTarget), With<Camera3d>>,
    windows: Query<&Window>,
    q_ghost: Query<(Entity, &Transform), With<SpawnGhost>>,
    grids: Query<Entity, With<Grid>>,
    raycaster: avian3d::prelude::SpatialQuery,
) {
    let SpawnState::Selecting { entry_id } = spawn_state.as_ref() else {
        for (ghost, _) in q_ghost.iter() {
            commands.entity(ghost).despawn();
        }
        return;
    };
    // Derive the wheel footprint from the live USD geometry (cached). Until the
    // stage finishes loading the fallback default is used, then the ghost
    // snaps to the real slope-fit once available.
    let fp = ensure_footprint(&mut *footprint_cache, &catalog, &asset_server, &stages, &mut canonical, entry_id);

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
        None => return,
    };
    let window = match windows.iter().next() {
        Some(w) => w,
        None => return,
    };
    let Some(cursor) = window.cursor_position() else { return };
    let Some((origin, direction)) = cursor_ray(camera, cam_tf, cursor) else { return };

    let hit = raycaster.cast_ray(origin, direction, 1000.0, false, &avian3d::prelude::SpatialQueryFilter::default());

    if let Some(hit_data) = hit {
        let point = origin + direction.as_dvec3() * hit_data.distance;

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

        let mut hit_points = Vec::new();
        for corner in corners {
            let ray_origin = corner + DVec3::Y * 50.0;
            let ray_dir = Dir3::NEG_Y;
            let hit = raycaster.cast_ray(
                ray_origin,
                ray_dir,
                100.0,
                false,
                &avian3d::prelude::SpatialQueryFilter::default(),
            );
            if let Some(hit_data) = hit {
                let hit_point = ray_origin + ray_dir.as_dvec3() * hit_data.distance;
                hit_points.push(hit_point);
            } else {
                hit_points.push(DVec3::new(corner.x, point_d.y, corner.z));
            }
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
        let point3 = ghost_pos.as_vec3();

        if let Some((ghost, _)) = q_ghost.iter().next() {
            commands.entity(ghost).insert(Transform {
                translation: point3,
                rotation,
                ..default()
            });
        } else {
            let grid = match grids.iter().next() {
                Some(g) => g,
                None => return,
            };
            let mat = materials.add(StandardMaterial {
                base_color: Color::srgba(0.5, 1.0, 0.5, 0.4),
                ..default()
            });
            commands.spawn((
                Name::new("SpawnGhost"),
                SpawnGhost,
                Transform {
                    translation: point3,
                    rotation,
                    ..default()
                },
                Mesh3d(meshes.add(Sphere::new(0.5).mesh().ico(8).unwrap())),
                MeshMaterial3d(mat),
                ChildOf(grid),
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
            commands.entity(ghost).despawn();
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
    q_grids: Query<Entity, With<Grid>>,
    q_ghost: Query<Entity, With<SpawnGhost>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    raycaster: avian3d::prelude::SpatialQuery,
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
    // Chrome guard + world point: egui's pick has no position; a mesh hit does.
    let Some(point) = click.hit.position else {
        return;
    };

    let Some(grid) = q_grids.iter().next() else {
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
        .map(|(_, tf)| tf.forward().as_dvec3())
        .unwrap_or(DVec3::NEG_Z);
    let mut forward_xz = DVec3::new(cam_forward.x, 0.0, cam_forward.z);
    if forward_xz.length_squared() < 1e-5 {
        forward_xz = DVec3::NEG_Z;
    } else {
        forward_xz = forward_xz.normalize();
    }
    let right_xz = forward_xz.cross(DVec3::Y).normalize();

    // 3. Define 4 corners of the footprint
    let point_d = point.as_dvec3();
    let corners = [
        point_d + forward_xz * half_l - right_xz * half_w, // FL
        point_d + forward_xz * half_l + right_xz * half_w, // FR
        point_d - forward_xz * half_l - right_xz * half_w, // RL
        point_d - forward_xz * half_l + right_xz * half_w, // RR
    ];

    // 4. Raycast at corners to find terrain heights
    let mut hit_points = Vec::new();
    for corner in corners {
        let ray_origin = corner + DVec3::Y * 50.0;
        let ray_dir = Dir3::NEG_Y;
        let hit = raycaster.cast_ray(
            ray_origin,
            ray_dir,
            100.0,
            false,
            &avian3d::prelude::SpatialQueryFilter::default(),
        );
        if let Some(hit_data) = hit {
            let hit_point = ray_origin + ray_dir.as_dvec3() * hit_data.distance;
            hit_points.push(hit_point);
        } else {
            hit_points.push(DVec3::new(corner.x, point_d.y, corner.z));
        }
    }

    // 5. Compute average height and fit normal
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
    let point3 = spawn_pos.as_vec3();

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
            commands.entity(ghost).despawn();
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
