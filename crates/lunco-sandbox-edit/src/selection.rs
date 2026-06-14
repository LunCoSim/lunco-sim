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

/// Whether the window-space `cursor` (logical points) is over the live 3D
/// viewport tab rather than a docked chrome panel.
///
/// The viewport is a tab inside egui_dock, which renders every leaf as an egui
/// `Area` — so egui's pointer queries (`is_pointer_over_area`,
/// `wants_pointer_input`) read true over the viewport too and can't separate it
/// from the Inspector. The workbench records the viewport tab's screen rect in
/// [`PanelRects`] (physical px); that's the only usable signal.
///
/// **Fails open**: with no viewport rect recorded yet (a chrome-less
/// full-window perspective, or before the first paint) the whole window counts
/// as the scene — clicks keep working, they just aren't gated.
fn cursor_over_scene(
    panel_rects: &lunco_workbench::PanelRects,
    window: &Window,
    cursor: Vec2,
) -> bool {
    if panel_rects.is_empty() {
        return true;
    }
    panel_rects.any_contains(cursor * window.scale_factor())
}

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
/// A click that hits nothing (empty sky, ground, or a docked panel) leaves
/// the current selection intact — deselect is explicit (Escape / Explorer /
/// picking another entity) so editing an entity in the Inspector isn't undone
/// by a stray viewport click.
///
/// Hits walk up to the nearest `SelectableRoot`; ground/wheels/invisible
/// colliders are ignored.
pub fn handle_entity_selection(
    mut selected: ResMut<SelectedEntity>,
    spawn_state: Res<SpawnState>,
    // Use the AVATAR camera specifically — the same one `avatar_raycast_possession`
    // ray-casts from. The scene has >1 `Camera3d` (the avatar viewport camera plus
    // RTT/preview cameras), so the looser `With<Camera3d>` + `.next()` could grab a
    // different camera, producing a ray from the wrong viewpoint that misses what
    // the user clicked (it hit the ground while possession hit the rover). Matching
    // possession's `With<Avatar>` keeps the two in lockstep.
    cameras: Query<(&Camera, &GlobalTransform), With<lunco_core::Avatar>>,
    windows: Query<&Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    raycaster: SpatialQuery,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut drag_mode: ResMut<lunco_core::DragModeActive>,
    panel_rects: Res<lunco_workbench::PanelRects>,
    mut egui_contexts: bevy_egui::EguiContexts,
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

    // Escape / Backspace deselect (single canonical mutation path). When the
    // user wants to deselect after a viewport click nothing is focused, so
    // Bevy's `ButtonInput` sees the key; if an Inspector field is focused
    // bevy_egui absorbs it (correct — Backspace edits the field, Escape
    // defocuses first, a second Escape then reaches here).
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) {
        select(&mut commands, &mut selected, &q_selected_old, None);
        drag_mode.active = false;
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) { return; }
    let alt_held = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    let (camera, cam_tf) = match cameras.iter().next() {
        Some(c) => c,
        None => return,
    };
    let window = match windows.iter().next() {
        Some(w) => w,
        None => return,
    };
    let Some(cursor) = window.cursor_position() else { return };
    // Only raycast the scene when the click is over the 3D viewport tab, not a
    // docked panel (Inspector/Explorer) — else clicking a panel raycasts
    // through the chrome and could hit-select an entity behind it. egui can't
    // help distinguish them: egui_dock renders every dock leaf (the viewport
    // included) as an egui `Area`, so `is_pointer_over_area`/`wants_pointer_input`
    // are true over the viewport too. The workbench records the viewport tab's
    // rect in `PanelRects` — the only signal that separates it from the chrome.
    if !cursor_over_scene(&panel_rects, window, cursor) { return; }
    // An egui popup (open colour picker, combo dropdown, menu) floats as a
    // foreground `Area` that can extend OVER the viewport rect — a click inside
    // it passes the rect test above, but egui owns that click. Don't raycast:
    // otherwise picking a colour in the Inspector's colour-swatch popup falls
    // through to the 3D scene behind it and re-selects the terrain. (This is the
    // "click panel → terrain selected" bug.) See `pointer_over_egui_popup`.
    if let Ok(ctx) = egui_contexts.ctx_mut() {
        if lunco_workbench::pointer_over_egui_popup(ctx, cursor) {
            return;
        }
    }
    let Some((origin, direction)) = cursor_ray(camera, cam_tf, cursor) else { return };

    // No exclusions: ground/terrain are now selectable too (closest-hit means
    // they're only picked on a real bare-ground click, since any prop/rover in
    // front is hit first). Deselect is explicit — Escape / Explorer.
    let filter = SpatialQueryFilter::default();

    // Get the closest hit along the ray.
    let hit = raycaster.cast_ray(origin.into(), direction, 1000.0, false, &filter);

    let Some(hit_data) = hit else {
        // A viewport click that hits empty sky no longer clears the selection.
        // Deselect-on-empty-click fought the select→edit workflow: every camera
        // nudge or stray click nuked the Inspector's target before the user
        // could reach a slider/combobox. Deselect is explicit only — Escape,
        // the Explorer, or selecting another entity. (Gizmo handles aren't
        // colliders, so dragging one also lands here and is left untouched.)
        return;
    };

    // Resolve the hit to its selectable (nearest `SelectableRoot` ancestor, or
    // the hit entity itself for ground/props). Always `Some`, but keep the
    // Option shape so a future "ignore this hit" rule has a place to live.
    let Some(entity) = find_selectable(hit_data.entity, &q_selectable, &q_parents) else { return; };

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
