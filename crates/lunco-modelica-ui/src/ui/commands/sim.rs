//! Simulation-specific commands: SetModelInput.

use bevy::prelude::*;
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
