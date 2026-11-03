//! Initialization requests share preparation without creating live participants.

use super::*;

#[cfg(target_arch = "wasm32")]
pub(super) fn execute_wasm(
    state: &mut ModelicaWorkerState,
    command: ModelicaCommand,
) -> Result<ModelicaInitializationOutcome, String> {
    let ModelicaCommand::Initialize {
        model_name,
        source,
        doc_uri,
        extra_sources,
        parameter_overrides,
        authored_bindings,
        initial_inputs,
        probes,
        expected_library_revision,
        ..
    } = command
    else {
        return Err("initialization owner received a different command".into());
    };
    let mut unit = assemble_compile_unit(&source, extra_sources);
    unit.parameter_overrides = canonical_parameter_overrides(&parameter_overrides);
    unit.authored_bindings = authored_bindings;
    unit.expected_library_revision = expected_library_revision;
    let compiler = state.compiler.get_or_insert_with(ModelicaCompiler::new);
    let compiled = compile_shared(
        &mut state.compiled_artifacts,
        compiler,
        &model_name,
        &unit,
        &doc_uri,
        state.library_gen,
    )?;
    let library_revision = compiler.library_revision();
    let parameters = merge_instance_values(
        &parameter_overrides,
        &compiled.authored_bindings.parameter_overrides,
    );
    let inputs = merge_instance_values(&compiled.authored_bindings.initial_inputs, &initial_inputs);
    let plan = live_build_plan(
        solver::RuntimeProfile {
            live: true,
            predicted: false,
        },
        &parameters,
        &inputs,
        compiled.prepared_source_key,
        &compiled.structural_overrides,
        Some(library_revision),
    )
    .map_err(|error| error.to_string())?;
    if !state.prepared_solve_cache.contains_key(&plan.key) {
        let component = rumoca_sim::prepare_fmi_component(&compiled.compiled.dae, &plan.options)
            .map_err(|error| error.to_string())?;
        state
            .prepared_solve_cache
            .insert(plan.key.clone(), Arc::new(component));
    }
    let component = state
        .prepared_solve_cache
        .get(&plan.key)
        .ok_or("initialization prepared artifact was not retained")?;
    observe_initialization(component, plan, &probes, &source_state_names(&compiled))
        .map_err(|error| error.to_string())
}

fn observe_initialization(
    component: &rumoca_ir_solve::fmi::FmiComponent,
    plan: LiveBuildPlan,
    probes: &[String],
    state_names: &[String],
) -> Result<ModelicaInitializationOutcome, rumoca_sim::SimulationDiagnosticError> {
    let started = web_time::Instant::now();
    let requested = probes
        .iter()
        .chain(state_names)
        .cloned()
        .collect::<Vec<_>>();
    let mut values = rumoca_sim::initialize_fmi_component(component, &plan.options, &requested)?;
    let state_values = state_names
        .iter()
        .cloned()
        .zip(values.split_off(probes.len()))
        .collect();
    Ok(ModelicaInitializationOutcome {
        values,
        state_values,
        solver: plan.snapshot,
        construction_profile: lunco_modelica_runtime::ModelicaConstructionProfile {
            queue_wait_ns: None,
            // The host operation includes the requested probes. It performs
            // no metadata collection or separate post-construction observation.
            session_ns: started.elapsed().as_nanos() as u64,
            metadata_ns: 0,
            initial_observation_ns: 0,
            native_code_cache: None,
        },
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct PreparationRequests {
    active: std::collections::BTreeSet<ModelicaPreparationId>,
    responses: ModelicaResponseSender,
}

#[cfg(not(target_arch = "wasm32"))]
impl PreparationRequests {
    pub(super) fn new(responses: ModelicaResponseSender) -> Self {
        Self {
            active: Default::default(),
            responses,
        }
    }

    pub(super) fn admit(&mut self, request_id: ModelicaPreparationId) {
        if !self.active.insert(request_id) {
            self.finish(
                request_id,
                Err("duplicate preparation request identity".into()),
            );
        }
    }

    pub(super) fn contains(&self, request_id: ModelicaPreparationId) -> bool {
        self.active.contains(&request_id)
    }

    pub(super) fn finish(
        &mut self,
        request_id: ModelicaPreparationId,
        outcome: Result<ModelicaPreparationOutcome, String>,
    ) {
        if self.active.remove(&request_id) {
            let _ = self.responses.send(ModelicaPreparationResult {
                request_id,
                outcome,
            });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for PreparationRequests {
    fn drop(&mut self) {
        for request_id in std::mem::take(&mut self.active) {
            let _ = self.responses.send(ModelicaPreparationResult {
                request_id,
                outcome: Err("Modelica worker retired before preparation completed".into()),
            });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct PendingInitialization {
    pub(super) request_id: ModelicaPreparationId,
    pub(super) model_name: String,
    pub(super) parameter_overrides: Vec<(String, f64)>,
    pub(super) initial_inputs: Vec<(String, f64)>,
    pub(super) probes: Vec<String>,
    pub(super) library_gen: u64,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct PendingInterface {
    pub(super) request_id: ModelicaPreparationId,
    pub(super) entity: Entity,
    pub(super) session_id: u64,
    pub(super) model_name: String,
    pub(super) doc_uri: String,
    pub(super) library_gen: u64,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) enum PendingRequest {
    Interface(PendingInterface),
    Participant(PendingCompile),
    Initialization(PendingInitialization),
}

#[cfg(not(target_arch = "wasm32"))]
impl PendingRequest {
    pub(super) fn participant(&self) -> Option<&PendingCompile> {
        match self {
            Self::Participant(work) => Some(work),
            Self::Initialization(_) | Self::Interface(_) => None,
        }
    }
    pub(super) fn participant_mut(&mut self) -> Option<&mut PendingCompile> {
        match self {
            Self::Participant(work) => Some(work),
            Self::Initialization(_) | Self::Interface(_) => None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct InitializationWork {
    pub(super) request_id: ModelicaPreparationId,
    pub(super) model_name: String,
    pub(super) comp_res: CompiledModelArtifact,
    pub(super) plan: LiveBuildPlan,
    pub(super) probes: Vec<String>,
    pub(super) library_gen: u64,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn compiled_work(
    pending: PendingInitialization,
    artifact: BackendCompileResult,
) -> Result<InitializationWork, String> {
    let comp_res = artifact.outcome?;
    let parameters = merge_instance_values(
        &pending.parameter_overrides,
        &comp_res.authored_bindings.parameter_overrides,
    );
    let inputs = merge_instance_values(
        &comp_res.authored_bindings.initial_inputs,
        &pending.initial_inputs,
    );
    let plan = live_build_plan(
        solver::RuntimeProfile {
            live: true,
            predicted: false,
        },
        &parameters,
        &inputs,
        comp_res.prepared_source_key,
        &comp_res.structural_overrides,
        Some(artifact.library_revision),
    )
    .map_err(|error| error.to_string())?;
    Ok(InitializationWork {
        request_id: pending.request_id,
        model_name: pending.model_name,
        comp_res,
        plan,
        probes: pending.probes,
        library_gen: pending.library_gen,
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) enum PreparationWork {
    Participant(CompileWork),
    Initialization(InitializationWork),
}

#[cfg(not(target_arch = "wasm32"))]
impl PreparationWork {
    pub(super) fn participant(&self) -> Option<&CompileWork> {
        match self {
            Self::Participant(work) => Some(work),
            Self::Initialization(_) => None,
        }
    }
    pub(super) fn participant_mut(&mut self) -> Option<&mut CompileWork> {
        match self {
            Self::Participant(work) => Some(work),
            Self::Initialization(_) => None,
        }
    }
    pub(super) fn model_name(&self) -> &str {
        match self {
            Self::Participant(work) => &work.model_name,
            Self::Initialization(work) => &work.model_name,
        }
    }
    pub(super) fn plan(&self) -> &LiveBuildPlan {
        match self {
            Self::Participant(work) => &work.plan,
            Self::Initialization(work) => &work.plan,
        }
    }
    pub(super) fn compiled(&self) -> &CompiledModelArtifact {
        match self {
            Self::Participant(work) => &work.comp_res,
            Self::Initialization(work) => &work.comp_res,
        }
    }

    pub(super) fn current(
        &self,
        sessions: &HashMap<Entity, u64>,
        library_gen: u64,
        requests: &mut PreparationRequests,
        responses: &ModelicaResponseSender,
    ) -> bool {
        match self {
            Self::Participant(work) => {
                if work.cancelled || sessions.get(&work.entity).copied() != Some(work.session_id) {
                    return false;
                }
                if work.library_gen != library_gen {
                    compile_work_error(
                        responses,
                        work,
                        &rumoca_sim::SimulationDiagnosticError::Solver(
                            "source library changed during participant preparation".into(),
                        ),
                    );
                    return false;
                }
                true
            }
            Self::Initialization(work) => {
                if work.library_gen != library_gen {
                    requests.finish(
                        work.request_id,
                        Err("source library changed during initialization".into()),
                    );
                }
                requests.contains(work.request_id)
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) enum ConstructedPreparation {
    Participant(BuiltStepper<SessionHandle>),
    Initialization(ModelicaInitializationOutcome),
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn dispatch(
    id: u64,
    lane: thread_pool::Lane<()>,
    component: Arc<rumoca_ir_solve::fmi::FmiComponent>,
    work: &InitializationWork,
    pool: &ModelicaWorkerPools,
) -> Result<(), String> {
    let plan = work.plan.clone();
    let probes = work.probes.clone();
    let state_names = source_state_names(&work.comp_res);
    let native_cache = pool.native_cache.clone();
    let tx = pool.tx.clone();
    let queued = web_time::Instant::now();
    lane.submit(move |_| {
        let queue_wait_ns = queued.elapsed().as_nanos() as u64;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            native_cache
                .construct(|| observe_initialization(&component, plan, &probes, &state_names))
        }))
        .unwrap_or_else(|_| {
            Err(rumoca_sim::SimulationDiagnosticError::Solver(
                "initialization observation panicked".into(),
            ))
        })
        .map(|(mut outcome, native_profile)| {
            outcome.construction_profile.queue_wait_ns = Some(queue_wait_ns);
            outcome.construction_profile.native_code_cache = Some(native_profile);
            outcome
        });
        let _ = tx.send(WorkerPreparationResult::Initialization { id, result });
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn preparation_requests_have_one_terminal_owner_result() {
        let (participant_tx, participant_rx) = crossbeam_channel::unbounded();
        let (initialization_tx, preparation_rx) = crossbeam_channel::unbounded();
        let responses = ModelicaResponseSender::new(participant_tx, initialization_tx);
        let first = ModelicaPreparationId {
            scene_generation: 1,
            sequence: 0,
        };
        let second = ModelicaPreparationId {
            scene_generation: 2,
            sequence: 0,
        };
        let mut requests = PreparationRequests::new(responses);
        requests.admit(first);
        requests.admit(second);
        requests.finish(first, Err("cancelled".into()));
        requests.finish(first, Err("late completion".into()));
        let result = preparation_rx
            .try_recv()
            .expect("cancelled request completes");
        assert_eq!(result.request_id, first);
        assert_eq!(result.outcome.err().as_deref(), Some("cancelled"));
        drop(requests);
        let result = preparation_rx
            .try_recv()
            .expect("worker retirement completes pending requests");
        assert_eq!(result.request_id, second);
        assert!(result.outcome.is_err());
        assert!(preparation_rx.try_recv().is_err());
        assert!(participant_rx.try_recv().is_err());
    }
}
