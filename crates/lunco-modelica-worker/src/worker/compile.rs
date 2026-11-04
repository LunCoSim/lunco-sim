use super::*;
use lunco_modelica_runtime::CompileRequested;
use std::collections::{BTreeMap, BTreeSet};

type ModelicaDocuments =
    lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>;

#[derive(Resource, Default)]
pub struct PendingCompileAdmissions(
    Vec<CompileRequested>,
    HashMap<lunco_doc::DocumentId, (lunco_workspace::PinnedDocumentRuntimeOwner, bool)>,
);

impl PendingCompileAdmissions {
    /// Record the requested run state at the same ordered boundary as compile
    /// admission. A subsequent Pause or Resume replaces this intent.
    pub fn record_run_intent(&mut self, request: &CompileRequested) {
        self.1.insert(
            request.source.document,
            (request.source.clone(), request.resume_after_compile),
        );
    }
}

/// Apply run intent to the current document owner, including a compile that has
/// been requested before its runtime participant exists.
pub fn set_modelica_document_run_intent(
    world: &mut World,
    doc: lunco_doc::DocumentId,
    running: bool,
) -> Result<(), String> {
    let documents = world
        .get_resource::<ModelicaDocuments>()
        .ok_or("Modelica document registry is unavailable")?;
    documents
        .host(doc)
        .ok_or("Modelica run-control source document is closed")?;
    let entity = documents.simulator_for(doc);
    let workspace = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .map(|workspace| &workspace.0);
    let source = lunco_workspace::PinnedDocumentRuntimeOwner::for_document(doc, workspace)?;
    let replication = lunco_core_session::current_replication_owner_in(world);
    if !source.is_current(workspace, replication.as_ref()) {
        return Err("Modelica run-control source runtime has retired".into());
    }
    if entity.is_none()
        && !world
            .get_resource::<PendingCompileAdmissions>()
            .and_then(|pending| pending.1.get(&doc))
            .is_some_and(|(admitted, _)| admitted == &source)
    {
        return Err("The document has no runtime participant or admitted compilation".into());
    }
    if let Some(entity) = entity {
        let model = world
            .get::<ModelicaModel>(entity)
            .ok_or("The document has no live Modelica model")?;
        if model.document != doc
            || world.get::<lunco_workspace::PinnedDocumentRuntimeOwner>(entity) != Some(&source)
        {
            return Err(
                "Modelica run-control participant belongs to a different source runtime".into(),
            );
        }
        let mut model = world
            .get_mut::<ModelicaModel>(entity)
            .expect("validated run-control participant");
        model.paused = !running;
        model.resume_after_compile = running;
    }
    world.init_resource::<PendingCompileAdmissions>();
    world
        .resource_mut::<PendingCompileAdmissions>()
        .1
        .insert(doc, (source, running));
    Ok(())
}

/// Completion ownership follows the exact installed session. Replacing or
/// despawning its component cancels every remaining listener.
#[derive(Component)]
pub struct PendingLifecycleCompletion {
    pub(super) session_id: u64,
    completions: Vec<lunco_modelica_runtime::ModelicaLifecycleCompletion>,
}

impl PendingLifecycleCompletion {
    pub(super) fn complete(
        &mut self,
        result: Result<lunco_modelica_runtime::ModelicaLifecycleReceipt, String>,
    ) {
        for completion in self.completions.drain(..) {
            completion.complete(result.clone());
        }
    }
}

impl Drop for PendingLifecycleCompletion {
    fn drop(&mut self) {
        self.complete(Err(
            "Modelica lifecycle session retired or superseded before completion".into(),
        ));
    }
}

/// Admit a reset of the installed document instance without changing its source
/// snapshot. Draft changes remain stale until an explicit compile admits them.
pub fn reset_modelica_document(
    world: &mut World,
    doc: lunco_doc::DocumentId,
    completion: lunco_modelica_runtime::ModelicaLifecycleCompletion,
) -> Result<(), String> {
    let documents = world
        .get_resource::<ModelicaDocuments>()
        .ok_or("Modelica document registry is unavailable")?;
    documents
        .host(doc)
        .ok_or("Modelica reset source document is closed")?;
    let entity = documents
        .simulator_for(doc)
        .ok_or("The document has no runtime participant")?;
    let model = world
        .get::<ModelicaModel>(entity)
        .ok_or("The document has no live Modelica model")?;
    if !model.is_compiled || model.last_error.is_some() {
        return Err(
            "Reset requires an installed compiled Modelica session; compile the source first"
                .into(),
        );
    }
    if model.is_compiling || model.is_stepping || model.in_flight_step.is_some() {
        return Err(
            "Reset rejected while a Modelica transaction is pending; pause and wait for completion"
                .into(),
        );
    }
    if model.document != doc
        || !model
            .checked_interface
            .as_ref()
            .is_some_and(|interface| interface.model_name == model.model_name)
    {
        return Err("Reset participant has no matching checked source target".into());
    }
    let workspace = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .map(|workspace| &workspace.0);
    let replication = lunco_core_session::current_replication_owner_in(world);
    let source = lunco_workspace::PinnedDocumentRuntimeOwner::for_document(doc, workspace)?;
    if !source.is_current(workspace, replication.as_ref())
        || world.get::<lunco_workspace::PinnedDocumentRuntimeOwner>(entity) != Some(&source)
        || !model.source_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .documents
                .iter()
                .any(|(owner, _, _)| owner == &source)
        })
    {
        return Err(
            "Reset source runtime has retired or differs from the installed session".into(),
        );
    }
    if world.get::<ModelicaVariables>(entity).is_none() {
        return Err("Reset participant is missing its required variable state".into());
    }
    if let Some(error) = world
        .get_resource::<lunco_modelica_runtime::ModelicaWorkerFailure>()
        .and_then(|failure| failure.0.as_ref())
    {
        return Err(format!("Modelica worker is unavailable: {error}"));
    }
    let target = lunco_modelica_runtime::ModelicaResetTarget {
        session_id: model.session_id,
        model_name: model.model_name.clone(),
        library_revision: model
            .live_solver_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.admitted_library_revision)
            .ok_or("Reset session has no admitted source-library revision")?,
    };
    let session_id = model
        .session_id
        .max(
            world
                .get::<lunco_modelica_runtime::ModelicaSessionEpoch>(entity)
                .map_or(0, |epoch| epoch.0),
        )
        .checked_add(1)
        .ok_or("Modelica reset session identity exhausted")?;
    let mut initial_inputs: Vec<_> = model
        .inputs
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect();
    initial_inputs.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    world
        .get_resource::<ModelicaChannels>()
        .ok_or("Modelica worker transport is unavailable")?
        .tx
        .send(ModelicaCommand::Reset {
            entity,
            session_id,
            target,
            initial_inputs,
        })
        .map_err(|error| format!("Modelica reset dispatch failed: {error}"))?;
    let mut model = world
        .get_mut::<ModelicaModel>(entity)
        .expect("exclusive validated reset participant");
    model.session_id = session_id;
    model.is_stepping = true;
    model.is_compiling = true;
    model.resume_after_compile = !model.paused;
    drop(model);
    world.entity_mut(entity).insert((
        lunco_modelica_runtime::ModelicaSessionEpoch(session_id),
        PendingLifecycleCompletion {
            session_id,
            completions: vec![completion],
        },
    ));
    Ok(())
}

/// Consume compile intent in the execution layer so headless and interactive
/// hosts share one document-to-worker dispatch path.
pub fn dispatch_modelica_compile_requests(
    mut requests: MessageReader<CompileRequested>,
    mut commands: Commands,
    pending: Option<ResMut<PendingCompileAdmissions>>,
    documents: Option<Res<ModelicaDocuments>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let mut queued = pending
        .map(|mut pending| {
            pending.1.retain(|doc, (source, _)| {
                documents
                    .as_deref()
                    .is_some_and(|documents| documents.host(*doc).is_some())
                    && source.is_current(
                        workspace.as_deref().map(|workspace| &workspace.0),
                        replication.as_ref(),
                    )
            });
            std::mem::take(&mut pending.0)
        })
        .unwrap_or_default();
    queued.extend(requests.read().cloned());
    let mut requests = queued;
    requests.dedup_by(|later, earlier| {
        if later.source == earlier.source
            && later.entity == earlier.entity
            && later.class == earlier.class
        {
            earlier.completions.append(&mut later.completions);
            earlier.force |= later.force;
            earlier.resume_after_compile = later.resume_after_compile;
            true
        } else {
            false
        }
    });
    if !requests.is_empty() {
        commands.queue(move |world: &mut World| {
            let mut sampling = None;
            for request in requests {
                dispatch_one(world, request, &mut sampling);
            }
        });
    }
}

fn dispatch_one(
    world: &mut World,
    mut request: CompileRequested,
    sampling: &mut Option<Result<(), String>>,
) {
    let doc = request.source.document;
    if let Some((source, running)) = world
        .get_resource::<PendingCompileAdmissions>()
        .and_then(|pending| pending.1.get(&doc))
        && source == &request.source
    {
        request.resume_after_compile = *running;
    }
    let workspace = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .map(|workspace| &workspace.0);
    let replication = lunco_core_session::current_replication_owner_in(world);
    // Deferred compile intent belongs to its exact source lease. Retirement
    // cancels it before diagnostics or process-wide worker failures can publish.
    if !request.source.is_current(workspace, replication.as_ref()) {
        bevy::log::debug!(document = ?doc, entity = ?request.entity,
            "[Modelica] discarded compile request from retired runtime owner");
        return;
    }
    if request.entity.is_some_and(|entity| {
        world.get_entity(entity).is_err() || world.get::<ModelicaModel>(entity).is_none()
    }) {
        bevy::log::debug!(document = ?doc, entity = ?request.entity,
            "[Modelica] discarded compile request for retired participant");
        return;
    }
    if let Some(error) = world
        .get_resource::<lunco_modelica_runtime::ModelicaWorkerFailure>()
        .and_then(|failure| failure.0.clone())
    {
        fail_request(
            &request.completions,
            world,
            doc,
            request.entity,
            request.class.as_deref().unwrap_or_default(),
            &error,
        );
        return;
    }
    if doc.is_unassigned() {
        fail_request(
            &request.completions,
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
            &request.completions,
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
            &request.completions,
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
            &request.completions,
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
            &request.completions,
            world,
            doc,
            request.entity,
            "",
            &format!("Modelica document {doc} is not open"),
        );
        return;
    };
    let document = host.document();
    let generated =
        lunco_modelica_runtime::generated_source::is_generated_origin(document.origin());
    let mut sibling_documents: Vec<_> = documents
        .docs()
        .filter(|(other_doc, host)| {
            !generated
                && *other_doc != doc
                && request
                    .source
                    .shares_runtime_with(*other_doc, workspace, replication.as_ref())
                && !lunco_modelica_source_roots::is_library_document(host.document())
        })
        .map(|(other_doc, _)| other_doc)
        .collect();
    sibling_documents.sort_unstable_by_key(|other_doc| other_doc.raw());
    // Source analysis is asynchronous. Retain this current source owner's
    // intent and completion listeners while analysis is pending, including
    // requests that already acquired the initial-input admission hold.
    // Once analysis settles, invalid source takes the terminal path below.
    if document.syntax_is_stale()
        || document.ast_is_stale()
        || sibling_documents.iter().any(|id| {
            documents.host(*id).is_some_and(|host| {
                host.document().syntax_is_stale() || host.document().ast_is_stale()
            })
        })
    {
        world.init_resource::<PendingCompileAdmissions>();
        world
            .resource_mut::<PendingCompileAdmissions>()
            .0
            .push(request);
        return;
    }
    let Some(ast) = document.strict_ast() else {
        fail_request(
            &request.completions,
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
        Some(class) => lunco_modelica_core::sim_target::resolve_requested_class(
            class,
            &candidates,
            document.index().within_path.as_deref(),
        )
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
                &request.completions,
                world,
                doc,
                request.entity,
                request.class.as_deref().unwrap_or_default(),
                &error,
            );
            return;
        }
    };
    let generation = document.generation_owned();
    let source_snapshot = match lunco_modelica_core::source_snapshot::capture(
        documents,
        &request.source,
        workspace,
        replication.as_ref(),
        world.get_resource::<lunco_modelica_source_roots::SourceRootRegistry>(),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            fail_request(
                &request.completions,
                world,
                doc,
                target_entity,
                &model_name,
                &error,
            );
            return;
        }
    };

    let existing = target_entity.and_then(|entity| {
        world.get::<ModelicaModel>(entity).map(|model| {
            (
                model.session_id,
                model.inputs.clone(),
                model.communication_period_secs,
                model.is_compiled,
                model.is_compiling,
                model.compiled_generation,
            )
        })
    });
    let communication_period = existing
        .as_ref()
        .map(|(_, _, period, _, _, _)| *period)
        .unwrap_or(lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS);
    if let Err(error) =
        lunco_modelica_runtime::validate_communication_period_secs(communication_period)
    {
        fail_request(
            &request.completions,
            world,
            doc,
            target_entity,
            &model_name,
            &error,
        );
        return;
    }
    if !request.force
        && target_entity
            .and_then(|entity| world.get::<ModelicaModel>(entity))
            .is_some_and(|model| {
                model.model_name == model_name
                    && model.source_snapshot.as_ref() == Some(&source_snapshot)
            })
        && existing.as_ref().is_some_and(
            |(_, _, _, is_compiled, is_compiling, compiled_generation)| {
                *is_compiled && !*is_compiling && *compiled_generation == generation
            },
        )
    {
        let entity = target_entity.expect("compiled participant exists");
        let model = world
            .get::<ModelicaModel>(entity)
            .expect("compiled participant exists");
        for completion in &request.completions {
            completion.complete(Ok(lunco_modelica_runtime::ModelicaLifecycleReceipt {
                entity,
                session_id: model.session_id,
                generation,
            }));
        }
        return;
    }

    if let Some(entity) = target_entity
        && world
            .get::<ModelicaModel>(entity)
            .is_some_and(|model| model.in_flight_step.is_some())
    {
        let mut model = world
            .get_mut::<ModelicaModel>(entity)
            .expect("admitted step owner");
        model.paused = true;
        model.is_compiling = true;
        model.resume_after_compile = request.resume_after_compile;
        drop(model);
        world.init_resource::<PendingCompileAdmissions>();
        world
            .resource_mut::<PendingCompileAdmissions>()
            .0
            .push(request);
        return;
    }

    let connected_inputs = if target_entity.is_some_and(|entity| {
        world
            .get::<lunco_cosim_core::UsdSourcedCosim>(entity)
            .is_some()
    }) {
        let entity = target_entity.expect("selected USD participant");
        // Physical providers sample the same admitted coordinate for this ordered
        // batch; initialization never advances their clocks or applies actuators.
        let admission = if world
            .get::<lunco_port_core::PortSurfacePending>(entity)
            .is_some()
            || world
                .get::<lunco_cosim_core::SimComponent>(entity)
                .is_none()
        {
            Ok(None)
        } else if world
            .get::<lunco_modelica_runtime::ModelicaResolvedInterface>(entity)
            .is_none()
        {
            Err("Modelica source interface has not been admitted for this participant".to_owned())
        } else {
            let sampled = sampling.get_or_insert_with(|| {
                world
                    .try_run_schedule(lunco_cosim_core::schedule::InitializationSampling)
                    .map_err(|error| format!("initial sampling schedule unavailable: {error}"))
            });
            sampled.as_ref().map_err(Clone::clone).and_then(|()| {
                lunco_cosim_core::initialization::initialization_inputs(world, entity)
            })
        };
        match admission {
            Ok(Some(inputs)) => inputs,
            Ok(None) => {
                if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
                    model.is_compiling = true;
                    model.last_error = None;
                }
                world.init_resource::<PendingCompileAdmissions>();
                world
                    .resource_mut::<PendingCompileAdmissions>()
                    .0
                    .push(request);
                return;
            }
            Err(error) => {
                fail_request(
                    &request.completions,
                    world,
                    doc,
                    Some(entity),
                    &model_name,
                    &error,
                );
                return;
            }
        }
    } else {
        Default::default()
    };

    let (expected_library_revision, parameter_overrides, authored_bindings) = if let Some(entity) =
        target_entity.filter(|entity| {
            world
                .get::<lunco_cosim_core::UsdSourcedCosim>(*entity)
                .is_some()
        }) {
        let Some(interface) =
            world.get::<lunco_modelica_runtime::ModelicaResolvedInterface>(entity)
        else {
            fail_request(
                &request.completions,
                world,
                doc,
                Some(entity),
                &model_name,
                "Modelica source interface has not been admitted for this participant",
            );
            return;
        };
        (
            Some(interface.0.library_revision),
            interface.0.parameter_overrides.clone(),
            interface.0.authored_bindings.clone(),
        )
    } else {
        (None, Vec::new(), Default::default())
    };

    // Pending inputs return before copying source text or walking dependencies.
    let Some(documents) = world.get_resource::<ModelicaDocuments>() else {
        fail_request(
            &request.completions,
            world,
            doc,
            target_entity,
            &model_name,
            "Modelica document registry retired during initial sampling",
        );
        return;
    };
    let Some(host) = documents.host(doc) else {
        fail_request(
            &request.completions,
            world,
            doc,
            target_entity,
            &model_name,
            "Modelica document retired during initial sampling",
        );
        return;
    };
    let document = host.document();
    let source = lunco_modelica_source_roots::compile_overlay_source(document);
    let source_uri = document.origin().session_uri();
    if !lunco_modelica_core::source_snapshot::is_current(
        &source_snapshot,
        documents,
        &request.source,
        world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .map(|workspace| &workspace.0),
        replication.as_ref(),
        world.get_resource::<lunco_modelica_source_roots::SourceRootRegistry>(),
    ) {
        world.init_resource::<PendingCompileAdmissions>();
        world
            .resource_mut::<PendingCompileAdmissions>()
            .0
            .push(request);
        return;
    }

    let mut claimed: BTreeMap<_, _> = ast
        .declared_class_names()
        .into_iter()
        .map(|name| (name, doc))
        .collect();
    let mut dependency_roots = rumoca_compile::compile::source_root_dependencies(&ast);
    let mut extra_sources = Vec::new();
    for other_doc in sibling_documents {
        let host = documents
            .host(other_doc)
            .expect("current participating source snapshot");
        let other_document = host.document();
        if let Some(other_ast) = other_document.strict_ast() {
            let names = other_ast.declared_class_names();
            if let Some((name, previous_doc)) = names
                .iter()
                .find_map(|name| claimed.get(name).map(|previous| (name, *previous)))
            {
                fail_request(
                    &request.completions,
                    world,
                    doc,
                    target_entity,
                    &model_name,
                    &format!(
                        "duplicate Modelica declaration '{name}' in documents {previous_doc} and {other_doc}"
                    ),
                );
                return;
            }
            claimed.extend(names.into_iter().map(|name| (name, other_doc)));
            dependency_roots.extend(rumoca_compile::compile::source_root_dependencies(
                &other_ast,
            ));
        }
        extra_sources.push((
            format!("doc_{}.mo", other_doc.raw()),
            other_document.source().to_string(),
        ));
    }

    if !dependency_roots.is_empty() {
        if !world.contains_resource::<lunco_modelica_source_roots::SourceRootRegistry>() {
            fail_request(
                &request.completions,
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
                &request.completions,
                world,
                doc,
                request.entity,
                request.class.as_deref().unwrap_or_default(),
                &format!("Modelica source-root admission failed: {error}"),
            );
            return;
        }
    }

    // Retain explicit runtime values. Omitted inputs are initialized by the
    // compiled DAE, never by an index spanning multiple declaring classes.
    // Root admission may have issued operations since the reuse check. Their
    // exact ids must travel with the compile while readiness settles.
    let source_snapshot = match lunco_modelica_core::source_snapshot::capture(
        world.resource::<ModelicaDocuments>(),
        &request.source,
        world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .map(|w| &w.0),
        replication.as_ref(),
        world.get_resource::<lunco_modelica_source_roots::SourceRootRegistry>(),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            fail_request(
                &request.completions,
                world,
                doc,
                target_entity,
                &model_name,
                &error,
            );
            return;
        }
    };
    let mut inputs = existing
        .as_ref()
        .map(|(_, inputs, _, _, _, _)| inputs.clone())
        .unwrap_or_default();

    let initial_state_values = connected_inputs.state_values;
    inputs.extend(connected_inputs.inputs);
    let initial_inputs = super::bridge::ordered_modelica_inputs(&inputs);

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
            &request.completions,
            world,
            doc,
            target_entity,
            &model_name,
            "Modelica compile session id is exhausted",
        );
        return;
    };
    if let Some(mut admission) = target_entity.and_then(|entity| {
        world.get_mut::<lunco_cosim_core::initialization::CoupledInitializationPending>(entity)
    }) {
        if admission.expected_session.is_none() {
            admission.expected_session = Some(session_id);
        }
    }
    let entity = match target_entity {
        Some(entity) => entity,
        None => world
            .spawn((
                lunco_modelica_runtime::EditorModelicaActor,
                lunco_core::Provenance::Authoritative,
            ))
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
        fail_request(
            &request.completions,
            world,
            doc,
            Some(entity),
            &model_name,
            &error.to_string(),
        );
        return;
    }
    let resume_after_compile = request.resume_after_compile;
    let model = ModelicaModel {
        model_name: model_name.clone(),
        source_uri: source_uri.clone(),
        current_time: 0.0,
        target_time: 0.0,
        communication_period_secs: communication_period,
        master_clock: Default::default(),
        communication_tick: 0,
        next_communication_time: communication_period,
        last_step_time: 0.0,
        session_id,
        paused: true,
        parameters: Default::default(),
        inputs,
        pending_step_inputs: None,
        compiled_input_names: BTreeSet::new(),
        compiled_parameter_names: BTreeSet::new(),
        checked_interface: None,
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
            .map_or(0, |(_, _, _, _, _, generation)| *generation),
        live_solver_snapshot: None,
        variable_metadata: None,
        construction_profile: None,
        pending_generation: generation,
        source_snapshot: Some(source_snapshot),
        resume_after_compile,
    };
    let mut entry = world.entity_mut(entity);
    if entry.contains::<lunco_modelica_runtime::EditorModelicaActor>() || !entry.contains::<Name>()
    {
        entry.insert(Name::new(model_name.clone()));
    }
    entry.insert((model, super::ModelicaVariables::default()));
    entry.remove::<PendingLifecycleCompletion>();
    if !request.completions.is_empty() {
        entry.insert(PendingLifecycleCompletion {
            session_id,
            completions: request.completions.clone(),
        });
    }
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
            &request.completions,
            world,
            doc,
            Some(entity),
            &model_name,
            "Modelica worker channel is not available",
        );
        return;
    };
    let tx = channels.tx.clone();
    if tx
        .send(ModelicaCommand::Compile {
            execution_domain: if world
                .get::<lunco_modelica_runtime::EditorModelicaActor>(entity)
                .is_some()
            {
                lunco_modelica_runtime::ModelicaExecutionDomain::Document
            } else {
                lunco_modelica_runtime::ModelicaExecutionDomain::Scene
            },
            entity,
            session_id,
            model_name: model_name.clone(),
            source,
            doc_uri: source_uri,
            extra_sources,
            parameter_overrides,
            authored_bindings,
            expected_library_revision,
            initial_inputs,
            initial_state_values,
            realtime_safe: false,
        })
        .is_err()
    {
        fail_request(
            &request.completions,
            world,
            doc,
            Some(entity),
            &model_name,
            "Modelica worker channel closed; compile was not dispatched",
        );
    }
}

fn fail_request(
    completions: &[lunco_modelica_runtime::ModelicaLifecycleCompletion],
    world: &mut World,
    doc: lunco_doc::DocumentId,
    entity: Option<Entity>,
    model_name: &str,
    cause: &str,
) {
    let message = format!("Compile failed for '{model_name}': {cause}");
    for completion in completions {
        completion.complete(Err(message.clone()));
    }
    if let Some(entity) = entity {
        if let Some(mut completion) = world.get_mut::<PendingLifecycleCompletion>(entity) {
            completion.complete(Err(message.clone()));
        }
        if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
            model.paused = true;
            model.is_compiling = false;
            model.is_stepping = false;
            model.is_compiled = false;
            model.live_solver_snapshot = None;
            model.variable_metadata = None;
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
    fn replacement_compile_drains_the_shared_step_before_retiring_its_session() {
        let mut documents = ModelicaDocuments::default();
        let doc = documents.allocate(
            "model Plant Real x(start=0,fixed=true); equation der(x)=1; end Plant;".into(),
            PathlessOrigin::untitled("Plant"),
        );
        documents
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .refresh_ast_now();
        let generation = documents.host(doc).unwrap().document().generation_owned();
        let source = lunco_workspace::PinnedDocumentRuntimeOwner::for_document(doc, None).unwrap();
        let snapshot =
            lunco_modelica_core::source_snapshot::capture(&documents, &source, None, None, None)
                .unwrap();
        let mut app = App::new();
        app.insert_resource(documents)
            .add_message::<CompileRequested>()
            .add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .init_resource::<lunco_core_runtime::SimulationBarrier>()
            .init_resource::<CosimLag>()
            .insert_resource(lunco_core_runtime::SimTick(3))
            .add_systems(First, super::super::handle_modelica_responses)
            .add_systems(Update, dispatch_modelica_compile_requests)
            .add_systems(FixedUpdate, super::super::spawn_modelica_requests);
        let (commands, received_commands) = crossbeam_channel::unbounded();
        let (results, received_results) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: commands,
            rx: received_results,
            preparation_rx: crossbeam_channel::never(),
        });
        let mut model = ModelicaModel {
            document: doc,
            model_name: "Plant".into(),
            session_id: 7,
            is_compiled: true,
            is_stepping: true,
            compiled_generation: generation,
            source_snapshot: Some(snapshot),
            in_flight_step: Some(InFlightModelicaStep {
                step_id: 1,
                start_time: 0.0,
                stop_time: 0.05,
                stop_tick: 3,
                sampled_inputs: Vec::new(),
                submitted_at: web_time::Instant::now(),
            }),
            ..Default::default()
        };
        for tick in 1..=3 {
            model
                .observe_master_tick(lunco_core_runtime::SimTick(tick), true)
                .unwrap();
        }
        let mut variables = ModelicaVariables::default();
        variables.values.insert("x".into(), 0.0);
        let entity = app
            .world_mut()
            .spawn((
                model,
                variables,
                source.clone(),
                lunco_modelica_runtime::ModelicaSessionEpoch(7),
            ))
            .id();
        app.world_mut()
            .resource_mut::<ModelicaDocuments>()
            .link(entity, doc)
            .unwrap();
        let mut participants = lunco_core_runtime::SimulationBarrierParticipants::default();
        participants.replace_modelica_entities([entity]);
        participants.replace([entity]);
        app.insert_resource(participants);
        let (completion, reply) = lunco_modelica_runtime::ModelicaLifecycleCompletion::channel();
        app.world_mut().write_message(CompileRequested {
            source,
            entity: Some(entity),
            class: Some("Plant".into()),
            force: true,
            resume_after_compile: false,
            completions: vec![completion],
        });
        app.world_mut().run_schedule(Update);
        let pending = app.world().get::<ModelicaModel>(entity).unwrap();
        assert!(pending.paused && pending.is_compiling && pending.is_compiled);
        assert!(pending.in_flight_step.is_some());
        assert_eq!(pending.session_id, 7);
        assert_eq!(
            app.world().get::<ModelicaVariables>(entity).unwrap().values["x"],
            0.0
        );
        assert!(received_commands.is_empty() && reply.try_take().is_none());
        app.world_mut().run_schedule(First);
        assert!(
            app.world()
                .resource::<lunco_core_runtime::SimulationBarrier>()
                .held
        );
        results
            .send(ModelicaResult {
                entity,
                session_id: 6,
                step_id: Some(1),
                new_time: 0.05,
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        app.world_mut().run_schedule(Update);
        assert!(
            app.world()
                .resource::<lunco_core_runtime::SimulationBarrier>()
                .held
        );
        assert!(received_commands.is_empty() && reply.try_take().is_none());
        results
            .send(ModelicaResult {
                entity,
                session_id: 7,
                step_id: Some(1),
                new_time: 0.05,
                outputs: vec![("x".into(), 1.0)],
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        assert_eq!(
            app.world().get::<ModelicaVariables>(entity).unwrap().values["x"],
            1.0
        );
        assert!(
            !app.world()
                .resource::<lunco_core_runtime::SimulationBarrier>()
                .held
        );
        set_modelica_document_run_intent(app.world_mut(), doc, true).unwrap();
        app.world_mut()
            .resource_mut::<lunco_core_runtime::SimTick>()
            .0 = 4;
        app.world_mut().run_schedule(FixedUpdate);
        assert!(
            received_commands.is_empty(),
            "Resume cannot admit another old-session Step"
        );
        assert_eq!(
            app.world()
                .get::<ModelicaModel>(entity)
                .unwrap()
                .master_clock
                .elapsed_ticks(),
            3
        );
        app.world_mut().run_schedule(Update);
        let command = received_commands
            .try_recv()
            .expect("settled replacement compile");
        assert!(
            matches!(command, ModelicaCommand::Compile { entity: target, session_id: 8, .. } if target == entity)
        );
        assert!(received_commands.is_empty() && reply.try_take().is_none());
        results
            .send(ModelicaResult {
                entity,
                session_id: 8,
                is_new_model: true,
                checked_interface: Some(crate::worker::test_checked_interface()),
                variable_metadata: Some(Default::default()),
                detected_symbols: vec![("x".into(), 0.0)],
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        assert_eq!(reply.try_take().unwrap().unwrap().session_id, 8);
        let installed = app.world().get::<ModelicaModel>(entity).unwrap();
        assert!(installed.is_compiled && !installed.is_compiling && !installed.paused);
    }

    #[test]
    fn compile_completion_retires_once_with_its_session_owner() {
        let (completion, reply) = lunco_modelica_runtime::ModelicaLifecycleCompletion::channel();
        let duplicate = completion.clone();
        let owner = PendingLifecycleCompletion {
            session_id: 7,
            completions: vec![completion],
        };
        drop(owner);
        duplicate.complete(Ok(lunco_modelica_runtime::ModelicaLifecycleReceipt {
            entity: Entity::from_raw_u32(1).unwrap(),
            session_id: 7,
            generation: 3,
        }));
        assert!(
            reply
                .try_take()
                .unwrap()
                .unwrap_err()
                .contains("retired or superseded")
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn paused_compile_completion_requires_accepted_exact_session() {
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .add_systems(Update, super::super::handle_modelica_responses);
        let (tx, _commands) = crossbeam_channel::unbounded();
        let (results, rx) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx,
            rx,
            preparation_rx: crossbeam_channel::never(),
        });
        let (completion, reply) = lunco_modelica_runtime::ModelicaLifecycleCompletion::channel();
        let entity = app
            .world_mut()
            .spawn((
                ModelicaModel {
                    model_name: "Plant".into(),
                    session_id: 7,
                    is_compiling: true,
                    is_stepping: true,
                    paused: true,
                    ..Default::default()
                },
                ModelicaVariables::default(),
                PendingLifecycleCompletion {
                    session_id: 7,
                    completions: vec![completion],
                },
            ))
            .id();
        for session_id in [6, 7] {
            results
                .send(ModelicaResult {
                    entity,
                    session_id,
                    is_new_model: true,
                    checked_interface: Some(crate::worker::test_checked_interface()),
                    variable_metadata: Some(Default::default()),
                    ..Default::default()
                })
                .unwrap();
            app.world_mut().run_schedule(Update);
            if session_id == 6 {
                assert!(reply.try_take().is_none());
            }
        }
        let receipt = reply.try_take().unwrap().unwrap();
        assert_eq!(receipt.entity, entity);
        assert_eq!(receipt.session_id, 7);
        let model = app.world().get::<ModelicaModel>(entity).unwrap();
        assert!(model.is_compiled && model.paused && !model.is_compiling);
    }

    #[test]
    fn pending_compile_batch_defers_sampling_and_releases_retired_requests() {
        #[derive(Resource, Default)]
        struct SamplingCount(usize);

        let mut documents = ModelicaDocuments::default();
        let doc = documents.allocate(
            "model Plant Real x; equation der(x)=1; end Plant;".into(),
            PathlessOrigin::untitled("Plant"),
        );
        documents
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .refresh_ast_now();
        let mut app = App::new();
        app.insert_resource(documents)
            .init_resource::<SamplingCount>()
            .add_message::<CompileRequested>()
            .add_message::<ModelicaNotice>()
            .add_systems(Update, dispatch_modelica_compile_requests)
            .add_systems(
                lunco_cosim_core::schedule::InitializationSampling,
                |mut count: ResMut<SamplingCount>| count.0 += 1,
            );
        let mut entities = Vec::new();
        let mut replies = Vec::new();
        for _ in 0..2 {
            let entity = app
                .world_mut()
                .spawn((
                    ModelicaModel {
                        document: doc,
                        ..Default::default()
                    },
                    lunco_cosim_core::UsdSourcedCosim,
                ))
                .id();
            entities.push(entity);
            let (completion, reply) =
                lunco_modelica_runtime::ModelicaLifecycleCompletion::channel();
            replies.push(reply);
            app.world_mut().write_message(CompileRequested {
                completions: vec![completion],
                source: lunco_workspace::PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                },
                entity: Some(entity),
                class: Some("Plant".into()),
                force: false,
                resume_after_compile: true,
            });
        }
        for _ in 0..2 {
            app.world_mut().run_schedule(Update);
            assert_eq!(app.world().resource::<SamplingCount>().0, 0);
            assert_eq!(
                app.world().resource::<PendingCompileAdmissions>().0.len(),
                2
            );
        }
        // The pending owner must retain both intents after the document is
        // edited while initial-input admission is still waiting.
        app.world_mut()
            .resource_mut::<Messages<CompileRequested>>()
            .clear();
        {
            let mut documents = app.world_mut().resource_mut::<ModelicaDocuments>();
            let host = documents.host_mut(doc).unwrap();
            host.apply(lunco_modelica_document::ModelicaOp::ReplaceSource {
                new: "model Plant Real x; equation der(x)=2; end Plant;".into(),
            })
            .unwrap();
            assert!(host.document().syntax_is_stale() || host.document().ast_is_stale());
        }
        app.world_mut().run_schedule(Update);
        assert_eq!(
            app.world().resource::<PendingCompileAdmissions>().0.len(),
            2
        );
        assert!(replies.iter().all(|reply| reply.try_take().is_none()));
        assert!(entities.iter().all(|entity| {
            app.world()
                .get::<ModelicaModel>(*entity)
                .unwrap()
                .is_compiling
        }));
        app.world_mut().despawn(entities[0]);
        app.world_mut().run_schedule(Update);
        assert_eq!(app.world().resource::<SamplingCount>().0, 0);
        assert_eq!(
            app.world().resource::<PendingCompileAdmissions>().0.len(),
            1
        );
        assert!(
            app.world()
                .resource::<Messages<ModelicaNotice>>()
                .is_empty()
        );
        assert!(replies[0].try_take().unwrap().is_err());
        assert!(replies[1].try_take().is_none());
        app.world_mut()
            .resource_mut::<ModelicaDocuments>()
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .refresh_ast_now();
        app.world_mut().run_schedule(Update);
        assert_eq!(
            app.world().resource::<PendingCompileAdmissions>().0.len(),
            1
        );
        assert!(replies[1].try_take().is_none());
        app.world_mut().despawn(entities[1]);
        app.world_mut().run_schedule(Update);
        assert!(
            app.world()
                .resource::<PendingCompileAdmissions>()
                .0
                .is_empty()
        );
        assert!(replies[1].try_take().unwrap().is_err());
    }

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
            app.insert_resource(ModelicaChannels {
                tx,
                rx: result_rx,
                preparation_rx: crossbeam_channel::never(),
            });
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
                    completions: Vec::new(),
                    source: lunco_workspace::PinnedDocumentRuntimeOwner {
                        document: doc,
                        runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                    },
                    entity: Some(entity),
                    class: Some("Plant".to_owned()),
                    force: false,
                    resume_after_compile: true,
                },
                &mut None,
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
            preparation_rx: crossbeam_channel::never(),
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
            completions: Vec::new(),
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
    fn retired_compile_requests_publish_no_failure_or_actor() {
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
        // A worker failure must not turn an already retired request into a
        // fresh diagnostic in the successor scene.
        world.insert_resource(lunco_modelica_runtime::ModelicaWorkerFailure(Some(
            "worker unavailable".into(),
        )));
        let (tx, rx) = crossbeam_channel::unbounded();
        let (_results, result_rx) = crossbeam_channel::unbounded();
        world.insert_resource(ModelicaChannels {
            tx,
            rx: result_rx,
            preparation_rx: crossbeam_channel::never(),
        });
        let source = lunco_workspace::PinnedDocumentRuntimeOwner {
            document: doc,
            runtime: lunco_workspace::DocumentRuntimeOwner::LocalTwin(
                lunco_workspace::TwinId::new(7),
            ),
        };
        dispatch_one(
            &mut world,
            CompileRequested {
                completions: Vec::new(),
                source,
                entity: None,
                class: Some("Plant".into()),
                force: true,
                resume_after_compile: true,
            },
            &mut None,
        );
        assert!(rx.try_recv().is_err());
        assert!(world.resource::<Messages<ModelicaNotice>>().is_empty());
        let target = world.spawn_empty().id();
        world.despawn(target);
        dispatch_one(
            &mut world,
            CompileRequested {
                completions: Vec::new(),
                source: lunco_workspace::PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                },
                entity: Some(target),
                class: Some("Plant".into()),
                force: true,
                resume_after_compile: true,
            },
            &mut None,
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world
                .query::<&lunco_modelica_runtime::EditorModelicaActor>()
                .iter(&world)
                .count(),
            0
        );
        assert!(world.resource::<Messages<ModelicaNotice>>().is_empty());
        let target = world
            .spawn(ModelicaModel {
                document: doc,
                ..Default::default()
            })
            .id();
        dispatch_one(
            &mut world,
            CompileRequested {
                completions: Vec::new(),
                source: lunco_workspace::PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                },
                entity: Some(target),
                class: Some("Plant".into()),
                force: true,
                resume_after_compile: true,
            },
            &mut None,
        );
        assert_eq!(
            world.resource::<Messages<ModelicaNotice>>().len(),
            1,
            "a current request still reports a genuine worker failure"
        );
        assert!(
            world
                .get::<ModelicaModel>(target)
                .unwrap()
                .last_error
                .as_ref()
                .is_some_and(|error| error.contains("worker unavailable"))
        );
    }
}
