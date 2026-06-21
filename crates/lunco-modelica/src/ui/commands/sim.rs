//! Simulation-specific commands: SetModelInput.

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_core::{Command, on_command};
use crate::state::ModelicaDocumentRegistry;

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

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SetModelInputError {
    NoActiveDocument,
    NoLinkedEntity { doc: u64 },
    EntityMissingModel { doc: u64 },
    UnknownInput {
        doc: u64,
        name: String,
        model_name: String,
        known_inputs: Vec<String>,
    },
}

impl SetModelInputError {
    pub fn message(&self) -> String {
        match self {
            Self::NoActiveDocument => "no active document (pass `doc` explicitly)".into(),
            Self::NoLinkedEntity { doc } => format!(
                "doc {doc} has no linked entity — compile the model before setting inputs"
            ),
            Self::EntityMissingModel { doc } => format!(
                "doc {doc}'s linked entity has no `ModelicaModel` component"
            ),
            Self::UnknownInput { name, model_name, known_inputs, .. } => format!(
                "input `{name}` not declared on `{model_name}`. \
                 Known inputs: [{}]",
                known_inputs.join(", ")
            ),
        }
    }
}

pub fn apply_set_model_input(
    world: &mut World,
    doc_raw: DocumentId,
    name: &str,
    value: f64,
) -> Result<DocumentId, SetModelInputError> {
    let doc = if doc_raw.is_unassigned() {
        super::resolve_active_doc(world).ok_or(SetModelInputError::NoActiveDocument)?
    } else {
        doc_raw
    };
    let registry = world.resource::<ModelicaDocumentRegistry>();
    let entities = registry.entities_linked_to(doc);
    let Some(entity) = entities.first().copied() else {
        return Err(SetModelInputError::NoLinkedEntity { doc: doc.raw() });
    };
    let Some(mut model) = world.get_mut::<crate::ModelicaModel>(entity) else {
        return Err(SetModelInputError::EntityMissingModel { doc: doc.raw() });
    };
    if !model.inputs.contains_key(name) {
        let known: Vec<String> = model.inputs.keys().cloned().collect();
        return Err(SetModelInputError::UnknownInput {
            doc: doc.raw(),
            name: name.to_string(),
            model_name: model.model_name.clone(),
            known_inputs: known,
        });
    }
    model.inputs.insert(name.to_string(), value);
    bevy::log::debug!("[SetModelInput] doc={} {}={}", doc.raw(), name, value);
    Ok(doc)
}
