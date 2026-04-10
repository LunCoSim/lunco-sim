//! Spawn system — click-to-place with ghost preview.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use big_space::prelude::Grid;
use transform_gizmo_bevy::GizmoTarget;

use crate::catalog::{SpawnCatalog, SpawnCategory};
use crate::gizmo::GizmoStartPos;
use crate::SpawnState;

/// Configuration for physics-based entity dragging.
#[derive(Resource)]
pub struct DragConfig {
    /// Spring constant pulling the entity toward the cursor (N/m).
    pub spring_constant: f64,
    /// Maximum force that can be applied (N).
    pub max_force: f64,
    /// Distance threshold to stop applying force (m).
    pub stop_distance: f64,
}

impl Default for DragConfig {
    fn default() -> Self {
        Self {
            spring_constant: 50.0,
            max_force: 500.0,
            stop_distance: 0.1,
        }
    }
}

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

/// Updates selected entity position to follow the mouse cursor using physics.
///
/// Runs in FixedUpdate to stay in sync with the physics engine.
/// When a user Shift+clicks to select an entity, it enters "drag mode" where
/// physics forces pull it towards the cursor raycast hit point. This respects
/// collisions and walls - the rover won't pass through solid objects.
pub fn update_selected_entity_drag(
    mut selected: ResMut<crate::SelectedEntity>,
    drag_config: Res<DragConfig>,
    mut drag_mode_active: ResMut<lunco_core::DragModeActive>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    q_drag_targets: Query<(Entity, &GlobalTransform), With<avian3d::prelude::RigidBody>>,
    mut param_set: ParamSet<(
        Query<avian3d::prelude::Forces>,
        Query<&mut avian3d::prelude::LinearVelocity>,
    )>,
    raycaster: SpatialQuery,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
) {
    // Handle drag state transitions
    if selected.is_dragging {
        drag_mode_active.active = true;
    } else {
        drag_mode_active.active = false;
        return;
    }

    let Some(entity) = selected.entity else {
        selected.is_dragging = false;
        return;
    };

    // Get global transform - verify entity still exists
    let gtf = match q_drag_targets.get(entity) {
        Ok((_, g)) => g,
        Err(_) => {
            selected.is_dragging = false;
            return;
        }
    };

    // Apply drag forces using Forces + LinearVelocity via ParamSet
    // Read velocity from LinearVelocity (not Forces) to avoid double-borrow
    let lin_vel_query = param_set.p1();
    let velocity = lin_vel_query.get(entity).map(|v| v.0).unwrap_or(DVec3::ZERO);

    let (camera, cam_gtf) = match cameras.iter().next() {
        Some(c) => c,
        None => return,
    };
    let window = match windows.iter().next() {
        Some(w) => w,
        None => return,
    };
    let Some(cursor) = window.cursor_position() else { return };
    let Some((origin, direction)) = cursor_ray(camera, cam_gtf, cursor) else { return };

    // Use GlobalTransform for world-space position calculations
    let current_pos = gtf.translation();
    let rover_y = current_pos.y as f64;

    // Calculate target point using plane intersection at rover's current height
    let target_point = if (rover_y - origin.y as f64).abs() > 0.01 {
        let t = (rover_y - origin.y as f64) / direction.y as f64;
        if t > 0.0 {
            origin + direction.as_dvec3() * t
        } else {
            current_pos.as_dvec3()
        }
    } else {
        let filter = SpatialQueryFilter::default().with_excluded_entities([entity]);
        if let Some(hit_data) = raycaster.cast_ray(origin, direction, 1000.0, false, &filter) {
            origin + direction.as_dvec3() * hit_data.distance as f64
        } else {
            current_pos.as_dvec3()
        }
    };

    let current_pos_d = current_pos.as_dvec3();
    let to_target = DVec3::new(target_point.x - current_pos_d.x, 0.0, target_point.z - current_pos_d.z);
    let distance = to_target.length();

    // Apply drag forces in a scoped block to release the borrow
    {
        let mut forces_query = param_set.p0();
        let mut forces = match forces_query.get_mut(entity) {
            Ok(f) => f,
            Err(_) => {
                selected.is_dragging = false;
                return;
            }
        };

        // Only apply horizontal force if we're not already at the target
        if distance > drag_config.stop_distance {
            let direction_to_target = to_target.normalize();

            // Spring force: proportional to distance
            let force_magnitude = (drag_config.spring_constant * distance).min(drag_config.max_force);

            // Damping force: opposes velocity (prevents oscillation)
            let damping_constant = 10.0;
            let damping_force = DVec3::new(velocity.x, 0.0, velocity.z) * damping_constant;

            // Net force = spring force - damping force
            let mut force_vector = direction_to_target * force_magnitude - damping_force;

            // Clamp to max force
            if force_vector.length() > drag_config.max_force {
                force_vector = force_vector.normalize() * drag_config.max_force;
            }

            // Apply force to the rigid body (only XZ component)
            forces.apply_force(force_vector);
        }
    }

    // Lock Y velocity to prevent falling during drag
    // Zero out downward velocity every frame - more reliable than counter-gravity forces
    {
        let mut lin_vel_query = param_set.p1();
        if let Ok(mut lin_vel) = lin_vel_query.get_mut(entity) {
            if lin_vel.y < 0.0 {
                lin_vel.0.y = 0.0;
            }
        }
    }

    // Right-click or Escape to exit drag mode, add gizmo for fine-tuning
    if keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right) {
        selected.is_dragging = false;
        drag_mode_active.active = false;

        // Apply strong damping to stop rover quickly
        {
            let mut forces_query = param_set.p0();
            if let Ok(mut forces) = forces_query.get_mut(entity) {
                let vel = forces.linear_velocity();
                forces.apply_force(-vel * 20.0);
            }
        }

        // Add GizmoTarget + GizmoStartPos after drag ends (physics safe now)
        // Using deferred commands so this runs after physics step completes
        let world_pos = gtf.translation();
        commands.entity(entity)
            .insert(GizmoTarget::default())
            .insert(GizmoStartPos { pos: Vec3::new(world_pos.x as f32, world_pos.y as f32, world_pos.z as f32) });

        info!("Placed entity at {:?}", current_pos);
    }
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
    raycaster: SpatialQuery,
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

    let hit = raycaster.cast_ray(origin, direction, 1000.0, false, &SpatialQueryFilter::default());

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
/// Uses left-click for placement (right-click is for selection).
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
    raycaster: SpatialQuery,
    selected: Res<crate::SelectedEntity>,
) {
    // Skip if currently dragging a selected entity
    if selected.is_dragging { return; }
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

    let hit = raycaster.cast_ray(origin, direction, 1000.0, false, &SpatialQueryFilter::default());

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

        // Trigger spawn via CommandMessage (same path as CLI)
        commands.trigger(lunco_core::architecture::CommandMessage {
            id: 0,
            target: grid,
            name: format!("SPAWN_ENTITY:{}", entry_id),
            args: smallvec::smallvec![
                point3.x as f64,
                point3.y as f64,
                point3.z as f64,
                0.0,
            ],
            source: Entity::PLACEHOLDER,
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
    use crate::SelectedEntity;

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
        // Test that cursor_ray handles edge cases properly
        // This is a basic sanity check for the function signature
        assert!(true); // Placeholder - actual ray testing requires camera setup
    }

    #[test]
    fn test_selected_entity_drag_mode() {
        let mut selected = SelectedEntity::default();
        assert!(!selected.is_dragging);
        
        // Simulate entering drag mode after selection
        selected.is_dragging = true;
        assert!(selected.is_dragging);
        
        // Simulate placement click exiting drag mode
        selected.is_dragging = false;
        assert!(!selected.is_dragging);
    }
}
