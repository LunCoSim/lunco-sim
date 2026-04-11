//! Entity selection via Shift+Left-click.
//!
//! Uses Shift+Left-click to avoid conflict with regular left-click camera possession.
//! Selects the entity closest to the camera under the cursor and immediately
//! attaches a transform gizmo for manipulation.

use bevy::prelude::*;
use transform_gizmo_bevy::GizmoTarget;
use avian3d::prelude::*;
use avian3d::spatial_query::SpatialQueryFilter;

use crate::{SpawnState, SelectedEntity};

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
/// Walks up the parent chain and returns the first entity that has
/// the `SelectableRoot` marker component.
///
/// This uses ECS markers instead of name string matching, which is more robust
/// and compile-time safe.
fn find_selectable(
    mut entity: Entity,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut depth = 0;
    const MAX_DEPTH: usize = 5;

    loop {
        // Check if this entity is marked as selectable
        if q_selectable.get(entity).is_ok() {
            return Some(entity);
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
/// Shift+Left-click selects the entity closest to the camera under the cursor
/// and immediately enables the transform gizmo.
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
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
    q_parents: Query<&ChildOf>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut drag_mode: ResMut<lunco_core::DragModeActive>,
    mut commands: Commands,
) {
    // Skip if in spawn mode
    if !matches!(spawn_state.as_ref(), SpawnState::Idle) { return; }

    // Escape deselects
    if keys.just_pressed(KeyCode::Escape) {
        for old in q_selected_old.iter() {
            commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
        }
        selected.entity = None;
        drag_mode.active = false;
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

    // Build exclusion filter: ground entities are excluded from raycast hits.
    // All other selectable entities (ramps, rover bodies, solar panels, props) pass through.
    let exclude: Vec<Entity> = q_ground.iter().collect();
    let filter = SpatialQueryFilter::default().with_excluded_entities(exclude);

    // Get the closest hit among selectable entities
    let hit = raycaster.cast_ray(origin.into(), direction, 1000.0, false, &filter);

    let Some(hit_data) = hit else {
        // Missed everything — deselect
        for old in q_selected_old.iter() {
            commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
        }
        selected.entity = None;
        drag_mode.active = false;
        return;
    };

    // Find the best selectable entity from the hit (walks up parent chain)
    let target = find_selectable(hit_data.entity, &q_selectable, &q_parents);

    let Some(entity) = target else {
        // No valid selectable target — deselect
        for old in q_selected_old.iter() {
            commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
        }
        selected.entity = None;
        drag_mode.active = false;
        return;
    };

    // Clear old selection
    for old in q_selected_old.iter() {
        commands.entity(old).remove::<Selected>().remove::<GizmoTarget>();
    }

    // Select the target entity and enable gizmo immediately
    commands.entity(entity)
        .insert(Selected)
        .insert(GizmoTarget::default());
    selected.entity = Some(entity);
    drag_mode.active = true;
    info!("Selected entity {:?} - gizmo enabled", entity);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selected_entity_default() {
        let selected = SelectedEntity::default();
        assert!(selected.entity.is_none());
    }
}
