//! Entity selection via Shift+Left-click.
//!
//! Uses Shift+Left-click to avoid conflict with regular left-click camera possession.
//! Selects the entity closest to the camera under the cursor and immediately
//! attaches a transform gizmo for manipulation.

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;

use bevy::camera::primitives::Aabb;
use bevy::math::primitives::Cuboid;
use bevy::math::Isometry3d;

use crate::SpawnState;
use lunco_controller::ControllerLink;
use lunco_core::{on_command, register_commands, Avatar, Command, LocalAvatar};
use lunco_scene_commands::SelectedEntities;
use lunco_usd_bevy::UsdPrimPath;

/// Component marking an entity as currently selected.
#[derive(Component)]
pub struct Selected;

/// Entity-keyed selection intent emitted by editor panels that already hold
/// the concrete entity. This preserves the shared selection mutation without
/// exposing a mutable `World` to UI code.
#[derive(Event, Clone, Copy)]
pub(crate) struct SelectEntityTarget {
    pub(crate) target: Entity,
    pub(crate) extend: bool,
    pub(crate) toggle: bool,
}

pub(crate) fn on_select_entity_target(
    trigger: On<SelectEntityTarget>,
    mut selected: ResMut<SelectedEntities>,
    q_old: Query<Entity, With<Selected>>,
    mut commands: Commands,
) {
    let request = trigger.event();
    apply_selection(
        &mut commands,
        &mut selected,
        q_old.iter(),
        request.target,
        request.extend,
        request.toggle,
    );
    commands.trigger(lunco_core::command_telemetry_event("SelectEntity"));
}

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
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`. `0` clears the selection.
    pub entity_id: u64,
    /// If true, maintains the previous selection and adds this entity to it (like Shift-click)
    pub extend: bool,
    /// If true, toggles the selection state of the entity (like Cmd/Ctrl-click)
    pub toggle: bool,
}

/// Select a composed USD prim by its authored path.
///
/// The path is resolved against the live `UsdPrimPath` projection rather than
/// an episode-specific entity id. This keeps scripted presentation commands
/// stable across scene reloads and across duplicated asset instances.
#[Command(default)]
pub struct SelectEntityByPath {
    pub path: String,
    pub extend: bool,
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
                commands
                    .entity(e)
                    .remove::<Selected>()
                    .remove::<crate::gizmo::GizmoSelected>();
            }
        }
        selected.entities.clear();
    }

    if toggle && selected.entities.contains(&target) {
        commands
            .entity(target)
            .remove::<Selected>()
            .remove::<crate::gizmo::GizmoSelected>();
        selected.entities.retain(|e| *e != target);
    } else {
        commands
            .entity(target)
            .try_insert((Selected, crate::gizmo::GizmoSelected));
        if !selected.entities.contains(&target) {
            selected.entities.push(target);
        }
    }
}

/// Replaces the Inspector/command focus without enabling edit manipulation.
///
/// `SelectedEntities` and `Selected` are the established focus source for the
/// Inspector, Explorer, and Command Deck. `GizmoSelected` is deliberately not
/// part of that contract: only an explicit editor selection may enable a
/// transform handle. Possession uses this path.
fn apply_focus(
    commands: &mut Commands,
    selected: &mut SelectedEntities,
    old_selected: impl IntoIterator<Item = Entity>,
    target: Entity,
) {
    for entity in old_selected {
        if entity != target {
            commands
                .entity(entity)
                .remove::<Selected>()
                .remove::<crate::gizmo::GizmoSelected>();
        }
    }
    selected.entities.clear();
    commands
        .entity(target)
        .try_insert(Selected)
        .remove::<crate::gizmo::GizmoSelected>();
    selected.entities.push(target);
}

/// Clears the whole selection (highlight + gizmo + resource). Shared by the
/// id-0 `SelectEntity` and the Escape/Backspace path.
pub(crate) fn clear_selection(
    commands: &mut Commands,
    selected: &mut SelectedEntities,
    old_selected: impl IntoIterator<Item = Entity>,
) {
    for e in old_selected {
        commands
            .entity(e)
            .remove::<Selected>()
            .remove::<crate::gizmo::GizmoSelected>();
    }
    selected.entities.clear();
}

/// Makes the controlled vessel the existing Inspector/command focus.
///
/// Possession is the user's active vehicle context, so leaving the Inspector on
/// a previously Shift-selected object is surprising. This deliberately calls
/// focus state remains the sole source used by the Inspector, Explorer, and
/// Command Deck. It intentionally does not activate the separate editor gizmo.
/// Releasing control leaves the last vessel focused, just as it leaves the
/// camera at its current view.
pub fn select_possessed_vessel(
    q_avatar: Query<Ref<ControllerLink>, (With<Avatar>, With<LocalAvatar>)>,
    q_old: Query<Entity, With<Selected>>,
    mut selected: ResMut<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    mut commands: Commands,
) {
    for link in q_avatar.iter() {
        if !link.is_changed() || selected.primary() == Some(link.vessel_entity) {
            continue;
        }
        apply_focus(
            &mut commands,
            &mut selected,
            q_old.iter(),
            link.vessel_entity,
        );
        inspector_target.part = None;
    }
}

// Resolves the api_id and routes through the shared `apply_selection` (or
// `clear_selection` on id 0).
// `SelectEntity` is editor-only (Inspector highlight + gizmo), so it is registered
// by `SceneEditPlugin` rather than the headless `SpawnCommandPlugin` — but it goes
// through the SAME type+observer registration as every other verb.
register_commands!(on_select_entity, on_select_entity_by_path);

#[on_command(SelectEntity)]
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

    apply_selection(
        &mut commands,
        &mut selected,
        q_old.iter(),
        target,
        cmd.extend,
        cmd.toggle,
    );
    info!(
        "SELECT_ENTITY: selected api_id={} ({target:?})",
        cmd.entity_id
    );
}

#[on_command(SelectEntityByPath)]
pub fn on_select_entity_by_path(
    trigger: On<SelectEntityByPath>,
    q_paths: Query<(Entity, &UsdPrimPath)>,
    mut selected: ResMut<SelectedEntities>,
    q_old: Query<Entity, With<Selected>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let Some(target) = q_paths
        .iter()
        .find(|(_, path)| path.path == cmd.path)
        .map(|(entity, _)| entity)
    else {
        warn!("SELECT_ENTITY_BY_PATH: no composed prim at `{}`", cmd.path);
        if !cmd.extend && !cmd.toggle {
            clear_selection(&mut commands, &mut selected, q_old.iter());
        }
        return;
    };

    apply_selection(
        &mut commands,
        &mut selected,
        q_old.iter(),
        target,
        cmd.extend,
        cmd.toggle,
    );
    info!("SELECT_ENTITY_BY_PATH: selected {}", cmd.path);
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
) -> Entity {
    // Deep enough to climb an imported glTF node tree (scene→node→…→mesh) up to
    // the SelectableRoot prim that wraps it — an 8-level cap left tall glb
    // hierarchies resolving to the clicked leaf instead of the model root.
    const MAX_DEPTH: usize = 32;
    let mut entity = hit;
    let mut depth = 0;

    loop {
        // A `SelectableRoot` ancestor wins — clicking a rover wheel selects the
        // rover root, not the wheel mesh.
        if q_selectable.get(entity).is_ok() {
            return entity;
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
    hit
}

/// The nearest PRIM-BACKED entity on the chain from `hit` up to (excluding)
/// `root` — the drill target the Inspector's USD-parameter section aims at.
///
/// The picked leaf is often a synthesized visual child (a wheel's `*_visual`
/// split, a glTF node) that carries no `UsdPrimPath`; the prim entity — the one
/// whose attributes `ApplyUsdOp` can address — is an ancestor. `None` when
/// nothing strictly below the root is prim-backed (the drill then keeps the raw
/// hit for material-scoped editing).
fn find_prim_part(
    hit: Entity,
    root: Entity,
    q_prims: &Query<Entity, With<lunco_usd_bevy::UsdPrimPath>>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    const MAX_DEPTH: usize = 32;
    let mut entity = hit;
    for _ in 0..MAX_DEPTH {
        if entity == root {
            return None;
        }
        if q_prims.get(entity).is_ok() {
            return Some(entity);
        }
        entity = q_parents.get(entity).ok()?.parent();
    }
    None
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
/// - **Alt+Shift+click on a sub-part** of the already-selected primary DRILLS the
///   Inspector to that part (plain Shift+click stays the selection toggle, so
///   deselecting a compound object still works).
///
/// Deselect is explicit (Escape/Backspace via [`handle_deselect_keys`], the
/// Explorer, or selecting another entity) — a click on empty space or a panel
/// never clears the selection.
pub fn on_scene_click_select(
    mut click: On<Pointer<Click>>,
    spawn_state: Res<SpawnState>,
    terrain_tool_active: Res<lunco_core::TerrainToolActive>,
    waypoint_tool_active: Res<lunco_core::WaypointToolActive>,
    armed_script_tool: Res<lunco_core::ArmedScriptTool>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_prims: Query<Entity, With<lunco_usd_bevy::UsdPrimPath>>,
    q_parents: Query<&ChildOf>,
    selected: Res<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    mut commands: Commands,
) {
    // Left button only.
    if click.button != PointerButton::Primary {
        return;
    }
    // Shared egui-vs-scene guard (viewport-rect aware) — the same robust check
    // possession and placement use. Empty/chrome clicks resolve to no
    // `SelectableRoot` below, so they select nothing regardless.
    if egui_focus.wants_pointer {
        return;
    }
    // Spawn tool armed: clicks place objects, not select.
    if !matches!(spawn_state.as_ref(), SpawnState::Idle) {
        return;
    }
    // Terrain brush armed: clicks sculpt the terrain, not select.
    if terrain_tool_active.0 {
        return;
    }
    // Waypoint Move/Insert armed: that click places the waypoint, not select.
    if waypoint_tool_active.0 {
        return;
    }
    // A script tool is armed: that click belongs to the tool, not to selection.
    if armed_script_tool.armed() {
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

    // `Pointer<Click>` auto-propagates leaf→parent→…→window; a global observer
    // would otherwise fire at every ancestor and select the wrong (top) one. We
    // resolve the `SelectableRoot` ourselves via `find_selectable`, so stop the
    // bubble at the picked leaf — this runs target-first, so we're at the leaf.
    click.propagate(false);

    let hit_entity = click.entity;
    let prev_selected = selected.primary();

    // Resolve the picked mesh to its selectable (nearest `SelectableRoot`
    // ancestor, or the hit entity itself for ground/props).
    let entity = find_selectable(hit_entity, &q_selectable, &q_parents);

    // DRILL: **Alt+Shift+click** on a sub-part of the ALREADY-selected primary
    // aims the Inspector at that part. Its own modifier, NOT plain Shift+click:
    // Shift+click is the multi-selection toggle, and for any compound object
    // (a rover — the picked mesh is always a descendant, never the root) a
    // shift-drill would SHADOW deselect entirely. Resolved to the nearest
    // PRIM-BACKED ancestor of the picked leaf (a wheel's `*_visual` mesh
    // drills to the wheel PRIM, whose `lunco:wheel:*` params the USD section
    // can then edit); the raw hit is kept only when nothing below the root
    // carries a prim path.
    let alt_held = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if alt_held && prev_selected == Some(entity) && hit_entity != entity {
        inspector_target.part =
            Some(find_prim_part(hit_entity, entity, &q_prims, &q_parents).unwrap_or(hit_entity));
        return;
    }

    // Shift+click toggles this entity in the multi-selection (extend + toggle)
    // through the same internal selection event used by the Explorer. The
    // event observer owns mutation and the shared script event, so this path
    // cannot drift from other selection surfaces.
    commands.trigger(SelectEntityTarget {
        target: entity,
        extend: true,
        toggle: true,
    });
    inspector_target.part = None;
}

/// The `Cancel` intent clears the selection and gizmo. Split out of the click
/// path because it's keyboard-driven, not a pointer pick. Deselects through the same
/// `SelectEntity` mutation path.
///
/// Reads [`lunco_core::CancelIntent`] rather than raw `Escape`/`Backspace`: the
/// bindings live in `assets/config/keybindings.json`, so one rebind moves every
/// "back out" at once, and the intent already stands down while an Inspector field has
/// keyboard focus (so Backspace there edits text).
///
/// Gated on [`lunco_core::CursorModeActive`] so Cancel unwinds the INNERMOST mode
/// first: while a waypoint placement/menu, the spawn ghost or the terrain brush is up,
/// that Cancel belongs to the mode — clearing the selection as a side effect would be
/// two undos for one keypress.
pub fn handle_deselect_keys(
    cancel: lunco_core::CancelIntent,
    cursor_mode: lunco_core::CursorModeActive,
    q_selected_old: Query<Entity, With<Selected>>,
    mut selected: ResMut<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    mut commands: Commands,
) {
    if cursor_mode.any() || !cancel.just_pressed() {
        return;
    }
    clear_selection(&mut commands, &mut selected, q_selected_old.iter());
    inspector_target.part = None;
    // `DragModeActive` is driven by `gizmo::sync_gizmo_dragging_marker` from the
    // gizmo's active state; removing the `GizmoTarget`s above clears it next tick.
}

/// Draws an AABB highlight for selected objects using Bevy Gizmos.
///
/// **Subtree Filtering**:
/// To prevent non-body utility subtrees (such as orbital trajectory lines, RF link
/// beams, or nested spatial grids) from corrupting the selection box:
/// The `q_aabb` query filters for entities with `Mesh3d`, excluding
/// `TrajectoryMeshMarker` lines and program-driven beam markers
/// (`ProgramDriverId`). The `q_skip_tree` query prevents `queue` from stepping
/// into child grids, trajectory paths, or program drivers during traversal.
/// Computes the body-frame bounding box (min, max) for an editable entity tree,
/// excluding non-body subtrees (link beams, trajectory lines, sub-grids, program drivers).
///
/// The returned points are in `body_transform`'s local frame.  This matters for
/// a rotated rover: a world-axis AABB is visually misleading and grows/shrinks
/// as the body turns.  Gizmos instead receive the body's orientation and this
/// stable, body-frame extent.
pub fn compute_selection_aabb(
    selected_ent: Entity,
    body_transform: &GlobalTransform,
    q_aabb: &Query<
        (&GlobalTransform, &Aabb),
        (
            With<Mesh3d>,
            Without<lunco_celestial::TrajectoryMeshMarker>,
            Without<lunco_core::programs::ProgramDriverId>,
            Without<lunco_core::NoSelectionBounds>,
        ),
    >,
    q_children: &Query<&Children>,
    q_skip_tree: &Query<
        (),
        Or<(
            With<big_space::prelude::Grid>,
            With<big_space::prelude::CellCoord>,
            With<lunco_celestial::TrajectoryMeshMarker>,
            With<lunco_core::programs::ProgramDriverId>,
            With<lunco_core::NoSelectionBounds>,
        )>,
    >,
    queue: &mut Vec<Entity>,
) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let mut has_aabb = false;
    let body_from_world = body_transform.affine().inverse();

    queue.clear();
    queue.push(selected_ent);
    while let Some(e) = queue.pop() {
        if let Ok((gtf, aabb)) = q_aabb.get(e) {
            let ext = Vec3::from(aabb.half_extents);
            let center = Vec3::from(aabb.center);
            for x in [-ext.x, ext.x] {
                for y in [-ext.y, ext.y] {
                    for z in [-ext.z, ext.z] {
                        let local_p = center + Vec3::new(x, y, z);
                        let world_p = gtf.transform_point(local_p);
                        let body_p = body_from_world.transform_point3(world_p);
                        min = min.min(body_p);
                        max = max.max(body_p);
                    }
                }
            }
            has_aabb = true;
        }
        if let Ok(children) = q_children.get(e) {
            for child in children.iter() {
                if !q_skip_tree.contains(child) {
                    queue.push(child);
                }
            }
        }
    }

    if has_aabb {
        Some((min, max))
    } else {
        None
    }
}

/// Draws body-frame bounds for objects explicitly selected for gizmo editing.
///
/// Control focus (`Selected`) is deliberately not sufficient: possession keeps
/// the controlled vessel visible in the Inspector without turning control into
/// an editor operation or drawing an AABB.
pub fn draw_selection_bounds(
    q_selected: Query<(Entity, &GlobalTransform), With<crate::gizmo::GizmoSelected>>,
    q_aabb: Query<
        (&GlobalTransform, &Aabb),
        (
            With<Mesh3d>,
            Without<lunco_celestial::TrajectoryMeshMarker>,
            Without<lunco_core::programs::ProgramDriverId>,
            Without<lunco_core::NoSelectionBounds>,
        ),
    >,
    q_children: Query<&Children>,
    q_skip_tree: Query<
        (),
        Or<(
            With<big_space::prelude::Grid>,
            With<big_space::prelude::CellCoord>,
            With<lunco_celestial::TrajectoryMeshMarker>,
            With<lunco_core::programs::ProgramDriverId>,
            With<lunco_core::NoSelectionBounds>,
        )>,
    >,
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

    for (selected_ent, body_transform) in q_selected.iter() {
        if let Some((min, max)) = compute_selection_aabb(
            selected_ent,
            body_transform,
            &q_aabb,
            &q_children,
            &q_skip_tree,
            &mut queue,
        ) {
            let center = body_transform.affine().transform_point3((min + max) * 0.5);
            let size = max - min;
            let (_, rotation, _) = body_transform.to_scale_rotation_translation();
            gizmos.primitive_3d(
                &Cuboid {
                    half_size: size * 0.5,
                },
                Isometry3d::new(center, rotation),
                color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::primitives::Aabb;

    #[test]
    fn possession_selects_the_controlled_vessel_for_the_inspector() {
        let mut app = App::new();
        app.init_resource::<SelectedEntities>()
            .init_resource::<crate::InspectorTarget>()
            .add_systems(Update, select_possessed_vessel);

        let previously_selected = app.world_mut().spawn(Selected).id();
        app.world_mut()
            .resource_mut::<SelectedEntities>()
            .entities
            .push(previously_selected);
        let vessel = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((
            Avatar,
            LocalAvatar,
            ControllerLink {
                vessel_entity: vessel,
            },
        ));

        app.update();

        let selected = app.world().resource::<SelectedEntities>();
        assert_eq!(selected.primary(), Some(vessel));
        assert!(app.world().get::<Selected>(vessel).is_some());
        assert!(
            app.world()
                .get::<crate::gizmo::GizmoSelected>(vessel)
                .is_none(),
            "possession focus must not activate an edit gizmo"
        );
        assert!(app.world().get::<Selected>(previously_selected).is_none());
    }

    #[test]
    fn test_selected_entities_default() {
        let selected = SelectedEntities::default();
        assert!(selected.primary().is_none());
    }

    #[test]
    fn test_draw_selection_bounds_excludes_link_beams() {
        let mut app = App::new();

        let rover = app
            .world_mut()
            .spawn((Selected, Transform::IDENTITY, GlobalTransform::IDENTITY))
            .id();
        let chassis = app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                Aabb {
                    center: Vec3A::ZERO,
                    half_extents: Vec3A::new(1.0, 0.5, 1.5),
                },
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                ChildOf(rover),
            ))
            .id();

        let beam = app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                Aabb {
                    center: Vec3A::ZERO,
                    half_extents: Vec3A::new(10.0, 10.0, 50000.0),
                },
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                lunco_core::NoSelectionBounds,
                ChildOf(rover),
            ))
            .id();

        let mut q = app.world_mut().query_filtered::<Entity, (
            With<Mesh3d>,
            Without<lunco_celestial::TrajectoryMeshMarker>,
            Without<lunco_core::programs::ProgramDriverId>,
            Without<lunco_core::NoSelectionBounds>,
        )>();

        let matched: Vec<Entity> = q.iter(app.world()).collect();
        assert_eq!(matched, vec![chassis]);
        assert!(!matched.contains(&beam));
    }

    #[test]
    fn test_compute_selection_aabb_returns_tight_vehicle_bounds() {
        let mut app = App::new();

        let rover = app
            .world_mut()
            .spawn((Selected, Transform::IDENTITY, GlobalTransform::IDENTITY))
            .id();
        let _chassis = app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                Aabb {
                    center: Vec3A::ZERO,
                    half_extents: Vec3A::new(1.0, 0.5, 1.5),
                },
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                ChildOf(rover),
            ))
            .id();

        let _beam = app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                Aabb {
                    center: Vec3A::ZERO,
                    half_extents: Vec3A::new(10.0, 10.0, 50000.0),
                },
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                lunco_core::NoSelectionBounds,
                ChildOf(rover),
            ))
            .id();

        let mut state_aabb = app
            .world_mut()
            .query_filtered::<(&GlobalTransform, &Aabb), (
                With<Mesh3d>,
                Without<lunco_celestial::TrajectoryMeshMarker>,
                Without<lunco_core::programs::ProgramDriverId>,
                Without<lunco_core::NoSelectionBounds>,
            )>();
        let mut state_children = app.world_mut().query::<&Children>();
        let mut state_skip = app.world_mut().query_filtered::<(), Or<(
            With<big_space::prelude::Grid>,
            With<big_space::prelude::CellCoord>,
            With<lunco_celestial::TrajectoryMeshMarker>,
            With<lunco_core::programs::ProgramDriverId>,
            With<lunco_core::NoSelectionBounds>,
        )>>();

        let mut queue = Vec::new();
        let (min, max) = compute_selection_aabb(
            rover,
            &GlobalTransform::IDENTITY,
            &state_aabb.query(app.world()),
            &state_children.query(app.world()),
            &state_skip.query(app.world()),
            &mut queue,
        )
        .expect("Selection AABB should exist for rover chassis");

        let size = max - min;
        assert!((size.x - 2.0).abs() < 1e-4);
        assert!((size.y - 1.0).abs() < 1e-4);
        assert!((size.z - 3.0).abs() < 1e-4);
        assert!(
            size.max_element() < 5.0,
            "Selection AABB must be tight (< 5m), got {size}"
        );
    }

    #[test]
    fn selection_bounds_stay_in_the_rotated_body_frame() {
        let mut app = App::new();
        let body_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let body_global = GlobalTransform::from(Transform::from_rotation(body_rotation));
        let rover = app.world_mut().spawn((Selected, body_global)).id();

        // The mesh is two metres along the body's local +X.  Its render-world
        // position is therefore along -Z after the body rotation.
        let _chassis = app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                Aabb {
                    center: Vec3A::ZERO,
                    half_extents: Vec3A::new(1.0, 0.5, 1.5),
                },
                GlobalTransform::from(
                    Transform::from_translation(Vec3::new(0.0, 0.0, -2.0))
                        .with_rotation(body_rotation),
                ),
                ChildOf(rover),
            ))
            .id();
        let mut state_aabb = app
            .world_mut()
            .query_filtered::<(&GlobalTransform, &Aabb), (
                With<Mesh3d>,
                Without<lunco_celestial::TrajectoryMeshMarker>,
                Without<lunco_core::programs::ProgramDriverId>,
                Without<lunco_core::NoSelectionBounds>,
            )>();
        let mut state_children = app.world_mut().query::<&Children>();
        let mut state_skip = app.world_mut().query_filtered::<(), Or<(
            With<big_space::prelude::Grid>,
            With<big_space::prelude::CellCoord>,
            With<lunco_celestial::TrajectoryMeshMarker>,
            With<lunco_core::programs::ProgramDriverId>,
            With<lunco_core::NoSelectionBounds>,
        )>>();

        let mut queue = Vec::new();
        let (min, max) = compute_selection_aabb(
            rover,
            &body_global,
            &state_aabb.query(app.world()),
            &state_children.query(app.world()),
            &state_skip.query(app.world()),
            &mut queue,
        )
        .expect("Selection AABB should exist for rover chassis");

        assert!((min.x - 1.0).abs() < 1e-4, "min={min}");
        assert!((max.x - 3.0).abs() < 1e-4, "max={max}");
        assert!(((max - min).z - 3.0).abs() < 1e-4);
    }
}
