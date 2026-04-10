//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` to provide translate/rotate gizmos on
//! the selected entity. Applies gizmo drag results back to entity transforms.

use bevy::prelude::*;
use transform_gizmo_bevy::{GizmoCamera, GizmoMode, GizmoOptions, GizmoOrientation, GizmoTarget};

use crate::{SelectedEntity, ToolMode};

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

/// Applies gizmo drag results back to entity transforms.
///
/// This is the critical system that makes gizmo manipulation actually work.
/// When the user drags a gizmo handle, `GizmoTarget.latest_result()` returns
/// the result. This system reads those results and applies them to the entity's Transform.
pub fn apply_gizmo_results(
    selected: Res<SelectedEntity>,
    gizmo_targets: Query<&GizmoTarget>,
    mut q_transforms: Query<&mut Transform>,
) {
    let Some(entity) = selected.entity else { return; };

    if let Ok(gizmo_target) = gizmo_targets.get(entity) {
        if let Some(result) = gizmo_target.latest_result() {
            if let Ok(mut tf) = q_transforms.get_mut(entity) {
                match result {
                    transform_gizmo_bevy::GizmoResult::Translation { total, .. } => {
                        tf.translation = Vec3::new(total.x as f32, total.y as f32, total.z as f32);
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
        }
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
