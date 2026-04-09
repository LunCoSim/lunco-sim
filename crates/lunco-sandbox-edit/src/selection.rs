//! Entity selection via mouse pick.

use bevy::prelude::*;
use bevy::math::DVec3;
use transform_gizmo_bevy::GizmoTarget;
use avian3d::prelude::*;

use crate::{SpawnState, SelectedEntity, ToolMode};

/// Computes a world-space ray from the camera through the cursor position.
fn cursor_ray(
    camera: &Camera,
    cam_tf: &GlobalTransform,
    cursor: Vec2,
) -> Option<(DVec3, Dir3)> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    Some((ray.origin.as_dvec3(), ray.direction))
}

/// Handles entity selection via mouse click.
pub fn handle_entity_selection(
    mut selected: ResMut<SelectedEntity>,
    spawn_state: Res<SpawnState>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    raycaster: SpatialQuery,
    q_names: Query<(Entity, &Name, &GlobalTransform)>,
    mut commands: Commands,
) {
    if !matches!(spawn_state.as_ref(), SpawnState::Idle) { return; }
    if !mouse.just_pressed(MouseButton::Left) { return; }

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
        let hit_point = origin + direction.as_dvec3() * hit_data.distance;
        let mut best_entity = None;
        let mut best_dist = f64::MAX;

        for (entity, _, gtf) in q_names.iter() {
            let pos = gtf.translation();
            let dist = (pos.as_dvec3() - hit_point).length();
            if dist < best_dist && dist < 2.0 {
                best_dist = dist;
                best_entity = Some(entity);
            }
        }

        if let Some(entity) = best_entity {
            if let Some(old) = selected.entity {
                commands.entity(old).remove::<GizmoTarget>();
            }
            commands.entity(entity).insert(GizmoTarget::default());
            selected.entity = Some(entity);
            if selected.mode == ToolMode::Select {
                selected.mode = ToolMode::Translate;
            }
            return;
        }
    }

    // Clicked empty space — deselect
    if let Some(old) = selected.entity {
        commands.entity(old).remove::<GizmoTarget>();
    }
    selected.entity = None;
    selected.mode = ToolMode::Select;

    // Tool mode hotkeys
    if keys.just_pressed(KeyCode::KeyG) {
        selected.mode = ToolMode::Translate;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        selected.mode = ToolMode::Rotate;
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        selected.mode = ToolMode::Select;
    }
}

/// Keeps the GizmoTarget on the selected entity in sync.
pub fn sync_gizmo_target(
    selected: Res<SelectedEntity>,
    mut commands: Commands,
) {
    let Some(entity) = selected.entity else { return };

    let mode_supports_gizmo = matches!(selected.mode, ToolMode::Translate | ToolMode::Rotate);

    if mode_supports_gizmo {
        commands.entity(entity).insert(GizmoTarget::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpawnState;
    use crate::UndoStack;

    #[test]
    fn test_selected_entity_default() {
        let selected = SelectedEntity::default();
        assert!(selected.entity.is_none());
        assert_eq!(selected.mode, ToolMode::Select);
    }
}
