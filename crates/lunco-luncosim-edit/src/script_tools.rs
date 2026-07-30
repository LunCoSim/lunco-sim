//! Script-authored CLICK TOOLS — the editor half of the `lunco_tools` registry.
//!
//! A tool library that exposes `on_click(entity_id)` becomes an armable tool in
//! the Tools palette. Arm it, click a thing in the scene, and the tool's own
//! rhai handler runs with the clicked entity's id. Nothing else is required:
//! there is no registration call, no palette edit, no Rust per tool. Drop
//! `assets/scripting/tools/<name>.rhai` with an `on_click` in it and the button
//! is there next launch.
//!
//! ```rhai
//! // assets/scripting/tools/recover.rhai
//! fn ui_label() { "Recover" }
//! fn ui_hint()  { "Click a stuck vessel to right it" }
//! fn on_click(id) { vessel(id); }
//! ```
//!
//! WHY DISCOVERY BY SIGNATURE. `Tool::functions()` already reports `name/arity`
//! for every registered tool (for rhai tools it comes from parsing the source),
//! so "can this be clicked?" is answerable from what the tool actually
//! implements. A separate list of palette entries could disagree with the code —
//! a button with no handler, or a handler nobody can reach. This cannot.
//!
//! The click is handed over as a `RunRhai` call into the tool's namespace. That
//! is the existing, world-aware script entry point: the snippet is queued and
//! run by `drain_world_scripts` next `FixedUpdate` with the prelude and every
//! tool in scope, so a tool handler can do anything a scenario can.

use bevy::prelude::*;

/// Build the call a click dispatches: `<tool>::on_click(<id>)`.
///
/// Split out and unit-tested because it is the one place a tool name and an
/// entity id become code. Tool names come from the registry (a file stem), not
/// from user text, so there is nothing to escape — but the shape of the call is
/// worth pinning so a rename cannot silently produce a snippet that parses and
/// does nothing.
fn click_call(tool: &str, entity_id: u64) -> String {
    format!("{tool}::on_click({entity_id})")
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

/// Scene click while a script tool is armed: hand the picked entity to the
/// tool's `on_click`.
pub fn on_scene_click_script_tool(
    mut click: On<Pointer<Click>>,
    armed: Res<lunco_core::ArmedScriptTool>,
    egui_focus: Res<lunco_core::EguiFocus>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_ids: Query<&lunco_core::GlobalEntityId>,
    q_parents: Query<&ChildOf>,
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
    // belongs to. A tool addresses objects, not triangles.
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
    // Empty space or scenery: say nothing. A tool that scolds you for missing is
    // worse than one that does nothing.
    let Some(root) = root else { return };
    let Ok(gid) = q_ids.get(root) else {
        warn!("[script-tool] {root:?} has no GlobalEntityId — cannot address it");
        return;
    };
    commands.trigger(lunco_scripting::commands::RunRhai {
        code: click_call(&tool, gid.get()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_call_targets_the_tool_namespace() {
        assert_eq!(click_call("recover", 42), "recover::on_click(42)");
    }
}
