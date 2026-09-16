//! Script-authored CLICK TOOLS — the editor half of the `lunco_tools` registry.
//!
//! A tool library that exposes `on_click(context)` becomes an armable tool in
//! the Tools palette. Arm it, click in the scene, and the tool's own Rhai
//! handler receives the canonical click context, including whether the click
//! used the primary, secondary, or middle button. Nothing else is required:
//! there is no registration call, no palette edit, no Rust per tool. Drop
//! `assets/scripting/tools/<name>.rhai` with an `on_click` in it and the button
//! is there next launch.
//!
//! ```rhai
//! // assets/scripting/tools/recover.rhai
//! fn ui_label() { "Recover" }
//! fn ui_hint()  { "Click a stuck vessel to right it" }
//! fn on_click(context) { vessel(context.target_entity_id); }
//! ```
//!
//! WHY DISCOVERY BY SIGNATURE. `Tool::functions()` already reports `name/arity`
//! for every registered tool (for rhai tools it comes from parsing the source),
//! so "can this be clicked?" is answerable from what the tool actually
//! implements. A separate list of palette entries could disagree with the code —
//! a button with no handler, or a handler nobody can reach. This cannot.
//!
//! The click is handed over as a typed tool-hook command. It is queued and run
//! by `drain_world_scripts` with the prelude and every tool in scope, so an
//! authored interaction policy can do anything a scenario can.

use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::prelude::*;
use lunco_avatar_core::roles::TheLocalAvatar;
use lunco_core::{TelemetryEvent, TelemetryValue};
use lunco_cosim_core::ControlLink;
use lunco_input_core::InputBindingsSettings;
use lunco_scene_selection::SelectedEntities;
use lunco_spatial::coords::{
    ActiveFrameCoordinates, RenderPos, ACTIVE_FRAME_NAME, RENDER_FRAME_NAME,
};
use std::collections::HashSet;

/// Build the language-neutral map passed to a script tool. The map is an
/// interaction contract, not an API serialization format; the scripting
/// backend converts it directly to the target runtime's native value.
pub(crate) fn tool_map(entries: Vec<(String, TelemetryValue)>) -> TelemetryValue {
    TelemetryValue::Map(entries.into_iter().collect())
}

/// Pointer events bubble through every authored parent. Keep one dispatch key
/// for the duration of the frame so the generic scene event is emitted once,
/// while the normal selection and possession observers can still receive the
/// original pointer event.
#[derive(Resource, Default)]
pub struct ScenePointerDispatch {
    seen: HashSet<ScenePointerKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ScenePointerKey {
    pointer: PointerId,
    button: PointerButton,
    click_count: u8,
    screen_position: [u32; 2],
}

pub fn clear_scene_pointer_dispatch(mut dispatch: ResMut<ScenePointerDispatch>) {
    dispatch.seen.clear();
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SceneToolWorld<'w, 's> {
    q_selectable: Query<'w, 's, Entity, With<lunco_core::SelectableRoot>>,
    q_ids: Query<'w, 's, &'static lunco_core::GlobalEntityId>,
    q_prim: Query<'w, 's, &'static lunco_usd_bevy_scene::UsdPrimPath>,
    q_scene_roots: Query<
        'w,
        's,
        &'static lunco_usd_bevy_scene::UsdPrimPath,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
    q_parents: Query<'w, 's, &'static ChildOf>,
    selected: Res<'w, SelectedEntities>,
    local_avatar: Res<'w, TheLocalAvatar>,
    q_links: Query<'w, 's, &'static ControlLink>,
    input_bindings: Res<'w, InputBindingsSettings>,
    backed: Res<'w, lunco_usd_bevy_twin::DocBackedTwinScenes>,
    asset_server: Res<'w, AssetServer>,
    coordinates: ActiveFrameCoordinates<'w, 's>,
}

/// Disarm the armed script tool on Cancel (Esc), like every other cursor mode.
///
/// Arming is done by the palette (a click writes the tool name); this only
/// handles the keyboard exit, so that every mode backs out on the same key —
/// which is the whole point of `CancelIntent` being a shared intent rather than
/// a `KeyCode::Escape` test per tool.
pub fn disarm_script_tool_on_cancel(
    mut armed: ResMut<lunco_core::ArmedScriptTool>,
    cancel: lunco_control_core::CancelIntent,
) {
    if armed.armed() && cancel.just_pressed() {
        armed.0 = None;
    }
}

/// Forget an armed tool that is no longer registered.
///
/// Tool libraries are hot-replaceable (`RegisterToolLibrary`, and the Twin scan
/// on open), so the armed name can outlive the tool it names. Without this the
/// palette would show nothing armed while clicks still went to a dead namespace
/// and failed one snippet at a time.
pub fn forget_missing_script_tool(mut armed: ResMut<lunco_core::ArmedScriptTool>) {
    let Some(name) = armed.0.clone() else { return };
    if !lunco_tools::has_function(&name, lunco_tools::UI_CLICK_FN) {
        warn!("[script-tool] '{name}' is no longer registered — disarming");
        armed.0 = None;
    }
}

/// Scene click while a script tool is armed: hand a generic, structured context
/// to the tool's `on_click`. Target resolution is semantic where possible, but
/// an empty-space/terrain click is still a valid context for tools that operate
/// on positions rather than entities.
pub(crate) fn on_scene_click_script_tool(
    mut click: On<Pointer<Click>>,
    armed: Res<lunco_core::ArmedScriptTool>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<lunco_control_core::EguiFocus>,
    world: SceneToolWorld,
    mut commands: Commands,
) {
    let Some(tool) = armed.0.clone() else { return };
    // Shared egui-vs-scene guard, as used by selection and placement: a click on
    // panel chrome is not a click on the world.
    if egui_focus.wants_pointer {
        return;
    }
    // `Pointer<Click>` bubbles leaf→parent→…→window. We resolve the target
    // ourselves, so stop the bubble here (this runs target-first, i.e. at the
    // picked leaf) rather than firing the tool once per ancestor.
    click.propagate(false);

    let context = scene_tool_context(
        &click,
        &keys,
        &world.q_selectable,
        &world.q_ids,
        &world.q_prim,
        &world.q_scene_roots,
        &world.q_parents,
        &world.selected,
        &world.local_avatar,
        &world.q_links,
        &world.backed,
        &world.asset_server,
        &world.coordinates,
        &world.input_bindings,
    );
    commands.trigger(lunco_scripting::commands::RunRhaiTool {
        tool,
        args: context,
    });
}

/// Build the shared typed pointer context for every scene-tool gesture. The
/// editor resolves identity and document ownership; the Rhai handler owns the
/// meaning of the gesture.
#[allow(clippy::too_many_arguments)]
fn scene_tool_context(
    click: &Pointer<Click>,
    keys: &ButtonInput<KeyCode>,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_ids: &Query<&lunco_core::GlobalEntityId>,
    q_prim: &Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_scene_roots: &Query<
        &lunco_usd_bevy_scene::UsdPrimPath,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
    q_parents: &Query<&ChildOf>,
    selected: &SelectedEntities,
    local_avatar: &TheLocalAvatar,
    q_links: &Query<&ControlLink>,
    backed: &lunco_usd_bevy_twin::DocBackedTwinScenes,
    asset_server: &AssetServer,
    coordinates: &ActiveFrameCoordinates<'_, '_>,
    input_bindings: &InputBindingsSettings,
) -> TelemetryValue {
    let mut cursor = click.entity;
    let root = loop {
        if q_selectable.contains(cursor) {
            break Some(cursor);
        }
        match q_parents.get(cursor) {
            Ok(parent) => cursor = parent.0,
            Err(_) => break None,
        }
    };

    let mut prim_paths = Vec::new();
    let mut ancestor = Some(click.entity);
    for _ in 0..32 {
        let Some(entity) = ancestor else { break };
        if let Ok(path) = q_prim.get(entity) {
            prim_paths.push(TelemetryValue::String(path.path.clone()));
        }
        ancestor = q_parents.get(entity).ok().map(|parent| parent.0);
    }

    let target_prim = root
        .and_then(|entity| q_prim.get(entity).ok())
        .or_else(|| q_prim.get(click.entity).ok());
    let selected_entity = selected.primary();
    let controlled_entity = local_avatar
        .0
        .and_then(|avatar| q_links.get(avatar).ok().map(|link| link.target));
    let context_prim = controlled_entity
        .and_then(|entity| q_prim.get(entity).ok())
        .or_else(|| selected_entity.and_then(|entity| q_prim.get(entity).ok()))
        .or(target_prim);

    let modifiers = tool_map(vec![
        (
            "alt".to_string(),
            TelemetryValue::Bool(keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])),
        ),
        (
            "shift".to_string(),
            TelemetryValue::Bool(keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])),
        ),
        (
            "ctrl".to_string(),
            TelemetryValue::Bool(keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])),
        ),
    ]);
    let button = match click.button {
        PointerButton::Primary => "primary",
        PointerButton::Secondary => "secondary",
        PointerButton::Middle => "middle",
    };
    let pointer_intents = input_bindings.pointer_intents(
        button,
        keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]),
        keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]),
        keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]),
    );
    let mut context = vec![
        (
            "button".to_string(),
            TelemetryValue::String(button.to_string()),
        ),
        (
            "pointer_intents".to_string(),
            TelemetryValue::Array(
                pointer_intents
                    .into_iter()
                    .map(TelemetryValue::String)
                    .collect(),
            ),
        ),
        (
            "screen_position".to_string(),
            TelemetryValue::Array(
                click
                    .pointer_location
                    .position
                    .to_array()
                    .into_iter()
                    .map(|value| TelemetryValue::F64(value as f64))
                    .collect(),
            ),
        ),
        ("prim_paths".to_string(), TelemetryValue::Array(prim_paths)),
        ("modifiers".to_string(), modifiers),
    ];
    if let Ok(id) = q_ids.get(click.entity) {
        context.push((
            "hit_entity_id".to_string(),
            TelemetryValue::I64(id.get() as i64),
        ));
    }
    if let Some(root) = root {
        if let Ok(id) = q_ids.get(root) {
            context.push((
                "target_entity_id".to_string(),
                TelemetryValue::I64(id.get() as i64),
            ));
        }
    }
    if let Ok(path) = q_prim.get(click.entity) {
        context.push((
            "hit_path".to_string(),
            TelemetryValue::String(path.path.clone()),
        ));
    }
    if let Some(path) = target_prim {
        context.push((
            "target_path".to_string(),
            TelemetryValue::String(path.path.clone()),
        ));
        if let Some(doc) =
            lunco_usd_bevy_twin::scene_document_for(backed, asset_server, path.stage_handle.id())
        {
            context.push(("doc_id".to_string(), TelemetryValue::I64(doc.raw() as i64)));
        }
    }
    if let Some(entity) = selected_entity {
        if let Ok(id) = q_ids.get(entity) {
            context.push((
                "selected_entity_id".to_string(),
                TelemetryValue::I64(id.get() as i64),
            ));
        }
        if let Ok(path) = q_prim.get(entity) {
            context.push((
                "selected_path".to_string(),
                TelemetryValue::String(path.path.clone()),
            ));
        }
    }
    if let Some(entity) = controlled_entity {
        if let Ok(id) = q_ids.get(entity) {
            context.push((
                "controlled_entity_id".to_string(),
                TelemetryValue::I64(id.get() as i64),
            ));
        }
        if let Ok(path) = q_prim.get(entity) {
            context.push((
                "controlled_path".to_string(),
                TelemetryValue::String(path.path.clone()),
            ));
        }
    }
    if let Some(path) = context_prim {
        if let Some(doc) =
            lunco_usd_bevy_twin::scene_document_for(backed, asset_server, path.stage_handle.id())
        {
            if !context.iter().any(|(name, _)| name == "doc_id") {
                context.push(("doc_id".to_string(), TelemetryValue::I64(doc.raw() as i64)));
            }
        }
        if let Some(scene_root) = q_scene_roots
            .iter()
            .find(|root| root.stage_handle.id() == path.stage_handle.id())
        {
            context.push((
                "scene_root_path".to_string(),
                TelemetryValue::String(scene_root.path.clone()),
            ));
        }
    }
    if let Some(position) = click.hit.position {
        let render_position = RenderPos::from_render_f32(position);
        context.push((
            "render_position".to_string(),
            coordinate_point(render_position.0, RENDER_FRAME_NAME, "pointer_hit"),
        ));
        if let Some(world_position) = coordinates.render_to_active(render_position) {
            context.push((
                "world_position".to_string(),
                coordinate_point(world_position.0, ACTIVE_FRAME_NAME, "pointer_hit"),
            ));
        } else {
            context.push((
                "position_error".to_string(),
                TelemetryValue::String("active scene coordinate frame is unavailable".to_string()),
            ));
        }
    }
    tool_map(context)
}

/// Encode a point for the Rhai coordinate contract. The values stay native
/// `TelemetryValue` data; this is not a JSON transport or an untagged vector.
fn coordinate_point(position: bevy::math::DVec3, frame: &str, source: &str) -> TelemetryValue {
    tool_map(vec![
        (
            "kind".to_string(),
            TelemetryValue::String("point3".to_string()),
        ),
        (
            "values".to_string(),
            TelemetryValue::Array(
                position
                    .to_array()
                    .into_iter()
                    .map(TelemetryValue::F64)
                    .collect(),
            ),
        ),
        (
            "frame".to_string(),
            TelemetryValue::String(frame.to_string()),
        ),
        (
            "source".to_string(),
            TelemetryValue::String(source.to_string()),
        ),
    ])
}

/// Publish one typed scene-pointer event for Rhai policy programs. The event
/// contains the same context as an armed tool, including all modifier flags;
/// no Rust code assigns meaning to Alt, Shift, or Ctrl.
#[allow(clippy::too_many_arguments)]
pub(crate) fn on_scene_pointer_event(
    click: On<Pointer<Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    armed: Res<lunco_core::ArmedScriptTool>,
    spawn_state: Res<lunco_luncosim_edit_core::SpawnState>,
    terrain_active: Res<lunco_core::TerrainToolActive>,
    egui_focus: Res<lunco_control_core::EguiFocus>,
    mut dispatch: ResMut<ScenePointerDispatch>,
    world: SceneToolWorld,
    mut commands: Commands,
) {
    if armed.armed()
        || !matches!(
            spawn_state.as_ref(),
            lunco_luncosim_edit_core::SpawnState::Idle
        )
        || terrain_active.0
        || egui_focus.wants_pointer
    {
        return;
    }
    if click.hit.position.is_none() && world.q_prim.get(click.entity).is_err() {
        return;
    }
    let key = ScenePointerKey {
        pointer: click.pointer_id,
        button: click.button,
        click_count: click.count,
        screen_position: [
            click.pointer_location.position.x.to_bits(),
            click.pointer_location.position.y.to_bits(),
        ],
    };
    if !dispatch.seen.insert(key) {
        return;
    }
    let context = scene_tool_context(
        &click,
        &keys,
        &world.q_selectable,
        &world.q_ids,
        &world.q_prim,
        &world.q_scene_roots,
        &world.q_parents,
        &world.selected,
        &world.local_avatar,
        &world.q_links,
        &world.backed,
        &world.asset_server,
        &world.coordinates,
        &world.input_bindings,
    );
    let source = world
        .q_ids
        .get(click.entity)
        .map(|id| id.get())
        .unwrap_or_default();
    for tool in lunco_tools::ui_pointer_tools() {
        commands.trigger(lunco_scripting::commands::RunRhaiToolHook {
            tool: tool.name,
            hook: "on_pointer".to_string(),
            args: context.clone(),
        });
    }
    commands.trigger(TelemetryEvent {
        name: "scene.pointer".to_string(),
        source,
        severity: lunco_core::Severity::Info,
        data: context,
        timestamp: 0.0,
    });
}
