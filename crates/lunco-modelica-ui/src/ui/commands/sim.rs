//! Simulation-specific commands: SetModelInput.

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_modelica_ui_core::SetModelicaParameter;

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

/// Apply an inspector parameter edit to both the authored Modelica document
/// and the live worker session.
pub fn on_set_modelica_parameter(trigger: On<SetModelicaParameter>, mut commands: Commands) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use lunco_modelica_core::document::ModelicaOp;
        use lunco_modelica_core::state::ModelicaDocumentRegistry;
        use lunco_modelica_runtime::{ModelicaChannels, ModelicaCommand, ModelicaModel};

        let mut session_id = 0u64;
        let mut model_name = String::new();
        if let Some(mut model) = world.get_mut::<ModelicaModel>(request.entity) {
            if let Some(slot) = model.parameters.get_mut(&request.key) {
                *slot = request.value;
            }
            model.session_id += 1;
            session_id = model.session_id;
            model.is_stepping = true;
            model_name = model.model_name.clone();
        }

        let (doc_id, class_name) = {
            let registry = world.resource::<ModelicaDocumentRegistry>();
            let doc = registry.document_of(request.entity);
            let class = doc.and_then(|doc| registry.host(doc)).and_then(|host| {
                lunco_modelica_ast::ast_extract::extract_model_name_from_ast(
                    host.document().syntax().ast(),
                )
            });
            (doc, class)
        };
        let (Some(doc_id), Some(class_name)) = (doc_id, class_name) else {
            return;
        };
        lunco_modelica_core::doc_ops::apply_ops_as(
            world,
            doc_id,
            vec![ModelicaOp::SetParameter {
                class: class_name,
                component: request.key.clone(),
                param: String::new(),
                value: request.value.to_string(),
            }],
            lunco_twin_journal::AuthorTag::local_user(),
        );
        let new_source = world
            .resource::<ModelicaDocumentRegistry>()
            .host(doc_id)
            .map(|host| host.document().source().to_string());
        if let (Some(new_source), Some(channels)) =
            (new_source, world.get_resource::<ModelicaChannels>())
        {
            let _ = channels.tx.send(ModelicaCommand::UpdateParameters {
                entity: request.entity,
                session_id,
                model_name,
                source: new_source,
            });
        }
    });
}
