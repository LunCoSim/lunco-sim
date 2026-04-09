//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` to provide translate/rotate gizmos on
//! the selected entity.

use bevy::prelude::*;
use transform_gizmo_bevy::{GizmoMode, GizmoOptions, GizmoOrientation, GizmoTarget};
use lunco_avatar::{FreeFlightCamera, OrbitCamera, SpringArmCamera};

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
            let modes = GizmoMode::all();
            modes.iter().find(|m| {
                matches!(m, 
                    GizmoMode::TranslateX | GizmoMode::TranslateY | 
                    GizmoMode::TranslateZ | GizmoMode::TranslateView)
            })
        }
        ToolMode::Rotate => {
            let modes = GizmoMode::all();
            modes.iter().find(|m| {
                matches!(m, 
                    GizmoMode::RotateX | GizmoMode::RotateY | 
                    GizmoMode::RotateZ | GizmoMode::RotateView)
            })
        }
        _ => None,
    };
}

/// Adds GizmoTarget to the selected entity.
/// Skips avatar entities (those with FreeFlightCamera, OrbitCamera, SpringArmCamera)
/// to prevent gizmo interference with camera controls.
pub fn sync_gizmo_target(
    selected: Res<SelectedEntity>,
    q_avatars: Query<(), Or<(
        With<FreeFlightCamera>,
        With<OrbitCamera>,
        With<SpringArmCamera>,
    )>>,
    mut commands: Commands,
) {
    let Some(entity) = selected.entity else { return };

    // Don't add gizmo to avatar entities
    if q_avatars.get(entity).is_ok() { return; }

    let mode_supports_gizmo = matches!(selected.mode, ToolMode::Translate | ToolMode::Rotate);

    if mode_supports_gizmo {
        commands.entity(entity).insert(GizmoTarget::default());
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
