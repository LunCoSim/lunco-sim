//! Script-authored CLICK TOOLS — the editor half of the `lunco_tools` registry.
//!
//! A tool library that exposes `on_click(context)` becomes an armable tool in
//! the Tools palette. Arm it, click in the scene, and the tool's own Rhai
//! handler receives the canonical click context. Nothing else is required:
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
//! The click is handed over as a typed `RunRhaiTool` command. It is queued and
//! run by `drain_world_scripts` with the prelude and every tool in scope, so a
//! tool handler can do anything a scenario can.

use bevy::prelude::*;
use lunco_core::TelemetryValue;

/// Build the language-neutral map passed to a script tool. The map is an
/// interaction contract, not an API serialization format; the scripting
/// backend converts it directly to the target runtime's native value.
pub(crate) fn tool_map(entries: Vec<(String, TelemetryValue)>) -> TelemetryValue {
    TelemetryValue::Map(entries.into_iter().collect())
}

pub(crate) fn tool_string(value: impl Into<String>) -> TelemetryValue {
    TelemetryValue::String(value.into())
}

pub(crate) fn tool_i64(value: u64) -> TelemetryValue {
    TelemetryValue::I64(value as i64)
}

pub(crate) fn tool_bool(value: bool) -> TelemetryValue {
    TelemetryValue::Bool(value)
}

pub(crate) fn tool_vec3(value: [f64; 3]) -> TelemetryValue {
    TelemetryValue::Array(value.into_iter().map(TelemetryValue::F64).collect())
}

/// Disarm the armed script tool on Cancel (Esc), like every other cursor mode.
///
/// Arming is done by the palette (a click writes the tool name); this only
/// handles the keyboard exit, so that every mode backs out on the same key —
/// which is the whole point of `CancelIntent` being a shared intent rather than
/// a `KeyCode::Escape` test per tool.
pub fn disarm_script_tool_on_cancel(
    mut armed: ResMut<lunco_core::ArmedScriptTool>,
    cancel: lunco_core::CancelIntent,
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
pub fn on_scene_click_script_tool(
    mut click: On<Pointer<Click>>,
    armed: Res<lunco_core::ArmedScriptTool>,
    egui_focus: Res<lunco_core::EguiFocus>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_ids: Query<&lunco_core::GlobalEntityId>,
    q_prim: Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    backed: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let Some(tool) = armed.0.clone() else { return };
    if click.button != PointerButton::Primary {
        return;
    }
    // Shared egui-vs-scene guard, as used by selection and placement: a click on
    // panel chrome is not a click on the world.
    if egui_focus.wants_pointer {
        return;
    }
    // `Pointer<Click>` bubbles leaf→parent→…→window. We resolve the target
    // ourselves, so stop the bubble here (this runs target-first, i.e. at the
    // picked leaf) rather than firing the tool once per ancestor.
    click.propagate(false);

    // The picked mesh is a wheel, a panel, a dish — walk up to the thing it
    // belongs to. A tool addresses objects, not triangles. There need not be a
    // semantic root: terrain tools can consume the hit position alone.
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
    let target_prim = root
        .and_then(|entity| q_prim.get(entity).ok())
        .or_else(|| q_prim.get(click.entity).ok());
    let mut context = vec![
        (
            "button".to_string(),
            TelemetryValue::String("primary".to_string()),
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
    ];
    if let Some(id) = q_ids.get(click.entity).ok() {
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
    if let Some(path) = q_prim.get(click.entity).ok() {
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
        if let Some(doc) = lunco_usd::twin_projection::scene_document_for(
            &backed,
            &asset_server,
            path.stage_handle.id(),
        ) {
            context.push(("doc_id".to_string(), TelemetryValue::I64(doc.raw() as i64)));
        }
    }
    if let Some(position) = click.hit.position {
        context.push((
            "world_position".to_string(),
            TelemetryValue::Array(
                position
                    .to_array()
                    .into_iter()
                    .map(|value| TelemetryValue::F64(value as f64))
                    .collect(),
            ),
        ));
    }
    commands.trigger(lunco_scripting::commands::RunRhaiTool {
        tool,
        args: tool_map(context),
    });
}
