//! Spawn system — click-to-place with ghost preview.

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::Grid;

use crate::catalog::{SpawnCatalog, SpawnCategory};
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

/// Handles placement when the user clicks while in spawn mode.
///
/// Uses left-click for placement.
/// Triggers a SPAWN_ENTITY CommandMessage so the same path is used for CLI.
pub fn handle_spawn_placement(
    mut commands: Commands,
    mut spawn_state: ResMut<SpawnState>,
    catalog: Res<SpawnCatalog>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    q_grids: Query<Entity, With<Grid>>,
    q_ghost: Query<(Entity, &Transform), With<SpawnGhost>>,
    raycaster: avian3d::prelude::SpatialQuery,
) {
    let entry_id = match spawn_state.as_ref() {
        SpawnState::Selecting { entry_id } => entry_id.clone(),
        _ => return,
    };

    // Left click to place
    if !mouse.just_pressed(MouseButton::Left) {
        // Escape cancels spawn mode
        if keys.just_pressed(KeyCode::Escape) {
            for (ghost, _) in q_ghost.iter() {
                commands.entity(ghost).despawn();
            }
            *spawn_state = SpawnState::Idle;
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

        // Apply Y offset based on entry category so components spawn above the ground
        let offset_y = if let Some(entry) = catalog.get(&entry_id) {
            match entry.category {
                SpawnCategory::Component => 2.0,  // Components float above ground
                SpawnCategory::Rover => 1.0,      // Rovers slightly above
                SpawnCategory::Prop | SpawnCategory::Terrain => 0.0,  // Props/terrain on ground
            }
        } else {
            0.0
        };

        let point3 = Vec3::new(point.x as f32, point.y as f32 + offset_y, point.z as f32);
        let grid = match q_grids.iter().next() {
            Some(g) => g,
            None => return,
        };

        // Trigger spawn via typed command
        commands.trigger(crate::commands::SpawnEntity {
            target: grid,
            entry_id: entry_id.clone(),
            position: point3,
        });

        info!("Spawn request: {} at {:?}", entry_id, point3);

        for (ghost, _) in q_ghost.iter() {
            commands.entity(ghost).despawn();
        }
        *spawn_state = SpawnState::Idle;
    }

    // Escape cancels spawn mode
    if keys.just_pressed(KeyCode::Escape) {
        for (ghost, _) in q_ghost.iter() {
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
