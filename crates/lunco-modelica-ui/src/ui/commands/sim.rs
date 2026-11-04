//! Simulation-specific commands: SetModelInput.

use bevy::prelude::*;
use lunco_doc::DocumentId;

// The shared admission and application owner is UI-free and lives in
// `crate::model_commands`; this adapter sends canvas and telemetry changes
// through that same fixed-tick path.

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
        let local_producer = world
            .get_resource::<lunco_core_session::LocalSession>()
            .map(
                |local| lunco_core_session::SessionInputProducer::LocalUser {
                    session_id: local.0,
                },
            );
        let result = crate::model_commands::execute_set_model_input(
            world,
            doc,
            None,
            &name,
            value,
            None,
            None,
            lunco_command_contracts::OpId::new().0,
            local_producer,
        );
        if let Err(err) = result {
            bevy::log::warn!(
                "[CanvasDiagram] in-canvas input write failed: name={} value={} err={err:?}",
                name,
                value
            );
        }
    });
}
