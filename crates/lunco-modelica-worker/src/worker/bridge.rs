use super::*;

/// Tears the model down on the worker when its `ModelicaModel` component goes away, so a
/// despawned entity does not leave a `SimulationSession` alive in the worker thread.
/// Registered in `lib.rs` (`.add_observer(worker::on_remove_modelica)`).
pub fn on_remove_modelica(
    trigger: On<Remove<ModelicaModel>>,
    channels: Res<ModelicaChannels>,
    models: Query<(
        &ModelicaModel,
        Option<&lunco_modelica_runtime::ModelicaSessionEpoch>,
    )>,
    mut variables: Query<&mut ModelicaVariables>,
    step_diagnostics: Option<ResMut<lunco_modelica_runtime::ModelicaStepDiagnostics>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if let Ok(mut variables) = variables.get_mut(entity) {
        variables.values.clear();
    }
    commands.queue(move |world: &mut World| {
        if let Ok(mut owner) = world.get_entity_mut(entity) {
            if !owner.contains::<ModelicaModel>() {
                owner.remove::<ModelicaVariables>();
            }
        }
    });
    if let Ok((model, epoch)) = models.get(entity) {
        let retained = model
            .session_id
            .max(epoch.map(|epoch| epoch.0).unwrap_or(0));
        commands.queue(move |world: &mut World| {
            if let Ok(mut entity) = world.get_entity_mut(entity) {
                let current = entity
                    .get::<lunco_modelica_runtime::ModelicaSessionEpoch>()
                    .map_or(0, |epoch| epoch.0);
                entity.insert(lunco_modelica_runtime::ModelicaSessionEpoch(
                    retained.max(current),
                ));
            }
        });
    }
    if let Some(mut diagnostics) = step_diagnostics {
        diagnostics.remove(entity);
    }
    // Scene teardown can remove `ModelicaModel` as part of despawning the
    // entity. `try_remove` makes the ownership transition safe in both cases:
    // explicit component removal and an entity that has already gone away.
    commands
        .entity(entity)
        .try_remove::<lunco_signal::SignalSource>();
    let _ = channels.tx.send(ModelicaCommand::Despawn { entity });
    info!(
        "[modelica] observer: sent Despawn to Modelica for entity {:?}",
        entity
    );
}

fn reject_missing_modelica_variables(
    entity: Entity,
    model: &mut ModelicaModel,
    faults: Option<&mut lunco_core::RuntimeFaults>,
) {
    let detail = format!(
        "Modelica participant `{}` has no required ModelicaVariables state",
        model.model_name
    );
    let first_report = model.last_error.as_deref() != Some(detail.as_str());
    if !first_report {
        return;
    }
    model.paused = true;
    model.is_compiling = false;
    model.is_compiled = false;
    model.is_stepping = false;
    model.in_flight_step = None;
    model.pending_step_inputs = None;
    model.last_accepted_step = None;
    model.resume_after_compile = false;
    model.live_solver_snapshot = None;
    model.variable_metadata = None;
    model.last_error = Some(detail.clone());
    if let Some(faults) = faults {
        faults.raise(
            "modelica-variables-missing",
            Some(entity),
            model.model_name.clone(),
            detail.clone(),
        );
    }
    error!("[modelica] {detail}");
}

/// Fault live participants whose required output state was removed. Despawned
/// participants never match this query; ordinary teardown remains a close edge.
pub fn validate_modelica_variable_ownership(
    mut models: Query<
        (
            Entity,
            &mut ModelicaModel,
            Option<&lunco_core::GlobalEntityId>,
        ),
        Without<ModelicaVariables>,
    >,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    let mut missing: Vec<_> = models.iter_mut().collect();
    missing.sort_unstable_by_key(|(entity, _, gid)| {
        (gid.map(lunco_core::GlobalEntityId::get), entity.to_bits())
    });
    for (entity, mut model, _) in missing {
        reject_missing_modelica_variables(entity, &mut model, faults.as_deref_mut());
    }
}

/// Admit first-compile intent from the application lifecycle cycle.
///
/// Compilation can take place while scene admission holds `Time<Virtual>`, so
/// compile intent cannot depend on a fixed simulation tick. Solver steps remain
/// in [`spawn_modelica_requests`] and are admitted only after the simulation
/// clock resumes.
pub fn request_modelica_compiles(
    mut compile_requests: MessageWriter<CompileRequested>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut notices: MessageWriter<ModelicaNotice>,
    mut commands: Commands,
    models: Query<(
        Entity,
        &ModelicaModel,
        Option<&lunco_core::GlobalEntityId>,
        Option<&lunco_workspace::PinnedDocumentRuntimeOwner>,
    )>,
) {
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let mut models: Vec<_> = models.iter().collect();
    models.sort_unstable_by_key(|(entity, _, global_id, _)| {
        (
            global_id.map(lunco_core::GlobalEntityId::get),
            entity.to_bits(),
        )
    });

    for (entity, model, _, pinned) in models {
        if model.paused
            || model.is_compiled
            || model.is_compiling
            || model.is_stepping
            || model.document.is_unassigned()
            || model.validated_communication_period_secs().is_err()
        {
            continue;
        }

        let admission = pinned.cloned().map_or_else(|| lunco_workspace::PinnedDocumentRuntimeOwner::for_document(model.document, workspace.as_deref().map(|workspace| &workspace.0)), |source| {
            if source.document == model.document && source.is_current(workspace.as_deref().map(|workspace| &workspace.0), replication.as_ref()) { Ok(source) }
            else { Err("automatic compile participant belongs to a retired or changed document runtime".to_owned()) }
        });
        let source = match admission {
            Ok(source)
                if source.is_current(
                    workspace.as_deref().map(|workspace| &workspace.0),
                    replication.as_ref(),
                ) =>
            {
                source
            }
            Ok(_) => {
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Error,
                    text: "automatic compile source runtime has retired".into(),
                });
                continue;
            }
            Err(error) => {
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Error,
                    text: error.clone(),
                });
                commands.queue(move |world: &mut World| {
                    if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
                        model.last_error = Some(error);
                        model.paused = true;
                    }
                });
                continue;
            }
        };
        compile_requests.write(CompileRequested {
            source,
            entity: Some(entity),
            class: if model.model_name.is_empty() {
                None
            } else {
                Some(model.model_name.clone())
            },
            force: false,
            // This request is emitted only for an unpaused model. Preserve that
            // run intent while compilation temporarily pauses stepping.
            resume_after_compile: true,
        });
    }
}

/// Hold the causal simulation while an active Modelica participant is not
/// compiled for its current session and has not failed terminally.
///
/// Runs in the lifecycle cycle before the time spine. The worker compile path
/// remains live while `Time<Virtual>` is held; a successful compile result is
/// committed in `Update`, and this system releases the exact participant hold
/// on the next lifecycle pass. A terminal Modelica error releases the
/// preparation hold; the readiness owner continues to report the failure.
/// Intentionally paused models do not gate world time. Every projected active
/// model must finish preparation before consuming physics time.
pub fn reconcile_modelica_preparation_progress(
    models: Query<(
        Entity,
        &ModelicaModel,
        Option<&ModelicaVariables>,
        Option<&lunco_core::GlobalEntityId>,
    )>,
    mut removed_models: RemovedComponents<ModelicaModel>,
    participants: Option<Res<lunco_core_runtime::SimulationBarrierParticipants>>,
    progress: Option<ResMut<lunco_core_runtime::SimulationProgress>>,
) {
    let Some(mut progress) = progress else {
        return;
    };
    let mut models: Vec<_> = models.iter().collect();
    models.sort_unstable_by_key(|(entity, _, _, global_id)| {
        (
            global_id.map(lunco_core::GlobalEntityId::get),
            entity.to_bits(),
        )
    });

    for (entity, model, variables, _) in models {
        let key = lunco_core_runtime::SimulationProgressKey::modelica_participant(entity);
        let causal_participant = participants.as_deref().is_some_and(|participants| {
            participants.modelica_entities.contains(&entity)
                && participants.requires_barrier(entity)
        });
        let run_requested = !model.paused || model.resume_after_compile;
        let preparation_pending = model.last_error.is_none()
            && (!model.is_compiled
                || model.is_compiling
                || (!model.paused
                    && variables.is_none_or(|variables| variables.values.is_empty())));

        if causal_participant && run_requested && preparation_pending {
            progress.acquire(
                key,
                format!("Preparing Modelica participant `{}`", model.model_name),
            );
        } else {
            progress.release(key);
        }
    }

    for entity in removed_models.read() {
        progress.release(lunco_core_runtime::SimulationProgressKey::modelica_participant(entity));
    }
}

#[cfg(test)]
mod compile_admission_tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Resource, Default)]
    struct CapturedCompileRequests(Vec<CompileRequested>);

    fn capture_compile_requests(
        mut requests: MessageReader<CompileRequested>,
        mut captured: ResMut<CapturedCompileRequests>,
    ) {
        captured.0.extend(requests.read().cloned());
    }

    #[test]
    fn compile_intent_is_admitted_while_virtual_time_is_paused() {
        let mut app = App::new();
        app.add_message::<CompileRequested>()
            .add_message::<ModelicaNotice>()
            .init_resource::<CapturedCompileRequests>()
            .init_resource::<Time<Virtual>>()
            .add_systems(
                Update,
                (request_modelica_compiles, capture_compile_requests).chain(),
            );

        let mut model = ModelicaModel::default();
        model.document = lunco_doc::DocumentId::new(7);
        model.model_name = "RoverPlant".to_owned();
        let entity = app.world_mut().spawn(model).id();
        app.world_mut().resource_mut::<Time<Virtual>>().pause();

        app.world_mut().run_schedule(Update);

        assert!(app.world().resource::<Time<Virtual>>().is_paused());
        let requests = &app.world().resource::<CapturedCompileRequests>().0;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source.document, lunco_doc::DocumentId::new(7));
        assert_eq!(requests[0].entity, Some(entity));
        assert_eq!(requests[0].class.as_deref(), Some("RoverPlant"));
        assert!(requests[0].resume_after_compile);
    }

    #[test]
    fn modelica_step_inputs_use_stable_name_order() {
        let inputs = HashMap::from([
            ("throttle".to_owned(), 0.75),
            ("altitude".to_owned(), 12.0),
            ("temperature".to_owned(), 280.0),
        ]);

        assert_eq!(
            ordered_modelica_inputs(&inputs),
            vec![
                ("altitude".to_owned(), 12.0),
                ("temperature".to_owned(), 280.0),
                ("throttle".to_owned(), 0.75),
            ]
        );
    }

    #[test]
    fn modelica_preparation_holds_active_projected_participants() {
        use lunco_core_runtime::{SimulationProgress, SimulationProgressOwner};

        let mut app = App::new();
        app.init_resource::<SimulationProgress>()
            .init_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
            .add_systems(PreUpdate, reconcile_modelica_preparation_progress);

        let active = app
            .world_mut()
            .spawn(ModelicaModel {
                model_name: "ActivePlant".to_owned(),
                paused: false,
                is_compiled: false,
                ..Default::default()
            })
            .id();
        let paused = app
            .world_mut()
            .spawn(ModelicaModel {
                model_name: "PausedPlant".to_owned(),
                paused: true,
                is_compiled: false,
                ..Default::default()
            })
            .id();
        let unprojected = app
            .world_mut()
            .spawn(ModelicaModel {
                model_name: "UnprojectedPlant".to_owned(),
                paused: false,
                is_compiled: false,
                ..Default::default()
            })
            .id();

        let mut participants = app
            .world_mut()
            .resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>();
        participants.replace([active, paused]);
        participants.replace_modelica_entities([active, paused]);
        assert!(!participants.requires_barrier(unprojected));
        drop(participants);

        app.world_mut().run_schedule(PreUpdate);

        let progress = app.world().resource::<SimulationProgress>();
        assert!(progress.is_held());
        let blockers: Vec<_> = progress.blockers().collect();
        assert_eq!(blockers.len(), 1);
        assert_eq!(
            blockers[0].key,
            lunco_core_runtime::SimulationProgressKey::modelica_participant(active)
        );
        assert_eq!(
            blockers[0].key.owner,
            SimulationProgressOwner::ModelicaPreparation
        );

        app.world_mut()
            .get_mut::<ModelicaModel>(active)
            .unwrap()
            .is_compiled = true;
        app.world_mut().run_schedule(PreUpdate);
        assert!(app.world().resource::<SimulationProgress>().is_held());

        app.world_mut()
            .get_mut::<ModelicaVariables>(active)
            .unwrap()
            .values
            .insert("x".to_owned(), 1.0);
        app.world_mut().run_schedule(PreUpdate);
        assert!(!app.world().resource::<SimulationProgress>().is_held());
    }
}

fn ordered_modelica_inputs(inputs: &HashMap<String, f64>) -> Vec<(String, f64)> {
    let mut ordered = inputs
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect::<Vec<_>>();
    ordered.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    ordered
}

/// Locate a differing ordered sample without formatting every solver variable.
/// Full sample traces remain necessary to identify the differing values.
fn scalar_sample_fingerprint(values: &[(String, f64)]) -> u64 {
    let mut hash = lunco_hash::Fnv1a::new();
    hash.write_u64(values.len() as u64);
    for (name, value) in values {
        hash.write_u64(name.len() as u64)
            .write_bytes(name.as_bytes())
            .write_u64(value.to_bits());
    }
    hash.finish()
}

/// Sends `Step` commands for each active model — **the co-simulation master's
/// macro-step dispatch**.
///
/// Runs in [`FixedUpdate`]. Each live model's clock is driven toward
/// `target_time`, projected from admitted integer physics ticks. Model time is
/// therefore a pure function of the master clock: it does
/// not depend on the render frame rate, on GPU load, or on window focus.
///
/// Also measures the model-vs-world lag and publishes it to [`CosimLag`] — the
/// only thing in the system that compares the two clocks at all.
pub fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    master_tick: Res<lunco_core_runtime::SimTick>,
    mut q_models: Query<(
        Entity,
        &mut ModelicaModel,
        Option<&ModelicaVariables>,
        Option<&lunco_core::GlobalEntityId>,
    )>,
    suspended: Query<(), With<lunco_core::SimulationObjectPaused>>,
    mut lag: ResMut<CosimLag>,
    participants: Option<Res<lunco_core_runtime::SimulationBarrierParticipants>>,
    coupling: Option<ResMut<lunco_core_runtime::SimulationBarrier>>,
    faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    // A fault raised earlier in this fixed transaction must prevent the
    // solver from advancing with withdrawn or stale physical inputs.
    if faults
        .as_deref()
        .is_some_and(lunco_core::RuntimeFaults::active)
    {
        return;
    }
    let mut worst_secs = 0.0_f64;
    let mut worst_entity = None;
    let mut live_models = 0usize;
    let mut shared_clock_models = 0usize;
    let mut coupling_held = false;
    let mut faults = faults;

    // The worker owns one serialized command stream. Stable identity ordering
    // makes compile/step service order independent of ECS archetype layout.
    // Unaddressable local models use their world-local entity key only as a
    // tie-breaker; networked/authored participants carry GlobalEntityId.
    let mut models: Vec<_> = q_models.iter_mut().collect();
    models.sort_unstable_by_key(|(entity, _, _, global_id)| {
        (
            global_id.map(lunco_core::GlobalEntityId::get),
            entity.to_bits(),
        )
    });

    for (entity, mut model, variables, _) in models {
        if suspended.contains(entity) {
            model.paused = true;
            model.resume_after_compile = false;
        }
        if variables.is_none() {
            reject_missing_modelica_variables(entity, &mut model, faults.as_deref_mut());
            continue;
        }
        let shared_clock_participant = participants
            .as_deref()
            .is_none_or(|participants| participants.requires_barrier(entity));

        if let Err(error) = model.validated_communication_period_secs() {
            let first_report = model.last_error.as_deref() != Some(error.as_str());
            model.paused = true;
            model.is_compiled = false;
            model.is_stepping = false;
            model.live_solver_snapshot = None;
            model.variable_metadata = None;
            model.last_error = Some(error.clone());
            if first_report {
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "invalid-modelica-communication-period",
                        Some(entity),
                        model.model_name.clone(),
                        error.clone(),
                    );
                }
                error!("[modelica] {error} for `{}`", model.model_name);
            }
            continue;
        }

        // Worker completion never decides how much physical time elapsed.
        // The shared barrier prevents another master interval while a step is
        // pending; participant pause/preparation alone freezes its child clock.
        let active = !model.paused && model.is_compiled;
        if let Err(error) = model.observe_master_tick(*master_tick, active) {
            model.paused = true;
            model.last_error = Some(error.clone());
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "modelica-master-clock-invalid",
                    Some(entity),
                    model.model_name.clone(),
                    error,
                );
            }
            continue;
        }

        if model.paused {
            // A paused model's clock is frozen WITH the world's: the target does
            // not advance, so unpausing does not trigger a catch-up burst for
            // time the model was never supposed to simulate.
            continue;
        }

        // Compile intent is admitted in the application lifecycle cycle. This
        // fixed-step path only waits for the prepared stepper; it never starts
        // compilation as a side effect of consuming simulation time.
        if !model.is_compiled {
            // Don't ship a Step this tick either way — let the
            // compile flow run. The model isn't running yet, so its target
            // clock stays put (no phantom catch-up debt accrues while the
            // compile is in flight).
            continue;
        }

        // Every admitted participant synchronizes at its own communication
        // points. A slower exchange period changes those points, never the
        // authoritative clock or the requirement to commit the result first.
        if model.is_stepping {
            coupling_held = true;
            shared_clock_models += 1;
            live_models += 1;
            let lag_secs = (model.target_time - model.current_time).abs();
            if lag_secs > worst_secs {
                worst_secs = lag_secs;
                worst_entity = Some(entity);
            }
            continue;
        }

        // Inputs belong to the interval's start coordinate, including for a
        // subrate participant. Later native ticks may update `model.inputs`,
        // but cannot rewrite the sample that this interval integrates.
        if model.pending_step_inputs.is_none() {
            model.pending_step_inputs = Some(ordered_modelica_inputs(&model.inputs));
        }

        // ── Lag measurement (A3.2) ─────────────────────────────────────────
        live_models += 1;
        if shared_clock_participant {
            shared_clock_models += 1;
        }
        let lag_secs = (model.target_time - model.current_time).abs();
        if lag_secs > worst_secs {
            worst_secs = lag_secs;
            worst_entity = Some(entity);
        }

        // ── Macro step to the next declared communication point ─────────────
        let stop_tick = match model.next_communication_tick() {
            Ok(tick) => tick,
            Err(error) => {
                model.paused = true;
                model.last_error = Some(error.clone());
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "modelica-master-clock-invalid",
                        Some(entity),
                        model.model_name.clone(),
                        error,
                    );
                }
                continue;
            }
        };
        if model.master_clock.elapsed_ticks() < stop_tick {
            continue;
        }
        let start_time = lunco_core_runtime::SimTick(model.communication_tick).seconds();
        let stop_time = lunco_core_runtime::SimTick(stop_tick).seconds();
        let dt = stop_time - start_time;

        let sampled_inputs = model
            .pending_step_inputs
            .take()
            .expect("the current communication interval owns its latched input sample");

        let Some(next_step_id) = model.next_step_id.checked_add(1) else {
            let error = format!(
                "Modelica communication-point sequence exhausted for `{}`",
                model.model_name
            );
            model.paused = true;
            model.is_compiled = false;
            model.live_solver_snapshot = None;
            model.variable_metadata = None;
            model.last_error = Some(error.clone());
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "modelica-step-sequence-exhausted",
                    Some(entity),
                    model.model_name.clone(),
                    error,
                );
            }
            continue;
        };
        let step_id = model.next_step_id;
        let sent = channels.tx.send(ModelicaCommand::Step {
            entity,
            session_id: model.session_id,
            step_id,
            start_time,
            stop_time,
            model_name: model.model_name.clone(),
            inputs: sampled_inputs.clone(),
            dt,
        });
        if sent.is_ok() {
            model.next_step_id = next_step_id;
            model.in_flight_step = Some(InFlightModelicaStep {
                step_id,
                stop_tick,
                start_time,
                stop_time,
                sampled_inputs,
                submitted_at: web_time::Instant::now(),
            });
            model.is_stepping = true;
            coupling_held |= shared_clock_participant;
        } else {
            model.paused = true;
            model.is_compiled = false;
            model.live_solver_snapshot = None;
            model.variable_metadata = None;
            model.last_error = Some("Modelica worker channel closed".to_string());
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "modelica-worker-unavailable",
                    Some(entity),
                    model.model_name.clone(),
                    "the Modelica worker channel closed while dispatching a fixed-step request",
                );
            }
            error!(
                "[modelica] worker channel closed while dispatching a step for `{}`",
                model.model_name
            );
        }
    }

    lag.worst_secs = worst_secs;
    lag.worst_entity = worst_entity;
    lag.models = live_models;

    if let Some(mut coupling) = coupling {
        if faults
            .as_deref()
            .is_some_and(lunco_core::RuntimeFaults::active)
        {
            coupling_held = true;
        }
        coupling.held = coupling_held;
        coupling.active_participants = live_models;
        coupling.shared_clock_participants = shared_clock_models;
        coupling.worst_lag_secs = worst_secs;
        coupling.worst_entity = worst_entity;
    }

    // Solver latency is measured in wall time, but it never grants a second
    // simulation clock or permits crossing an unfinished communication point.
    if lag.cooldown > 0 {
        lag.cooldown -= 1;
    } else if worst_secs > LAG_WARN_SECS {
        warn!(
            "[cosim] Modelica participant is {:.3}s behind its communication point \
             (entity {:?}, {} live model(s)); the shared simulation waits \
             for the result.",
            worst_secs, worst_entity, live_models,
        );
        lag.cooldown = LAG_WARN_COOLDOWN_TICKS;
    }
}

/// Describes values that were not produced by a failed live Modelica step.
///
/// The worker reports the solver cause; the main-thread model still owns the
/// last accepted output sample. Sorting keeps this diagnostic stable across
/// runs despite the model's hash-map storage.
fn failed_modelica_step_detail(
    model: &ModelicaModel,
    variables: &ModelicaVariables,
    error: &str,
) -> String {
    let mut values = variables.values.iter().collect::<Vec<_>>();
    values.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let mut detail = format!(
        "{error}\n\nValues not produced by the failed Modelica step (communication target t={} s; last accepted sample t={} s):",
        model.next_communication_time, model.current_time
    );
    if values.is_empty() {
        detail.push_str("\n  No previously calculated Modelica values are available.");
    } else {
        for (name, value) in values {
            detail.push_str(&format!("\n  {name} = {value} (last accepted value)"));
        }
    }
    detail
}

fn include_compile_explanation(diagnostics: &mut [lunco_doc::Diagnostic], error: &str) {
    let Some((_, explanation)) = error.split_once("Why simulation did not start:") else {
        return;
    };
    let index = diagnostics
        .iter()
        .position(|diagnostic| diagnostic.severity == lunco_doc::DiagnosticSeverity::Error)
        .or_else(|| (!diagnostics.is_empty()).then_some(0));
    if let Some(diagnostic) = index.and_then(|index| diagnostics.get_mut(index)) {
        if !diagnostic.message.contains("Why simulation did not start:") {
            diagnostic
                .message
                .push_str("\n\nWhy simulation did not start: ");
            diagnostic.message.push_str(explanation);
        }
    }
}

/// System that processes results from the background worker.
///
/// Updates `ModelicaModel` components with fresh simulation outputs, handles
/// session fencing to ignore stale results. On `is_new_model`, clears old data
/// and unpauses the simulation.
pub fn handle_modelica_responses(
    channels: Res<ModelicaChannels>,
    inactive: Query<
        (),
        Or<(
            With<lunco_cosim_core::initialization::CoupledInitializationPending>,
            With<lunco_core::SimulationObjectPaused>,
        )>,
    >,
    mut worker_failure: Option<ResMut<lunco_modelica_runtime::ModelicaWorkerFailure>>,
    mut q_models: Query<(
        Entity,
        &mut ModelicaModel,
        Option<&mut ModelicaVariables>,
        Option<&lunco_workspace::PinnedDocumentRuntimeOwner>,
        Option<&lunco_modelica_runtime::ModelicaSessionEpoch>,
        Option<&lunco_core::GlobalEntityId>,
    )>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    // Core compile-state (UI-agnostic). Optional so headless cosim tests run
    // without it.
    compile_states: Option<ResMut<lunco_doc_bevy::DocumentDiagnostics>>,
    // Generated USD networks establish their document before dispatch. Keep
    // the registry available as the authoritative generation source for a
    // late-linked model too; a successful compile must never be marked stale
    // merely because its document generation was assigned after dispatch.
    documents: Option<
        Res<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
    >,
    // Lifecycle messages leave as core events; the reactive UI console observer
    // projects them. Core no longer references the console panel.
    mut notices: MessageWriter<ModelicaNotice>,
    // Presentation retires actor-scoped plot bindings after a checked session
    // installation; retained state stays owned by shared telemetry.
    mut session_starts: MessageWriter<ModelicaSessionStarted>,
    source_context: (
        Option<ResMut<lunco_modelica_source_roots::SourceRootRegistry>>,
        Option<Res<lunco_modelica_core::engine_resource::ModelicaEngineHandle>>,
    ),
    participants: Option<Res<lunco_core_runtime::SimulationBarrierParticipants>>,
    step_diagnostics: Option<ResMut<lunco_modelica_runtime::ModelicaStepDiagnostics>>,
    coupling: Option<ResMut<lunco_core_runtime::SimulationBarrier>>,
    faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    let (source_roots, authoring) = source_context;
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let mut compile_states = compile_states;
    let mut source_roots = source_roots;
    let mut faults = faults;
    let mut step_diagnostics = step_diagnostics;
    let mut transport_failure = None;
    loop {
        let mut result = match channels.rx.try_recv() {
            Ok(result) => result,
            Err(crossbeam_channel::TryRecvError::Empty) => break,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                if worker_failure
                    .as_ref()
                    .is_some_and(|failure| failure.0.is_none())
                {
                    transport_failure =
                        Some("Modelica worker result channel disconnected".to_owned());
                }
                break;
            }
        };
        if let Some(error) = result.worker_failure {
            transport_failure = Some(error);
            break;
        }
        if worker_failure
            .as_ref()
            .is_some_and(|failure| failure.0.is_some())
        {
            continue;
        }
        if let Some(root_id) = result.unloaded_source_root_id.as_ref() {
            if let (Some(handle), Some(operation)) =
                (authoring.as_deref(), result.source_root_operation_id)
            {
                handle.update_admitted_source_root_async(root_id.clone(), operation, None);
            }
            if let Some(error) = result.error.as_ref() {
                let detail = format!(
                    "Modelica source root `{root_id}` unload operation {:?} failed: {error}",
                    result.source_root_operation_id
                );
                bevy::log::error!("[source-roots] {detail}");
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Error,
                    text: detail.clone(),
                });
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "modelica-source-root-unload-failed",
                        None,
                        "Modelica source-root registry",
                        detail,
                    );
                }
            }
            continue;
        }
        // Source-root load ack: route to the registry and short-
        // circuit before any of the sim-result handling below
        // (which keys on `result.entity` — LoadSourceRoot uses
        // `Entity::PLACEHOLDER`).
        if let Some(root_id) = result.loaded_source_root_id.as_ref() {
            if let Some(roots) = source_roots.as_deref_mut() {
                if let Some(operation_id) = result.source_root_operation_id {
                    if roots.complete_load(
                        root_id,
                        operation_id,
                        result.loaded_source_root_namespaces.clone(),
                        result.loaded_source_root_sources.clone(),
                        result.error.clone(),
                    ) {
                        if let Some(handle) = authoring.as_deref() {
                            let files = result
                                .error
                                .is_none()
                                .then(|| std::mem::take(&mut result.loaded_source_root_files));
                            handle.update_admitted_source_root_async(
                                root_id.clone(),
                                operation_id,
                                files,
                            );
                        }
                        match roots.state(root_id) {
                            Some(lunco_modelica_source_roots::LoadState::Ready) => {
                                bevy::log::info!(
                                    "[source-roots] `{}` is now Ready namespaces={:?}",
                                    root_id,
                                    result.loaded_source_root_namespaces
                                );
                            }
                            Some(lunco_modelica_source_roots::LoadState::Failed(error)) => {
                                bevy::log::warn!(
                                    "[source-roots] `{}` load failed: {}",
                                    root_id,
                                    error
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Status-bar projection of this load result is handled by the
            // reactive UI observer of `SourceRootRegistry` — core only sets the
            // registry state above.
            continue;
        }

        if result.entity == Entity::PLACEHOLDER {
            let msg = "Simulation worker crashed and restarted.";
            warn!("{msg}");
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: msg.to_string(),
            });
            continue;
        }

        let lifecycle_result = result.is_new_model || result.is_parameter_update || result.is_reset;
        // Restoring a component cannot revive a faulted participant through an
        // already queued worker result. Scene teardown retires this fault.
        if faults
            .as_deref()
            .and_then(|faults| faults.first.as_ref())
            .is_some_and(|fault| {
                fault.kind == "modelica-variables-missing" && fault.entity == Some(result.entity)
            })
        {
            continue;
        }
        if let Ok((_, mut model, variables, source, epoch, global_id)) =
            q_models.get_mut(result.entity)
        {
            // Terminal participants have no pending lifecycle operation. This
            // also protects participants beyond the first recorded scene fault.
            if lifecycle_result
                && model.last_error.is_some()
                && !model.is_compiling
                && !model.is_stepping
            {
                continue;
            }
            if source.is_some_and(|source| {
                !source.is_current(
                    workspace.as_deref().map(|workspace| &workspace.0),
                    replication.as_ref(),
                )
            }) || epoch.is_some_and(|epoch| result.session_id < epoch.0)
            {
                continue;
            }
            // ALWAYS check session ID before resetting is_stepping
            // Stale results must NOT reset the flag.
            if result.session_id < model.session_id {
                if lifecycle_result {
                    warn!(
                        "[Modelica] ignoring stale lifecycle result for `{}`: result session {} < model session {}",
                        model.model_name, result.session_id, model.session_id
                    );
                }
                continue;
            }
            let Some(mut variables) = variables else {
                reject_missing_modelica_variables(result.entity, &mut model, faults.as_deref_mut());
                continue;
            };
            let mut variables_changed = false;
            if result.session_id > model.session_id {
                let detail = format!(
                    "Modelica worker returned future session {} for `{}` (current session {})",
                    result.session_id, model.model_name, model.session_id
                );
                warn!("[Modelica] protocol violation: {detail}");
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "modelica-session-protocol-violation",
                        Some(result.entity),
                        model.model_name.clone(),
                        detail.clone(),
                    );
                }
                model.in_flight_step = None;
                model.pending_step_inputs = None;
                model.is_stepping = false;
                model.paused = true;
                model.is_compiled = false;
                model.live_solver_snapshot = None;
                model.variable_metadata = None;
                model.last_error = Some(detail);
                continue;
            }

            // A session fences worker commands, while the document generation
            // fences the source snapshot that produced a compile artifact. A
            // user can edit the document while Rumoca is compiling; accepting
            // that older stepper would let a later FixedUpdate advance against
            // equations that no longer match the authoritative document.
            if result.is_new_model
                && model.pending_generation != 0
                && !model.document.is_unassigned()
            {
                let current_generation = documents
                    .as_ref()
                    .and_then(|registry| registry.host(model.document))
                    .map(|host| host.document().generation_owned());
                let Some(current_generation) = current_generation else {
                    let detail = format!(
                        "Modelica source document {} closed before compile session {} completed",
                        model.document, model.session_id
                    );
                    warn!("[Modelica] {detail}");
                    notices.write(ModelicaNotice {
                        level: NoticeLevel::Error,
                        text: detail.clone(),
                    });
                    if let Some(cs) = compile_states.as_mut() {
                        cs.mark_finished(model.document, lunco_doc::CompileState::Error);
                        cs.set_error_message(model.document, detail.clone());
                    }
                    let active = model.resume_after_compile
                        || participants.as_deref().is_some_and(|participants| {
                            participants.requires_barrier(result.entity)
                        });
                    if active {
                        if let Some(faults) = faults.as_deref_mut() {
                            faults.raise(
                                "modelica-compile-source-closed",
                                Some(result.entity),
                                model.model_name.clone(),
                                detail.clone(),
                            );
                        }
                    }
                    model.in_flight_step = None;
                    model.pending_step_inputs = None;
                    model.is_stepping = false;
                    model.is_compiling = false;
                    model.is_compiled = false;
                    model.paused = true;
                    model.resume_after_compile = false;
                    model.last_error = Some(detail);
                    continue;
                };
                if current_generation != model.pending_generation {
                    let active = model.resume_after_compile;
                    let detail = format!(
                        "discarded stale Modelica compile for `{}` (compiled generation {}, current generation {})",
                        model.model_name, model.pending_generation, current_generation,
                    );
                    warn!("[Modelica] {detail}");
                    notices.write(ModelicaNotice {
                        level: NoticeLevel::Warn,
                        text: detail,
                    });
                    if let Some(cs) = compile_states.as_mut() {
                        cs.mark_finished(model.document, lunco_doc::CompileState::Idle);
                        cs.clear_error(model.document);
                    }
                    model.in_flight_step = None;
                    model.pending_step_inputs = None;
                    model.is_stepping = false;
                    model.is_compiling = false;
                    model.is_compiled = false;
                    model.last_error = None;
                    // Preserve the user's run intent. The lifecycle admission
                    // pass will request the now-current revision; paused,
                    // manually compiled models remain paused and await an
                    // explicit compile request.
                    model.paused = !active;
                    continue;
                }
            }

            if let Some(phase) = result.preparation_phase.as_ref() {
                if model.pending_generation != 0
                    && !model.document.is_unassigned()
                    && documents
                        .as_ref()
                        .and_then(|registry| registry.host(model.document))
                        .map(|host| host.document().generation_owned())
                        != Some(model.pending_generation)
                {
                    continue;
                }
                use lunco_modelica_runtime::ModelicaPreparationPhase;
                let detail = match phase {
                    ModelicaPreparationPhase::QueuedForCompilation => {
                        model.construction_profile = None;
                        "queued for compilation".to_owned()
                    }
                    ModelicaPreparationPhase::CheckingSolverCache => {
                        "compiled; checking the prepared solver cache".to_owned()
                    }
                    ModelicaPreparationPhase::LoweringEquations => {
                        "preparing simulation equations and telemetry (solver cache miss)"
                            .to_owned()
                    }
                    ModelicaPreparationPhase::CachedSolverLoaded => {
                        "loaded the prepared solver from cache".to_owned()
                    }
                    ModelicaPreparationPhase::SolverPrepared { elapsed_secs } => format!(
                        "solver preparation finished in {elapsed_secs:.2} s; awaiting initialization"
                    ),
                    ModelicaPreparationPhase::LiveSolverConstructed(profile) => {
                        model.construction_profile = Some(*profile);
                        bevy::log::info!(model = %model.model_name,
                            queue_wait_ns = ?profile.queue_wait_ns,
                            session_ns = profile.session_ns,
                            metadata_ns = profile.metadata_ns,
                            initial_observation_ns = profile.initial_observation_ns,
                            native_code_cache = ?profile.native_code_cache,
                            "[modelica-runtime] live solver construction phases completed");
                        format!(
                            "live initialization finished: solver {:.1} ms, metadata {:.1} ms, initial observation {:.1} ms",
                            profile.session_ns as f64 / 1e6,
                            profile.metadata_ns as f64 / 1e6,
                            profile.initial_observation_ns as f64 / 1e6
                        )
                    }
                };
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Info,
                    text: format!("[{}] {detail}", model.model_name),
                });
                continue;
            }

            if lifecycle_result && result.error.is_none() && result.variable_metadata.is_none() {
                result.error = Some(
                    "Modelica worker omitted the checked scalar metadata inventory".to_owned(),
                );
                result.detected_symbols.clear();
                result.outputs.clear();
            }

            // Retain the worker's resolved plan only after session fencing.
            // Compile results also pass the source-generation check above;
            // step-transaction failures below clear the snapshot before this
            // system returns. Failures cannot leave facts for a dead stepper.
            if result.error.is_some() {
                model.live_solver_snapshot = None;
                model.construction_profile = None;
                model.variable_metadata = None;
            } else if lifecycle_result {
                model.live_solver_snapshot = result.live_solver_snapshot.clone();
            } else if let Some(snapshot) = result.live_solver_snapshot.as_ref() {
                model.live_solver_snapshot = Some(snapshot.clone());
            }

            // A plain result is a response to exactly one master-issued Step.
            // Validate its sequence and communication point before clearing the
            // in-flight flag or touching the model clock. This is the local
            // equivalent of an FMI master's `doStep` transaction fence.
            let accepted_step = if !lifecycle_result {
                let Some(in_flight) = model.in_flight_step.as_ref() else {
                    let detail = format!(
                        "Modelica worker returned step {} for `{}` without an in-flight request",
                        result
                            .step_id
                            .map_or_else(|| "<missing>".to_string(), |id| id.to_string()),
                        model.model_name
                    );
                    warn!("[Modelica] protocol violation: {detail}");
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "modelica-step-protocol-violation",
                            Some(result.entity),
                            model.model_name.clone(),
                            detail.clone(),
                        );
                    }
                    model.is_stepping = false;
                    model.paused = true;
                    model.is_compiled = false;
                    model.live_solver_snapshot = None;
                    model.variable_metadata = None;
                    model.last_error = Some(detail);
                    continue;
                };
                let identity_matches = result.step_id == Some(in_flight.step_id);
                let endpoint_matches = result.error.is_some()
                    || (result.new_time.is_finite()
                        && communication_times_close(result.new_time, in_flight.stop_time));
                if !identity_matches || !endpoint_matches {
                    let detail = format!(
                        "Modelica worker returned invalid step transaction for `{}`: step_id={:?}, expected {}, new_time={:.12}, expected_stop={:.12}",
                        model.model_name,
                        result.step_id,
                        in_flight.step_id,
                        result.new_time,
                        in_flight.stop_time,
                    );
                    warn!("[Modelica] protocol violation: {detail}");
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "modelica-step-protocol-violation",
                            Some(result.entity),
                            model.model_name.clone(),
                            detail.clone(),
                        );
                    }
                    model.in_flight_step = None;
                    model.pending_step_inputs = None;
                    model.is_stepping = false;
                    model.paused = true;
                    model.is_compiled = false;
                    model.live_solver_snapshot = None;
                    model.variable_metadata = None;
                    model.last_error = Some(detail);
                    continue;
                }
                if let (Some(diagnostics), Some(worker_duration_ns)) = (
                    step_diagnostics.as_deref_mut(),
                    result.worker_step_duration_ns,
                ) {
                    diagnostics.record_step(
                        result.entity,
                        result.session_id,
                        worker_duration_ns,
                        in_flight.submitted_at.elapsed().as_nanos() as u64,
                        result.worker_backlog_count,
                    );
                }
                let in_flight = model
                    .in_flight_step
                    .take()
                    .expect("the validated step transaction remains live through admission");
                result.error.is_none().then_some(in_flight)
            } else {
                // A lifecycle transition supersedes any older transaction only
                // after its session has advanced. It starts a fresh sequence.
                if let Some(diagnostics) = step_diagnostics.as_deref_mut() {
                    diagnostics.begin_session(result.entity, result.session_id);
                }
                model.in_flight_step = None;
                model.pending_step_inputs = None;
                model.last_accepted_step = None;
                model.next_step_id = 1;
                None
            };

            if result.error.is_none()
                && let Some(metadata) = result.variable_metadata
            {
                model.variable_metadata = Some(std::sync::Arc::new(metadata));
            }

            let outputs = result.outputs;

            if lifecycle_result {
                info!(
                    "[Modelica] applying lifecycle result for `{}`: result session {} model session {} new={} update={} reset={} error={}",
                    model.model_name,
                    result.session_id,
                    model.session_id,
                    result.is_new_model,
                    result.is_parameter_update,
                    result.is_reset,
                    result.error.is_some()
                );
            }

            model.is_stepping = false;
            // Compile-shaped results (new model / parameter update /
            // reset) close out the corresponding `is_compiling` window
            // the execution dispatcher opened for the worker operation. Step results don't
            // touch this flag — they were never compile-flagged.
            if result.is_new_model || result.is_parameter_update || result.is_reset {
                model.is_compiling = false;
            }

            // Forward log messages to console via bevy_workbench's console system
            if let Some(msg) = &result.log_message {
                debug!("[Modelica] {msg}");
                // Only forward lifecycle notes (compile / reset / param
                // update). Skip the per-Step logs so the console doesn't
                // flood at 60 Hz.
                if result.is_new_model || result.is_reset || result.is_parameter_update {
                    notices.write(ModelicaNotice {
                        level: NoticeLevel::Info,
                        text: format!("[{}] {msg}", model.model_name),
                    });
                }
            }

            // Transition compile state for this entity's document, but only on
            // compile-shaped lifecycle results (new-model / parameter-update /
            // reset) — the same grouping the `is_compiling` and log blocks above
            // use. Plain Step results arrive continuously and must not clobber
            // Ready/Error classifications. `is_reset` MUST be included: a
            // successful reset means the model re-initialised healthy, so it has
            // to reconcile `state` back to `Ready`. Without it, the success
            // branch below still clears the diagnostics list while `state` stays
            // `Error`, leaving the UI stuck on a red "compilation failed" chip
            // with no underlying message.
            let is_compile_result =
                result.is_new_model || result.is_parameter_update || result.is_reset;
            if is_compile_result && !model.document.is_unassigned() {
                let new_state = if result.error.is_some() {
                    lunco_doc::CompileState::Error
                } else {
                    lunco_doc::CompileState::Ready
                };
                if let Some(cs) = compile_states.as_mut() {
                    let elapsed = cs.mark_finished(model.document, new_state);
                    if let Some(dur) = elapsed {
                        let ms = dur.as_secs_f64() * 1000.0;
                        let human = if ms >= 1000.0 {
                            format!("{:.2} s", ms / 1000.0)
                        } else {
                            format!("{:.0} ms", ms)
                        };
                        match new_state {
                            lunco_doc::CompileState::Error => {
                                warn!(
                                    "[Modelica] Compile finished with error for `{}` in {}",
                                    model.model_name, human
                                );
                                notices.write(ModelicaNotice {
                                    level: NoticeLevel::Error,
                                    text: format!(
                                        "⏹ Compile FAILED: '{}' in {}",
                                        model.model_name, human
                                    ),
                                });
                            }
                            lunco_doc::CompileState::Ready => {
                                debug!(
                                    "[Modelica] Compile finished for `{}` in {}",
                                    model.model_name, human
                                );
                                notices.write(ModelicaNotice {
                                    level: NoticeLevel::Info,
                                    text: format!(
                                        "✓ Compile finished: '{}' in {}",
                                        model.model_name, human
                                    ),
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }

            if let Some(err) = &result.error {
                if let Some(cs) = compile_states.as_mut() {
                    // Carry structured located diagnostics when the worker
                    // shipped them (compile failures) so the panel can
                    // render click-to-source rows; empty for solver/reset
                    // errors falls back to the flat `err` string.
                    let mut diags = if result.compile_diagnostics.is_empty() {
                        vec![lunco_doc::Diagnostic::message_only(err.clone())]
                    } else {
                        result.compile_diagnostics.clone()
                    };
                    include_compile_explanation(&mut diags, err);
                    cs.set_error(model.document, diags);
                }
                if result.is_new_model
                    && model.resume_after_compile
                    && participants
                        .as_deref()
                        .is_some_and(|participants| participants.requires_barrier(result.entity))
                {
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "modelica-compile-failed",
                            Some(result.entity),
                            model.model_name.clone(),
                            err.clone(),
                        );
                    }
                }
                warn!("[Modelica] {err}");
                // Classify for the console: compile-time errors are
                // distinct from solver blowups during Step. Both are
                // Error-level; the prefix tells the user where it came
                // from at a glance.
                let prefix = if result.is_new_model {
                    "Compile error"
                } else if result.is_parameter_update {
                    "Parameter update error"
                } else if result.is_reset {
                    "Reset error"
                } else {
                    "Solver error"
                };
                let detail = if result.step_id.is_some() {
                    failed_modelica_step_detail(&model, &variables, err)
                } else {
                    err.clone()
                };
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Error,
                    text: format!("[{}] {prefix}: {detail}", model.model_name),
                });
                // A failed in-flight solver step has no valid replacement
                // state. It is a terminal shared-simulation fault, not a
                // reason to release the barrier and let Avian/Rhai continue
                // against stale Modelica outputs. Compile/reset/parameter
                // diagnostics remain scoped to their document lifecycle; only
                // an error on an already-running step reaches this terminal
                // runtime boundary.
                if !lifecycle_result {
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "modelica-step-failed",
                            Some(result.entity),
                            model.model_name.clone(),
                            detail.clone(),
                        );
                    }
                }
                model.paused = true;
                // A failed Compile/Step must not silently auto-play on a
                // later, unrelated successful compile: clear the resume
                // intent that an earlier `RunActiveModel` may have set.
                model.resume_after_compile = false;
                // Solver errors destroy the stepper in the worker
                // (lib.rs ~1176 removes it). Clear the flag so the
                // next Run after the user fixes things triggers a
                // fresh Compile rather than a doomed Step. Compile
                // errors flip this in the `is_new_model` block below.
                model.is_compiled = false;
                model.last_error = Some(detail);
            } else {
                model.last_error = None;
                if let Some(cs) = compile_states.as_mut()
                    && cs
                        .get(model.document)
                        .is_some_and(|entry| !entry.diagnostics.is_empty())
                {
                    cs.clear_error(model.document);
                }
            }

            if result.is_new_model {
                let values = &mut variables.bypass_change_detection().values;
                variables_changed |= !values.is_empty();
                values.clear();
                // A successful Compile leaves the model PAUSED/ready — we do
                // NOT auto-start a live realtime sim. The one exception is
                // `RunActiveModel`, which set `resume_after_compile = true`
                // before triggering the compile; in that case we unpause here
                // so the user-requested play begins as soon as the stepper is
                // installed. `is_compiled = true` records that the worker
                // installed a stepper. We promote `pending_generation` (the
                // generation captured at dispatch) to `compiled_generation` so
                // staleness checks see the model as up to date.
                if result.error.is_none() {
                    model.compiled_generation = if model.pending_generation != 0 {
                        model.pending_generation
                    } else if !model.document.is_unassigned() {
                        documents
                            .as_ref()
                            .and_then(|registry| registry.host(model.document))
                            .map(|host| host.document().generation_owned())
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    model.paused = inactive.contains(result.entity) || !model.resume_after_compile;
                    model.resume_after_compile = false;
                    // Worker has installed a stepper for this entity.
                    // `spawn_modelica_requests` now admits its next fixed step.
                    model.is_compiled = true;
                } else {
                    model.is_compiled = false;
                }

                // Preserve explicit authored values, then seed omitted inputs
                // from the worker's initialized solver observation. Library
                // member defaults are already applied there; inventing zeros
                // here would overwrite them on the first simulation step.
                let ui_inputs: HashMap<String, f64> = std::mem::take(&mut model.inputs);
                model.compiled_input_names = result.detected_input_names.iter().cloned().collect();
                for name in &result.detected_input_names {
                    if let Some(value) = ui_inputs.get(name).copied().or_else(|| {
                        result
                            .detected_symbols
                            .iter()
                            .find(|(symbol, _)| symbol == name)
                            .map(|(_, value)| *value)
                    }) {
                        model.inputs.insert(name.clone(), value);
                    }
                }
                for (name, val) in ui_inputs {
                    model.inputs.entry(name).or_insert(val);
                }

                model.clear_clock();

                info!(
                    "[Modelica] lifecycle state for `{}`: compiled={} compiling={} stepping={} paused={}",
                    model.model_name,
                    model.is_compiled,
                    model.is_compiling,
                    model.is_stepping,
                    model.paused
                );
            } else if result.is_parameter_update {
                model.clear_clock();
            } else if result.is_reset {
                model.clear_clock();
                let values = &mut variables.bypass_change_detection().values;
                variables_changed |= !values.is_empty();
                values.clear();
                // Preserve inputs and parameters
            }

            // Update observable variables from detected symbols and step outputs
            for (name, val) in result.detected_symbols.iter().chain(outputs.iter()) {
                if !model.inputs.contains_key(name) && !model.parameters.contains_key(name) {
                    let values = &mut variables.bypass_change_detection().values;
                    if let Some(current) = values.get_mut(name) {
                        if current.to_bits() != val.to_bits() {
                            *current = *val;
                            variables_changed = true;
                        }
                    } else {
                        values.insert(name.clone(), *val);
                        variables_changed = true;
                    }
                }
            }
            if variables_changed {
                variables.set_changed();
            }

            // CQ-524: only advance the model clock on a genuine step or
            // compile/reset result. Pure acks (LoadSourceRoot → carries
            // `loaded_source_root_id`; worker-panic/error reports → carry
            // `error`) all set `new_time = 0.0`; assigning that would
            // momentarily zero a running sim's clock. An errored step also
            // didn't progress, so leave the clock where it was.
            if result.error.is_none() && result.loaded_source_root_id.is_none() {
                if let Some(in_flight) = &accepted_step {
                    model.communication_tick = in_flight.stop_tick;
                }
                model.current_time = result.new_time;
                model.last_step_time = result.new_time;
                if let Err(error) = model.reset_communication_schedule() {
                    model.paused = true;
                    model.is_compiled = false;
                    model.live_solver_snapshot = None;
                    model.variable_metadata = None;
                    model.last_error = Some(error.clone());
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "invalid-modelica-communication-period",
                            Some(result.entity),
                            model.model_name.clone(),
                            error,
                        );
                    }
                }
            }
            if let Some(in_flight) = accepted_step {
                let sample = lunco_modelica_runtime::ModelicaStepSample {
                    session_id: model.session_id,
                    step_id: in_flight.step_id,
                    input_time_s: in_flight.start_time,
                    output_time_s: result.new_time,
                    inputs: in_flight.sampled_inputs,
                    outputs,
                };
                debug!(
                    target: "lunco_modelica_worker::step_audit",
                    entity = ?result.entity,
                    api_id = ?global_id.map(lunco_core::GlobalEntityId::get),
                    model = %model.model_name,
                    session_id = sample.session_id,
                    step_id = sample.step_id,
                    input_time_s = sample.input_time_s,
                    output_time_s = sample.output_time_s,
                    input_fingerprint = %format_args!("{:016x}", scalar_sample_fingerprint(&sample.inputs)),
                    output_fingerprint = %format_args!("{:016x}", scalar_sample_fingerprint(&sample.outputs)),
                    "accepted Modelica communication sample"
                );
                bevy::log::trace!(
                    target: "lunco_modelica_worker::step_audit",
                    entity = ?result.entity,
                    api_id = ?global_id.map(lunco_core::GlobalEntityId::get),
                    model = %model.model_name,
                    sample = ?sample,
                    "accepted Modelica communication values"
                );
                model.last_accepted_step = Some(sample);
            }
            if result.is_new_model && result.error.is_none() && model.is_compiled {
                session_starts.write(ModelicaSessionStarted {
                    entity: result.entity,
                    session_id: model.session_id,
                });
            }
        } else if lifecycle_result {
            warn!(
                "[Modelica] dropped lifecycle result for missing entity {:?}: session {} new={} update={} reset={}",
                result.entity,
                result.session_id,
                result.is_new_model,
                result.is_parameter_update,
                result.is_reset
            );
        }
    }

    if let Some(error) = transport_failure {
        let first = worker_failure.as_deref_mut().is_none_or(|failure| {
            if failure.0.is_some() {
                false
            } else {
                failure.0 = Some(error.clone());
                true
            }
        });
        if first {
            bevy::log::error!("[Modelica] {error}");
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: error.clone(),
            });
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "modelica-worker-failed",
                    None,
                    "Modelica worker",
                    error.clone(),
                );
            }
            for (entity, mut model, _, _, _, _) in &mut q_models {
                model.paused = true;
                model.is_compiling = false;
                model.is_compiled = false;
                model.is_stepping = false;
                model.in_flight_step = None;
                model.pending_step_inputs = None;
                model.resume_after_compile = false;
                model.live_solver_snapshot = None;
                model.variable_metadata = None;
                model.last_error = Some(error.clone());
                if let Some(states) = compile_states.as_deref_mut() {
                    states.set_error_message(model.document, error.clone());
                }
                if let Some(diagnostics) = step_diagnostics.as_deref_mut() {
                    diagnostics.remove(entity);
                }
            }
            if let Some(roots) = source_roots.as_deref_mut() {
                for root in roots.roots.values_mut() {
                    if matches!(
                        root.state,
                        lunco_modelica_source_roots::LoadState::Loading { .. }
                            | lunco_modelica_source_roots::LoadState::Ready
                    ) {
                        root.state = lunco_modelica_source_roots::LoadState::Failed(error.clone());
                    }
                }
            }
        }
    }

    // A result landing is the only release edge for the coupling barrier.
    // Admission precedes clock projection; outstanding participants keep it held.
    if let Some(mut coupling) = coupling {
        coupling.held = q_models.iter().any(|(entity, model, _, _, _, _)| {
            let shared_clock_participant = participants
                .as_deref()
                .is_none_or(|participants| participants.requires_barrier(entity));
            shared_clock_participant && !model.paused && model.is_compiled && model.is_stepping
        });
        if faults
            .as_deref()
            .is_some_and(lunco_core::RuntimeFaults::active)
        {
            coupling.held = true;
        }
    }
}

#[cfg(test)]
mod compile_fault_tests {
    use super::*;

    #[test]
    fn communication_sample_fingerprint_retains_names_order_and_float_bits() {
        let sample = vec![("a".to_owned(), 0.0), ("bc".to_owned(), 1.0)];
        let fingerprint = scalar_sample_fingerprint(&sample);
        for changed in [
            vec![("a".to_owned(), -0.0), ("bc".to_owned(), 1.0)],
            vec![("ab".to_owned(), 0.0), ("c".to_owned(), 1.0)],
            vec![("bc".to_owned(), 1.0), ("a".to_owned(), 0.0)],
            vec![
                ("a".to_owned(), 0.0),
                ("bc".to_owned(), f64::from_bits(1.0_f64.to_bits() + 1)),
            ],
        ] {
            assert_ne!(fingerprint, scalar_sample_fingerprint(&changed));
        }
        assert_eq!(fingerprint, scalar_sample_fingerprint(&sample));
    }

    #[derive(Resource, Default)]
    struct CapturedCompileRequests(Vec<CompileRequested>);

    fn test_solver_snapshot() -> Arc<lunco_modelica_runtime::ModelicaLiveSolverSnapshot> {
        Arc::new(lunco_modelica_runtime::ModelicaLiveSolverSnapshot {
            solver_id: "rk4".to_owned(),
            capabilities: lunco_modelica_runtime::ModelicaSolverCapabilities {
                usable_live: true,
                fixed_step: true,
                deterministic: true,
            },
            profile: lunco_modelica_runtime::ModelicaRuntimeProfile {
                live: true,
                predicted: false,
            },
            parameters: lunco_modelica_runtime::ModelicaSolverParameters {
                atol: 1.0e-8,
                rtol: 1.0e-8,
                h0: Some(1.0 / 60.0),
                t_start: 0.0,
                t_end: super::LIVE_INTERNAL_STEP_SECS,
            },
            parameter_overrides: vec![("mass".to_owned(), 2.0)],
            initial_inputs: Vec::new(),
        })
    }

    fn capture_compile_requests(
        mut requests: MessageReader<CompileRequested>,
        mut captured: ResMut<CapturedCompileRequests>,
    ) {
        captured.0.extend(requests.read().cloned());
    }

    #[test]
    fn worker_failure_and_disconnect_retire_live_work_without_revival() {
        for explicit_failure in [false, true] {
            let mut app = App::new();
            app.add_message::<ModelicaNotice>()
                .add_message::<CompileRequested>()
                .add_message::<ModelicaSessionStarted>()
                .init_resource::<lunco_modelica_runtime::ModelicaWorkerFailure>()
                .init_resource::<lunco_core::RuntimeFaults>()
                .init_resource::<lunco_doc_bevy::DocumentDiagnostics>()
                .init_resource::<lunco_core_runtime::SimulationBarrier>()
                .add_systems(First, handle_modelica_responses);
            let (tx_result, rx_result) = crossbeam_channel::unbounded();
            let (tx_command, _rx_command) = crossbeam_channel::unbounded();
            app.insert_resource(ModelicaChannels {
                tx: tx_command,
                rx: rx_result,
            });
            let document = lunco_doc::DocumentId::new(1);
            let entity = app
                .world_mut()
                .spawn(ModelicaModel {
                    model_name: "Plant".into(),
                    document,
                    session_id: 4,
                    is_compiling: true,
                    is_stepping: true,
                    is_compiled: true,
                    resume_after_compile: true,
                    live_solver_snapshot: Some(test_solver_snapshot()),
                    ..Default::default()
                })
                .id();
            if explicit_failure {
                tx_result
                    .send(ModelicaResult::worker_failure("thread admission failed"))
                    .unwrap();
                tx_result
                    .send(ModelicaResult {
                        entity,
                        session_id: 4,
                        is_new_model: true,
                        variable_metadata: Some(std::collections::BTreeMap::new()),
                        live_solver_snapshot: Some(test_solver_snapshot()),
                        ..Default::default()
                    })
                    .unwrap();
            }
            drop(tx_result);
            app.world_mut().run_schedule(First);
            let failure = app
                .world()
                .resource::<lunco_modelica_runtime::ModelicaWorkerFailure>()
                .0
                .clone()
                .expect("worker failure retained");
            assert!(app.world().resource::<lunco_core::RuntimeFaults>().active());
            assert!(
                app.world()
                    .resource::<lunco_core_runtime::SimulationBarrier>()
                    .held
            );
            let model = app.world().get::<ModelicaModel>(entity).unwrap();
            assert!(model.paused);
            assert!(
                !model.is_compiling
                    && !model.is_compiled
                    && !model.is_stepping
                    && !model.resume_after_compile
            );
            assert!(model.live_solver_snapshot.is_none());
            assert_eq!(model.last_error.as_deref(), Some(failure.as_str()));
            assert_eq!(app.world().resource::<Messages<ModelicaNotice>>().len(), 1);
            // Scene fault reset cannot re-admit a dead application transport,
            // and a buffered success cannot resurrect its participant.
            app.world_mut()
                .resource_mut::<lunco_core::RuntimeFaults>()
                .clear();
            app.world_mut().run_schedule(First);
            assert!(
                !app.world()
                    .get::<ModelicaModel>(entity)
                    .unwrap()
                    .is_compiled
            );
            assert_eq!(
                app.world()
                    .resource::<lunco_modelica_runtime::ModelicaWorkerFailure>()
                    .0
                    .as_deref(),
                Some(failure.as_str())
            );
            let source = lunco_workspace::PinnedDocumentRuntimeOwner {
                document,
                runtime: lunco_workspace::DocumentRuntimeOwner::Application,
            };
            app.world_mut().write_message(CompileRequested {
                source,
                entity: Some(entity),
                class: Some("Plant".into()),
                force: true,
                resume_after_compile: true,
            });
            app.add_systems(Update, super::dispatch_modelica_compile_requests);
            app.world_mut().run_schedule(Update);
            assert!(
                !app.world()
                    .get::<ModelicaModel>(entity)
                    .unwrap()
                    .is_compiling
            );
            assert!(
                app.world()
                    .get::<ModelicaModel>(entity)
                    .unwrap()
                    .last_error
                    .as_ref()
                    .unwrap()
                    .contains(&failure)
            );
        }
    }

    #[test]
    fn missing_variable_state_rejects_late_lifecycle_results_after_restoration() {
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .add_systems(
                First,
                (
                    validate_modelica_variable_ownership,
                    handle_modelica_responses,
                )
                    .chain(),
            );
        let (results, received) = crossbeam_channel::unbounded();
        let (commands, _requests) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: commands,
            rx: received,
        });
        let entities: Vec<_> = [1, u64::MAX]
            .into_iter()
            .map(|session_id| {
                app.world_mut()
                    .spawn(ModelicaModel {
                        model_name: "Plant".to_owned(),
                        session_id,
                        is_compiled: true,
                        ..Default::default()
                    })
                    .id()
            })
            .collect();
        for &entity in &entities {
            app.world_mut()
                .entity_mut(entity)
                .remove::<ModelicaVariables>();
        }
        app.world_mut().run_schedule(First);
        let details: Vec<_> = entities
            .iter()
            .map(|&entity| {
                app.world()
                    .get::<ModelicaModel>(entity)
                    .unwrap()
                    .last_error
                    .clone()
            })
            .collect();
        for &entity in &entities {
            let session_id = app.world().get::<ModelicaModel>(entity).unwrap().session_id;
            app.world_mut()
                .entity_mut(entity)
                .insert(ModelicaVariables::default());
            results
                .send(ModelicaResult {
                    entity,
                    session_id,
                    is_new_model: true,
                    variable_metadata: Some(std::collections::BTreeMap::new()),
                    live_solver_snapshot: Some(test_solver_snapshot()),
                    outputs: vec![("x".to_owned(), 1.0)],
                    ..Default::default()
                })
                .unwrap();
        }
        app.world_mut().run_schedule(First);
        for (entity, detail) in entities.into_iter().zip(details) {
            let model = app.world().get::<ModelicaModel>(entity).unwrap();
            assert!(model.paused && !model.is_compiled && !model.is_compiling);
            assert_eq!(model.last_error, detail);
            assert!(model.live_solver_snapshot.is_none());
            assert!(
                app.world()
                    .get::<ModelicaVariables>(entity)
                    .unwrap()
                    .values
                    .is_empty()
            );
        }
        assert!(
            app.world()
                .resource::<Messages<ModelicaSessionStarted>>()
                .is_empty()
        );
    }

    #[test]
    fn accepted_step_reuses_variable_keys_and_transfers_sample_buffers() {
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .init_resource::<lunco_doc_bevy::DocumentDiagnostics>()
            .add_systems(First, handle_modelica_responses);
        let document = lunco_doc::DocumentId::new(1);
        app.world_mut()
            .resource_mut::<lunco_doc_bevy::DocumentDiagnostics>()
            .set_ok(document);
        let (results, received) = crossbeam_channel::unbounded();
        let (commands, _requests) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: commands,
            rx: received,
        });
        let inputs = vec![("command".to_owned(), 0.12345678901234567)];
        let input_key = inputs[0].0.as_ptr();
        let entity = app
            .world_mut()
            .spawn((
                ModelicaModel {
                    document,
                    session_id: 1,
                    is_compiled: true,
                    is_stepping: true,
                    inputs: [("command".to_owned(), 0.25)].into(),
                    parameters: [("gain".to_owned(), 2.0)].into(),
                    in_flight_step: Some(lunco_modelica_runtime::InFlightModelicaStep {
                        step_id: 1,
                        stop_tick: 3,
                        start_time: 0.0,
                        stop_time: 0.05,
                        sampled_inputs: inputs,
                        submitted_at: web_time::Instant::now(),
                    }),
                    ..Default::default()
                },
                ModelicaVariables {
                    values: [("x".to_owned(), 0.0)].into(),
                },
            ))
            .id();
        let variable_key = app
            .world()
            .get::<ModelicaVariables>(entity)
            .unwrap()
            .values
            .get_key_value("x")
            .unwrap()
            .0
            .as_ptr();
        let outputs = vec![
            ("x".to_owned(), -0.0),
            ("new".to_owned(), 1.2345678901234567),
            ("command".to_owned(), 0.25),
            ("gain".to_owned(), 2.0),
        ];
        let output_key = outputs[0].0.as_ptr();
        let expected = outputs.clone();
        results
            .send(ModelicaResult {
                entity,
                session_id: 0,
                step_id: Some(1),
                new_time: 0.05,
                outputs: vec![("x".to_owned(), 99.0)],
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        assert!(
            app.world()
                .resource::<Messages<ModelicaSessionStarted>>()
                .is_empty()
        );
        assert_eq!(
            app.world().get::<ModelicaVariables>(entity).unwrap().values["x"],
            0.0
        );
        results
            .send(ModelicaResult {
                entity,
                session_id: 1,
                step_id: Some(1),
                new_time: 0.05,
                outputs,
                detected_symbols: vec![("x".to_owned(), 8.0)],
                ..Default::default()
            })
            .unwrap();
        app.world_mut().clear_trackers();
        app.world_mut().run_schedule(First);
        assert!(
            !app.world()
                .resource_ref::<lunco_doc_bevy::DocumentDiagnostics>()
                .is_changed(),
            "successful live steps must not invalidate unchanged document diagnostics"
        );
        let model = app.world().get::<ModelicaModel>(entity).unwrap();
        let variables = &app.world().get::<ModelicaVariables>(entity).unwrap().values;
        assert_eq!(
            variables.get_key_value("x").unwrap().0.as_ptr(),
            variable_key
        );
        assert_eq!(variables["x"].to_bits(), (-0.0_f64).to_bits());
        assert_eq!(variables["new"].to_bits(), 1.2345678901234567_f64.to_bits());
        assert!(!variables.contains_key("command"));
        assert!(!variables.contains_key("gain"));
        let accepted = model.last_accepted_step.as_ref().unwrap();
        assert_eq!(accepted.inputs[0].0.as_ptr(), input_key);
        assert_eq!(&*accepted.outputs, expected);
        assert!(model.in_flight_step.is_none());
        assert_eq!(accepted.outputs[0].0.as_ptr(), output_key);
        let retained_outputs = accepted.outputs.clone();
        app.world_mut()
            .resource_mut::<lunco_doc_bevy::DocumentDiagnostics>()
            .set_error_message(document, "previous run failed");
        app.world_mut().clear_trackers();
        results
            .send(ModelicaResult {
                entity,
                session_id: 1,
                is_reset: true,
                variable_metadata: Some(std::collections::BTreeMap::new()),
                outputs: vec![("x".to_owned(), 3.0)],
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        let model = app.world().get::<ModelicaModel>(entity).unwrap();
        let variables = &app.world().get::<ModelicaVariables>(entity).unwrap().values;
        assert_eq!(variables.len(), 1);
        assert_eq!(variables["x"], 3.0);
        assert!(model.last_accepted_step.is_none());
        assert_eq!(&*retained_outputs, expected);
        let diagnostics = app
            .world()
            .resource_ref::<lunco_doc_bevy::DocumentDiagnostics>();
        assert!(diagnostics.is_changed());
        let entry = diagnostics.get(document).unwrap();
        assert_eq!(entry.state, lunco_doc::CompileState::Ready);
        assert!(entry.diagnostics.is_empty());
    }

    /// Measures stable-name publication through real step-transaction fences.
    #[test]
    #[ignore = "manual named-observation admission measurement"]
    fn named_observation_admission_benchmark() {
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .add_systems(First, handle_modelica_responses);
        let (results, received) = crossbeam_channel::unbounded();
        let (commands, _requests) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: commands,
            rx: received,
        });
        let entity = app
            .world_mut()
            .spawn(ModelicaModel {
                document: lunco_doc::DocumentId::new(1),
                session_id: 1,
                is_compiled: true,
                inputs: [("command".to_owned(), 0.25)].into(),
                parameters: [("gain".to_owned(), 2.0)].into(),
                ..Default::default()
            })
            .id();
        let names = (0..2304)
            .map(|index| format!("subsystem.feed.branches[{index}].pressure"))
            .collect::<Vec<_>>();
        let began = std::time::Instant::now();
        for step_id in 1..=512 {
            let start_time = (step_id - 1) as f64 / 60.0;
            let stop_time = step_id as f64 / 60.0;
            let mut model = app.world_mut().get_mut::<ModelicaModel>(entity).unwrap();
            model.is_stepping = true;
            model.in_flight_step = Some(lunco_modelica_runtime::InFlightModelicaStep {
                step_id,
                stop_tick: step_id,
                start_time,
                stop_time,
                sampled_inputs: vec![("command".to_owned(), 0.25)],
                submitted_at: web_time::Instant::now(),
            });
            let mut outputs = names
                .iter()
                .map(|name| (name.clone(), step_id as f64))
                .collect::<Vec<_>>();
            outputs.extend([("command".to_owned(), 0.25), ("gain".to_owned(), 2.0)]);
            results
                .send(ModelicaResult {
                    entity,
                    session_id: 1,
                    step_id: Some(step_id),
                    new_time: stop_time,
                    outputs,
                    ..Default::default()
                })
                .unwrap();
            app.world_mut().run_schedule(First);
        }
        let elapsed = began.elapsed();
        let variables = &app.world().get::<ModelicaVariables>(entity).unwrap().values;
        assert_eq!(variables.len(), names.len());
        assert!(
            variables
                .values()
                .all(|value| value.to_bits() == 512.0_f64.to_bits())
        );
        assert_eq!(
            app.world()
                .get::<ModelicaModel>(entity)
                .unwrap()
                .current_time
                .to_bits(),
            (512.0_f64 / 60.0).to_bits()
        );
        println!(
            "NAMED_OBSERVATION_ADMISSION elapsed_ns={} steps=512 variables=2304 final_bits={:016x}",
            elapsed.as_nanos(),
            512.0_f64.to_bits()
        );
    }

    #[test]
    fn checked_solver_facts_are_fenced_and_missing_metadata_is_terminal() {
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .add_systems(First, handle_modelica_responses);

        let (tx_result, rx_result) = crossbeam_channel::unbounded();
        let (tx_command, _rx_command) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: tx_command,
            rx: rx_result,
        });
        let snapshot = test_solver_snapshot();
        let metadata = std::collections::BTreeMap::from([(
            "power".to_owned(),
            lunco_modelica_runtime::ModelicaScalarMetadata {
                unit: Some("W".to_owned()),
                description: Some("Power".to_owned()),
                ..Default::default()
            },
        )]);
        let entity = app
            .world_mut()
            .spawn(ModelicaModel {
                model_name: "Plant".to_owned(),
                session_id: 1,
                is_compiling: true,
                ..Default::default()
            })
            .id();

        tx_result
            .send(ModelicaResult {
                entity,
                session_id: 1,
                is_new_model: true,
                variable_metadata: Some(metadata.clone()),
                live_solver_snapshot: Some(snapshot.clone()),
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        assert_eq!(
            app.world()
                .get::<ModelicaModel>(entity)
                .unwrap()
                .live_solver_snapshot,
            Some(snapshot.clone())
        );
        let retained = app
            .world()
            .get::<ModelicaModel>(entity)
            .unwrap()
            .live_solver_snapshot
            .as_ref()
            .unwrap();
        assert!(
            std::ptr::eq(snapshot.solver_id.as_ptr(), retained.solver_id.as_ptr()),
            "the accepted lifecycle receipt must share immutable solver facts"
        );

        let admitted = app
            .world()
            .get::<ModelicaModel>(entity)
            .unwrap()
            .variable_metadata
            .clone()
            .unwrap();
        assert_eq!(*admitted, metadata);
        tx_result
            .send(ModelicaResult {
                entity,
                session_id: 0,
                is_new_model: true,
                variable_metadata: Some(std::collections::BTreeMap::new()),
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        assert!(std::sync::Arc::ptr_eq(
            &admitted,
            app.world()
                .get::<ModelicaModel>(entity)
                .unwrap()
                .variable_metadata
                .as_ref()
                .unwrap()
        ));

        tx_result
            .send(ModelicaResult {
                entity,
                session_id: 1,
                is_new_model: true,
                variable_metadata: None,
                ..Default::default()
            })
            .unwrap();
        app.world_mut().run_schedule(First);
        assert_eq!(
            app.world()
                .get::<ModelicaModel>(entity)
                .unwrap()
                .live_solver_snapshot,
            None
        );
        let failed = app.world().get::<ModelicaModel>(entity).unwrap();
        assert!(failed.variable_metadata.is_none());
        assert!(!failed.is_compiled && failed.paused);
        assert!(
            failed
                .last_error
                .as_deref()
                .unwrap()
                .contains("checked scalar metadata inventory")
        );
    }

    #[test]
    fn active_causal_compile_failure_faults_before_progress_can_resume() {
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .init_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
            .init_resource::<lunco_core_runtime::SimulationBarrier>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .add_systems(First, handle_modelica_responses);

        let (tx_result, rx_result) = crossbeam_channel::unbounded();
        let (tx_command, _rx_command) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: tx_command,
            rx: rx_result,
        });

        let entity = app
            .world_mut()
            .spawn(ModelicaModel {
                model_name: "ActivePlant".to_owned(),
                session_id: 4,
                is_compiling: true,
                resume_after_compile: true,
                ..Default::default()
            })
            .id();
        let mut participants = app
            .world_mut()
            .resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>();
        participants.replace([entity]);
        participants.replace_modelica_entities([entity]);
        drop(participants);

        tx_result
            .send(ModelicaResult {
                entity,
                session_id: 4,
                is_new_model: true,
                variable_metadata: Some(std::collections::BTreeMap::new()),
                error: Some("authored equation did not compile".to_owned()),
                ..Default::default()
            })
            .unwrap();

        app.world_mut().run_schedule(First);

        let fault = app
            .world()
            .resource::<lunco_core::RuntimeFaults>()
            .first
            .as_ref()
            .expect("active causal compile failure is terminal");
        assert_eq!(fault.kind, "modelica-compile-failed");
        assert_eq!(fault.entity, Some(entity));
        let model = app.world().get::<ModelicaModel>(entity).unwrap();
        assert!(model.paused);
        assert!(!model.is_compiled);
        assert!(!model.resume_after_compile);
    }

    #[test]
    fn stale_compile_result_keeps_active_model_held_for_current_revision() {
        use lunco_doc::Document;
        use lunco_doc_bevy::DocumentRegistry;
        use lunco_modelica_document::{ModelicaDocument, ModelicaOp};

        let source = "model ActivePlant\n  Real position;\nequation\n  der(position) = 1;\nend ActivePlant;\n";
        let mut documents = DocumentRegistry::<ModelicaDocument>::default();
        let doc = documents.allocate(
            source.to_owned(),
            lunco_doc::PathlessOrigin::untitled("ActivePlant"),
        );
        let host = documents.host_mut(doc).unwrap();
        host.document_mut().refresh_ast_now();
        let compile_generation = host.document().generation_owned();
        host.document_mut()
            .apply(ModelicaOp::ReplaceSource {
                new: source.replace("1;", "2;"),
            })
            .unwrap();
        let current_generation = host.document().generation_owned();
        assert!(current_generation > compile_generation);

        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<CompileRequested>()
            .add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .init_resource::<lunco_doc_bevy::DocumentDiagnostics>()
            .init_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
            .init_resource::<lunco_core_runtime::SimulationBarrier>()
            .init_resource::<lunco_core_runtime::SimulationProgress>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .init_resource::<CapturedCompileRequests>()
            .insert_resource(documents);

        let (tx_result, rx_result) = crossbeam_channel::unbounded();
        let (tx_command, rx_command) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels {
            tx: tx_command,
            rx: rx_result,
        });

        let entity = app
            .world_mut()
            .spawn(ModelicaModel {
                model_name: "ActivePlant".to_owned(),
                document: doc,
                session_id: 4,
                pending_generation: compile_generation,
                is_compiling: true,
                resume_after_compile: true,
                ..Default::default()
            })
            .id();
        let mut participants = app
            .world_mut()
            .resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>();
        participants.replace([entity]);
        participants.replace_modelica_entities([entity]);
        drop(participants);
        app.world_mut()
            .resource_mut::<lunco_doc_bevy::DocumentRegistry<ModelicaDocument>>()
            .link(entity, doc)
            .unwrap();

        tx_result
            .send(ModelicaResult {
                entity,
                session_id: 4,
                is_new_model: true,
                variable_metadata: Some(std::collections::BTreeMap::new()),
                live_solver_snapshot: Some(test_solver_snapshot()),
                ..Default::default()
            })
            .unwrap();

        app.add_systems(First, handle_modelica_responses);
        app.add_systems(
            Update,
            (request_modelica_compiles, capture_compile_requests).chain(),
        );
        app.add_systems(PreUpdate, reconcile_modelica_preparation_progress);
        app.world_mut().run_schedule(First);
        app.world_mut().run_schedule(Update);
        app.world_mut().run_schedule(PreUpdate);

        let model = app.world().get::<ModelicaModel>(entity).unwrap();
        assert!(!model.paused);
        assert!(!model.is_compiled);
        assert!(!model.is_compiling);
        assert!(model.resume_after_compile);
        assert!(model.live_solver_snapshot.is_none());
        assert_eq!(model.pending_generation, compile_generation);
        assert_eq!(
            app.world()
                .resource::<lunco_doc_bevy::DocumentDiagnostics>()
                .state_of(doc),
            lunco_doc::CompileState::Idle
        );
        assert!(
            app.world()
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );

        let requests = &app.world().resource::<CapturedCompileRequests>().0;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source.document, doc);
        assert_eq!(requests[0].entity, Some(entity));
        assert!(requests[0].resume_after_compile);
        assert!(rx_command.try_recv().is_err());
    }
}

// ===========================================================================
// The macro-step contract
// ===========================================================================

/// Retire only live participation admitted to the exact outgoing Twin.
pub fn retire_closed_twin_models(
    trigger: On<lunco_workspace::TwinClosed>,
    models: Query<
        (
            Entity,
            &lunco_workspace::PinnedDocumentRuntimeOwner,
            Has<lunco_modelica_runtime::EditorModelicaActor>,
        ),
        With<ModelicaModel>,
    >,
    mut commands: Commands,
) {
    retire_models_owned_by(
        &lunco_workspace::DocumentRuntimeOwner::LocalTwin(trigger.event().twin),
        &models,
        &mut commands,
    );
}

/// Release live Modelica participation when its exact remote mount/connection retires.
pub fn retire_replication_models(
    trigger: On<lunco_core_session::ReplicationOwnerRetired>,
    models: Query<
        (
            Entity,
            &lunco_workspace::PinnedDocumentRuntimeOwner,
            Has<lunco_modelica_runtime::EditorModelicaActor>,
        ),
        With<ModelicaModel>,
    >,
    mut commands: Commands,
) {
    retire_models_owned_by(
        &lunco_workspace::DocumentRuntimeOwner::Replicated(trigger.event().owner.clone()),
        &models,
        &mut commands,
    );
}

fn retire_models_owned_by(
    owner: &lunco_workspace::DocumentRuntimeOwner,
    models: &Query<
        (
            Entity,
            &lunco_workspace::PinnedDocumentRuntimeOwner,
            Has<lunco_modelica_runtime::EditorModelicaActor>,
        ),
        With<ModelicaModel>,
    >,
    commands: &mut Commands,
) {
    for (entity, source, editor_actor) in models.iter() {
        if &source.runtime != owner {
            continue;
        }
        if editor_actor {
            commands.entity(entity).try_despawn();
        } else {
            commands
                .entity(entity)
                .try_remove::<(ModelicaModel, lunco_workspace::PinnedDocumentRuntimeOwner)>();
        }
    }
}

#[cfg(test)]
mod document_runtime_owner_tests {
    use super::*;
    #[derive(Component)]
    struct Geometry;
    #[test]
    fn twin_retirement_preserves_geometry_epoch_and_rejects_previous_session_results() {
        use lunco_workspace::PinnedDocumentRuntimeOwner;
        let a = lunco_workspace::TwinId::new(7);
        let b = lunco_workspace::TwinId::new(8);
        let doc = lunco_doc::DocumentId::new(1);
        let mut app = App::new();
        app.add_message::<ModelicaNotice>()
            .add_message::<ModelicaSessionStarted>()
            .add_observer(on_remove_modelica)
            .add_observer(retire_closed_twin_models)
            .add_systems(First, handle_modelica_responses);
        let (tx, rx) = crossbeam_channel::unbounded();
        let (results, result_rx) = crossbeam_channel::unbounded();
        app.insert_resource(ModelicaChannels { tx, rx: result_rx });
        let geometry = app
            .world_mut()
            .spawn((
                Geometry,
                ModelicaModel {
                    document: doc,
                    session_id: 1,
                    ..Default::default()
                },
                PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::LocalTwin(a),
                },
            ))
            .id();
        let editor = app
            .world_mut()
            .spawn((
                lunco_modelica_runtime::EditorModelicaActor,
                ModelicaModel::default(),
                PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::LocalTwin(a),
                },
            ))
            .id();
        let application = app
            .world_mut()
            .spawn((
                ModelicaModel::default(),
                PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                },
            ))
            .id();
        let other = app
            .world_mut()
            .spawn((
                ModelicaModel::default(),
                PinnedDocumentRuntimeOwner {
                    document: doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::LocalTwin(b),
                },
            ))
            .id();
        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: a,
            root: Default::default(),
            was_active: true,
        });
        app.world_mut().flush();
        assert!(app.world().get_entity(editor).is_err());
        assert!(app.world().get::<Geometry>(geometry).is_some());
        assert!(app.world().get::<ModelicaModel>(geometry).is_none());
        assert_eq!(
            app.world()
                .get::<lunco_modelica_runtime::ModelicaSessionEpoch>(geometry)
                .expect("retained epoch")
                .0,
            1
        );
        assert!(app.world().get::<ModelicaModel>(application).is_some());
        assert!(app.world().get::<ModelicaModel>(other).is_some());
        assert_eq!(
            rx.try_iter()
                .filter(|command| matches!(command, ModelicaCommand::Despawn { .. }))
                .count(),
            2
        );
        app.world_mut().entity_mut(geometry).insert((
            ModelicaModel {
                document: doc,
                session_id: 2,
                is_compiling: true,
                ..Default::default()
            },
            PinnedDocumentRuntimeOwner {
                document: doc,
                runtime: lunco_workspace::DocumentRuntimeOwner::Application,
            },
            lunco_modelica_runtime::ModelicaSessionEpoch(2),
        ));
        results
            .send(ModelicaResult {
                entity: geometry,
                session_id: 1,
                is_new_model: true,
                variable_metadata: Some(std::collections::BTreeMap::new()),
                outputs: vec![("x".into(), 99.0)],
                ..Default::default()
            })
            .expect("buffered old result");
        app.update();
        let model = app
            .world()
            .get::<ModelicaModel>(geometry)
            .expect("replacement participant");
        assert!(model.is_compiling);
        assert!(!model.is_compiled);
        assert!(
            app.world()
                .get::<ModelicaVariables>(geometry)
                .unwrap()
                .values
                .is_empty()
        );
        assert!(
            app.world()
                .resource::<Messages<ModelicaSessionStarted>>()
                .is_empty()
        );
    }
}
