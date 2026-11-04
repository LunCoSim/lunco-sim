//! Checked parameter authoring and live-session replacement in every host.

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{ActiveCommandId, on_command, register_commands};
use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::{ModelicaDocument, ModelicaOp};
use lunco_modelica_runtime::{
    ModelicaChannels, ModelicaCommand, ModelicaModel, ModelicaSessionEpoch, SetModelicaParameter,
};
use lunco_workspace::PinnedDocumentRuntimeOwner;

struct PendingParameterCommand {
    entity: Entity,
    source: PinnedDocumentRuntimeOwner,
    session: u64,
    generation: u64,
    command_id: Option<u64>,
    correlation_id: Option<u64>,
}

/// Application command replies retain exact entity, document, session and source
/// leases. At most one request exists for each busy participant. Scene teardown
/// retires only requests whose actual participant or source lease retires;
/// Application document sessions survive unrelated scene changes.
#[derive(Resource, Default)]
struct PendingParameterCommands(Vec<PendingParameterCommand>);

pub(crate) fn install(app: &mut App) {
    register_all_commands(app);
    app.init_resource::<PendingParameterCommands>()
        .add_systems(Update, finish_parameter_commands);
    #[cfg(feature = "api")]
    {
        use lunco_api::executor::DeferredCommandAppExt;
        app.register_deferred_command::<SetModelicaParameter>();
    }
}

fn report_parameter_error(world: &mut World, error: &str) {
    warn!("[SetModelicaParameter] {error}");
    world.trigger(lunco_core::RuntimeError {
        name: "modelica-parameter-edit-rejected".into(),
        message: error.to_owned(),
    });
}

fn complete(world: &mut World, request: PendingParameterCommand, result: Result<Ack, String>) {
    if let Err(error) = &result {
        report_parameter_error(world, error);
    }
    #[cfg(feature = "api")]
    lunco_api::executor::finish_command_result(
        world,
        request.command_id,
        request.correlation_id,
        result,
        lunco_api_core::ApiErrorCode::CommandRejected,
    );
    #[cfg(not(feature = "api"))]
    if let Some(id) = request.command_id {
        world
            .resource_mut::<lunco_core::CommandResults>()
            .record(id, result);
    }
}

#[on_command(SetModelicaParameter)]
fn on_set_modelica_parameter(
    trigger: On<SetModelicaParameter>,
    mut commands: Commands,
    active: Res<ActiveCommandId>,
    #[cfg(feature = "api")] pending: Option<Res<lunco_api::executor::PendingApiRequest>>,
) {
    let command = trigger.event().clone();
    let command_id = active.get();
    #[cfg(feature = "api")]
    let correlation_id = pending
        .map(|pending| pending.correlation_id)
        .filter(|id| *id != 0);
    #[cfg(not(feature = "api"))]
    let correlation_id = None;
    commands.queue(move |world: &mut World| {
        match dispatch_parameter_edit(world, &command) {
            Ok(mut request) => {
                request.command_id = command_id;
                request.correlation_id = correlation_id;
                world
                    .resource_mut::<PendingParameterCommands>()
                    .0
                    .push(request);
            }
            Err(error) => {
                // Rejections do not own a document or participant lease.
                report_parameter_error(world, &error);
                #[cfg(feature = "api")]
                lunco_api::executor::finish_command_result(
                    world,
                    command_id,
                    correlation_id,
                    Err(error),
                    lunco_api_core::ApiErrorCode::CommandRejected,
                );
                #[cfg(not(feature = "api"))]
                if let Some(id) = command_id {
                    world
                        .resource_mut::<lunco_core::CommandResults>()
                        .record(id, Err(error));
                }
            }
        }
    });
}

register_commands!(on_set_modelica_parameter);

fn dispatch_parameter_edit(
    world: &mut World,
    request: &SetModelicaParameter,
) -> Result<PendingParameterCommand, String> {
    if !request.value.is_finite() || request.key.is_empty() {
        return Err("parameter name must be nonempty and its value finite".into());
    }
    let model = world
        .get::<ModelicaModel>(request.entity)
        .ok_or("target has no Modelica participant")?;
    if model.document.is_unassigned() {
        return Err(
            "participant has no editable Modelica document; edit its authored USD parameters"
                .into(),
        );
    }
    if !model.is_compiled || model.last_error.is_some() {
        return Err("compile the current source before editing a live parameter".into());
    }
    if model.is_compiling || model.is_stepping {
        return Err("participant has an outstanding transaction; wait for its completion".into());
    }
    if !model.compiled_parameter_names.contains(&request.key)
        || !model.parameters.contains_key(&request.key)
    {
        return Err(format!(
            "'{}' is not a checked scalar parameter of '{}'",
            request.key, model.model_name
        ));
    }
    let checked = model
        .checked_interface
        .as_ref()
        .filter(|interface| interface.model_name == model.model_name)
        .ok_or("installed session has no checked parameter interface; compile the source first")?;
    let value_source = checked
        .declarations
        .get(&request.key)
        .ok_or_else(|| format!("parameter '{}' has no checked declaration", request.key))?
        .parameter_edit_source(request.value)
        .map_err(|error| format!("parameter '{}': {error}", request.key))?;
    if !crate::source_snapshot::model_is_current(world, model) {
        return Err("parameter source closure changed; compile the current sources first".into());
    }
    let mut source_snapshot = model
        .source_snapshot
        .clone()
        .ok_or("installed session has no admitted source closure")?;
    let primary_index = source_snapshot
        .documents
        .iter()
        .position(|(owner, _, _)| owner.document == model.document)
        .ok_or("admitted source closure omits the selected document")?;
    if let Some(metadata) = model
        .variable_metadata
        .as_ref()
        .and_then(|m| m.get(&request.key))
    {
        if metadata.minimum.is_some_and(|min| request.value < min)
            || metadata.maximum.is_some_and(|max| request.value > max)
        {
            return Err(format!(
                "parameter '{}' value {} is outside its checked bounds",
                request.key, request.value
            ));
        }
    }
    let doc = model.document;
    let model_name = model.model_name.clone();
    let class = model_name.clone();
    let compiled_generation = model.compiled_generation;
    let session = model
        .session_id
        .max(
            world
                .get::<ModelicaSessionEpoch>(request.entity)
                .map_or(0, |e| e.0),
        )
        .checked_add(1)
        .ok_or("Modelica session sequence exhausted")?;
    let mut initial_inputs: Vec<_> = model
        .inputs
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect();
    initial_inputs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let resume = !model.paused;
    let documents = world
        .get_resource::<DocumentRegistry<ModelicaDocument>>()
        .ok_or("Modelica document registry is unavailable")?;
    if documents.document_of(request.entity) != Some(doc) {
        return Err("participant is not linked to its selected source document".into());
    }
    let document = documents
        .host(doc)
        .ok_or("parameter source document is closed")?
        .document();
    if document.is_read_only() {
        return Err("parameter source document is read-only".into());
    }
    if document.syntax_is_stale() || document.ast_is_stale() {
        return Err(
            "parameter source analysis is pending; wait for the current source to be parsed".into(),
        );
    }
    if document.generation_owned() != compiled_generation {
        return Err(
            "parameter source differs from the installed session; compile the current source first"
                .into(),
        );
    }
    let candidates = document.index().simulation_candidates();
    let class = crate::sim_target::resolve_requested_class(
        &class,
        &candidates,
        document.index().within_path.as_deref(),
    )
    .map_err(|error| format!("selected Modelica class '{class}' {error}"))?;
    let ast = document
        .strict_ast()
        .ok_or("parameter source has no valid parsed definition")?;
    ast.get_class_by_qualified_name(&class)
        .and_then(|class| class.components.get(&request.key))
        .ok_or_else(|| format!("parameter '{}' is not directly declared in selected class '{class}'; edit its owning declaration", request.key))?;
    let workspace = world.get_resource::<lunco_workspace::WorkspaceResource>();
    let source = PinnedDocumentRuntimeOwner::for_document(doc, workspace.map(|w| &w.0))?;
    let replication = lunco_core_session::current_replication_owner_in(world);
    if !source.is_current(workspace.map(|w| &w.0), replication.as_ref())
        || world
            .get::<PinnedDocumentRuntimeOwner>(request.entity)
            .is_some_and(|owner| owner != &source)
    {
        return Err("parameter source runtime has retired or differs from the participant".into());
    }
    let channels = world
        .get_resource::<ModelicaChannels>()
        .ok_or("Modelica worker is unavailable")?;
    let tx = channels.tx.clone();
    let edit = crate::doc_ops::apply_one_op_as(
        world,
        doc,
        ModelicaOp::SetParameter {
            class,
            component: request.key.clone(),
            param: String::new(),
            value: value_source,
        },
        lunco_twin_journal::AuthorTag::local_user(),
    )
    .map_err(|error| format!("parameter source edit was rejected: {error}"))?;
    let generation = edit
        .new_gen
        .ok_or("parameter source edit did not commit synchronously")?;
    source_snapshot.documents[primary_index].1 = generation;
    let source_text = world
        .resource::<DocumentRegistry<ModelicaDocument>>()
        .host(doc)
        .ok_or("parameter source closed during its edit")?
        .document()
        .source()
        .to_owned();
    tx.send(ModelicaCommand::UpdateParameters {
        entity: request.entity,
        session_id: session,
        model_name,
        source: source_text,
        parameter_name: request.key.clone(),
        parameter_value: request.value,
        initial_inputs,
    })
    .map_err(|_| {
        "worker dispatch failed; the authored edit remains, and the live session was not replaced"
            .to_owned()
    })?;
    {
        let mut model = world
            .get_mut::<ModelicaModel>(request.entity)
            .expect("exclusive validated participant");
        model.session_id = session;
        model.parameters.insert(request.key.clone(), request.value);
        model.is_compiling = true;
        model.is_stepping = true;
        model.pending_generation = generation;
        model.source_snapshot = Some(source_snapshot);
        model.resume_after_compile = resume;
        model.pending_step_inputs = None;
        model.last_accepted_step = None;
    }
    world
        .entity_mut(request.entity)
        .insert(ModelicaSessionEpoch(session));
    Ok(PendingParameterCommand {
        entity: request.entity,
        source,
        session,
        generation,
        command_id: None,
        correlation_id: None,
    })
}

fn finish_parameter_commands(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<PendingParameterCommands>().0);
    for request in requests {
        let workspace = world.get_resource::<lunco_workspace::WorkspaceResource>();
        let replication = lunco_core_session::current_replication_owner_in(world);
        let generation = world
            .get_resource::<DocumentRegistry<ModelicaDocument>>()
            .and_then(|registry| registry.host(request.source.document))
            .map(|host| host.document().generation_owned());
        let result = if !request
            .source
            .is_current(workspace.map(|w| &w.0), replication.as_ref())
            || generation != Some(request.generation)
        {
            Some(Err(
                "parameter source retired or changed before session installation".into(),
            ))
        } else if let Some(model) = world.get::<ModelicaModel>(request.entity) {
            if model.session_id != request.session {
                Some(Err(
                    "parameter update was superseded by another session".into()
                ))
            } else if let Some(error) = &model.last_error {
                Some(Err(error.clone()))
            } else if !model.is_compiling {
                Some(
                    if model.is_compiled && model.compiled_generation == request.generation {
                        Ok(Ack {
                            new_gen: Some(request.generation),
                            ..Ack::new(OpId::new())
                        })
                    } else {
                        Err("parameter update did not install its checked source generation".into())
                    },
                )
            } else {
                None
            }
        } else {
            Some(Err(
                "parameter participant retired before installation".into()
            ))
        };
        match result {
            Some(result) => complete(world, request, result),
            None => world
                .resource_mut::<PendingParameterCommands>()
                .0
                .push(request),
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use lunco_core::{CommandOutcome, CommandResults, SceneOwned};

    #[test]
    fn application_parameter_reply_survives_unrelated_scene_retirement() {
        let mut app = App::new();
        app.init_resource::<CommandResults>()
            .init_resource::<DocumentRegistry<ModelicaDocument>>();
        install(&mut app);
        let document = app
            .world_mut()
            .resource_mut::<DocumentRegistry<ModelicaDocument>>()
            .allocate(
                "model Probe parameter Real gain=1; end Probe;".into(),
                lunco_doc::PathlessOrigin::untitled("Application parameter"),
            );
        let generation = app
            .world()
            .resource::<DocumentRegistry<ModelicaDocument>>()
            .host(document)
            .unwrap()
            .document()
            .generation_owned();
        let model = || ModelicaModel {
            document,
            session_id: 7,
            pending_generation: generation,
            is_compiled: true,
            is_compiling: true,
            is_stepping: true,
            ..Default::default()
        };
        let application = app.world_mut().spawn(model()).id();
        let scene = app.world_mut().spawn((model(), SceneOwned)).id();
        let source = PinnedDocumentRuntimeOwner::for_document(document, None).unwrap();
        for (entity, id) in [(application, 11), (scene, 12)] {
            app.world_mut()
                .resource_mut::<PendingParameterCommands>()
                .0
                .push(PendingParameterCommand {
                    entity,
                    source: source.clone(),
                    session: 7,
                    generation,
                    command_id: Some(id),
                    correlation_id: None,
                });
        }
        lunco_core::run_scene_teardown_with_entities(app.world_mut(), []);
        finish_parameter_commands(app.world_mut());
        assert!(
            app.world().resource::<CommandResults>().get(11).is_none(),
            "unrelated scene retirement cannot reject a live Application request"
        );
        assert!(
            matches!(
                app.world().resource::<CommandResults>().get(12),
                Some(CommandOutcome::Rejected(_) | CommandOutcome::Failed(_))
            ),
            "the actually retired scene participant must terminate its reply"
        );
        assert_eq!(
            app.world().resource::<PendingParameterCommands>().0.len(),
            1
        );
        {
            let mut installed = app
                .world_mut()
                .get_mut::<ModelicaModel>(application)
                .unwrap();
            installed.is_compiling = false;
            // The resumed participant can already be stepping again when its
            // parameter command reaches the application completion pass.
            installed.is_stepping = true;
            installed.compiled_generation = generation;
        }
        finish_parameter_commands(app.world_mut());
        assert!(matches!(app.world().resource::<CommandResults>().get(11),
            Some(CommandOutcome::Succeeded(ack)) if ack.new_gen == Some(generation)));
        assert!(
            app.world()
                .resource::<PendingParameterCommands>()
                .0
                .is_empty()
        );

        app.world_mut()
            .resource_mut::<PendingParameterCommands>()
            .0
            .push(PendingParameterCommand {
                entity: application,
                source,
                session: 7,
                generation,
                command_id: Some(13),
                correlation_id: None,
            });
        app.world_mut()
            .resource_mut::<DocumentRegistry<ModelicaDocument>>()
            .remove_document(document);
        finish_parameter_commands(app.world_mut());
        assert!(
            matches!(
                app.world().resource::<CommandResults>().get(13),
                Some(CommandOutcome::Rejected(_) | CommandOutcome::Failed(_))
            ),
            "closing the actual source document must terminate its reply"
        );
        assert!(
            app.world()
                .resource::<PendingParameterCommands>()
                .0
                .is_empty()
        );
    }
}
