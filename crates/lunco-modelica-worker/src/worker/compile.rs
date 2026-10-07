use super::*;
use lunco_modelica_runtime::CompileRequested;
use std::collections::{BTreeSet, HashMap, HashSet};

type ModelicaDocuments =
    lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>;

/// Consume compile intent in the execution layer so headless and interactive
/// hosts share one document-to-worker dispatch path.
pub fn dispatch_modelica_compile_requests(
    mut requests: MessageReader<CompileRequested>,
    mut commands: Commands,
) {
    let mut requests: Vec<_> = requests.read().cloned().collect();
    requests.sort_unstable_by_key(|request| {
        (
            request.source.document.raw(),
            request.entity.map(Entity::to_bits),
            request.class.clone(),
        )
    });
    for request in requests {
        commands.queue(move |world: &mut World| dispatch_one(world, request));
    }
}

fn dispatch_one(world: &mut World, request: CompileRequested) {
    let doc = request.source.document;
    if let Some(error) = world
        .get_resource::<lunco_modelica_runtime::ModelicaWorkerFailure>()
        .and_then(|failure| failure.0.clone())
    {
        fail_request(
            world,
            doc,
            request.entity,
            request.class.as_deref().unwrap_or_default(),
            &error,
        );
        return;
    }
    let workspace = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .map(|workspace| &workspace.0);
    let replication = lunco_core_session::current_replication_owner_in(world);
    if !request.source.is_current(workspace, replication.as_ref()) {
        fail_request(
            world,
            doc,
            request.entity,
            request.class.as_deref().unwrap_or_default(),
            "compile source runtime owner has retired or changed",
        );
        return;
    }
    if request.entity.is_some_and(|entity| {
        world.get_entity(entity).is_err() || world.get::<ModelicaModel>(entity).is_none()
    }) {
        fail_request(
            world,
            doc,
            None,
            request.class.as_deref().unwrap_or_default(),
            "compile target participant has retired",
        );
        return;
    }
    if doc.is_unassigned() {
        fail_request(
            world,
            doc,
            request.entity,
            "",
            "compile request has no document",
        );
        return;
    }
    let Some(documents) = world.get_resource::<ModelicaDocuments>() else {
        fail_request(
            world,
            doc,
            request.entity,
            "",
            "Modelica document registry is not installed",
        );
        return;
    };
    let linked = world
        .get_resource::<ModelicaDocuments>()
        .and_then(|documents| documents.simulator_for(doc));
    let target_entity = request
        .entity
        .or_else(|| linked.filter(|entity| world.get::<ModelicaModel>(*entity).is_some()));
    if target_entity
        .and_then(|entity| world.get::<lunco_workspace::PinnedDocumentRuntimeOwner>(entity))
        .is_some_and(|source| *source != request.source)
    {
        fail_request(
            world,
            doc,
            None,
            request.class.as_deref().unwrap_or_default(),
            "compile target belongs to a different admitted document runtime",
        );
        return;
    }
    if target_entity
        .and_then(|entity| world.get::<ModelicaModel>(entity))
        .is_some_and(|model| model.document != doc)
    {
        fail_request(
            world,
            doc,
            None,
            request.class.as_deref().unwrap_or_default(),
            "Modelica participant is linked to a different document",
        );
        return;
    }
    let Some(host) = documents.host(doc) else {
        fail_request(
            world,
            doc,
            request.entity,
            "",
            &format!("Modelica document {doc} is not open"),
        );
        return;
    };
    let document = host.document();
    // Editable documents publish their parsed index asynchronously. Automatic
    // requests are re-admitted on the next lifecycle pass while this revision
    // is pending; interactive callers reject the same stale snapshot before
    // creating a request.
    if document.syntax_is_stale() || document.ast_is_stale() {
        return;
    }
    let Some(ast) = document.strict_ast() else {
        fail_request(
            world,
            doc,
            request.entity,
            request.class.as_deref().unwrap_or_default(),
            "Modelica source has no valid parsed definition",
        );
        return;
    };

    let candidates = document.index().simulation_candidates();
    let model_name = match request.class.as_deref() {
        Some(class) => lunco_modelica_core::sim_target::resolve_requested_class(class, &candidates)
            .map_err(|error| format!("class `{class}` {error}")),
        None if candidates.len() == 1 || document.index().simulation_preferred_count() == 1 => {
            candidates
                .first()
                .cloned()
                .ok_or_else(|| "Modelica source contains no simulatable class".to_owned())
        }
        None => Err(format!(
            "Modelica class is ambiguous; choose one of [{}]",
            candidates.join(", ")
        )),
    };
    let model_name = match model_name {
        Ok(model_name) => model_name,
        Err(error) => {
            fail_request(
                world,
                doc,
                request.entity,
                request.class.as_deref().unwrap_or_default(),
                &error,
            );
            return;
        }
    };
    let source = lunco_modelica_source_roots::compile_overlay_source(document);
    let source_uri = document.origin().session_uri();
    let generation = document.generation_owned();
    let generated =
        lunco_modelica_runtime::generated_source::is_generated_origin(document.origin());

    let mut parameters = HashMap::new();
    for entry in document.index().components_in_class(&model_name) {
        if matches!(
            entry.variability,
            lunco_modelica_index::index::Variability::Parameter
                | lunco_modelica_index::index::Variability::Constant
        ) && let Some(value) = entry
            .binding
            .as_ref()
            .and_then(|value| value.parse::<f64>().ok())
        {
            parameters.insert(entry.name.clone(), value);
        }
    }

    let mut claimed: HashSet<String> = ast.classes.iter().map(|(name, _)| name.clone()).collect();
    let mut dependency_roots =
        lunco_modelica_ast::ast_extract::required_source_roots_from_ast(&ast);
    let mut extra_sources = Vec::new();
    let mut sibling_documents: Vec<_> = documents
        .docs()
        // A synthesized network contains its complete unit definitions and
        // admits external classes through source roots. Editor documents are
        // not dependencies of that immutable production compile snapshot.
        .filter(|(other_doc, _)| !generated && *other_doc != doc)
        .collect();
    sibling_documents.sort_unstable_by_key(|(other_doc, _)| other_doc.raw());
    for (other_doc, host) in sibling_documents {
        let other_document = host.document();
        if !request.source.shares_runtime_with(
            other_doc,
            world
                .get_resource::<lunco_workspace::WorkspaceResource>()
                .map(|workspace| &workspace.0),
            replication.as_ref(),
        ) {
            continue;
        }
        if lunco_modelica_source_roots::is_library_document(other_document) {
            continue;
        }
        if let Some(other_ast) = other_document.strict_ast() {
            let names: Vec<_> = other_ast
                .classes
                .iter()
                .map(|(name, _)| name.clone())
                .collect();
            if names.iter().any(|name| claimed.contains(name)) {
                continue;
            }
            claimed.extend(names);
            dependency_roots.extend(
                lunco_modelica_ast::ast_extract::required_source_roots_from_ast(&other_ast),
            );
        }
        extra_sources.push((
            format!("doc_{}.mo", other_doc.raw()),
            other_document.source().to_string(),
        ));
    }

    if !dependency_roots.is_empty() {
        if !world.contains_resource::<lunco_modelica_source_roots::SourceRootRegistry>() {
            fail_request(
                world,
                doc,
                request.entity,
                request.class.as_deref().unwrap_or_default(),
                "Modelica source-root registry is not installed",
            );
            return;
        }
        let admission = world.resource_scope(|world, mut roots| {
            let Some(channels) = world.get_resource::<ModelicaChannels>() else {
                return Err("Modelica worker channel is not available".to_owned());
            };
            lunco_modelica_source_roots::admit_compile_roots(&mut roots, dependency_roots, channels)
        });
        if let Err(error) = admission {
            fail_request(
                world,
                doc,
                request.entity,
                request.class.as_deref().unwrap_or_default(),
                &format!("Modelica source-root admission failed: {error}"),
            );
            return;
        }
    }

    let existing = target_entity.and_then(|entity| {
        world.get::<ModelicaModel>(entity).map(|model| {
            (
                model.session_id,
                model.inputs.clone(),
                model.communication_period_secs,
                model.is_compiled,
                model.is_compiling,
                model.compiled_generation,
                model.resume_after_compile,
            )
        })
    });
    let communication_period = existing
        .as_ref()
        .map(|(_, _, period, _, _, _, _)| *period)
        .unwrap_or(lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS);
    if let Err(error) =
        lunco_modelica_runtime::validate_communication_period_secs(communication_period)
    {
        fail_request(world, doc, target_entity, &model_name, &error);
        return;
    }
    if !request.force
        && existing.as_ref().is_some_and(
            |(_, _, _, is_compiled, is_compiling, compiled_generation, _)| {
                *is_compiled && !*is_compiling && *compiled_generation == generation
            },
        )
    {
        return;
    }

    // Retain explicit runtime values. Omitted inputs are initialized by the
    // compiled DAE, never by an index spanning multiple declaring classes.
    let inputs = existing
        .as_ref()
        .map(|(_, inputs, _, _, _, _, _)| inputs.clone())
        .unwrap_or_default();

    let previous_session = existing
        .as_ref()
        .map(|(session_id, ..)| *session_id)
        .unwrap_or(0)
        .max(
            target_entity
                .and_then(|entity| {
                    world.get::<lunco_modelica_runtime::ModelicaSessionEpoch>(entity)
                })
                .map(|epoch| epoch.0)
                .unwrap_or(0),
        );
    let Some(session_id) = previous_session.checked_add(1) else {
        fail_request(
            world,
            doc,
            target_entity,
            &model_name,
            "Modelica compile session id is exhausted",
        );
        return;
    };
    let entity = match target_entity {
        Some(entity) => entity,
        None => world
            .spawn(lunco_modelica_runtime::EditorModelicaActor)
            .id(),
    };
    world.entity_mut(entity).insert((
        request.source,
        lunco_modelica_runtime::ModelicaSessionEpoch(session_id),
    ));
    let link_error = if let Some(mut documents) = world.get_resource_mut::<ModelicaDocuments>() {
        if documents.document_of(entity) != Some(doc) {
            documents.link(entity, doc).err()
        } else {
            None
        }
    } else {
        Some(lunco_doc::DocumentError::ValidationFailed(
            "Modelica document registry is not installed".to_owned(),
        ))
    };
    if let Some(error) = link_error {
        fail_request(world, doc, Some(entity), &model_name, &error.to_string());
        return;
    }
    let resume_after_compile = request.resume_after_compile
        || existing
            .as_ref()
            .is_some_and(|(_, _, _, _, _, _, resume)| *resume);
    let model = ModelicaModel {
        model_name: model_name.clone(),
        source_uri: source_uri.clone(),
        current_time: 0.0,
        target_time: 0.0,
        communication_period_secs: communication_period,
        next_communication_time: communication_period,
        last_step_time: 0.0,
        session_id,
        paused: true,
        parameters: parameters.clone(),
        inputs,
        compiled_input_names: BTreeSet::new(),
        variables: HashMap::new(),
        last_error: None,
        document: doc,
        is_stepping: true,
        in_flight_step: None,
        last_accepted_step: None,
        next_step_id: 1,
        is_compiling: true,
        is_compiled: false,
        compiled_generation: existing
            .as_ref()
            .map_or(0, |(_, _, _, _, _, generation, _)| *generation),
        live_solver_snapshot: None,
        pending_generation: generation,
        resume_after_compile,
    };
    let mut entry = world.entity_mut(entity);
    if !entry.contains::<Name>() {
        entry.insert(Name::new(model_name.clone()));
    }
    entry.insert(model);
    if let Some(mut diagnostics) = world.get_resource_mut::<lunco_doc_bevy::DocumentDiagnostics>() {
        diagnostics.mark_started(doc);
    }
    write_notice(
        world,
        NoticeLevel::Info,
        format!("⏵ Compile started: '{model_name}'"),
    );

    let Some(channels) = world.get_resource::<ModelicaChannels>() else {
        fail_request(
            world,
            doc,
            Some(entity),
            &model_name,
            "Modelica worker channel is not available",
        );
        return;
    };
    let tx = channels.tx.clone();
    let stream = world
        .get_resource_mut::<lunco_signal::SimRegistry>()
        .map(|mut registry| registry.get_or_insert(entity));
    if tx
        .send(ModelicaCommand::Compile {
            entity,
            session_id,
            model_name: model_name.clone(),
            source,
            doc_uri: source_uri,
            extra_sources,
            parameter_overrides: Vec::new(),
            stream,
            realtime_safe: false,
        })
        .is_err()
    {
        fail_request(
            world,
            doc,
            Some(entity),
            &model_name,
            "Modelica worker channel closed; compile was not dispatched",
        );
    }
}

fn fail_request(
    world: &mut World,
    doc: lunco_doc::DocumentId,
    entity: Option<Entity>,
    model_name: &str,
    cause: &str,
) {
    let message = format!("Compile failed for '{model_name}': {cause}");
    if let Some(entity) = entity {
        if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
            model.paused = true;
            model.is_compiling = false;
            model.is_stepping = false;
            model.is_compiled = false;
            model.live_solver_snapshot = None;
            model.resume_after_compile = false;
            model.last_error = Some(message.clone());
        }
    }
    if let Some(mut diagnostics) = world.get_resource_mut::<lunco_doc_bevy::DocumentDiagnostics>() {
        diagnostics.set_error_message(doc, message.clone());
    }
    write_notice(world, NoticeLevel::Error, message.clone());
    bevy::log::error!("[Modelica] {message}");

    let active_causal = entity.is_some_and(|entity| {
        world
            .get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
            .is_some_and(|participants| participants.requires_barrier(entity))
    });
    if active_causal {
        if let (Some(entity), Some(mut faults)) = (
            entity,
            world.get_resource_mut::<lunco_core::RuntimeFaults>(),
        ) {
            faults.raise(
                "modelica-compile-dispatch-failed",
                Some(entity),
                model_name.to_owned(),
                message.clone(),
            );
        }
    }
    world
        .commands()
        .trigger(lunco_telemetry_core::TelemetryEvent {
            name: "COMPILE_DISPATCH_FAILED".into(),
            source: 0,
            severity: lunco_telemetry_core::Severity::Error,
            data: lunco_telemetry_core::TelemetryValue::String(message),
            timestamp: 0.0,
            sim_secs: 0.0,
            sim_tick: 0,
        });
}

fn write_notice(world: &mut World, level: NoticeLevel, text: String) {
    if world.contains_resource::<Messages<ModelicaNotice>>() {
        world.write_message(ModelicaNotice { level, text });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::PathlessOrigin;

    #[test]
    fn generated_compile_source_set_is_independent_of_editor_documents() {
        for generated in [true, false] {
            let mut documents = ModelicaDocuments::default();
            let source = "model Plant Real x; equation der(x) = 1; end Plant;";
            let origin = if generated {
                PathlessOrigin::bundled("generated/Plant.mo")
            } else {
                PathlessOrigin::untitled("Plant")
            };
            let doc = documents.allocate(source.to_owned(), origin);
            let other = documents.allocate(
                "model Other Real y; equation der(y) = 2; end Other;".to_owned(),
                PathlessOrigin::untitled("Other"),
            );
            for id in [doc, other] {
                documents
                    .host_mut(id)
                    .unwrap()
                    .document_mut()
                    .refresh_ast_now();
            }
            let mut app = App::new();
            app.add_message::<ModelicaNotice>()
                .init_resource::<lunco_doc_bevy::DocumentDiagnostics>()
                .insert_resource(documents);
            let (tx, rx) = crossbeam_channel::unbounded();
            let (_result_tx, result_rx) = crossbeam_channel::unbounded();
            app.insert_resource(ModelicaChannels { tx, rx: result_rx });
            let entity = app
                .world_mut()
                .spawn(ModelicaModel {
                    model_name: "Plant".to_owned(),
                    document: doc,
                    ..Default::default()
                })
                .id();
            dispatch_one(
                app.world_mut(),
                CompileRequested {
                    source: lunco_workspace::PinnedDocumentRuntimeOwner {
                        document: doc,
                        runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                    },
                    entity: Some(entity),
                    class: Some("Plant".to_owned()),
                    force: false,
                    resume_after_compile: true,
                },
            );
            let ModelicaCommand::Compile {
                source: compiled,
                extra_sources,
                doc_uri,
                ..
            } = rx.try_recv().expect("compile dispatch")
            else {
                panic!("expected compile command");
            };
            assert_eq!(compiled, source);
            if generated {
                assert_eq!(doc_uri, "generated/Plant.mo");
                assert!(
                    extra_sources.is_empty(),
                    "generated network must be self-contained"
                );
            } else {
                assert_eq!(
                    extra_sources.len(),
                    1,
                    "authored multi-document compilation remains intact"
                );
                assert!(extra_sources[0].1.contains("model Other"));
            }
        }
    }

    #[test]
    fn document_compile_dispatch_runs_without_ui_resources() {
        let source =
            "model RoverPlant\n  Real position;\nequation\n  der(position) = 1;\nend RoverPlant;\n";
        let mut documents = ModelicaDocuments::default();
        let doc = documents.allocate(source.to_owned(), PathlessOrigin::untitled("RoverPlant"));
        documents
            .host_mut(doc)
            .expect("installed document")
            .document_mut()
            .refresh_ast_now();

        let mut app = App::new();
        app.add_message::<CompileRequested>()
            .add_message::<ModelicaNotice>()
            .init_resource::<lunco_doc_bevy::DocumentDiagnostics>()
            .insert_resource(documents)
            .add_systems(Update, dispatch_modelica_compile_requests);

        let (tx_command, rx_command) = crossbeam_channel::unbounded();
        let (_tx_result, rx_result) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: tx_command,
            rx: rx_result,
        });
        let entity = app
            .world_mut()
            .spawn(ModelicaModel {
                model_name: "RoverPlant".to_owned(),
                document: doc,
                ..Default::default()
            })
            .id();
        app.world_mut()
            .resource_mut::<ModelicaDocuments>()
            .link(entity, doc)
            .expect("document link");
        app.world_mut().write_message(CompileRequested {
            source: lunco_workspace::PinnedDocumentRuntimeOwner {
                document: doc,
                runtime: lunco_workspace::DocumentRuntimeOwner::Application,
            },
            entity: Some(entity),
            class: Some("RoverPlant".to_owned()),
            force: false,
            resume_after_compile: true,
        });

        app.world_mut().run_schedule(Update);

        let command = rx_command.try_recv().expect("compile dispatched");
        match command {
            ModelicaCommand::Compile {
                entity: compile_entity,
                model_name,
                source: compile_source,
                session_id,
                ..
            } => {
                assert_eq!(compile_entity, entity);
                assert_eq!(model_name, "RoverPlant");
                assert_eq!(compile_source, source);
                assert_eq!(session_id, 1);
            }
            _ => panic!("expected Modelica Compile command"),
        }
        let model = app.world().get::<ModelicaModel>(entity).unwrap();
        assert!(model.is_compiling);
        assert!(!model.is_compiled);
        assert!(model.paused);
        assert!(model.resume_after_compile);
        assert_eq!(model.pending_generation, 1);
    }
    #[test]
    fn retired_compile_source_and_missing_target_fail_without_spawning_actor() {
        use lunco_doc::PathlessOrigin;
        let mut documents = ModelicaDocuments::default();
        let doc = documents.allocate(
            "model Plant Real x; equation der(x)=1; end Plant;".into(),
            PathlessOrigin::untitled("Plant"),
        );
        documents
            .host_mut(doc)
            .expect("fixture")
            .document_mut()
            .refresh_ast_now();
        let mut world = World::new();
        world.insert_resource(documents);
        world.insert_resource(lunco_doc_bevy::DocumentDiagnostics::default());
        world.insert_resource(Messages::<ModelicaNotice>::default());
        let (tx, rx) = crossbeam_channel::unbounded();
        let (_results, result_rx) = crossbeam_channel::unbounded();
        world.insert_resource(ModelicaChannels { tx, rx: result_rx });
        let source = lunco_workspace::PinnedDocumentRuntimeOwner {
            document: doc,
            runtime: lunco_workspace::DocumentRuntimeOwner::LocalTwin(
                lunco_workspace::TwinId::new(7),
            ),
        };
        dispatch_one(
            &mut world,
            CompileRequested {
                source,
                entity: None,
                class: Some("Plant".into()),
                force: true,
                resume_after_compile: true,
            },
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(world.resource::<Messages<ModelicaNotice>>().len(), 1);
        let target = world.spawn_empty().id();
        world.despawn(target);
        dispatch_one(
            &mut world,
            CompileRequested {
                source: lunco_workspace::PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                },
                entity: Some(target),
                class: Some("Plant".into()),
                force: true,
                resume_after_compile: true,
            },
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world
                .query::<&lunco_modelica_runtime::EditorModelicaActor>()
                .iter(&world)
                .count(),
            0
        );
        assert_eq!(world.resource::<Messages<ModelicaNotice>>().len(), 2);
    }
}
