//! Simulation-specific commands: SetModelInput.

use bevy::prelude::*;
use lunco_core::{on_command, Command};
use lunco_doc::DocumentId;

// The actual mutation (`apply_set_model_input`) + its error type are UI-free and
// live in `crate::model_commands`; UI code calls that owning module directly.

/// Apply a canvas control-widget write through the same model-input path as
/// the API command.
#[derive(Event)]
pub(crate) struct SetModelInputRequested {
    pub(crate) doc: DocumentId,
    pub(crate) name: String,
    pub(crate) value: f64,
}

pub(crate) fn on_set_model_input_requested(
    trigger: On<SetModelInputRequested>,
    mut commands: Commands,
) {
    let doc = trigger.doc;
    let name = trigger.name.clone();
    let value = trigger.value;
    commands.queue(move |world: &mut World| {
        if let Err(err) = crate::model_commands::apply_set_model_input(world, doc, &name, value) {
            bevy::log::warn!(
                "[CanvasDiagram] in-canvas input write failed: name={} value={} err={err:?}",
                name,
                value
            );
        }
    });
}

// ─── Command Structs ─────────────────────────────────────────────────────────

/// Push a runtime input value into a compiled model's stepper.
#[Command(default)]
pub struct SetModelInput {
    pub doc: DocumentId,
    pub name: String,
    pub value: f64,
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(SetModelInput)]
pub fn on_set_model_input(trigger: On<SetModelInput>, mut commands: Commands) {
    let doc_raw = trigger.event().doc;
    let name = trigger.event().name.clone();
    let value = trigger.event().value;
    commands.queue(move |world: &mut World| {
        match crate::model_commands::apply_set_model_input(world, doc_raw, &name, value) {
            Ok(_) => {}
            Err(e) => {
                bevy::log::warn!("[SetModelInput] {}", e.message());
            }
        }
    });
}
