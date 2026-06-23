//! Entity selection via Shift+Left-click.
//!
//! Uses Shift+Left-click to avoid conflict with regular left-click camera possession.
//! Selects the entity closest to the camera under the cursor and immediately
//! attaches a transform gizmo for manipulation.

use bevy::prelude::*;
use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use transform_gizmo_bevy::GizmoTarget;

use bevy::camera::primitives::Aabb;
use bevy::math::Isometry3d;
use bevy::math::primitives::Cuboid;

use crate::{SpawnState, SelectedEntities};
use lunco_core::Command;

/// Component marking an entity as currently selected.
#[derive(Component)]
pub struct Selected;

/// Select an entity by API id — the headless/scriptable equivalent of a
/// Shift+Left-click in the viewport. Drives the same [`SelectedEntities`]
/// resource and [`Selected`] highlight the mouse path uses, so the Inspector
/// immediately shows that entity's components (Transform, Physics, Shader
/// Parameters, …). Pass `entity_id == 0` to clear the selection.
///
/// Selection is an editor concept (it targets the Inspector/gizmo), so this
/// command lives in the `ui`-gated selection module — a headless server exposes
/// no selection.
#[Command(default)]
pub struct SelectEntity {
    /// API id from `ListEntities` — `u64` "Pattern B", resolved in the
    /// observer via `ApiEntityRegistry` (same as `FocusEntityById`). `0`
    /// clears the selection.
    pub entity_id: u64,
    /// If true, maintains the previous selection and adds this entity to it (like Shift-click)
    pub extend: bool,
    /// If true, toggles the selection state of the entity (like Cmd/Ctrl-click)
    pub toggle: bool,
}

/// THE single selection-mutation, shared by every selection surface: the
/// viewport-click observer ([`on_scene_click_select`]), the `SelectEntity` API
/// command ([`on_select_entity`]), and the Explorer list (`ui::entity_list`).
///
/// Keyed by `Entity`, **never** by api_id — multiple instances of one USD asset
/// can share an api_id, so resolving id→entity returns the wrong instance.
/// Highlights with `Selected` + a `GizmoTarget` (so the transform gizmo can move
/// the object) and maintains [`SelectedEntities`].
///
/// It deliberately does **not** touch [`lunco_core::DragModeActive`]: selecting
/// only highlights and never blocks camera possession (plain-click). Possession
/// is suppressed only while a gizmo handle is *actively dragged*, driven from
/// `GizmoTarget::is_active()` in `gizmo::sync_gizmo_dragging_marker`.
///
/// - `!extend && !toggle` → replace the selection with `target`.
/// - `toggle` and `target` already selected → remove it.
/// - otherwise → add `target`.
pub(crate) fn apply_selection(
    commands: &mut Commands,
    selected: &mut SelectedEntities,
    old_selected: impl IntoIterator<Item = Entity>,
    target: Entity,
    extend: bool,
    toggle: bool,
) {
    if !extend && !toggle {
        for e in old_selected {
            if e != target {
                commands.entity(e).remove::<Selected>().remove::<GizmoTarget>();
            }
        }
        selected.entities.clear();
    }

    if toggle && selected.entities.contains(&target) {
        commands.entity(target).remove::<Selected>().remove::<GizmoTarget>();
        selected.entities.retain(|e| *e != target);
    } else {
        commands.entity(target).insert((Selected, GizmoTarget::default()));
        if !selected.entities.contains(&target) {
            selected.entities.push(target);
        }
    }
}

/// Clears the whole selection (highlight + gizmo + resource). Shared by the
/// id-0 `SelectEntity` and the Escape/Backspace path.
pub(crate) fn clear_selection(
    commands: &mut Commands,
    selected: &mut SelectedEntities,
    old_selected: impl IntoIterator<Item = Entity>,
) {
    for e in old_selected {
        commands.entity(e).remove::<Selected>().remove::<GizmoTarget>();
    }
    selected.entities.clear();
}

/// Observer for [`SelectEntity`]: resolves the api_id and routes through the
/// shared [`apply_selection`] (or [`clear_selection`] on id 0).
pub fn on_select_entity(
    trigger: On<SelectEntity>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut selected: ResMut<SelectedEntities>,
    q_old: Query<Entity, With<Selected>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    if cmd.entity_id == 0 {
        clear_selection(&mut commands, &mut selected, q_old.iter());
        info!("SELECT_ENTITY: cleared selection");
        return;
    }

    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("SELECT_ENTITY: no api_id={} in registry", cmd.entity_id);
        if !cmd.extend && !cmd.toggle {
            clear_selection(&mut commands, &mut selected, q_old.iter());
        }
        return;
    };

    apply_selection(&mut commands, &mut selected, q_old.iter(), target, cmd.extend, cmd.toggle);
    info!("SELECT_ENTITY: selected api_id={} ({target:?})", cmd.entity_id);
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
/// - **Shift+click** toggles the entity under the cursor in the multi-selection
///   and attaches a `GizmoTarget`. This is the *only* path that selects — a plain
///   (un-modified) click is owned by the avatar's possess/follow/focus observer
///   (`avatar_raycast_possession`). Selecting only highlights; possession stays
///   available (a gizmo handle drag, not mere selection, blocks possession).
/// - **Shift+click on a sub-part** of the already-selected primary DRILLS the
///   Inspector to that part instead of toggling the whole object off.
///
/// Deselect is explicit (Escape/Backspace via [`handle_deselect_keys`], the
/// Explorer, or selecting another entity) — a click on empty space or a panel
/// never clears the selection.
pub fn on_scene_click_select(
    mut click: On<Pointer<Click>>,
    spawn_state: Res<SpawnState>,
    keys: Res<ButtonInput<KeyCode>>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut selected: ResMut<SelectedEntities>,
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

    // Selection is **Shift+click** only. A plain (un-modified) left-click is
    // reserved for the avatar's possess/follow/focus path
    // (`avatar_raycast_possession`, the other global `Pointer<Click>` observer):
    // partitioning by the Shift modifier is what keeps the two observers from
    // both acting on one click. Without this gate a plain click on a rover would
    // BOTH possess it AND select-with-gizmo it — the gizmo makes the body
    // kinematic and `DragModeActive` blocks possession, which is what broke
    // joint-rover possession and made Shift-select appear to "not work".
    let shift_held = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if !shift_held {
        return;
    }

    let hit_entity = click.entity;
    let prev_selected = selected.primary();

    // Resolve the picked mesh to its selectable (nearest `SelectableRoot`
    // ancestor, or the hit entity itself for ground/props).
    let Some(entity) = find_selectable(hit_entity, &q_selectable, &q_parents) else {
        return;
    };

    // DRILL: Shift+click on a sub-part of the ALREADY-selected primary aims the
    // Inspector at that part instead of toggling the whole object out of the
    // selection.
    if prev_selected == Some(entity) && hit_entity != entity {
        inspector_target.part = Some(hit_entity);
        return;
    }

    // Shift+click toggles this entity in the multi-selection (extend + toggle),
    // through the same `apply_selection` the API command and Explorer use.
    apply_selection(&mut commands, &mut selected, q_selected_old.iter(), entity, true, true);
    inspector_target.part = None;
}

/// Escape / Backspace clears the selection and gizmo. Split out of the click
/// path because it's keyboard-driven, not a pointer pick. When an Inspector
/// field is focused bevy_egui absorbs the key (correct); otherwise it reaches
/// here and deselects through the same `SelectEntity` mutation path.
pub fn handle_deselect_keys(
    keys: Res<ButtonInput<KeyCode>>,
    q_selected_old: Query<Entity, With<Selected>>,
    mut selected: ResMut<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    mut commands: Commands,
) {
    if !(keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace)) {
        return;
    }
    clear_selection(&mut commands, &mut selected, q_selected_old.iter());
    inspector_target.part = None;
    // `DragModeActive` is driven by `gizmo::sync_gizmo_dragging_marker` from the
    // gizmo's active state; removing the `GizmoTarget`s above clears it next tick.
}

/// Draws an AABB highlight for selected objects using Bevy Gizmos.
pub fn draw_selection_bounds(
    q_selected: Query<Entity, With<Selected>>,
    q_aabb: Query<(&GlobalTransform, &Aabb)>,
    q_children: Query<&Children>,
    mut gizmos: Gizmos,
    theme: Res<lunco_theme::Theme>,
    mut queue: Local<Vec<Entity>>,
) {
    let color32 = theme.tokens.accent;
    let [r, g, b, a] = color32.to_srgba_unmultiplied();
    let color = Color::srgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    );
    
    for selected_ent in q_selected.iter() {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut has_aabb = false;

        queue.clear();
        queue.push(selected_ent);
        while let Some(e) = queue.pop() {
            if let Ok((gtf, aabb)) = q_aabb.get(e) {
                // To properly calculate the AABB, we take the 8 corners of the local AABB,
                // transform them to global space, and expand our min/max.
                let ext = Vec3::from(aabb.half_extents);
                let center = Vec3::from(aabb.center);
                for x in [-ext.x, ext.x] {
                    for y in [-ext.y, ext.y] {
                        for z in [-ext.z, ext.z] {
                            let local_p = center + Vec3::new(x, y, z);
                            let global_p = gtf.transform_point(local_p);
                            min = min.min(global_p);
                            max = max.max(global_p);
                        }
                    }
                }
                has_aabb = true;
            }
            if let Ok(children) = q_children.get(e) {
                queue.extend(children.iter());
            }
        }

        if has_aabb {
            let center = (min + max) * 0.5;
            let size = max - min;
            gizmos.primitive_3d(
                &Cuboid { half_size: size * 0.5 },
                Isometry3d::from_translation(center),
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selected_entities_default() {
        let selected = SelectedEntities::default();
        assert!(selected.primary().is_none());
    }
}
