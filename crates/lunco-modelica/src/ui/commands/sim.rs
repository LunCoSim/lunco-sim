//! Simulation-specific commands: SetModelInput.

use bevy::prelude::*;
use lunco_core::{on_command, Command};
use lunco_doc::DocumentId;

// The actual mutation (`apply_set_model_input`) + its error type are UI-free and
// live in `crate::model_commands` so the headless API server can call them.
// Re-exported so existing `ui::commands::{apply_set_model_input,
// SetModelInputError}` paths keep resolving.
pub use crate::model_commands::{apply_set_model_input, SetModelInputError};

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
        match apply_set_model_input(world, doc_raw, &name, value) {
            Ok(_) => {}
            Err(e) => {
                bevy::log::warn!("[SetModelInput] {}", e.message());
            }
        }
    });
}
