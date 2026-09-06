//! Entity selection via semantic mouse intents.
//!
//! A plain left click replaces the selection, Shift+left click extends it, and
//! Ctrl+left click removes only the clicked entity. The viewport, API, and
//! editor panels all route through the same selection mutation owner.

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;

use bevy::camera::primitives::Aabb;
use bevy::math::Isometry3d;
use bevy::math::primitives::Cuboid;

use crate::SpawnState;
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, Command, LocalAvatar, on_command, register_commands};
use lunco_scene_commands::SelectedEntities;
use lunco_usd::ui::viewport::{UsdPreviewId, UsdViewportState};
use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};

/// Component marking an entity as currently selected.
#[derive(Component)]
pub struct Selected;

/// Read-only public view of the live editor selection.
///
/// The selection itself remains owned by `SelectedEntities`; this provider only
/// translates the established Entity-keyed state back to stable API ids for
/// authored scenarios and external clients.
pub(crate) struct InspectSelectionProvider;

impl lunco_api::queries::ApiQueryProvider for InspectSelectionProvider {
    fn name(&self) -> &'static str {
        "InspectSelection"
    }

    fn execute(
        &self,
        world: &World,
        _params: &serde_json::Value,
    ) -> lunco_api::schema::ApiResponse {
        let Some(selected) = world.get_resource::<SelectedEntities>() else {
            return lunco_api::schema::ApiResponse::error(
                lunco_api::schema::ApiErrorCode::InternalError,
                "InspectSelection: SelectedEntities resource is not present",
            );
        };
        let Some(registry) = world.get_resource::<lunco_api::registry::ApiEntityRegistry>() else {
            return lunco_api::schema::ApiResponse::error(
                lunco_api::schema::ApiErrorCode::InternalError,
                "InspectSelection: ApiEntityRegistry resource is not present",
            );
        };

        let selected_ids: Vec<u64> = selected
            .entities
            .iter()
            .filter_map(|entity| registry.api_id_for(*entity).map(|id| id.get()))
            .collect();
        lunco_api::schema::ApiResponse::ok(serde_json::json!({
            "selected": selected_ids,
            "primary": selected_ids.last().copied(),
            "stale_count": selected.entities.len() - selected_ids.len(),
        }))
    }
}

/// The semantic operation represented by one editor selection gesture.
///
/// Modifier decoding belongs at the pointer boundary; selection mutation then
/// consumes this enum so every input surface shares one contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionIntent {
    Replace,
    Extend,
    Toggle,
    Remove,
}

/// Decode the viewport modifier chord. Ctrl wins when both modifiers are held,
/// making Ctrl+Shift a deterministic remove operation rather than an accidental
/// extend-and-remove combination.
pub(crate) fn selection_intent(shift_held: bool, ctrl_held: bool) -> SelectionIntent {
    if ctrl_held {
        SelectionIntent::Remove
    } else if shift_held {
        SelectionIntent::Extend
    } else {
        SelectionIntent::Replace
    }
}

fn command_selection_intent(extend: bool, toggle: bool, remove_only: bool) -> SelectionIntent {
    if remove_only {
        SelectionIntent::Remove
    } else if toggle {
        SelectionIntent::Toggle
    } else if extend {
        SelectionIntent::Extend
    } else {
        SelectionIntent::Replace
    }
}

fn usd_selection_intent(extend: bool, toggle: bool) -> SelectionIntent {
    if toggle {
        SelectionIntent::Toggle
    } else if extend {
        SelectionIntent::Extend
    } else {
        SelectionIntent::Replace
    }
}

/// Entity-keyed selection intent emitted by editor panels that already hold
/// the concrete entity. This preserves the shared selection mutation without
/// exposing a mutable `World` to UI code.
#[derive(Event, Clone, Copy)]
pub(crate) struct SelectEntityTarget {
    pub(crate) target: Entity,
    pub(crate) intent: SelectionIntent,
}

pub(crate) fn on_select_entity_target(
    trigger: On<SelectEntityTarget>,
    mut selected: ResMut<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    q_old: Query<Entity, With<Selected>>,
    mut commands: Commands,
) {
    let request = trigger.event();
    apply_selection(
        &mut commands,
        &mut selected,
        q_old.iter(),
        request.target,
        request.intent,
    );
    inspector_target.part = None;
    commands.trigger(lunco_core::command_telemetry_event("SelectEntity"));
}

/// Select an entity by API id — the headless/scriptable equivalent of a
/// viewport selection gesture. Drives the same [`SelectedEntities`]
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
    /// If true, removes this entity without adding it when it is not selected
    /// (the Ctrl+Left-click viewport intent).
    pub remove_only: bool,
}

/// Select a composed USD prim in one explicit open and focused preview.
///
/// The preview lease is part of the identity. A path is not globally unique:
/// the running scene and every open editor document can contain the same
/// authored path. Resolving only by path can therefore select an entity from
/// the wrong document. The handler first validates the lease and then scopes
/// the projection lookup to its stage handle and preview root.
#[Command(default)]
pub struct SelectUsdPrim {
    /// The isolated USD preview that owns the selection.
    pub preview: UsdPreviewId,
    /// Absolute composed USD prim path within that preview's stage.
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
/// only highlights. Plain clicks on editor-owned roots are claimed by the
/// selection observer; possession is suppressed only for that boundary or
/// while a gizmo handle is actively dragged.
///
/// - [`SelectionIntent::Replace`] → replace the selection with `target`.
/// - [`SelectionIntent::Extend`] → add `target` while retaining the old set.
/// - [`SelectionIntent::Toggle`] → add or remove `target`.
/// - [`SelectionIntent::Remove`] → remove `target`, never add it.
pub(crate) fn apply_selection(
    commands: &mut Commands,
    selected: &mut SelectedEntities,
    old_selected: impl IntoIterator<Item = Entity>,
    target: Entity,
    intent: SelectionIntent,
) {
    if intent == SelectionIntent::Replace {
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

    match intent {
        SelectionIntent::Remove => {
            commands
                .entity(target)
                .remove::<Selected>()
                .remove::<crate::gizmo::GizmoSelected>();
            selected.entities.retain(|e| *e != target);
        }
        SelectionIntent::Toggle if selected.entities.contains(&target) => {
            commands
                .entity(target)
                .remove::<Selected>()
                .remove::<crate::gizmo::GizmoSelected>();
            selected.entities.retain(|e| *e != target);
        }
        SelectionIntent::Replace | SelectionIntent::Extend | SelectionIntent::Toggle => {
            commands
                .entity(target)
                .try_insert((Selected, crate::gizmo::GizmoSelected));
            if !selected.entities.contains(&target) {
                selected.entities.push(target);
            }
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
/// a previously selected object is surprising. This deliberately keeps focus
/// state as the sole source used by the Inspector, Explorer, and
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
register_commands!(on_select_entity, on_select_usd_prim);

#[on_command(SelectEntity)]
pub fn on_select_entity(
    trigger: On<SelectEntity>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut selected: ResMut<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    q_old: Query<Entity, With<Selected>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    if cmd.entity_id == 0 {
        clear_selection(&mut commands, &mut selected, q_old.iter());
        inspector_target.part = None;
        info!("SELECT_ENTITY: cleared selection");
        return;
    }

    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("SELECT_ENTITY: no api_id={} in registry", cmd.entity_id);
        if command_selection_intent(cmd.extend, cmd.toggle, cmd.remove_only)
            == SelectionIntent::Replace
        {
            clear_selection(&mut commands, &mut selected, q_old.iter());
            inspector_target.part = None;
        }
        return;
    };

    apply_selection(
        &mut commands,
        &mut selected,
        q_old.iter(),
        target,
        command_selection_intent(cmd.extend, cmd.toggle, cmd.remove_only),
    );
    inspector_target.part = None;
    info!(
        "SELECT_ENTITY: selected api_id={} ({target:?})",
        cmd.entity_id
    );
}

#[on_command(SelectUsdPrim)]
pub fn on_select_usd_prim(
    trigger: On<SelectUsdPrim>,
    viewport: Option<Res<UsdViewportState>>,
    q_paths: Query<(Entity, &UsdPrimPath)>,
    q_parents: Query<&ChildOf>,
    mut selected: ResMut<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    q_old: Query<Entity, With<Selected>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let Some(viewport) = viewport.as_deref() else {
        warn!(
            "SELECT_USD_PRIM: preview {} is unavailable because USD previews are not installed",
            cmd.preview.0
        );
        return;
    };
    let Some(session) = viewport.session(cmd.preview) else {
        warn!("SELECT_USD_PRIM: preview {} is not open", cmd.preview.0);
        return;
    };
    if viewport.focused_preview_id() != Some(cmd.preview) {
        warn!(
            "SELECT_USD_PRIM: preview {} must be focused before selecting `{}`",
            cmd.preview.0, cmd.path
        );
        return;
    }
    let Some(target) = resolve_usd_prim_in_preview(
        session.scene_root(),
        session.stage_handle().id(),
        &cmd.path,
        &q_paths,
        &q_parents,
    ) else {
        warn!(
            "SELECT_USD_PRIM: preview {} has no composed prim at `{}`",
            cmd.preview.0, cmd.path
        );
        if usd_selection_intent(cmd.extend, cmd.toggle) == SelectionIntent::Replace {
            clear_selection(&mut commands, &mut selected, q_old.iter());
            inspector_target.part = None;
        }
        return;
    };

    apply_selection(
        &mut commands,
        &mut selected,
        q_old.iter(),
        target,
        usd_selection_intent(cmd.extend, cmd.toggle),
    );
    inspector_target.part = None;
    info!(
        "SELECT_USD_PRIM: preview {} selected {}",
        cmd.preview.0, cmd.path
    );
}

/// Resolve one authored path against the stage and hierarchy of one preview.
///
/// The stage check prevents collisions between separately projected documents;
/// the hierarchy check prevents a live-scene entity that happens to share the
/// stage handle from entering the editor selection. The preview session owns
/// both inputs, so callers never need a second identity map.
fn resolve_usd_prim_in_preview(
    preview_root: Entity,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    path: &str,
    q_paths: &Query<(Entity, &UsdPrimPath)>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    q_paths
        .iter()
        .find(|(entity, prim)| {
            prim.stage_handle.id() == stage_id
                && prim.path == path
                && crate::ui::is_editor_preview_entity(*entity, preview_root, q_parents)
        })
        .map(|(entity, _)| entity)
}

/// Finds the most appropriate entity to select from a hit entity.
///
/// Walks up the parent chain (up to `MAX_DEPTH`, matching the avatar
/// possession resolver). A mobility root is the semantic owner of a vehicle
/// assembly, so it wins over nested spawnable component markers. If the hit is
/// not inside a mobility realization, the nearest `SelectableRoot` owns the
/// selection.
///
/// If neither marker exists in the chain, it falls back to the clicked entity,
/// so ground, terrain and plain USD visual props remain selectable.
fn find_selectable(
    hit: Entity,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_mobility: &Query<Entity, With<lunco_core::MobilityRoot>>,
    q_parents: &Query<&ChildOf>,
) -> Entity {
    // Deep enough to climb an imported glTF node tree (scene→node→…→mesh) up to
    // the SelectableRoot prim that wraps it — an 8-level cap left tall glb
    // hierarchies resolving to the clicked leaf instead of the model root.
    const MAX_DEPTH: usize = 32;
    let mut entity = hit;
    let mut depth = 0;
    let mut selectable = None;

    loop {
        // `MobilityRoot` is stamped from the authored vehicle schema and owns
        // the complete assembly. It must be considered before a nested
        // spawnable component's `SelectableRoot`.
        if q_mobility.get(entity).is_ok() {
            return entity;
        }
        if selectable.is_none() && q_selectable.get(entity).is_ok() {
            selectable = Some(entity);
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

    selectable.unwrap_or(hit)
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
/// - **Left-click** replaces the selection and attaches a `GizmoTarget`.
/// - **Shift+left-click** extends the selection without toggling an existing
///   member off.
/// - **Ctrl+left-click** removes only the clicked entity and never adds it.
///   The avatar possession observer stands down for the same editor-owned hit,
///   so the two global click observers cannot both act on one gesture.
/// - **Alt+Shift+click on a sub-part** of the already-selected primary DRILLS the
///   Inspector to that part. Ctrl takes precedence, so Ctrl+Alt+Shift remains
///   removal rather than an Inspector drill.
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
    scene_interaction: Res<lunco_core::SceneInteractionMode>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_mobility: Query<Entity, With<lunco_core::MobilityRoot>>,
    q_prims: Query<Entity, With<lunco_usd_bevy::UsdPrimPath>>,
    q_parents: Query<&ChildOf>,
    selected: Res<SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    mut commands: Commands,
) {
    // View mode reserves plain clicks for avatar possession. Selection owns
    // the same pointer only in an editor-facing perspective.
    if !scene_interaction.selection_owns_primary_click() {
        return;
    }
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

    // Chrome and empty-space clicks carry no world hit. Keep the selection
    // unchanged; explicit Cancel/Escape owns deselection.
    if click.hit.position.is_none() {
        return;
    }

    let shift_held = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl_held = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let intent = selection_intent(shift_held, ctrl_held);

    // `Pointer<Click>` auto-propagates leaf→parent→…→window; a global observer
    // would otherwise fire at every ancestor and select the wrong (top) one. We
    // resolve the semantic target ourselves, so stop the bubble at the picked
    // leaf — this runs target-first, so we're at the leaf.
    click.propagate(false);

    let hit_entity = click.entity;
    let prev_selected = selected.primary();

    // Resolve the picked mesh to its semantic owner: a mobility root for a
    // vehicle assembly, otherwise the nearest selectable root or the hit
    // entity itself for ground/props.
    let entity = find_selectable(hit_entity, &q_selectable, &q_mobility, &q_parents);

    // DRILL: **Alt+Shift+click** on a sub-part of the ALREADY-selected primary
    // aims the Inspector at that part. Resolved to the nearest PRIM-BACKED
    // ancestor of the picked leaf (a wheel's `*_visual` mesh drills to the
    // wheel PRIM, whose `lunco:wheel:*` params the USD section can edit); the
    // raw hit is kept only when nothing below the root carries a prim path.
    let alt_held = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if matches!(intent, SelectionIntent::Extend)
        && alt_held
        && prev_selected == Some(entity)
        && hit_entity != entity
    {
        inspector_target.part =
            Some(find_prim_part(hit_entity, entity, &q_prims, &q_parents).unwrap_or(hit_entity));
        return;
    }

    // Route through the same internal selection event used by the Explorer. The
    // event observer owns mutation and the shared script event, so this path
    // cannot drift from other selection surfaces.
    commands.trigger(SelectEntityTarget {
        target: entity,
        intent,
    });
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

    if has_aabb { Some((min, max)) } else { None }
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

    const MINIMAL_USD: &str =
        "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\" {}\n";

    #[test]
    fn viewport_modifier_chord_maps_to_replace_extend_and_remove() {
        assert_eq!(selection_intent(false, false), SelectionIntent::Replace);
        assert_eq!(selection_intent(true, false), SelectionIntent::Extend);
        assert_eq!(selection_intent(false, true), SelectionIntent::Remove);
        assert_eq!(selection_intent(true, true), SelectionIntent::Remove);
    }

    #[test]
    fn extend_adds_and_remove_only_never_adds() {
        let mut app = App::new();
        app.init_resource::<SelectedEntities>()
            .init_resource::<crate::InspectorTarget>()
            .add_observer(on_select_entity_target);

        let first = app.world_mut().spawn_empty().id();
        let second = app.world_mut().spawn_empty().id();
        let unselected = app.world_mut().spawn_empty().id();

        for (target, intent) in [
            (first, SelectionIntent::Replace),
            (second, SelectionIntent::Extend),
            (first, SelectionIntent::Remove),
            (unselected, SelectionIntent::Remove),
        ] {
            app.world_mut()
                .trigger(SelectEntityTarget { target, intent });
            app.world_mut().flush();
        }

        assert_eq!(
            app.world().resource::<SelectedEntities>().entities,
            vec![second]
        );
        assert!(app.world().get::<Selected>(first).is_none());
        assert!(app.world().get::<Selected>(second).is_some());
        assert!(app.world().get::<Selected>(unselected).is_none());
    }

    #[test]
    fn public_select_entity_command_exposes_remove_only_semantics() {
        let mut app = App::new();
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>()
            .init_resource::<SelectedEntities>()
            .init_resource::<crate::InspectorTarget>()
            .add_observer(on_select_entity);

        let first = app.world_mut().spawn_empty().id();
        let second = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(first, lunco_core::GlobalEntityId::from_raw(41));
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(second, lunco_core::GlobalEntityId::from_raw(42));

        for command in [
            SelectEntity {
                entity_id: 41,
                ..default()
            },
            SelectEntity {
                entity_id: 42,
                extend: true,
                ..default()
            },
            SelectEntity {
                entity_id: 41,
                remove_only: true,
                ..default()
            },
            SelectEntity {
                entity_id: 41,
                remove_only: true,
                ..default()
            },
        ] {
            app.world_mut().trigger(command);
            app.world_mut().flush();
        }

        assert_eq!(
            app.world().resource::<SelectedEntities>().entities,
            vec![second]
        );
        assert!(app.world().get::<Selected>(first).is_none());
        assert!(app.world().get::<Selected>(second).is_some());
    }

    #[test]
    fn usd_prim_resolution_requires_the_requested_preview_scope() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>();

        let stage_a = app.world_mut().resource_mut::<Assets<UsdStageAsset>>().add(
            UsdStageAsset::from_recipe(lunco_usd_bevy::StageRecipe::from_source(
                "preview-a.usda",
                MINIMAL_USD,
            ))
            .expect("preview A stage asset"),
        );
        let stage_b = app.world_mut().resource_mut::<Assets<UsdStageAsset>>().add(
            UsdStageAsset::from_recipe(lunco_usd_bevy::StageRecipe::from_source(
                "preview-b.usda",
                MINIMAL_USD,
            ))
            .expect("preview B stage asset"),
        );

        let root_a = app.world_mut().spawn_empty().id();
        let root_b = app.world_mut().spawn_empty().id();
        let prim_a = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage_a.clone(),
                    path: "/World/Chassis".into(),
                },
                ChildOf(root_a),
            ))
            .id();
        // Put the same-stage collision before the valid preview entity so the
        // resolver must enforce the preview-root boundary instead of passing
        // because the entity iteration happened to find the right row first.
        let live_collision = app
            .world_mut()
            .spawn(UsdPrimPath {
                stage_handle: stage_b.clone(),
                path: "/World/Chassis".into(),
            })
            .id();
        let prim_b = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage_b.clone(),
                    path: "/World/Chassis".into(),
                },
                ChildOf(root_b),
            ))
            .id();

        let mut q_paths = app.world_mut().query::<(Entity, &UsdPrimPath)>();
        let mut q_parents = app.world_mut().query::<&ChildOf>();

        assert_eq!(
            resolve_usd_prim_in_preview(
                root_b,
                stage_b.id(),
                "/World/Chassis",
                &q_paths.query(app.world()),
                &q_parents.query(app.world()),
            ),
            Some(prim_b)
        );
        assert_ne!(
            resolve_usd_prim_in_preview(
                root_b,
                stage_b.id(),
                "/World/Chassis",
                &q_paths.query(app.world()),
                &q_parents.query(app.world()),
            ),
            Some(live_collision)
        );
        assert_ne!(
            resolve_usd_prim_in_preview(
                root_b,
                stage_b.id(),
                "/World/Chassis",
                &q_paths.query(app.world()),
                &q_parents.query(app.world()),
            ),
            Some(prim_a)
        );
    }

    #[test]
    fn mobility_root_owns_nested_spawnable_component_selection() {
        let mut app = App::new();
        let rover = app
            .world_mut()
            .spawn((lunco_core::MobilityRoot, lunco_core::SelectableRoot))
            .id();
        let battery = app
            .world_mut()
            .spawn((lunco_core::SelectableRoot, ChildOf(rover)))
            .id();
        let hit = app.world_mut().spawn(ChildOf(battery)).id();

        let mut q_selectable = app
            .world_mut()
            .query_filtered::<Entity, With<lunco_core::SelectableRoot>>();
        let mut q_mobility = app
            .world_mut()
            .query_filtered::<Entity, With<lunco_core::MobilityRoot>>();
        let mut q_parents = app.world_mut().query::<&ChildOf>();

        assert_eq!(
            find_selectable(
                hit,
                &q_selectable.query(app.world()),
                &q_mobility.query(app.world()),
                &q_parents.query(app.world()),
            ),
            rover
        );
    }

    #[test]
    fn standalone_spawnable_component_remains_selectable() {
        let mut app = App::new();
        let component = app.world_mut().spawn(lunco_core::SelectableRoot).id();
        let hit = app.world_mut().spawn(ChildOf(component)).id();

        let mut q_selectable = app
            .world_mut()
            .query_filtered::<Entity, With<lunco_core::SelectableRoot>>();
        let mut q_mobility = app
            .world_mut()
            .query_filtered::<Entity, With<lunco_core::MobilityRoot>>();
        let mut q_parents = app.world_mut().query::<&ChildOf>();

        assert_eq!(
            find_selectable(
                hit,
                &q_selectable.query(app.world()),
                &q_mobility.query(app.world()),
                &q_parents.query(app.world()),
            ),
            component
        );
    }

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
    fn toggled_selection_updates_highlights_without_starting_drag() {
        let mut app = App::new();
        app.init_resource::<SelectedEntities>()
            .init_resource::<crate::InspectorTarget>()
            .insert_resource(lunco_core::DragModeActive::default())
            .add_observer(on_select_entity_target);

        let first = app.world_mut().spawn_empty().id();
        let second = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(SelectEntityTarget {
            target: first,
            intent: SelectionIntent::Toggle,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world().resource::<SelectedEntities>().entities,
            vec![first]
        );
        assert!(app.world().get::<Selected>(first).is_some());
        assert!(
            app.world()
                .get::<crate::gizmo::GizmoSelected>(first)
                .is_some()
        );

        app.world_mut()
            .resource_mut::<crate::InspectorTarget>()
            .part = Some(first);

        app.world_mut().trigger(SelectEntityTarget {
            target: second,
            intent: SelectionIntent::Toggle,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world().resource::<SelectedEntities>().entities,
            vec![first, second]
        );
        assert!(app.world().get::<Selected>(second).is_some());
        assert!(
            app.world()
                .resource::<crate::InspectorTarget>()
                .part
                .is_none()
        );

        app.world_mut().trigger(SelectEntityTarget {
            target: first,
            intent: SelectionIntent::Toggle,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world().resource::<SelectedEntities>().entities,
            vec![second]
        );
        assert!(app.world().get::<Selected>(first).is_none());
        assert!(
            app.world()
                .get::<crate::gizmo::GizmoSelected>(first)
                .is_none()
        );
        assert!(!app.world().resource::<lunco_core::DragModeActive>().active);
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
