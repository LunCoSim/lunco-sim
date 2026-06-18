//! Entity selection via Shift+Left-click.
//!
//! Uses Shift+Left-click to avoid conflict with regular left-click camera possession.
//! Selects the entity closest to the camera under the cursor and immediately
//! attaches a transform gizmo for manipulation.

use bevy::prelude::*;
use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use transform_gizmo_bevy::GizmoTarget;

use crate::{SpawnState, SelectedEntity};
use crate::commands::SelectEntity;

/// Component marking an entity as currently selected.
#[derive(Component)]
pub struct Selected;

/// Finds the most appropriate entity to select from a hit entity.
///
/// Walks up the parent chain (up to `MAX_DEPTH`, matching the avatar
/// possession resolver) and returns the nearest ancestor carrying the
/// `SelectableRoot` marker — so clicking a rover wheel selects the rover root,
/// not the wheel mesh.
///
/// If no `SelectableRoot` exists in the chain, it falls back to the clicked
/// entity itself, so ground, terrain and plain USD visual props (decorative
/// cubes, ramps, the Perseverance placeholder) are all selectable — clicking
/// any one of them switches the Inspector to it. (Earlier this returned `None`
/// for un-tagged hits, which made those objects unselectable and left the
/// Inspector "stuck" on the previous selection. That fallback was safe to add
/// only once selection ray-cast from the correct camera — see the camera note
/// on `handle_entity_selection`.)
fn find_selectable(
    hit: Entity,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    const MAX_DEPTH: usize = 8;
    let mut entity = hit;
    let mut depth = 0;

    loop {
        // A `SelectableRoot` ancestor wins — clicking a rover wheel selects the
        // rover root, not the wheel mesh.
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

    // No `SelectableRoot` in the chain (ground, terrain, a plain prop) — select
    // the clicked entity itself so it's still editable.
    Some(hit)
}

/// Selects the entity under the pointer, driven by **bevy_picking**.
///
/// Registered as a global `On<Pointer<Click>>` observer. bevy_picking (with
/// bevy_egui's picking backend, enabled by default) resolves panel-vs-scene
/// occlusion for us: when the pointer is over any egui chrome, egui's backend
/// wins the pick and this fires with the egui-context entity — which carries no
/// world-space `hit.position` (egui emits `HitData` with `position: None`),
/// whereas a real 3D mesh hit always has one. So the `position.is_none()` guard
/// rejects every chrome click with no hand-rolled gate, no `ScenePointer`, no
/// manual ray-cast, and no cross-schedule staleness.
///
/// - **Plain click** selects the entity under the cursor (highlight only).
///   Possession (`on_scene_click_possess`) observes the same click to dispatch
///   `PossessVessel`/`FollowTarget`.
/// - **Alt+click** selects *and* attaches a `GizmoTarget`; sets `DragModeActive`
///   so the camera-click handler stays out of the way until Escape.
/// - Re-clicking the already-selected component on a sub-part DRILLS the
///   Inspector to that part.
///
/// Deselect is explicit (Escape/Backspace via [`handle_deselect_keys`], the
/// Explorer, or selecting another entity) — a click on empty space or a panel
/// never clears the selection.
pub fn on_scene_click_select(
    mut click: On<Pointer<Click>>,
    spawn_state: Res<SpawnState>,
    keys: Res<ButtonInput<KeyCode>>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut selected: ResMut<SelectedEntity>,
    mut drag_mode: ResMut<lunco_core::DragModeActive>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    mut commands: Commands,
) {
    // `Pointer<Click>` auto-propagates leaf→parent→…→window; a global observer
    // would otherwise fire at every ancestor and select the wrong (top) one. We
    // resolve the `SelectableRoot` ourselves via `find_selectable`, so stop the
    // bubble at the picked leaf — this runs target-first, so we're at the leaf.
    click.propagate(false);
    // Left button only.
    if click.button != PointerButton::Primary {
        return;
    }
    // Chrome guard — see the doc comment: egui's pick has no world position.
    if click.hit.position.is_none() {
        return;
    }
    // Spawn tool armed: clicks place objects, not select.
    if !matches!(spawn_state.as_ref(), SpawnState::Idle) {
        return;
    }

    // Selection state (the `Selected` highlight + `SelectedEntity` resource) is
    // owned by the `SelectEntity` command/observer — the single mutation path
    // the API and Explorer also use. Entities missing an API id fall back to a
    // direct mutation so they stay selectable.
    let select = |commands: &mut Commands,
                  selected: &mut SelectedEntity,
                  old: &Query<Entity, With<Selected>>,
                  entity: Option<Entity>| {
        match entity.and_then(|e| registry.api_id_for(e).map(|id| (e, id))) {
            Some((_, id)) => commands.trigger(SelectEntity { entity_id: id.get() }),
            None => {
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

    let hit_entity = click.entity;
    let prev_selected = selected.entity;
    let alt_held = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    // Resolve the picked mesh to its selectable (nearest `SelectableRoot`
    // ancestor, or the hit entity itself for ground/props).
    let Some(entity) = find_selectable(hit_entity, &q_selectable, &q_parents) else {
        return;
    };

    // DRILL: a plain click on the ALREADY-selected component, landing on one of
    // its sub-parts, aims the Inspector at that part instead of re-selecting.
    if !alt_held && prev_selected == Some(entity) && hit_entity != entity {
        inspector_target.part = Some(hit_entity);
        drag_mode.active = false;
        return;
    }

    select(&mut commands, &mut selected, &q_selected_old, Some(entity));
    inspector_target.part = None;
    if alt_held {
        commands.entity(entity).insert(GizmoTarget::default());
        drag_mode.active = true;
    } else {
        drag_mode.active = false;
    }
}

/// Escape / Backspace clears the selection and gizmo. Split out of the click
/// path because it's keyboard-driven, not a pointer pick. When an Inspector
/// field is focused bevy_egui absorbs the key (correct); otherwise it reaches
/// here and deselects through the same `SelectEntity` mutation path.
pub fn handle_deselect_keys(
    keys: Res<ButtonInput<KeyCode>>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut selected: ResMut<SelectedEntity>,
    mut drag_mode: ResMut<lunco_core::DragModeActive>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    mut commands: Commands,
) {
    if !(keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace)) {
        return;
    }
    for e in q_selected_old.iter() {
        commands.entity(e).remove::<Selected>().remove::<GizmoTarget>();
    }
    selected.entity = None;
    inspector_target.part = None;
    drag_mode.active = false;
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
