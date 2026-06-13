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
use crate::commands::SelectEntity;

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

/// Handles entity selection on Left-click.
///
/// - **Plain Left-click** selects the entity under the cursor (highlight only,
///   no gizmo). Camera-side observers (`avatar_raycast_possession`) run on
///   the same click to dispatch `PossessVessel`/`FollowTarget`.
/// - **Alt+Left-click** selects *and* attaches a `GizmoTarget` for transform
///   editing. `DragModeActive` is set so the camera-click handler stays out
///   of the way until the gizmo is dismissed (Escape).
/// - **Escape** clears the selection and gizmo.
///
/// Hits walk up to the nearest `SelectableRoot`; ground/wheels/invisible
/// colliders are ignored.
pub fn handle_entity_selection(
    mut selected: ResMut<SelectedEntity>,
    spawn_state: Res<SpawnState>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    raycaster: SpatialQuery,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
    q_parents: Query<&ChildOf>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut drag_mode: ResMut<lunco_core::DragModeActive>,
    mut commands: Commands,
) {
    // Selection state (the `Selected` highlight + `SelectedEntity` resource) is
    // owned by the `SelectEntity` command/observer — the single mutation path
    // the API and Explorer also use. This handler just turns a viewport click
    // into that command (id 0 = deselect). It still owns the *gizmo* concern
    // (alt-click attaches a `GizmoTarget`), which is layered on top of
    // selection. Entities missing an API id (rare — most selectables are
    // registry-registered) fall back to a direct mutation so they stay
    // selectable.
    let select = |commands: &mut Commands,
                  selected: &mut SelectedEntity,
                  old: &Query<Entity, With<Selected>>,
                  entity: Option<Entity>| {
        match entity.and_then(|e| registry.api_id_for(e).map(|id| (e, id))) {
            Some((_, id)) => commands.trigger(SelectEntity { entity_id: id.get() }),
            None => {
                // Fallback: no API id — mutate directly (mirrors the observer).
                for e in old.iter() {
                    commands.entity(e).remove::<Selected>().remove::<GizmoTarget>();
                }
                match entity {
                    Some(e) => {
                        commands.entity(e).insert(Selected);
                        selected.entity = Some(e);
                    }
                    None => selected.entity = None,
                }
            }
        }
    };
    // Skip if in spawn mode
    if !matches!(spawn_state.as_ref(), SpawnState::Idle) { return; }

    // Escape deselects
    if keys.just_pressed(KeyCode::Escape) {
        select(&mut commands, &mut selected, &q_selected_old, None);
        drag_mode.active = false;
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) { return; }
    let alt_held = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    // While a gizmo is up (drag_mode.active), the user is interacting with
    // the transform gizmo widget — its handles aren't physical colliders, so
    // raycasts will miss. Without this gate, every click on a gizmo handle
    // would fall through to the "miss → deselect" path below and tear down
    // the gizmo before the gizmo library could process the input. Gate: if
    // the gizmo is up, only re-process the click when it lands on a fresh
    // `SelectableRoot` collider (i.e. the user is reselecting). Otherwise
    // pass the click through to the gizmo by returning early.
    let gizmo_up = drag_mode.active;

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
        // While gizmo is up, a missed raycast almost certainly means the
        // click was on a gizmo handle — leave selection alone and let the
        // gizmo library handle the click.
        if gizmo_up { return; }
        // Otherwise truly missed everything → deselect.
        select(&mut commands, &mut selected, &q_selected_old, None);
        drag_mode.active = false;
        return;
    };

    // Find the best selectable entity from the hit (walks up parent chain)
    let target = find_selectable(hit_data.entity, &q_selectable, &q_parents);

    let Some(entity) = target else {
        // Same logic as the no-hit case — preserve selection if a gizmo is up.
        if gizmo_up { return; }
        select(&mut commands, &mut selected, &q_selected_old, None);
        drag_mode.active = false;
        return;
    };

    // Route selection through the command (clears old + Selected +
    // SelectedEntity). Alt-click additionally attaches the transform gizmo and
    // enters drag mode (which blocks the camera click handler so dragging a
    // gizmo handle doesn't flip possession).
    select(&mut commands, &mut selected, &q_selected_old, Some(entity));
    if alt_held {
        commands.entity(entity).insert(GizmoTarget::default());
        drag_mode.active = true;
    } else {
        drag_mode.active = false;
    }
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
