use super::*;

/// Tears the model down on the worker when its `ModelicaModel` component goes away, so a
/// despawned entity does not leave a `SimulationSession` alive in the worker thread.
/// Registered in `lib.rs` (`.add_observer(worker::on_remove_modelica)`).
pub fn on_remove_modelica(
    trigger: On<Remove, ModelicaModel>,
    channels: Res<ModelicaChannels>,
    mut sim_registry: ResMut<lunco_signal::SimRegistry>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    sim_registry.remove_entity(entity);
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

/// Decide this tick's macro step for one model.
///
/// **The macro-step contract** (A3), factored out as a pure function so it is
/// testable without a worker thread or an `App`:
///
/// * `target_time` — the world clock (model-local), advanced one fixed delta per
///   fixed tick when no step is in flight. NEVER a render-frame quantity.
/// * `current_time` — the model's own clock, from the last worker result.
/// * `in_flight` — a `Step` is already out at the worker for this model.
///
/// Returns the `dt` to request, or `None` for "nothing to do this tick".
///
/// The requested `dt` is the **whole deficit**, clamped to
/// [`MAX_MACRO_STEP_DT`]. A step in flight returns `None`. A coupled caller
/// holds the shared target clock until the result lands; an independent caller
/// may continue advancing its target clock while the worker runs, preserving
/// the same communication-point semantics without stalling unrelated physics.
///
/// While a step is in flight we do not dispatch another (one macro step per
/// model at a time — the worker owns one `SimulationSession` per entity).
pub(crate) fn plan_macro_step(target_time: f64, current_time: f64, in_flight: bool) -> Option<f64> {
    if in_flight {
        return None;
    }
    let deficit = target_time - current_time;
    if deficit < MIN_MACRO_STEP_DT {
        // Already at (or, through micro-step rounding, just past) the
        // communication point. Overshoot corrects itself: the deficit goes
        // slightly negative and the next tick's fixed delta absorbs it.
        return None;
    }
    Some(deficit.min(MAX_MACRO_STEP_DT))
}

/// Sends `Step` commands for each active model — **the co-simulation master's
/// macro-step dispatch**.
///
/// Runs in [`FixedUpdate`]. Each live model's clock is driven toward
/// `target_time`, which advances by exactly one `Time<Fixed>` delta per FIXED
/// TICK. Model time is therefore a pure function of the fixed-step clock: it does
/// not depend on the render frame rate, on GPU load, or on window focus.
///
/// Also measures the model-vs-world lag and publishes it to [`CosimLag`] — the
/// only thing in the system that compares the two clocks at all.
pub fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    mut fixed_time: ResMut<Time<Fixed>>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
    mut lag: ResMut<CosimLag>,
    participants: Option<Res<lunco_core_runtime::SimulationBarrierParticipants>>,
    coupling: Option<ResMut<lunco_core_runtime::SimulationBarrier>>,
    faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    // Auto-compile request goes out as a core event; the UI relays it to the
    // `CompileModel` command. Core no longer references the UI command.
    mut compile_requests: MessageWriter<CompileRequested>,
) {
    // The FIXED delta — constant (1/`FIXED_HZ`) by construction. `rate` bursts
    // show up as MORE fixed ticks, never as a longer one, so accumulating it
    // per tick is exactly "one tick of world time".
    let fixed_dt = fixed_time.delta_secs_f64();

    let mut worst_secs = 0.0_f64;
    let mut worst_entity = None;
    let mut live_models = 0usize;
    let mut shared_clock_models = 0usize;
    let mut coupling_held = false;
    let mut faults = faults;

    for (entity, mut model) in q_models.iter_mut() {
        let shared_clock_participant = participants
            .as_deref()
            .is_none_or(|participants| participants.requires_barrier(entity));

        if let Err(error) = model.validated_communication_period_secs() {
            let first_report = model.last_error.as_deref() != Some(error.as_str());
            model.paused = true;
            model.is_compiled = false;
            model.is_stepping = false;
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

        if model.paused {
            // A paused model's clock is frozen WITH the world's: the target does
            // not advance, so unpausing does not trigger a catch-up burst for
            // time the model was never supposed to simulate.
            continue;
        }

        // First-step path: model has been unpaused (user pressed Run)
        // but no Compile has succeeded yet — the worker has no stepper
        // and a Step would just bounce back as "Click Compile first".
        // Auto-trigger CompileModel instead. The observer flips
        // `is_compiling`/`is_stepping` and bumps `session_id`, so the guard
        // below stops us re-triggering on subsequent ticks; on a successful
        // result the response handler sets `is_compiled = true` and unpauses.
        if !model.is_compiled {
            let doc = model.document;
            let compile_in_flight = model.is_compiling || model.is_stepping;
            if doc != lunco_doc::DocumentId::default() && !compile_in_flight {
                compile_requests.write(CompileRequested {
                    doc,
                    class: if model.model_name.is_empty() {
                        None
                    } else {
                        Some(model.model_name.clone())
                    },
                    force: false,
                    // Compile-on-first-step: preserve whatever resume
                    // intent the model already carries (this path never
                    // arms a new one).
                    resume_after_compile: false,
                });
            }
            // Don't ship a Step this tick either way — let the
            // compile flow run. The model isn't running yet, so its target
            // clock stays put (no phantom catch-up debt accrues while the
            // compile is in flight).
            continue;
        }

        // A live Modelica step is a barrier only when the resolved topology says
        // this participant can affect shared state. A telemetry/electrical
        // participant still owns an explicit communication schedule and may be
        // in flight, but its zero-order-held outputs must not stop Avian or the
        // controllers. The unresolved topology state is fail-closed above.
        if model.is_stepping {
            if !shared_clock_participant {
                // The world keeps advancing while this independent participant
                // solves. Account for that world time instead of freezing its
                // target clock at the dispatch instant; the next communication
                // point then represents the actual shared-world timestamp.
                model.target_time += fixed_dt;
            } else {
                coupling_held = true;
                shared_clock_models += 1;
            }
            live_models += 1;
            let lag_secs = (model.target_time - model.current_time).abs();
            if lag_secs > worst_secs {
                worst_secs = lag_secs;
                worst_entity = Some(entity);
            }
            continue;
        }

        // ── The world clock advances by exactly one FIXED tick ──────────────
        model.target_time += fixed_dt;

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
        // The validation at the top of this iteration established the field's
        // invariant. Read the authored value directly so a malformed value can
        // never be converted into a different schedule here.
        let period = model.communication_period_secs;
        if model.target_time + COMMUNICATION_EPS < model.next_communication_time {
            continue;
        }
        let Some(dt) = plan_macro_step(model.next_communication_time, model.current_time, false)
        else {
            // The solver can land a tiny amount past a point because its
            // micro-step ladder is discrete. Move the schedule to the next
            // point instead of repeatedly dispatching a sub-micro-step.
            model.next_communication_time = model.current_time + period;
            continue;
        };

        let inputs: Vec<(String, f64)> = model
            .inputs
            .iter()
            .map(|(name, val)| (name.clone(), *val))
            .collect();

        let Some(next_step_id) = model.next_step_id.checked_add(1) else {
            let error = format!(
                "Modelica communication-point sequence exhausted for `{}`",
                model.model_name
            );
            model.paused = true;
            model.is_compiled = false;
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
        let start_time = model.current_time;
        let stop_time = model.next_communication_time;

        let sent = channels.tx.send(ModelicaCommand::Step {
            entity,
            session_id: model.session_id,
            step_id,
            start_time,
            stop_time,
            model_name: model.model_name.clone(),
            inputs,
            dt,
        });
        if sent.is_ok() {
            model.next_step_id = next_step_id;
            model.in_flight_step = Some(InFlightModelicaStep {
                step_id,
                start_time,
                stop_time,
            });
            model.is_stepping = true;
            coupling_held |= shared_clock_participant;
        } else {
            model.paused = true;
            model.is_compiled = false;
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

    if coupling_held {
        // Bevy's fixed runner may have accumulated several fixed periods for
        // this render frame before this first solver request was dispatched.
        // The current fixed iteration is the only valid one; discard the
        // remaining overstep so the runner cannot execute another tick after
        // the barrier has been raised. `project_time_transport` pauses the virtual
        // clock before the next frame, which then keeps every FixedUpdate
        // consumer (SimTick, Rhai, controllers, Modelica, and Avian) stopped
        // until the result is released in Update.
        lunco_time::discard_fixed_overstep(&mut fixed_time);
    }

    // Rate-limited divergence alarm. A coupled participant keeps the shared
    // simulation held while it waits; an independent participant is allowed
    // to finish asynchronously while its last validated output remains
    // zero-order-held. The worker is asynchronous in wall-clock execution,
    // never a second simulation-time authority.
    if lag.cooldown > 0 {
        lag.cooldown -= 1;
    } else if worst_secs > LAG_WARN_SECS {
        warn!(
            "[cosim] Modelica participant is {:.3}s behind its communication point \
             (entity {:?}, {} live model(s)); causal membership determines \
             whether the shared simulation waits for the result.",
            worst_secs, worst_entity, live_models,
        );
        lag.cooldown = LAG_WARN_COOLDOWN_TICKS;
    }
}

/// System that processes results from the background worker.
///
/// Updates `ModelicaModel` components with fresh simulation outputs, handles
/// session fencing to ignore stale results. On `is_new_model`, clears old data
/// and unpauses the simulation.
pub fn handle_modelica_responses(
    channels: Res<ModelicaChannels>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
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
    // Live sim samples leave the core handler through this UI-agnostic queue;
    // the reactive UI viz observer (`ui::core_observers::drain_sim_samples_to_viz`)
    // drains it into `lunco_viz`. Core no longer references any viz/plot types.
    mut sample_stream: ResMut<SimSampleStream>,
    runner_res: Option<Res<lunco_modelica_runner::ModelicaRunnerResource>>,
    source_roots: Option<ResMut<lunco_modelica_source_roots::SourceRootRegistry>>,
    participants: Option<Res<lunco_core_runtime::SimulationBarrierParticipants>>,
    coupling: Option<ResMut<lunco_core_runtime::SimulationBarrier>>,
    faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    let mut compile_states = compile_states;
    let mut source_roots = source_roots;
    let mut faults = faults;
    while let Ok(result) = channels.rx.try_recv() {
        // Source-root load ack: route to the registry and short-
        // circuit before any of the sim-result handling below
        // (which keys on `result.entity` — LoadSourceRoot uses
        // `Entity::PLACEHOLDER`).
        if let Some(root_id) = result.loaded_source_root_id.as_ref() {
            if let Some(roots) = source_roots.as_deref_mut() {
                if let Some(entry) = roots.roots.get_mut(root_id) {
                    if let Some(err) = result.error.as_ref() {
                        bevy::log::warn!("[source-roots] `{}` load failed: {}", root_id, err,);
                        entry.state = lunco_modelica_source_roots::LoadState::Failed(err.clone());
                    } else {
                        bevy::log::info!("[source-roots] `{}` is now Ready", root_id,);
                        entry.state = lunco_modelica_source_roots::LoadState::Ready;
                    }
                }
            }
            // Status-bar projection of this load result is handled by the
            // reactive UI observer of `SourceRootRegistry` — core only sets the
            // registry state above.
            continue;
        }

        // Pipe Modelica `experiment(...)` annotation values into the
        // experiments runner's per-ModelRef cache so the Fast Run
        // toolbar's bounds readout reflects the model rather than
        // always falling back to 0..1. Runs once per successful
        // Compile (is_new_model = true).
        if result.is_new_model && result.error.is_none() {
            if let (Some(runner), Some(name)) =
                (runner_res.as_ref(), result.compiled_model_name.as_ref())
            {
                runner.0.set_model_defaults(
                    lunco_experiments::ModelRef(name.clone()),
                    lunco_modelica_runner::ModelDefaults {
                        t_start: result.experiment_start_time,
                        t_end: result.experiment_stop_time,
                        tolerance: result.experiment_tolerance,
                        interval: result.experiment_interval,
                        // The live worker path carries `Interval` only; the
                        // `NumberOfIntervals` count flows through the batch
                        // experiments path (compile.rs ModelDefaults builder).
                        number_of_intervals: None,
                        // Resolve the annotation's solver name against the
                        // REGISTRY once here. A name nobody registered falls to
                        // `None` (= let the resolver pick from what the model
                        // needs) rather than being carried as a free string that
                        // some later layer parses differently.
                        solver: result.experiment_solver.as_deref().and_then(|s| {
                            lunco_modelica_solver::solver_backends::ensure_builtin_solvers();
                            let id = lunco_experiments::SolverId::from(s);
                            lunco_experiments::solver::get(&id).map(|spec| spec.id)
                        }),
                    },
                );
            }
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
        if let Ok((_, mut model)) = q_models.get_mut(result.entity) {
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
                model.is_stepping = false;
                model.paused = true;
                model.is_compiled = false;
                model.last_error = Some(detail);
                continue;
            }

            // A plain result is a response to exactly one master-issued Step.
            // Validate its sequence and communication point before clearing the
            // in-flight flag or touching the model clock. This is the local
            // equivalent of an FMI master's `doStep` transaction fence.
            if !lifecycle_result {
                let Some(in_flight) = model.in_flight_step else {
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
                    model.is_stepping = false;
                    model.paused = true;
                    model.is_compiled = false;
                    model.last_error = Some(detail);
                    continue;
                }
                model.in_flight_step = None;
            } else {
                // A lifecycle transition supersedes any older transaction only
                // after its session has advanced. It starts a fresh sequence.
                model.in_flight_step = None;
                model.next_step_id = 1;
            }

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
            // the `CompileModel` observer opened. Step results don't
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

            // Variable description strings now live on the document
            // index ([`ModelicaIndex::find_component_by_leaf`]); panels
            // read them directly. The worker no longer mirrors them
            // into ECS state.

            if let Some(err) = &result.error {
                if let Some(cs) = compile_states.as_mut() {
                    // Carry structured located diagnostics when the worker
                    // shipped them (compile failures) so the panel can
                    // render click-to-source rows; empty for solver/reset
                    // errors falls back to the flat `err` string.
                    let diags = if result.compile_diagnostics.is_empty() {
                        vec![lunco_doc::Diagnostic::message_only(err.clone())]
                    } else {
                        result.compile_diagnostics.clone()
                    };
                    cs.set_error(model.document, diags);
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
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Error,
                    text: format!("[{}] {prefix}: {err}", model.model_name),
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
                            err.clone(),
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
                model.last_error = Some(err.clone());
            } else {
                model.last_error = None;
                if let Some(cs) = compile_states.as_mut() {
                    cs.clear_error(model.document);
                }
            }

            if result.is_new_model {
                model.variables.clear();
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
                    model.paused = !model.resume_after_compile;
                    model.resume_after_compile = false;
                    // Worker has installed a stepper for this entity.
                    // `spawn_modelica_requests` reads this to decide
                    // whether to ship Step or trigger Compile-on-first-step.
                    model.is_compiled = true;
                } else {
                    model.is_compiled = false;
                }

                // Merge input names from the worker with values the UI already extracted from source.
                // The UI extracts defaults from source code (e.g., `input Real g = 9.81` → g: 9.81),
                // which is more reliable than the worker's DAE-discovered names (which may have 0.0).
                let ui_inputs: HashMap<String, f64> = std::mem::take(&mut model.inputs);
                model.compiled_input_names = result.detected_input_names.iter().cloned().collect();
                for name in &result.detected_input_names {
                    model
                        .inputs
                        .entry(name.clone())
                        .or_insert_with(|| *ui_inputs.get(name).unwrap_or(&0.0));
                }
                for (name, val) in ui_inputs {
                    model.inputs.entry(name).or_insert(val);
                }

                model.current_time = 0.0;
                model.target_time = 0.0;
                model.next_communication_time = 0.0;
                model.last_step_time = 0.0;

                info!(
                    "[Modelica] lifecycle state for `{}`: compiled={} compiling={} stepping={} paused={}",
                    model.model_name,
                    model.is_compiled,
                    model.is_compiling,
                    model.is_stepping,
                    model.paused
                );
            } else if result.is_parameter_update {
                model.current_time = 0.0;
                model.target_time = 0.0;
                model.next_communication_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_reset {
                model.current_time = 0.0;
                // The world clock this model is coupled to restarts WITH it —
                // otherwise the fresh model would immediately owe the catch-up
                // path every second the old one had run (A3).
                model.target_time = 0.0;
                model.next_communication_time = 0.0;
                model.last_step_time = 0.0;
                model.variables.clear();
                // Preserve inputs and parameters
            }

            // Update observable variables from detected symbols and step outputs
            for (name, val) in result.detected_symbols.iter().chain(result.outputs.iter()) {
                if !model.inputs.contains_key(name) && !model.parameters.contains_key(name) {
                    model.variables.insert(name.clone(), *val);
                }
            }

            // CQ-524: only advance the model clock on a genuine step or
            // compile/reset result. Pure acks (LoadSourceRoot → carries
            // `loaded_source_root_id`; worker-panic/error reports → carry
            // `error`) all set `new_time = 0.0`; assigning that would
            // momentarily zero a running sim's clock. An errored step also
            // didn't progress, so leave the clock where it was.
            if result.error.is_none() && result.loaded_source_root_id.is_none() {
                model.current_time = result.new_time;
                model.last_step_time = result.new_time;
                if let Err(error) = model.reset_communication_schedule() {
                    model.paused = true;
                    model.is_compiled = false;
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
            let time_val = model.current_time;

            // Emit this step's observable samples to the reactive UI layer.
            // The core handler no longer knows about plots / `lunco_viz`: it
            // just appends UI-agnostic samples that `ui::core_observers::
            // drain_sim_samples_to_viz` projects into the SignalRegistry (clear
            // on a fresh compile, push every scalar, attach doc-index meta, and
            // reset the default graph). Bounded at the producer so a headless
            // build (no drainer) can't grow the queue without limit.
            if sample_stream.batches.len() < 16_384 {
                let samples: Vec<(String, f64)> = result
                    .outputs
                    .iter()
                    .chain(result.detected_symbols.iter())
                    .map(|(n, v)| (n.clone(), *v))
                    .collect();
                sample_stream.batches.push(SimSampleBatch {
                    entity: result.entity,
                    document: model.document,
                    time: time_val,
                    samples,
                    is_new_model: result.is_new_model,
                    is_parameter_update: result.is_parameter_update,
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

    // A result landing is the only release edge for the coupling barrier. The
    // next FixedUpdate may dispatch the following step, but PreUpdate has
    // already observed this release, so the current physics step consumes only
    // the fresh output that just arrived.
    if let Some(mut coupling) = coupling {
        coupling.held = q_models.iter().any(|(entity, model)| {
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

// ===========================================================================
// The macro-step contract
// ===========================================================================
