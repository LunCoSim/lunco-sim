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
use lunco_core::{
    ActiveCommandId, Command, CommandOrigin, GlobalEntityId, on_command, register_commands,
};
use lunco_doc::DocumentId;
use lunco_modelica_runtime::ModelicaModel;

use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::ModelicaDocument;

/// Admit a live Modelica input at the next fixed simulation tick, or update an
/// editor-only model through its canonical input owner.
///
/// This command is owned by the UI-free Modelica core, so the same reflected
/// command is available to headless API hosts, the workbench, and Rhai. Its
/// live-session acknowledgement includes the target and admission stamp, and
/// its fixed-tick commit uses the same port-first helper as local UI writes.
#[Command(default)]
pub struct SetModelInput {
    /// Document id; zero selects the documented active-document default.
    pub doc_id: DocumentId,
    /// Stable live Modelica entity identity; selects a runtime participant directly.
    /// When provided, `doc_id` must be zero.
    pub target_gid: Option<GlobalEntityId>,
    /// Declared Modelica input name.
    pub name: String,
    /// Finite runtime input value; live-session inputs are admitted unchanged for their next tick.
    pub value: f64,
    /// Stable caller identity for API, direct typed, and actorless Rhai inputs.
    /// Twin Rhai calls use their actor identity and omit this field.
    pub producer_id: Option<u64>,
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
    let target_gid = cmd.target_gid;
    let name = cmd.name.clone();
    let value = cmd.value;
    let producer_id = cmd.producer_id;
    let command_id = active_id.get();
    let origin = active_id.origin();
    let request_correlation_id = pending
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let input_correlation_id = command_id
            .filter(|id| *id != 0)
            .or(request_correlation_id)
            .unwrap_or_else(|| OpId::new().0);
        let ack_result = execute_set_model_input(
            world,
            doc,
            target_gid,
            &name,
            value,
            producer_id,
            origin,
            input_correlation_id,
            None,
        );
        finish_command_result(
            world,
            command_id,
            request_correlation_id,
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
    let target_gid = cmd.target_gid;
    let name = cmd.name.clone();
    let value = cmd.value;
    let producer_id = cmd.producer_id;
    let command_id = active_id.get();
    let origin = active_id.origin();
    commands.queue(move |world: &mut World| {
        let correlation_id = command_id
            .filter(|id| *id != 0)
            .unwrap_or_else(|| OpId::new().0);
        let outcome = execute_set_model_input(
            world,
            doc,
            target_gid,
            &name,
            value,
            producer_id,
            origin,
            correlation_id,
            None,
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

/// Why the Modelica input owner could not apply the value.
#[derive(Debug, Clone)]
enum SetModelInputError {
    /// No `doc` passed and no active document to fall back to.
    NoActiveDocument,
    /// Both a document and a live entity identity were provided.
    ConflictingTargetSelectors,
    /// A live entity identity is reserved and cannot select an entity.
    InvalidTargetIdentity,
    /// The supplied stable identity does not resolve to a live entity.
    TargetNotFound {
        /// Raw stable identity.
        target: u64,
    },
    /// More than one live entity has the supplied identity.
    AmbiguousTargetIdentity {
        /// Raw stable identity.
        target: u64,
    },
    /// The selected live entity is not a Modelica participant.
    TargetMissingModel {
        /// Raw stable identity.
        target: u64,
    },
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
    /// The input name is empty.
    EmptyInputName {
        /// Raw document id.
        doc: u64,
    },
    /// The runtime value is not finite.
    NonFiniteValue {
        /// Raw document id.
        doc: u64,
        /// The rejected input name.
        name: String,
    },
    /// A declared shared port refused the write.
    PortWriteRejected {
        /// Raw document id.
        doc: u64,
        /// The rejected input name.
        name: String,
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
    fn message(&self) -> String {
        match self {
            Self::NoActiveDocument => "no active document (pass `doc` explicitly)".into(),
            Self::ConflictingTargetSelectors => {
                "SetModelInput accepts either `doc_id` or `target_gid`, not both".into()
            }
            Self::InvalidTargetIdentity => {
                "SetModelInput `target_gid` must be a nonzero stable entity identity".into()
            }
            Self::TargetNotFound { target } => {
                format!("SetModelInput target {target} does not resolve to a live entity")
            }
            Self::AmbiguousTargetIdentity { target } => {
                format!("SetModelInput target {target} resolves to multiple live entities")
            }
            Self::TargetMissingModel { target } => {
                format!("SetModelInput target {target} has no `ModelicaModel` component")
            }
            Self::NoLinkedEntity { doc } => {
                format!("doc {doc} has no linked entity — compile the model before setting inputs")
            }
            Self::EntityMissingModel { doc } => {
                format!("doc {doc}'s linked entity has no `ModelicaModel` component")
            }
            Self::EmptyInputName { doc } => {
                format!("doc {doc} Modelica input name must not be empty")
            }
            Self::NonFiniteValue { doc, name } => {
                format!("doc {doc} Modelica input `{name}` must have a finite value")
            }
            Self::PortWriteRejected { doc, name } => {
                format!("doc {doc} declared Modelica input port `{name}` rejected the write")
            }
            Self::UnknownInput {
                doc,
                name,
                model_name,
                known_inputs,
            } => format!(
                "doc {doc}: input `{name}` not declared on `{model_name}`. \
                 Known inputs: [{}]",
                known_inputs.join(", ")
            ),
        }
    }
}

fn set_model_input_ack(
    doc: DocumentId,
    name: &str,
    value: f64,
    correlation_id: u64,
    target: Option<GlobalEntityId>,
    producer: Option<lunco_core_session::SessionInputProducer>,
    admission: Option<lunco_control_core::SimulationInputOrder>,
) -> Ack {
    use lunco_hooks::HookValue;

    let admission = admission.map_or(HookValue::Unit, |admission| {
        HookValue::map([
            (
                "scene_generation",
                HookValue::UInt(admission.scene_generation),
            ),
            ("effective_tick", HookValue::UInt(admission.effective_tick)),
            ("sequence", HookValue::UInt(admission.sequence)),
        ])
    });
    Ack::with_data(
        OpId::new(),
        HookValue::map([
            ("doc_id", HookValue::UInt(doc.raw())),
            ("name", HookValue::str(name)),
            ("value", HookValue::Float(value)),
            ("correlation_id", HookValue::UInt(correlation_id)),
            (
                "target_gid",
                target.map_or(HookValue::Unit, |target| HookValue::UInt(target.get())),
            ),
            (
                "producer_kind",
                producer.map_or(HookValue::Unit, |producer| HookValue::str(producer.kind())),
            ),
            (
                "producer_id",
                producer
                    .and_then(lunco_core_session::SessionInputProducer::stable_id)
                    .map_or(HookValue::Unit, HookValue::UInt),
            ),
            ("admission", admission),
        ]),
    )
}

/// Apply editor writes immediately, while live-session writes enter the shared
/// fixed-tick admission queue. Simulation-clock Rhai writes remain derived from
/// deterministic scenario evaluation and stay in that evaluation pass.
pub fn execute_set_model_input(
    world: &mut World,
    doc_raw: DocumentId,
    target_gid: Option<GlobalEntityId>,
    name: &str,
    value: f64,
    requested_producer_id: Option<u64>,
    origin: Option<CommandOrigin>,
    correlation_id: u64,
    local_producer: Option<lunco_core_session::SessionInputProducer>,
) -> Result<Ack, String> {
    if correlation_id == 0 {
        return Err("SetModelInput correlation id must be nonzero".to_owned());
    }

    let (doc, entity) =
        resolve_model_input_target(world, doc_raw, target_gid).map_err(|error| error.message())?;
    validate_model_input_target(world, doc.raw(), entity, name, value)
        .map_err(|error| error.message())?;
    let target = world.get::<GlobalEntityId>(entity).copied();

    if let Some(target) = target.filter(|_| !deterministic_simulation_origin(origin)) {
        let scene_generation = world
            .get_resource::<lunco_core::SceneTransitionCoordinator>()
            .and_then(lunco_core::SceneTransitionCoordinator::completed_generation)
            .ok_or_else(|| {
                "SetModelInput live-session admission requires a committed scene generation"
                    .to_owned()
            })?;
        let effective_tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|tick| tick.0)
            .ok_or_else(|| "SetModelInput live-session admission requires SimTick".to_owned())?
            .checked_add(1)
            .ok_or_else(|| "SetModelInput effective simulation tick exhausted".to_owned())?;
        let producer = match local_producer {
            Some(producer) => producer,
            None if origin.is_none() && requested_producer_id.is_none() => {
                return Err(
                    "SetModelInput live-session admission requires a local session or producer_id"
                        .to_owned(),
                );
            }
            None => lunco_core_session::SessionInputProducer::from_command_origin(
                origin,
                requested_producer_id,
                "SetModelInput",
            )?,
        };
        if !world.contains_resource::<lunco_control_core::SimulationInputOrderAllocator>() {
            return Err(
                "SetModelInput live-session admission requires the shared input-order allocator"
                    .to_owned(),
            );
        }
        if !world.contains_resource::<lunco_core_session::PendingSessionInputs>() {
            return Err("SetModelInput live-session admission queue is unavailable".to_owned());
        }
        let admission = world.resource_scope(
            |world, mut pending: Mut<lunco_core_session::PendingSessionInputs>| {
                let mut order =
                    world.resource_mut::<lunco_control_core::SimulationInputOrderAllocator>();
                pending.admit(
                    &mut order,
                    producer,
                    target,
                    scene_generation,
                    effective_tick,
                    lunco_core_session::SessionInputPayload::ModelicaInputChange {
                        name: name.to_owned(),
                        value,
                        correlation_id,
                    },
                    origin,
                )
            },
        )?;
        return Ok(set_model_input_ack(
            doc,
            name,
            value,
            correlation_id,
            Some(target),
            Some(producer),
            Some(admission),
        ));
    }

    apply_model_input_to_entity(world, doc.raw(), entity, name, value)
        .map_err(|error| error.message())?;
    Ok(set_model_input_ack(
        doc,
        name,
        value,
        correlation_id,
        target,
        None,
        None,
    ))
}

fn deterministic_simulation_origin(origin: Option<CommandOrigin>) -> bool {
    matches!(
        origin,
        Some(CommandOrigin::Rhai {
            context: lunco_core::RuntimeExecutionContext {
                clock: lunco_core::RuntimeClock::Simulation,
                ..
            },
            ..
        })
    )
}

fn resolve_model_input_target(
    world: &World,
    doc_raw: DocumentId,
    target_gid: Option<GlobalEntityId>,
) -> Result<(DocumentId, Entity), SetModelInputError> {
    if let Some(target_gid) = target_gid {
        if !doc_raw.is_unassigned() {
            return Err(SetModelInputError::ConflictingTargetSelectors);
        }
        if target_gid.get() == 0 {
            return Err(SetModelInputError::InvalidTargetIdentity);
        }
        let mut matches = world
            .iter_entities()
            .filter(|entity| entity.get::<GlobalEntityId>() == Some(&target_gid))
            .map(|entity| entity.id());
        let entity = matches.next().ok_or(SetModelInputError::TargetNotFound {
            target: target_gid.get(),
        })?;
        if matches.next().is_some() {
            return Err(SetModelInputError::AmbiguousTargetIdentity {
                target: target_gid.get(),
            });
        }
        let Some(model) = world.get::<ModelicaModel>(entity) else {
            return Err(SetModelInputError::TargetMissingModel {
                target: target_gid.get(),
            });
        };
        return Ok((model.document, entity));
    }

    let doc = if doc_raw.is_unassigned() {
        world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|workspace| workspace.active_document)
            .ok_or(SetModelInputError::NoActiveDocument)?
    } else {
        doc_raw
    };
    let registry = world.resource::<DocumentRegistry<ModelicaDocument>>();
    let entity = registry
        .entities_linked_to(doc)
        .first()
        .copied()
        .ok_or(SetModelInputError::NoLinkedEntity { doc: doc.raw() })?;
    Ok((doc, entity))
}

fn validate_model_input_target(
    world: &World,
    doc_raw: u64,
    entity: Entity,
    name: &str,
    value: f64,
) -> Result<(), SetModelInputError> {
    if name.trim().is_empty() {
        return Err(SetModelInputError::EmptyInputName { doc: doc_raw });
    }
    if !value.is_finite() {
        return Err(SetModelInputError::NonFiniteValue {
            doc: doc_raw,
            name: name.to_owned(),
        });
    }
    let port_exists = world
        .get_resource::<lunco_port_core::ports::PortRegistry>()
        .is_some_and(|registry| registry.has_input_port(world, entity, name));
    if port_exists {
        return Ok(());
    }
    let Some(model) = world.get::<ModelicaModel>(entity) else {
        return Err(SetModelInputError::EntityMissingModel { doc: doc_raw });
    };
    if !model.inputs.contains_key(name) {
        return Err(SetModelInputError::UnknownInput {
            doc: doc_raw,
            name: name.to_owned(),
            model_name: model.model_name.clone(),
            known_inputs: model.inputs.keys().cloned().collect(),
        });
    }
    Ok(())
}

fn apply_model_input_to_entity(
    world: &mut World,
    doc_raw: u64,
    entity: Entity,
    name: &str,
    value: f64,
) -> Result<(), SetModelInputError> {
    validate_model_input_target(world, doc_raw, entity, name, value)?;
    // Port-first (doc 34, Decision 2). Route the write through the shared
    // `PortRegistry` so it lands in `SimComponent.inputs` — the source of truth
    // the co-sim sync (`sync_modelica_inputs`) copies into `ModelicaModel.inputs`
    // every tick. A *direct* `ModelicaModel.inputs` write would be clobbered
    // within one frame on any co-sim'd entity (wired lander, rover, …). Bare
    // workbench / batch models have no registered port, so their authoritative
    // input owner is the direct `ModelicaModel.inputs` state below (which also
    // owns the friendly `UnknownInput` validation for the no-cosim case).
    let registry = world
        .get_resource::<lunco_port_core::ports::PortRegistry>()
        .cloned();
    let has_port = registry
        .as_ref()
        .is_some_and(|registry| registry.has_input_port(world, entity, name));
    if has_port {
        if !registry.is_some_and(|registry| registry.write_port(world, entity, name, value)) {
            return Err(SetModelInputError::PortWriteRejected {
                doc: doc_raw,
                name: name.to_owned(),
            });
        }
        bevy::log::debug!(
            "[SetModelInput] doc={} {}={} (via port)",
            doc_raw,
            name,
            value
        );
        return Ok(());
    }

    let Some(mut model) = world.get_mut::<ModelicaModel>(entity) else {
        return Err(SetModelInputError::EntityMissingModel { doc: doc_raw });
    };
    if !model.inputs.contains_key(name) {
        let known: Vec<String> = model.inputs.keys().cloned().collect();
        return Err(SetModelInputError::UnknownInput {
            doc: doc_raw,
            name: name.to_string(),
            model_name: model.model_name.clone(),
            known_inputs: known,
        });
    }
    model.inputs.insert(name.to_string(), value);
    bevy::log::debug!("[SetModelInput] doc={} {}={}", doc_raw, name, value);
    Ok(())
}

/// Apply the Modelica input payload from the session's ordered fixed-tick
/// commit. Other session-input owners ignore this payload variant.
pub(crate) fn commit_session_input(
    trigger: On<lunco_core_session::SessionInputCommit>,
    mut commands: Commands,
) {
    let commit = trigger.event();
    let lunco_core_session::SessionInputPayload::ModelicaInputChange {
        name,
        value,
        correlation_id,
    } = &commit.record().payload
    else {
        return;
    };
    let target = commit.target();
    let target_gid = commit.record().target;
    let name = name.clone();
    let value = *value;
    let correlation_id = *correlation_id;
    commands.queue(move |world: &mut World| {
        let doc_raw = world
            .get_resource::<DocumentRegistry<ModelicaDocument>>()
            .and_then(|registry| registry.document_of(target))
            .map_or(0, DocumentId::raw);
        if let Err(error) = apply_model_input_to_entity(world, doc_raw, target, &name, value) {
            world.trigger(lunco_core::RuntimeError {
                name: "modelica-session-input".to_owned(),
                message: format!(
                    "SetModelInput correlation {correlation_id} for target {target_gid} failed at its admitted tick: {}",
                    error.message()
                ),
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{SetModelInputError, apply_model_input_to_entity};
    use bevy::prelude::*;
    use lunco_modelica_runtime::ModelicaModel;
    use lunco_port_core::ports::{PortBackend, PortDirection, PortRef, PortRegistry};

    #[test]
    fn declared_port_write_failure_does_not_mutate_model_snapshot() {
        fn list_declared_input(_world: &World, _entity: Entity, out: &mut Vec<PortRef>) {
            out.push(PortRef {
                name: "throttle".to_owned(),
                direction: PortDirection::In,
                value: 0.0,
            });
        }

        let mut world = World::new();
        let entity = world
            .spawn(ModelicaModel {
                model_name: "RoverDrivetrain".to_owned(),
                inputs: [("throttle".to_owned(), 0.0)].into(),
                ..Default::default()
            })
            .id();
        let mut ports = PortRegistry::default();
        ports.register(PortBackend {
            list_entities: |_world, _out| {},
            topology_key: |_world, _entity| 1,
            list: list_declared_input,
            metadata: None,
            read_output: |_world, _entity, _name| None,
            read_input: |_world, _entity, _name| None,
            write_input: |_world, _entity, _name, _value| false,
            resolve_output: None,
            resolve_input: None,
            read_slot: None,
            read_input_slot: None,
            write_slot: None,
        });
        world.insert_resource(ports);

        let error = apply_model_input_to_entity(&mut world, 17, entity, "throttle", 0.75)
            .expect_err("a declared port that rejects writes must fail visibly");

        assert!(matches!(
            error,
            SetModelInputError::PortWriteRejected { doc: 17, ref name }
                if name == "throttle"
        ));
        assert_eq!(
            world.get::<ModelicaModel>(entity).unwrap().inputs["throttle"],
            0.0,
            "the rejected port write must not fall through to ModelicaModel.inputs"
        );
    }
}
