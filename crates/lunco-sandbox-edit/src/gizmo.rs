//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` to provide translate/rotate gizmos on
//! the selected entity. Gizmo movement is applied via **direct transform changes**
//! while the body is made kinematic (no gravity, but collisions still work).
//!
//! ## Gizmo Coordinate System
//! The gizmo reports translation as a **delta** from the entity's position when
//! the gizmo drag started. `GizmoStartPos` is captured at the moment the user
//! begins dragging a gizmo handle, ensuring it reflects the entity's actual
//! current position (which may have changed due to physics since selection).
//!
//! ## Physics Integration
//! During gizmo drag:
//! 1. The body is made **kinematic** (not affected by gravity/forces)
//! 2. Direct transform changes position the entity precisely
//! 3. Collisions still work - kinematic bodies push dynamic bodies
//! 4. When gizmo drag ends, body is restored to **dynamic** for normal physics

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::{LinearVelocity, RigidBody};
use transform_gizmo_bevy::{GizmoCamera, GizmoMode, GizmoOptions, GizmoOrientation, GizmoTarget};

use crate::{SelectedEntity, ToolMode};

/// Tracks the entity's position when the gizmo drag started.
/// Used to calculate correct world position from gizmo's delta translation.
///
/// This is captured at the moment the user begins dragging a gizmo handle,
/// NOT when the entity is selected. This ensures the start position reflects
/// the entity's actual current position (which may have moved due to physics).
#[derive(Component)]
pub struct GizmoStartPos {
    /// Entity's translation when gizmo drag started.
    pub pos: Vec3,
}

/// Marker component indicating the entity was made kinematic by the gizmo system.
/// Used to detect when to restore the body to dynamic after gizmo drag ends.
#[derive(Component)]
pub struct GizmoKinematic;

/// Tracks the previous position of a kinematic gizmo body for velocity computation.
/// Velocity is needed for proper collision detection during kinematic movement.
#[derive(Component)]
pub struct GizmoPrevPos {
    /// Position in the previous frame.
    pub pos: Vec3,
}

/// Makes the selected entity kinematic when gizmo drag actually starts.
///
/// This runs in `Last` schedule (after transform-gizmo-bevy's update_gizmos)
/// so it sees the correct `is_active()` state for the current frame.
/// Only triggers when the gizmo is actively being dragged (not just hovered).
///
/// IMPORTANT: Skipped when the entity is possessed (has a ControllerLink),
/// because possession requires dynamic physics for proper vehicle control.
pub fn capture_gizmo_start_pos(
    selected: Res<SelectedEntity>,
    gizmo_targets: Query<&GizmoTarget>,
    q_transforms: Query<&Transform>,
    mut q_gizmo_starts: Query<&mut GizmoStartPos>,
    q_was_kinematic: Query<&GizmoKinematic>,
    q_controller_links: Query<&lunco_controller::ControllerLink>,
    mut commands: Commands,
) {
    // Skip while physics drag is active
    if selected.is_dragging { return; }

    let Some(entity) = selected.entity else { return; };

    // Skip if entity is possessed - gizmo would break physics control
    if q_controller_links.iter().any(|link| link.vessel_entity == entity) { return; }

    // Only trigger when gizmo is actively being dragged (not just hovered/focused)
    let Ok(gizmo_target) = gizmo_targets.get(entity) else { return; };
    if !gizmo_target.is_active() { return; }

    // Already captured - skip
    if q_was_kinematic.get(entity).is_ok() { return; }

    // Capture the entity's current actual position
    let Ok(tf) = q_transforms.get(entity) else { return; };

    // Insert GizmoStartPos if not present
    if let Ok(mut start_pos) = q_gizmo_starts.get_mut(entity) {
        start_pos.pos = tf.translation;
    }

    // Make body kinematic during gizmo drag (allows direct transform control, collisions still work)
    commands.entity(entity)
        .insert(RigidBody::Kinematic)
        .insert(GizmoKinematic)
        .insert(GizmoPrevPos { pos: tf.translation });
}

/// Restores gizmo-kinematic bodies to dynamic when gizmo drag ends.
///
/// This system runs in `Last` after `capture_gizmo_start_pos`.
/// It detects when a gizmo drag ends (was active, now not active) and restores
/// the body to dynamic so physics (gravity) takes over.
///
/// IMPORTANT: Skipped when the entity is possessed (has a ControllerLink).
pub fn restore_gizmo_dynamic(
    selected: Res<SelectedEntity>,
    gizmo_targets: Query<&GizmoTarget>,
    q_was_kinematic: Query<(Entity, &GizmoKinematic, &GizmoPrevPos)>,
    q_controller_links: Query<&lunco_controller::ControllerLink>,
    mut commands: Commands,
) {
    // Skip while physics drag is active
    if selected.is_dragging { return; }

    let Some(entity) = selected.entity else { return; };

    // Skip if entity is possessed - don't interfere with possession
    if q_controller_links.iter().any(|link| link.vessel_entity == entity) { return; }

    // Only process entities we made kinematic
    if q_was_kinematic.get(entity).is_err() { return; }

    // Check if gizmo is no longer active (drag ended)
    let Ok(gizmo_target) = gizmo_targets.get(entity) else { return; };
    if gizmo_target.is_active() { return; } // Still dragging gizmo

    // Gizmo drag ended - restore dynamic body
    commands.entity(entity)
        .insert(RigidBody::Dynamic)
        .remove::<GizmoKinematic>()
        .remove::<GizmoPrevPos>();
}

/// Applies gizmo drag results directly to entity transforms.
///
/// This system runs in `Last`. During gizmo drag, the entity is kinematic
/// so direct transform changes work without fighting gravity. Velocity is
/// computed from position change for proper collision detection.
///
/// IMPORTANT: Skipped when `SelectedEntity.is_dragging` is true (physics drag takes priority).
/// IMPORTANT: Skipped when the entity is possessed (has a ControllerLink) to avoid
/// conflicting with vehicle control physics.
///
/// ## Coordinate System
/// The gizmo reports translation as a **delta** from the starting position.
/// `GizmoStartPos` is captured by `capture_gizmo_start_pos` which runs before this system.
pub fn apply_gizmo_results(
    selected: Res<SelectedEntity>,
    gizmo_targets: Query<&GizmoTarget>,
    q_gizmo_starts: Query<&GizmoStartPos>,
    mut q_transforms: Query<&mut Transform>,
    mut q_lin_vel: Query<&mut LinearVelocity>,
    q_controller_links: Query<&lunco_controller::ControllerLink>,
    time: Res<Time>,
    mut commands: Commands,
) {
    // Skip gizmo transforms while dragging - physics handles movement
    if selected.is_dragging { return; }

    let Some(entity) = selected.entity else { return; };

    // Skip if entity is possessed - gizmo would break physics control
    if q_controller_links.iter().any(|link| link.vessel_entity == entity) { return; }

    let Ok(gizmo_target) = gizmo_targets.get(entity) else { return; };

    // Skip if gizmo is not active (no drag in progress)
    if !gizmo_target.is_active() { return; }

    let Ok(mut tf) = q_transforms.get_mut(entity) else { return; };

    // Get or capture start position (might not exist yet if first frame of drag)
    let start_pos = if let Ok(start) = q_gizmo_starts.get(entity) {
        start.pos
    } else {
        // First frame of gizmo drag - capture start position
        let pos = tf.translation;
        commands.entity(entity).insert(GizmoStartPos { pos });
        pos
    };

    if let Some(result) = gizmo_target.latest_result() {
        match result {
            transform_gizmo_bevy::GizmoResult::Translation { total, .. } => {
                // Apply delta to original position: new_pos = start_pos + total
                let delta = Vec3::new(total.x as f32, total.y as f32, total.z as f32);
                // Only apply if there's actual movement (prevents jitter at origin)
                if delta.length_squared() > 0.0001 {
                    let old_pos = tf.translation;
                    tf.translation = start_pos + delta;

                    // Compute velocity from position change for collision detection
                    let dt = time.delta_secs().max(0.0001);
                    let vel = Vec3::new(
                        (tf.translation.x - old_pos.x) / dt,
                        (tf.translation.y - old_pos.y) / dt,
                        (tf.translation.z - old_pos.z) / dt,
                    );
                    if let Ok(mut lin_vel) = q_lin_vel.get_mut(entity) {
                        lin_vel.0 = DVec3::new(vel.x as f64, vel.y as f64, vel.z as f64);
                    }
                }
            }
            transform_gizmo_bevy::GizmoResult::Rotation { axis, total, .. } => {
                let axis = Vec3::new(axis.x as f32, axis.y as f32, axis.z as f32);
                tf.rotation = Quat::from_axis_angle(axis, total as f32);
            }
            transform_gizmo_bevy::GizmoResult::Scale { total } => {
                tf.scale = Vec3::new(total.x as f32, total.y as f32, total.z as f32);
            }
            transform_gizmo_bevy::GizmoResult::Arcball { total, .. } => {
                let q = bevy::math::DQuat::from_xyzw(total.v.x, total.v.y, total.v.z, total.s);
                tf.rotation = Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32);
            }
        }
    }

    // Update GizmoPrevPos for next frame's velocity computation
    commands.entity(entity).insert(GizmoPrevPos { pos: tf.translation });
}

/// Updates gizmo configuration based on current tool mode.
pub fn sync_gizmo_mode(
    selected: Res<SelectedEntity>,
    mut gizmo_options: ResMut<GizmoOptions>,
) {
    gizmo_options.gizmo_orientation = GizmoOrientation::Global;

    // Use mode_override to force a specific gizmo mode
    gizmo_options.mode_override = match selected.mode {
        ToolMode::Translate => {
            GizmoMode::all().iter().find(|m| {
                matches!(m,
                    GizmoMode::TranslateX | GizmoMode::TranslateY |
                    GizmoMode::TranslateZ | GizmoMode::TranslateView)
            })
        }
        ToolMode::Rotate => {
            GizmoMode::all().iter().find(|m| {
                matches!(m,
                    GizmoMode::RotateX | GizmoMode::RotateY |
                    GizmoMode::RotateZ | GizmoMode::RotateView)
            })
        }
        _ => None,
    };
}

/// Ensures the camera has GizmoCamera marker.
/// Does NOT add GizmoTarget to anything - that's handled by selection.
pub fn sync_gizmo_camera(
    q_cameras: Query<Entity, (With<Camera3d>, Without<GizmoCamera>)>,
    mut commands: Commands,
) {
    for camera in q_cameras.iter() {
        commands.entity(camera).insert(GizmoCamera);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SelectedEntity, ToolMode};

    #[test]
    fn test_gizmo_options_default() {
        let options = GizmoOptions::default();
        assert!(!options.gizmo_modes.is_empty());
    }

    #[test]
    fn test_tool_mode_values() {
        assert_eq!(ToolMode::Translate, ToolMode::Translate);
        assert_eq!(ToolMode::Rotate, ToolMode::Rotate);
        assert_ne!(ToolMode::Select, ToolMode::Translate);
    }
}
