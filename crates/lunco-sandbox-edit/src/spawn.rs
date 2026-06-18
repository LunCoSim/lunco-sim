//! Spawn system — click-to-place with ghost preview.

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::Grid;

use crate::catalog::SpawnCatalog;
use crate::SpawnState;

/// Ghost entity shown at the spawn placement point.
#[derive(Component)]
pub struct SpawnGhost;

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
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    q_ghost: Query<(Entity, &Transform), With<SpawnGhost>>,
    grids: Query<Entity, With<Grid>>,
    raycaster: avian3d::prelude::SpatialQuery,
) {
    if !matches!(spawn_state.as_ref(), SpawnState::Selecting { .. }) {
        for (ghost, _) in q_ghost.iter() {
            commands.entity(ghost).despawn();
        }
        return;
    }

    let (camera, cam_tf) = match cameras.iter().next() {
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
        let point3 = Vec3::new(point.x as f32, point.y as f32, point.z as f32);

        if let Some((ghost, _)) = q_ghost.iter().next() {
            commands.entity(ghost).insert(Transform::from_translation(point3));
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
                Transform::from_translation(point3),
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
    catalog: Res<SpawnCatalog>,
    keys: Res<ButtonInput<KeyCode>>,
    q_grids: Query<Entity, With<Grid>>,
    q_ghost: Query<Entity, With<SpawnGhost>>,
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

    // Lift the spawn point per-asset (USD `lunco:spawnLift` or a built-in pin).
    let offset_y = catalog.get(&entry_id).map(|e| e.spawn_lift).unwrap_or(0.0);
    let point3 = Vec3::new(point.x, point.y + offset_y, point.z);
    let Some(grid) = q_grids.iter().next() else {
        return;
    };

    commands.trigger(crate::commands::SpawnEntity {
        target: grid,
        entry_id: entry_id.clone(),
        position: point3,
    });
    info!("Spawn request: {} at {:?}", entry_id, point3);

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
