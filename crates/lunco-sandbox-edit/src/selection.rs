//! Entity selection via Shift+Left-click.
//!
//! Uses Shift+Left-click to avoid conflict with regular left-click camera possession.
//! Selects the entity closest to the camera under the cursor.

use bevy::prelude::*;
use transform_gizmo_bevy::GizmoTarget;
use avian3d::prelude::*;
use avian3d::spatial_query::SpatialQueryFilter;
use lunco_core::Avatar;

use crate::{SpawnState, SelectedEntity, ToolMode};

/// Component marking an entity as currently selected.
#[derive(Component)]
pub struct Selected;

/// Computes a world-space ray from the camera through the cursor position.
fn cursor_ray(
    camera: &Camera,
    cam_tf: &GlobalTransform,
    cursor: Vec2,
) -> Option<(Vec3, Dir3)> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    Some((ray.origin, ray.direction))
}

/// Finds the most appropriate entity to select from a hit entity.
/// Walks up the parent chain and returns the first entity that:
/// - Has a Name that identifies it as a top-level object (rover body, panel, prop)
/// - Is NOT a wheel, collider, visual, ghost, ground, or child component
/// - Stops at the Grid or BigSpace root
fn find_selectable(
    mut entity: Entity,
    q_names: &Query<(Entity, &Name)>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut depth = 0;
    const MAX_DEPTH: usize = 5;

    loop {
        if let Ok((_, name)) = q_names.get(entity) {
            let name_str = name.as_str();
            // Only select top-level objects: rover bodies, ramps, solar panels, props
            // Reject wheels, colliders, visuals, ghosts, ground, and child components
            let is_selectable = !name_str.contains("Wheel")
                && !name_str.contains("Collider")
                && !name_str.contains("Visual")
                && !name_str.contains("Ghost")
                && name_str != "Ground";
            if is_selectable {
                return Some(entity);
            }
        }

        // Walk up one parent level
        if let Ok(parent) = q_parents.get(entity) {
            entity = parent.parent();
        } else {
            break;
        }

        depth += 1;
        if depth >= MAX_DEPTH {
            break;
        }
    }

    None
}

/// Handles entity selection via Shift+Left-click.
///
/// Regular Left-click is reserved for camera possession.
/// Shift+Left-click selects the entity closest to the camera under the cursor.
/// Only hits selectable entities (rover bodies, props, panels) — ignores ground,
/// wheels, and invisible colliders.
pub fn handle_entity_selection(
    mut selected: ResMut<SelectedEntity>,
    spawn_state: Res<SpawnState>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    raycaster: SpatialQuery,
    q_names: Query<(Entity, &Name)>,
    q_parents: Query<&ChildOf>,
    q_selectable: Query<(Entity, &Name, &GlobalTransform), Without<Avatar>>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut commands: Commands,
) {
    // Skip if in spawn mode
    if !matches!(spawn_state.as_ref(), SpawnState::Idle) { return; }

    // Escape exits transform mode and deselects
    if keys.just_pressed(KeyCode::Escape) {
        for old in q_selected_old.iter() {
            commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
        }
        selected.entity = None;
        selected.mode = ToolMode::Select;
        selected.is_picking_up = false;
        return;
    }

    // Use Shift+Left-click for selection (regular Left-click is for camera possession)
    if !mouse.just_pressed(MouseButton::Left) { return; }
    if !keys.pressed(KeyCode::ShiftLeft) && !keys.pressed(KeyCode::ShiftRight) { return; }

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

    // Build exclusion filter: ground, wheels, and other non-selectable colliders.
    // Ramps, rover bodies, solar panels, and props are all selectable.
    let exclude: Vec<Entity> = q_names.iter()
        .filter_map(|(e, name)| {
            let n = name.as_str();
            if n == "Ground" || n.contains("Wheel") || n.contains("Ghost") {
                Some(e)
            } else {
                None
            }
        })
        .collect();
    let filter = SpatialQueryFilter::default().with_excluded_entities(exclude);

    // Get the closest hit among selectable entities
    let hit = raycaster.cast_ray(origin.into(), direction, 1000.0, false, &filter);

    let Some(hit_data) = hit else {
        // Missed everything — deselect
        for old in q_selected_old.iter() {
            commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
        }
        selected.entity = None;
        selected.mode = ToolMode::Select;
        return;
    };

    // Find the best selectable entity from the hit (walks up parent chain)
    let target = find_selectable(hit_data.entity, &q_names, &q_parents);

    let Some(entity) = target else {
        // No valid selectable target — deselect
        for old in q_selected_old.iter() {
            commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
        }
        selected.entity = None;
        selected.mode = ToolMode::Select;
        return;
    };

    // Verify the target is in the selectable query
    let Ok((_, name, _)) = q_selectable.get(entity) else {
        // Not selectable — deselect
        for old in q_selected_old.iter() {
            commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
        }
        selected.entity = None;
        selected.mode = ToolMode::Select;
        return;
    };

    // Clear old selection
    for old in q_selected_old.iter() {
        commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
    }

    // Select the target entity
    commands.entity(entity).insert((Selected, GizmoTarget::default()));
    selected.entity = Some(entity);
    let name_str = name.as_str();
    info!("Selected entity {:?} ({})", entity, name_str);

    // Auto-switch to translate mode
    if selected.mode == ToolMode::Select {
        selected.mode = ToolMode::Translate;
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpawnState;

    #[test]
    fn test_selected_entity_default() {
        let selected = SelectedEntity::default();
        assert!(selected.entity.is_none());
        assert_eq!(selected.mode, ToolMode::Select);
    }
}
