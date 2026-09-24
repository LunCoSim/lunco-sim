//! Core (UI-free) Modelica command helpers shared by the egui workbench AND the
//! headless API server.
//!
//! These carry no egui — they read document/registry/runner state and mutate a
//! `ModelicaModel`. The reflected `SetModelInput` command and its shared
//! observer also live here, so every host uses the same API command path.

use bevy::prelude::*;
#[cfg(feature = "api")]
use lunco_api::executor::{PendingApiRequest, finish_command_result};
#[cfg(feature = "api")]
use lunco_api_core::ApiErrorCode;
use lunco_command_contracts::{Ack, OpId};
#[cfg(not(feature = "api"))]
use lunco_core::CommandResults;
use lunco_core::{ActiveCommandId, Command, on_command, register_commands};
use lunco_doc::DocumentId;
use lunco_modelica_runtime::ModelicaModel;

use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::ModelicaDocument;

/// Push a runtime input value into a compiled model's stepper.
///
/// This command is owned by the UI-free Modelica core, so the same reflected
/// command is available to headless API hosts, the workbench, Rhai, and any
/// future transport. Its observer queues the exclusive port/model write using
/// the same helper as the canvas path and reports the actual apply result.
#[Command(default)]
pub struct SetModelInput {
    /// Document id; zero selects the documented active-document default.
    pub doc_id: DocumentId,
    /// Declared Modelica input name.
    pub name: String,
    /// Runtime input value.
    pub value: f64,
}

#[cfg(feature = "api")]
#[on_command(SetModelInput)]
fn on_set_model_input(
    trigger: On<SetModelInput>,
    mut commands: Commands,
    active_id: Res<ActiveCommandId>,
    pending: Option<Res<PendingApiRequest>>,
) {
    let cmd = trigger.event();
    let doc = cmd.doc_id;
    let name = cmd.name.clone();
    let value = cmd.value;
    let command_id = active_id.get();
    let correlation_id = pending
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let ack_result = set_model_input_result(
            apply_set_model_input(world, doc, &name, value),
            &name,
            value,
        );
        finish_command_result(
            world,
            command_id,
            correlation_id,
            ack_result,
            ApiErrorCode::CommandRejected,
        );
    });
}

#[cfg(not(feature = "api"))]
#[on_command(SetModelInput)]
fn on_set_model_input(
    trigger: On<SetModelInput>,
    mut commands: Commands,
    active_id: Res<ActiveCommandId>,
) {
    let cmd = trigger.event();
    let doc = cmd.doc_id;
    let name = cmd.name.clone();
    let value = cmd.value;
    let command_id = active_id.get();
    commands.queue(move |world: &mut World| {
        let outcome = set_model_input_result(
            apply_set_model_input(world, doc, &name, value),
            &name,
            value,
        );
        if let Some(command_id) = command_id {
            world
                .resource_mut::<CommandResults>()
                .record(command_id, outcome);
        }
    });
}

register_commands!(on_set_model_input);

// ─── SetModelInput ───────────────────────────────────────────────────────────

/// Why [`apply_set_model_input`] could not apply the value.
#[derive(Debug, Clone)]
pub enum SetModelInputError {
    /// No `doc` passed and no active document to fall back to.
    NoActiveDocument,
    /// The document has no compiled/linked entity yet.
    NoLinkedEntity {
        /// Raw document id.
        doc: u64,
    },
    /// The linked entity is missing its `ModelicaModel` component.
    EntityMissingModel {
        /// Raw document id.
        doc: u64,
    },
    /// The named input isn't declared on the model.
    UnknownInput {
        /// Raw document id.
        doc: u64,
        /// The rejected input name.
        name: String,
        /// The model the lookup ran against.
        model_name: String,
        /// Inputs that *are* declared (for a helpful error).
        known_inputs: Vec<String>,
    },
}

impl SetModelInputError {
    /// Human-readable, API-friendly message.
    pub fn message(&self) -> String {
        match self {
            Self::NoActiveDocument => "no active document (pass `doc` explicitly)".into(),
            Self::NoLinkedEntity { doc } => {
                format!("doc {doc} has no linked entity — compile the model before setting inputs")
            }
            Self::EntityMissingModel { doc } => {
                format!("doc {doc}'s linked entity has no `ModelicaModel` component")
            }
            Self::UnknownInput {
                name,
                model_name,
                known_inputs,
                ..
            } => format!(
                "input `{name}` not declared on `{model_name}`. \
                 Known inputs: [{}]",
                known_inputs.join(", ")
            ),
        }
    }
}

fn set_model_input_result(
    result: Result<DocumentId, SetModelInputError>,
    name: &str,
    value: f64,
) -> Result<Ack, String> {
    result
        .map(|applied_doc| {
            Ack::with_data(
                OpId::new(),
                lunco_hooks::HookValue::map([
                    (
                        "doc_id",
                        lunco_hooks::HookValue::Int(applied_doc.raw() as i64),
                    ),
                    ("name", lunco_hooks::HookValue::str(name)),
                    ("value", lunco_hooks::HookValue::Float(value)),
                ]),
            )
        })
        .map_err(|error| error.message())
}

/// Push a runtime input value into a compiled model's stepper. `doc_raw`
/// unassigned selects the workspace's active document.
pub fn apply_set_model_input(
    world: &mut World,
    doc_raw: DocumentId,
    name: &str,
    value: f64,
) -> Result<DocumentId, SetModelInputError> {
    let doc = if doc_raw.is_unassigned() {
        // The documented active-document default (UI-free: reads the workspace
        // resource).
        world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document)
            .ok_or(SetModelInputError::NoActiveDocument)?
    } else {
        doc_raw
    };
    let entity = {
        let registry = world.resource::<DocumentRegistry<ModelicaDocument>>();
        let entities = registry.entities_linked_to(doc);
        match entities.first().copied() {
            Some(e) => e,
            None => return Err(SetModelInputError::NoLinkedEntity { doc: doc.raw() }),
        }
    };

    // Port-first (doc 34, Decision 2). Route the write through the shared
    // `PortRegistry` so it lands in `SimComponent.inputs` — the source of truth
    // the co-sim sync (`sync_modelica_inputs`) copies into `ModelicaModel.inputs`
    // every tick. A *direct* `ModelicaModel.inputs` write would be clobbered
    // within one frame on any co-sim'd entity (wired lander, rover, …). Bare
    // workbench / batch models have no registered port, so their authoritative
    // input owner is the direct `ModelicaModel.inputs` state below (which also
    // owns the friendly `UnknownInput` validation for the no-cosim case).
    if let Some(registry) = world
        .get_resource::<lunco_port_core::ports::PortRegistry>()
        .cloned()
    {
        if registry.write_port(world, entity, name, value) {
            bevy::log::debug!(
                "[SetModelInput] doc={} {}={} (via port)",
                doc.raw(),
                name,
                value
            );
            return Ok(doc);
        }
    }

    let Some(mut model) = world.get_mut::<ModelicaModel>(entity) else {
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
