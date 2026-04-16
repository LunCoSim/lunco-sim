//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` which **automatically applies transforms** to
//! entities with `GizmoTarget`. This module handles:
//! - Making bodies kinematic during gizmo drag
//! - Syncing Avian3D `Position` from `Transform` to prevent writeback overwrite
//! - Updating `GlobalTransform` so the mesh renders at the correct position
//! - Computing velocity for collision detection
//! - Restoring dynamic bodies when drag ends

use bevy::prelude::*;
use avian3d::prelude::RigidBody;
use transform_gizmo_bevy::{GizmoCamera, GizmoTarget};

use crate::SelectedEntity;

/// Tracks the previous position of a kinematic gizmo body for velocity computation.
#[derive(Component)]
pub struct GizmoPrevPos {
    /// Position in the previous frame.
    pub pos: Vec3,
    /// Original RigidBody type before drag started — restored on drag end.
    pub original_body: RigidBody,
}

/// Makes the selected entity kinematic when gizmo drag starts.
///
/// Runs in `Last` schedule, after transform-gizmo-bevy's `update_gizmos`.
/// Only triggers when the gizmo is actively being dragged.
///
/// Skips possessed entities (those with a `ControllerLink` pointing to them).
pub fn capture_gizmo_start(
    selected: Res<SelectedEntity>,
    gizmo_targets: Query<&GizmoTarget>,
    q_transforms: Query<&Transform>,
    q_rigid_bodies: Query<&RigidBody>,
    q_prev_pos: Query<&GizmoPrevPos>,
    q_controller_links: Query<&lunco_controller::ControllerLink>,
    mut commands: Commands,
) {
    let Some(entity) = selected.entity else { return; };

    // Skip if entity is possessed - gizmo would break physics control
    if q_controller_links.iter().any(|link| link.vessel_entity == entity) { return; }

    // Only trigger when gizmo is actively being dragged (not just hovered/focused)
    let Ok(gizmo_target) = gizmo_targets.get(entity) else { return; };
    if !gizmo_target.is_active() { return; }

    // Skip if already tracking this drag
    if q_prev_pos.get(entity).is_ok() { return; }

    // Capture the entity's current position (already updated by transform-gizmo-bevy)
    let Ok(tf) = q_transforms.get(entity) else { return; };

    // Record original body type so we can restore it correctly on drag end.
    let original_body = q_rigid_bodies.get(entity).copied().unwrap_or(RigidBody::Dynamic);

    commands.entity(entity)
        .insert(RigidBody::Kinematic)
        .insert(GizmoPrevPos { pos: tf.translation, original_body });
}

/// Syncs Avian3D `Position` and `GlobalTransform` from `Transform` during gizmo drag.
///
/// **Why GlobalTransform sync is needed:** `global_transform_propagation_system` runs in
/// `PostUpdate` (before the gizmo in `Last`). Without syncing, `GlobalTransform` is stale
/// and the mesh renders at the old position while the gizmo is at the new position.
///
/// **NOTE:** Position sync is intentionally DISABLED because syncing Position causes the
/// gizmo library to double-apply transforms on subsequent frames (it reads the synced
/// Position via Transform and adds the cumulative delta on top).
///
/// Skips possessed entities to avoid conflicting with vehicle control physics.
pub fn sync_gizmo_transforms(
    selected: Res<SelectedEntity>,
    gizmo_targets: Query<&GizmoTarget>,
    q_transforms: Query<&Transform>,
    q_child_of: Query<&ChildOf>,
    q_children: Query<&Children>,
    mut q_global_transforms: Query<&mut GlobalTransform>,
    q_controller_links: Query<&lunco_controller::ControllerLink>,
) {
    let Some(entity) = selected.entity else { return; };

    // Skip if entity is possessed - gizmo would break physics control
    if q_controller_links.iter().any(|link| link.vessel_entity == entity) { return; }

    // Only process active gizmo drags
    let Ok(gizmo_target) = gizmo_targets.get(entity) else { return; };
    if !gizmo_target.is_active() { return; }

    let Ok(tf) = q_transforms.get(entity) else { return; };

    // Compute correct GlobalTransform: parent_GlobalTransform * local_Transform
    let computed_gtf = if let Ok(child_of) = q_child_of.get(entity) {
        if let Ok(parent_gtf) = q_global_transforms.get(child_of.parent()) {
            parent_gtf.mul_transform(*tf)
        } else {
            GlobalTransform::from(*tf)
        }
    } else {
        GlobalTransform::from(*tf)
    };

    // Update GlobalTransform so the mesh renders at the correct position
    if let Ok(mut gtf) = q_global_transforms.get_mut(entity) {
        *gtf = computed_gtf;
    }

    // Propagate GlobalTransform to children recursively
    propagate_global_transform(entity, &computed_gtf, &q_transforms, &q_children, &mut q_global_transforms);
}

/// Recursively propagates GlobalTransform to all descendants of an entity.
fn propagate_global_transform(
    parent: Entity,
    parent_gtf: &GlobalTransform,
    q_transforms: &Query<&Transform>,
    q_children: &Query<&Children>,
    q_global_transforms: &mut Query<&mut GlobalTransform>,
) {
    let Ok(children) = q_children.get(parent) else { return };

    for child in children.iter() {
        // Compute child's GlobalTransform: parent_GTF * child_local_Transform
        if let Ok(child_tf) = q_transforms.get(child) {
            let child_gtf = parent_gtf.mul_transform(*child_tf);
            if let Ok(mut gtf) = q_global_transforms.get_mut(child) {
                *gtf = child_gtf;
            }
            // Recurse to grandchildren using the child's computed GlobalTransform
            propagate_global_transform(child, &child_gtf, q_transforms, q_children, q_global_transforms);
        }
    }
}

/// Restores gizmo-kinematic bodies to their original body type when gizmo drag ends.
///
/// Detects when a gizmo drag ends and restores the body to its original type
/// (Dynamic for physics objects, Kinematic for co-sim balloons, etc.).
///
/// Skips possessed entities to avoid interfering with possession.
pub fn restore_gizmo_dynamic(
    selected: Res<SelectedEntity>,
    gizmo_targets: Query<&GizmoTarget>,
    q_prev_pos: Query<&GizmoPrevPos>,
    q_controller_links: Query<&lunco_controller::ControllerLink>,
    mut commands: Commands,
) {
    let Some(entity) = selected.entity else { return; };

    // Skip if entity is possessed - don't interfere with possession
    if q_controller_links.iter().any(|link| link.vessel_entity == entity) { return; }

    // Only process entities we made kinematic
    let Ok(prev) = q_prev_pos.get(entity) else { return; };

    // Check if gizmo is no longer active (drag ended)
    let Ok(gizmo_target) = gizmo_targets.get(entity) else { return; };
    if gizmo_target.is_active() { return; } // Still dragging gizmo

    // Restore to the original body type and clear tracking data.
    // This preserves Kinematic for co-sim entities (e.g. balloon) instead of
    // unconditionally switching to Dynamic (which would enable Avian integration).
    commands.entity(entity)
        .insert(prev.original_body)
        .remove::<GizmoPrevPos>();
}

/// Ensures the camera has GizmoCamera marker.
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

    #[test]
    fn test_gizmo_prev_pos_component() {
        let pos = GizmoPrevPos { pos: Vec3::new(1.0, 2.0, 3.0), original_body: RigidBody::Dynamic };
        assert_eq!(pos.pos, Vec3::new(1.0, 2.0, 3.0));
    }
}
