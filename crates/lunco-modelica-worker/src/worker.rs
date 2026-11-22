//! Off-thread Modelica simulation worker + Bevy bridge.
//!
//! `modelica_worker` admits commands and ordered results on its own OS thread.
//! Bounded persistent solver workers own the `!Send` live sessions. The
//! Bevy systems `spawn_modelica_requests` and
//! `handle_modelica_responses` exchange `ModelicaCommand` /
//! `ModelicaResult` messages with it via crossbeam channels.

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashSet;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::{Receiver, Sender};

use lunco_experiments::solver;
use lunco_modelica_compiler::{ModelicaCompiler, PreparedSourceRoot};
use lunco_modelica_runtime::{
    CompileRequested, InFlightModelicaStep, LoadSourceRootPayload, MAX_MACRO_STEP_DT,
    ModelicaCacheLimits, ModelicaChannels, ModelicaCommand, ModelicaInitializationOutcome,
    ModelicaInterfaceOutcome, ModelicaLiveSolverSnapshot, ModelicaModel, ModelicaNotice,
    ModelicaPreparationOutcome, ModelicaPreparationResult, ModelicaResult, ModelicaRuntimeProfile,
    ModelicaSessionStarted, ModelicaSolverCapabilities, ModelicaSolverParameters,
    ModelicaVariables, ModelicaWorkerResponse, NoticeLevel,
};
#[cfg(not(target_arch = "wasm32"))]
use lunco_modelica_runtime::{ModelicaPreparationId, ModelicaResponseSender};
use lunco_modelica_solver::simulation_session::LiveStepper;
use rumoca_core::VarName;

mod initialization;
#[cfg(not(target_arch = "wasm32"))]
use initialization::{
    ConstructedPreparation, PendingRequest, PreparationRequests, PreparationWork,
};

mod cache;
use cache::CompiledArtifactCache;
#[cfg(not(target_arch = "wasm32"))]
pub use cache::ModelicaArtifactDiskLimits;
mod bridge;
mod compile;
pub use bridge::{
    ModelicaResponseBacklog, handle_modelica_responses, on_remove_modelica,
    reconcile_modelica_preparation_progress, request_modelica_compiles, retire_closed_twin_models,
    retire_replication_models, spawn_modelica_requests, validate_modelica_variable_ownership,
};
pub use compile::{
    PendingCompileAdmissions, PendingLifecycleCompletion, dispatch_modelica_compile_requests,
    reset_modelica_document, set_modelica_document_run_intent,
};
#[cfg(not(target_arch = "wasm32"))]
mod compiler_actor;
#[cfg(not(target_arch = "wasm32"))]
mod thread_pool;
#[cfg(not(target_arch = "wasm32"))]
use thread_pool::ThreadAffinePool;
#[cfg(not(target_arch = "wasm32"))]
mod live_session;
#[cfg(not(target_arch = "wasm32"))]
use live_session::{LiveConstructionResult, SessionHandle};
#[cfg(not(target_arch = "wasm32"))]
mod scheduling;
#[cfg(not(target_arch = "wasm32"))]
use compiler_actor::{CompilerActor, CompilerCompletion};
#[cfg(not(target_arch = "wasm32"))]
use scheduling::{
    PreparationOrder, enqueue_command, pending_preparation_entities, pop_ready_in_order,
    promote_unblocked_steps, take_runnable_compile_command, take_runnable_steps,
};

fn diagnostics_from_sim_error(
    err: &rumoca_sim::SimulationDiagnosticError,
    source: &str,
) -> Vec<lunco_doc::Diagnostic> {
    use lunco_doc::Diagnostic;

    let message = format!("[{}] {err}", err.diagnostic_code());
    match err.source_span() {
        Some(span) if span.start.0 <= source.len() => {
            let (line, column) = lunco_modelica_document::document::core::byte_offset_to_line_col(
                source,
                span.start.0,
            );
            vec![Diagnostic::error(message, Some(line), Some(column))]
        }
        _ => vec![Diagnostic::message_only(message)],
    }
}

/// Live solver selection and numerical settings. The shared master supplies
/// exact communication points; the selected backend owns internal integration.
/// Adaptive sessions use Rumoca's Model Exchange driver, while prediction
/// requires the registered fixed-step deterministic capability. Live tolerance
/// and initial/internal step size are independent of offline experiment settings.
fn live_stepper_options(
    profile: solver::RuntimeProfile,
) -> Result<
    (
        solver::SolverSpec,
        solver::SolverParams,
        rumoca_sim::SimOptions,
    ),
    solver::SolverError,
> {
    lunco_modelica_solver::solver_backends::ensure_builtin_solvers();

    let spec = solver::resolve(&solver::SolverRequest {
        profile,
        // The live path takes no authored override: an `experiment(...)`
        // annotation is an offline knob, and the same reasoning that keeps its
        // tolerance out of this loop keeps its solver out.
        authored: None,
    })?;

    let parameters = solver::SolverParams {
        atol: LIVE_TOL,
        rtol: LIVE_TOL,
        // Fixed prediction owns this internal lattice. Adaptive sessions
        // consume the same numerical setting through Rumoca's options adapter.
        h0: Some(LIVE_INTERNAL_STEP_SECS),
        // Live sessions are open. This span sets the initial root-scan scale;
        // it is not a stop time.
        t_start: 0.0,
        t_end: LIVE_INTERNAL_STEP_SECS,
    };
    let options = lunco_modelica_solver::solver_backends::rumoca_options(&spec, &parameters)?;
    Ok((spec, parameters, options))
}

/// Resolve execution requirements for one live participant. Prediction is
/// admitted only by a fixed-step deterministic backend.
fn profile_for(
    entity: Entity,
    realtime_models: &std::collections::HashSet<Entity>,
) -> solver::RuntimeProfile {
    solver::RuntimeProfile {
        // Everything the worker steps is driven by the frame loop.
        live: true,
        predicted: realtime_models.contains(&entity),
    }
}

fn canonical_parameter_overrides(values: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut canonical = values.to_vec();
    canonical.sort_unstable_by(|(left_name, left_value), (right_name, right_value)| {
        left_name
            .cmp(right_name)
            .then_with(|| left_value.to_bits().cmp(&right_value.to_bits()))
    });
    canonical
}

#[derive(Clone)]
struct LiveBuildPlan {
    spec: solver::SolverSpec,
    snapshot: Arc<ModelicaLiveSolverSnapshot>,
    options: rumoca_sim::SimOptions,
    initial_state_values: Option<Vec<(String, f64)>>,
}

fn source_state_names(compiled: &CompiledModelArtifact) -> Vec<String> {
    compiled.compiled.dae.inspect(|view| {
        view.variables()
            .filter(|(_, variable)| variable.role() == rumoca_ir_dae::VariableRole::State)
            .flat_map(|(_, variable)| variable.scalar_names())
            .collect()
    })
}

impl LiveBuildPlan {
    fn with_initial_state_values(
        mut self,
        compiled: &CompiledModelArtifact,
        values: Option<&[(String, f64)]>,
    ) -> Result<Self, rumoca_sim::SimulationDiagnosticError> {
        if let Some(values) = values {
            let declared = source_state_names(compiled)
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            let mut seen = std::collections::BTreeSet::new();
            for (name, value) in values {
                if !declared.contains(name) || !value.is_finite() || !seen.insert(name) {
                    return Err(rumoca_sim::SimulationDiagnosticError::Solver(format!(
                        "invalid or duplicate coupled initial state `{name}`; a finite declared continuous state is required"
                    )));
                }
            }
            self.initial_state_values = Some(values.to_vec());
        }
        Ok(self)
    }
}

type BuiltStepper<S> = (
    S,
    Arc<ModelicaLiveSolverSnapshot>,
    std::collections::BTreeMap<String, lunco_modelica_runtime::ModelicaScalarMetadata>,
    Vec<(String, f64)>,
    lunco_modelica_runtime::ModelicaConstructionProfile,
    LiveBuildPlan,
);

impl LiveBuildPlan {
    fn with_initial_inputs(&self, inputs: &[(String, f64)]) -> Self {
        let mut plan = self.clone();
        let inputs = canonical_parameter_overrides(inputs);
        plan.options.initial_inputs = inputs.clone();
        Arc::make_mut(&mut plan.snapshot).initial_inputs = inputs;
        plan
    }
}

#[cfg(test)]
fn test_checked_interface() -> Arc<lunco_modelica_ast::resolved_interface::ResolvedModelicaInterface>
{
    Arc::new(
        lunco_modelica_ast::resolved_interface::ResolvedModelicaInterface {
            model_name: "Plant".into(),
            declarations: Default::default(),
            input_groups: Vec::new(),
        },
    )
}

fn live_build_plan(
    profile: solver::RuntimeProfile,
    parameter_overrides: &[(String, f64)],
    initial_inputs: &[(String, f64)],
    _source_key: u64,
    structural_overrides: &[(String, f64)],
    _admitted_library_revision: Option<u64>,
) -> Result<LiveBuildPlan, rumoca_sim::SimulationDiagnosticError> {
    let parameter_overrides = canonical_parameter_overrides(parameter_overrides);
    let (spec, parameters, mut options) = live_stepper_options(profile).map_err(|e| {
        rumoca_sim::SimulationDiagnosticError::Solver(format!("solver selection failed: {e}"))
    })?;
    for (name, value) in structural_overrides {
        if !parameter_overrides
            .iter()
            .any(|(key, requested)| key == name && requested.to_bits() == value.to_bits())
        {
            return Err(rumoca_sim::SimulationDiagnosticError::Solver(format!(
                "compiled structural parameter `{name}` no longer matches the requested value; recompilation is required"
            )));
        }
    }
    // Rumoca instantiation owns structural values; lowering owns tunables and
    // recomputes their dependent bindings and initial-equation states.
    options.param_overrides = parameter_overrides
        .iter()
        .filter(|(name, _)| !structural_overrides.iter().any(|(key, _)| key == name))
        .cloned()
        .collect();
    let initial_inputs = canonical_parameter_overrides(initial_inputs);
    options.initial_inputs = initial_inputs.clone();
    let snapshot = Arc::new(ModelicaLiveSolverSnapshot {
        admitted_library_revision: _admitted_library_revision,
        solver_id: spec.id.to_string(),
        capabilities: ModelicaSolverCapabilities {
            usable_live: spec.caps.usable_live,
            fixed_step: spec.caps.fixed_step,
            deterministic: spec.caps.deterministic,
        },
        profile: ModelicaRuntimeProfile {
            live: profile.live,
            predicted: profile.predicted,
        },
        parameters: ModelicaSolverParameters {
            atol: parameters.atol,
            rtol: parameters.rtol,
            h0: parameters.h0,
            t_start: parameters.t_start,
            t_end: parameters.t_end,
        },
        parameter_overrides: parameter_overrides.clone(),
        initial_inputs: initial_inputs.clone(),
        initial_state_overrides: Vec::new(),
    });
    Ok(LiveBuildPlan {
        spec,
        snapshot,
        options,
        initial_state_values: None,
    })
}

#[cfg(target_arch = "wasm32")]
fn build_stepper(
    comp_res: &CompiledModelArtifact,
    profile: solver::RuntimeProfile,
    parameter_overrides: &[(String, f64)],
    initial_inputs: &[(String, f64)],
    initial_state_values: Option<&[(String, f64)]>,
    source_key: u64,
    library_revision: Option<u64>,
) -> Result<BuiltStepper<LiveStepper>, rumoca_sim::SimulationDiagnosticError> {
    let plan = {
        let _span = bevy::log::info_span!("modelica_live_plan_admission").entered();
        live_build_plan(
            profile,
            parameter_overrides,
            initial_inputs,
            source_key,
            &comp_res.structural_overrides,
            library_revision,
        )?
        .with_initial_state_values(comp_res, initial_state_values)?
    };
    build_live_stepper(&comp_res.compiled.dae, plan)
}

fn build_live_stepper(
    dae: &rumoca_ir_dae::Dae,
    mut plan: LiveBuildPlan,
) -> Result<BuiltStepper<LiveStepper>, rumoca_sim::SimulationDiagnosticError> {
    let started = web_time::Instant::now();
    if let Some(values) = &plan.initial_state_values {
        plan.options.start_overrides = canonical_parameter_overrides(values);
        Arc::make_mut(&mut plan.snapshot).initial_state_overrides = values.clone();
    }
    let retained = plan.clone();
    let stepper = {
        let _span = bevy::log::info_span!("modelica_live_session_construct").entered();
        lunco_modelica_solver::simulation_session::live_for_solver(
            dae,
            &plan.spec,
            plan.options.clone(),
        )?
    };
    let constructed = web_time::Instant::now();
    let variable_metadata = {
        let _span = bevy::log::info_span!("modelica_compiled_scalar_metadata").entered();
        lunco_modelica_compiler::ModelicaCompiler::scalar_declarations_from_checked_dae(
            dae,
            &plan.options.param_overrides,
        )
        .map_err(rumoca_sim::SimulationDiagnosticError::Solver)?
        .into_iter()
        .map(|(name, declaration)| {
            let bound = |value: Option<&lunco_modelica_ast::resolved_interface::ModelicaScalarBound>| {
                match value {
                    Some(lunco_modelica_ast::resolved_interface::ModelicaScalarBound::Known(value)) => Some(*value),
                    Some(lunco_modelica_ast::resolved_interface::ModelicaScalarBound::InitializationDependent) | None => None,
                }
            };
            (
                name,
                lunco_modelica_runtime::ModelicaScalarMetadata {
                    unit: declaration.unit,
                    description: declaration.description,
                    minimum: bound(declaration.minimum.as_ref()),
                    maximum: bound(declaration.maximum.as_ref()),
                },
            )
        })
        .collect()
    };
    let metadata_collected = web_time::Instant::now();
    let mut symbols = {
        let _span = bevy::log::info_span!("modelica_initial_observation").entered();
        collect_stepper_observables(&stepper)?
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect::<Vec<_>>()
    };
    symbols.extend(stepper.parameter_values().iter().cloned());
    if let Some(expected) = &plan.initial_state_values {
        for (name, expected) in expected {
            let actual = stepper.get(name)?.ok_or_else(|| {
                rumoca_sim::SimulationDiagnosticError::Solver(format!(
                    "initialized participant cannot observe coupled source state `{name}`"
                ))
            })?;
            let tolerance = plan.snapshot.parameters.atol
                + plan.snapshot.parameters.rtol * expected.abs().max(actual.abs());
            if !actual.is_finite() || (actual - expected).abs() > tolerance {
                return Err(rumoca_sim::SimulationDiagnosticError::Solver(format!(
                    "initialized state `{name}` is {actual}, coupled admission requires {expected}"
                )));
            }
        }
    }
    let construction_profile = lunco_modelica_runtime::ModelicaConstructionProfile {
        queue_wait_ns: None,
        session_ns: constructed.duration_since(started).as_nanos() as u64,
        metadata_ns: metadata_collected.duration_since(constructed).as_nanos() as u64,
        initial_observation_ns: metadata_collected.elapsed().as_nanos() as u64,
    };
    Ok((
        stepper,
        plan.snapshot,
        variable_metadata,
        symbols,
        construction_profile,
        retained,
    ))
}

#[cfg(not(target_arch = "wasm32"))]
/// Independent native preparation and live-owner budgets captured at startup.
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct ModelicaWorkerLimits {
    pub preparation: WorkerThreadBudget,
    pub live_execution: WorkerThreadBudget,
    pub document_execution: WorkerThreadBudget,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for ModelicaWorkerLimits {
    fn default() -> Self {
        Self {
            preparation: WorkerThreadBudget::default(),
            live_execution: WorkerThreadBudget::default(),
            document_execution: WorkerThreadBudget {
                max_worker_threads: 1,
                reserved_cpu_threads: 2,
            },
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug)]
pub struct WorkerThreadBudget {
    pub max_worker_threads: usize,
    pub reserved_cpu_threads: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for WorkerThreadBudget {
    fn default() -> Self {
        Self {
            max_worker_threads: 6,
            reserved_cpu_threads: 2,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl WorkerThreadBudget {
    fn worker_threads(&self, available: std::num::NonZeroUsize) -> Result<usize, String> {
        if self.max_worker_threads == 0 {
            return Err("Modelica worker capacity must be positive".to_owned());
        }
        Ok(available
            .get()
            .saturating_sub(self.reserved_cpu_threads)
            .max(1)
            .min(self.max_worker_threads))
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Isolates source/FMI preparation from bounded thread-affine live owners.
/// Each session is constructed, stepped and retired on one persistent thread;
/// the command owner admits only typed observations in command order.
struct ModelicaWorkerPools {
    pool: ThreadAffinePool<()>,
    live_pool: ThreadAffinePool<HashMap<u64, LiveStepper>>,
    document_pool: ThreadAffinePool<HashMap<u64, LiveStepper>>,
    tx: Sender<WorkerPreparationResult>,
    rx: Receiver<WorkerPreparationResult>,
    next_id: u64,
}

fn prepare_source_root_payload(id: String, payload: LoadSourceRootPayload) -> PreparedSourceRoot {
    let (label, files, diagnostics) = match payload {
        LoadSourceRootPayload::SourceLibrary => return PreparedSourceRoot::from_source_library(id),
        LoadSourceRootPayload::Disk { root_dir } => {
            let (files, diagnostics) =
                lunco_assets_runtime::discovery::read_files_with_extension(&root_dir, "mo");
            let diagnostics = diagnostics
                .into_iter()
                .map(|detail| format!("source root `{id}`: {detail}"))
                .collect();
            (root_dir.display().to_string(), files, diagnostics)
        }
        LoadSourceRootPayload::BundledModel { filename } => {
            let label = format!("bundled:{filename}");
            match lunco_assets_runtime::models::model_source(&filename) {
                Ok(Some(source)) => (label, vec![(filename, source)], Vec::new()),
                Ok(None) => (
                    label,
                    Vec::new(),
                    vec![format!("Modelica asset `{filename}` was not found")],
                ),
                Err(error) => (
                    label,
                    Vec::new(),
                    vec![format!("cannot load Modelica asset `{filename}`: {error}")],
                ),
            }
        }
        LoadSourceRootPayload::BundledPackage { root } => {
            let label = format!("bundled:{root}");
            match lunco_assets_runtime::models::package_files(&root) {
                Ok(files) if !files.is_empty() => (label, files, Vec::new()),
                Ok(_) => (
                    label,
                    Vec::new(),
                    vec![format!("Modelica package `{root}` has no source files")],
                ),
                Err(error) => (
                    label,
                    Vec::new(),
                    vec![format!("cannot load Modelica package `{root}`: {error}")],
                ),
            }
        }
        LoadSourceRootPayload::WorkspaceFile { path } => {
            let label = format!("workspace:{}", path.display());
            let uri = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace.mo")
                .to_owned();
            match lunco_modelica_runtime::source_asset::read_text_sync(&path) {
                Ok(source) => (label, vec![(uri, source)], Vec::new()),
                Err(error) => (
                    label,
                    Vec::new(),
                    vec![format!(
                        "workspace file `{}` read failed: {error}",
                        path.display()
                    )],
                ),
            }
        }
        LoadSourceRootPayload::InMemory { label, files } => (label, files, Vec::new()),
    };
    PreparedSourceRoot::prepare(id, label, files, diagnostics)
}

#[cfg(not(target_arch = "wasm32"))]
impl ModelicaWorkerPools {
    fn new(limits: ModelicaWorkerLimits) -> Result<Self, String> {
        let available = std::thread::available_parallelism()
            .map_err(|error| format!("cannot determine Modelica worker CPU capacity: {error}"))?;
        let preparation_threads = limits
            .preparation
            .worker_threads(available)
            .map_err(|error| format!("invalid preparation budget: {error}"))?;
        let live_threads = limits
            .live_execution
            .worker_threads(available)
            .map_err(|error| format!("invalid live-execution budget: {error}"))?;
        let document_threads = limits
            .document_execution
            .worker_threads(available)
            .map_err(|error| format!("invalid document-execution budget: {error}"))?;
        let (tx, rx) = crossbeam_channel::unbounded();
        let failure_tx = tx.clone();
        let pool = ThreadAffinePool::new(
            "modelica-prepare",
            preparation_threads,
            || (),
            move |error| {
                let _ = failure_tx.send(WorkerPreparationResult::Failed(error));
            },
        )?;
        let failure_tx = tx.clone();
        let live_pool =
            ThreadAffinePool::new("modelica-live", live_threads, HashMap::new, move |error| {
                let _ = failure_tx.send(WorkerPreparationResult::Failed(error));
            })?;
        let failure_tx = tx.clone();
        let document_pool = ThreadAffinePool::new(
            "modelica-document",
            document_threads,
            HashMap::new,
            move |error| {
                let _ = failure_tx.send(WorkerPreparationResult::Failed(error));
            },
        )?;
        bevy::log::info!(
            "[modelica-runtime] solve preparation pool ready: {preparation_threads} worker thread(s)"
        );
        bevy::log::info!(
            "[modelica-runtime] live execution pool ready: {live_threads} worker thread(s)"
        );
        bevy::log::info!(
            "[modelica-runtime] document execution pool ready: {document_threads} worker thread(s)"
        );
        Ok(Self {
            pool,
            live_pool,
            document_pool,
            tx,
            rx,
            next_id: 0,
        })
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Bound the full compile-to-solve pipeline while leaving two entries of
    /// lookahead beyond the largest preparation/scene/document worker count. This lets
    /// the serialized Rumoca owner prepare later independent programs while
    /// the solve pool is busy, without allowing an unbounded DAE backlog.
    fn can_admit(&self, pending_operations: usize) -> bool {
        pending_operations
            < self
                .pool
                .worker_count()
                .max(self.live_pool.worker_count())
                .max(self.document_pool.worker_count())
                .saturating_add(2)
    }

    fn submit_source_root(
        &mut self,
        id: String,
        source_root_operation_id: u64,
        payload: LoadSourceRootPayload,
    ) -> u64 {
        let operation_id = self.allocate_id();
        let tx = self.tx.clone();
        self.pool.spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                prepare_source_root_payload(id.clone(), payload)
            }))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown panic payload");
                PreparedSourceRoot::prepare(
                    id.clone(),
                    format!("source-root:{id}"),
                    Vec::new(),
                    vec![format!(
                        "source root `{id}` preparation panicked: {message}"
                    )],
                )
            });
            if tx
                .send(WorkerPreparationResult::SourceRoot(
                    SourceRootPreparationResult {
                        id: operation_id,
                        root_id: id,
                        source_root_operation_id,
                        prepared: result,
                    },
                ))
                .is_err()
            {
                log::error!(
                    "[modelica-runtime] source-root preparation result dropped id={operation_id}"
                );
            }
        });
        operation_id
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum WorkerPreparationResult {
    Live(LiveConstructionResult),
    Initialization {
        id: u64,
        result: Result<ModelicaInitializationOutcome, rumoca_sim::SimulationDiagnosticError>,
    },
    SourceRoot(SourceRootPreparationResult),
    Compiler(CompilerCompletion),
    Failed(String),
}

#[cfg(not(target_arch = "wasm32"))]
enum ConstructionLane {
    Participant(thread_pool::Lane<HashMap<u64, LiveStepper>>),
    Initialization(thread_pool::Lane<()>),
}

#[cfg(not(target_arch = "wasm32"))]
struct LivePreparationPipeline {
    order: PreparationOrder,
    constructed:
        HashMap<u64, Result<ConstructedPreparation, Arc<rumoca_sim::SimulationDiagnosticError>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl LivePreparationPipeline {
    fn new() -> Self {
        Self {
            order: PreparationOrder::new(),
            constructed: HashMap::new(),
        }
    }

    fn has_ready(&self) -> bool {
        !self.constructed.is_empty()
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingCompile {
    execution_domain: lunco_modelica_runtime::ModelicaExecutionDomain,
    entity: Entity,
    session_id: u64,
    cancelled: bool,
    model_name: String,
    source: String,
    doc_uri: String,
    raw_extras: Vec<(String, String)>,
    parameter_overrides: Vec<(String, f64)>,
    initial_inputs: Vec<(String, f64)>,
    initial_state_values: Option<Vec<(String, f64)>>,
    library_gen: u64,
    intent: CompileIntent,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
enum CompileIntent {
    Compile,
    Reset,
    UpdateParameters,
    StepInit(StepRequest),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct StepRequest {
    entity: Entity,
    session_id: u64,
    step_id: u64,
    start_time: f64,
    stop_time: f64,
    model_name: String,
    inputs: Vec<(String, f64)>,
    dt: f64,
}

#[cfg(not(target_arch = "wasm32"))]
impl StepRequest {
    fn into_command(self) -> ModelicaCommand {
        ModelicaCommand::Step {
            entity: self.entity,
            session_id: self.session_id,
            step_id: self.step_id,
            start_time: self.start_time,
            stop_time: self.stop_time,
            model_name: self.model_name,
            inputs: self.inputs,
            dt: self.dt,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(not(target_arch = "wasm32"))]
struct SourceRootPreparationResult {
    id: u64,
    root_id: String,
    source_root_operation_id: u64,
    prepared: PreparedSourceRoot,
}

#[cfg(not(target_arch = "wasm32"))]
struct CompileWork {
    execution_domain: lunco_modelica_runtime::ModelicaExecutionDomain,
    entity: Entity,
    session_id: u64,
    /// The entity was despawned while this immutable preparation was still
    /// running. The pool job cannot be interrupted safely, so the work stays
    /// counted until its result is drained and is then discarded without
    /// touching runtime state.
    cancelled: bool,
    model_name: String,
    source: String,
    doc_uri: String,
    raw_extras: Vec<(String, String)>,
    parameter_overrides: Vec<(String, f64)>,
    initial_inputs: Vec<(String, f64)>,
    initial_state_values: Option<Vec<(String, f64)>>,
    unit: CompileUnit,
    comp_res: CompiledModelArtifact,
    library_gen: u64,
    plan: LiveBuildPlan,
    intent: CompileIntent,
}

#[cfg(not(target_arch = "wasm32"))]
fn compile_work_error(
    tx: &ModelicaResponseSender,
    work: &CompileWork,
    error: &rumoca_sim::SimulationDiagnosticError,
) {
    send_compile_stepper_error(
        tx,
        work.entity,
        work.session_id,
        &work.unit.source,
        error,
        &work.intent,
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn send_compile_stepper_error(
    tx: &ModelicaResponseSender,
    entity: Entity,
    session_id: u64,
    source: &str,
    error: &rumoca_sim::SimulationDiagnosticError,
    intent: &CompileIntent,
) {
    let mut result = result_for_compile_intent(entity, session_id, intent);
    result.error = Some(match intent {
        CompileIntent::Compile => format!("Stepper Error: {error}"),
        CompileIntent::Reset => format!("Reset stepper error: {error}"),
        CompileIntent::UpdateParameters => format!("Parameter update stepper error: {error}"),
        CompileIntent::StepInit(_) => format!("Initialization Failed: stepper init: {error}"),
    });
    result.compile_diagnostics = diagnostics_from_sim_error(error, source);
    let _ = tx.send(result);
}

#[cfg(not(target_arch = "wasm32"))]
fn result_for_compile_intent(
    entity: Entity,
    session_id: u64,
    intent: &CompileIntent,
) -> ModelicaResult {
    match intent {
        CompileIntent::Compile => {
            let mut result = result_ok(entity, session_id);
            result.is_new_model = true;
            result
        }
        CompileIntent::Reset => {
            let mut result = result_ok(entity, session_id);
            result.is_reset = true;
            result
        }
        CompileIntent::UpdateParameters => {
            let mut result = result_ok(entity, session_id);
            result.is_parameter_update = true;
            result
        }
        CompileIntent::StepInit(step) => step_result_ok(entity, session_id, step.step_id),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn send_compile_artifact_error(
    tx: &ModelicaResponseSender,
    pending: &PendingCompile,
    message: String,
    diagnostics: Vec<lunco_doc::Diagnostic>,
) {
    let mut result = result_for_compile_intent(pending.entity, pending.session_id, &pending.intent);
    result.error = Some(match &pending.intent {
        CompileIntent::Compile => format!("Compiler Error: {message}"),
        CompileIntent::Reset => format!("Reset compile error: {message}"),
        CompileIntent::UpdateParameters => format!("Re-compile Error: {message}"),
        CompileIntent::StepInit(_) => {
            format!("Initialization Failed: recompile of cached source: {message}")
        }
    });
    result.compile_diagnostics = diagnostics;
    let _ = tx.send(result);
}

#[cfg(not(target_arch = "wasm32"))]
fn construction_lane(work: &PreparationWork, pool: &mut ModelicaWorkerPools) -> ConstructionLane {
    match work {
        PreparationWork::Participant(work) => {
            ConstructionLane::Participant(match work.execution_domain {
                lunco_modelica_runtime::ModelicaExecutionDomain::Scene => {
                    pool.live_pool.next_lane()
                }
                lunco_modelica_runtime::ModelicaExecutionDomain::Document => {
                    pool.document_pool.next_lane()
                }
            })
        }
        PreparationWork::Initialization(_) => {
            ConstructionLane::Initialization(pool.pool.next_lane())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn admit_preparation(
    work: PreparationWork,
    tx: &ModelicaResponseSender,
    pool: &mut ModelicaWorkerPools,
    pipeline: &mut LivePreparationPipeline,
    pending: &mut HashMap<u64, PreparationWork>,
) {
    let id = pool.allocate_id();
    let lane = construction_lane(&work, pool);
    pipeline.order.admit(id);
    dispatch_live_construction(id, lane, work, tx, pool, pipeline, pending);
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_live_construction(
    id: u64,
    lane: ConstructionLane,
    work: PreparationWork,
    tx: &ModelicaResponseSender,
    pool: &mut ModelicaWorkerPools,
    pipeline: &mut LivePreparationPipeline,
    pending: &mut HashMap<u64, PreparationWork>,
) {
    let plan = work.plan().clone();
    let lane = match (&work, lane) {
        (
            PreparationWork::Initialization(initialization),
            ConstructionLane::Initialization(lane),
        ) => {
            let dae = initialization.comp_res.compiled.dae.clone();
            let result = initialization::dispatch(id, lane, dae, initialization, pool);
            pending.insert(id, work);
            if let Err(error) = result {
                pipeline.constructed.insert(
                    id,
                    Err(Arc::new(rumoca_sim::SimulationDiagnosticError::Solver(
                        error,
                    ))),
                );
            }
            return;
        }
        (PreparationWork::Participant(_), ConstructionLane::Participant(lane)) => lane,
        _ => {
            pending.insert(id, work);
            pipeline.constructed.insert(
                id,
                Err(Arc::new(rumoca_sim::SimulationDiagnosticError::Solver(
                    "preparation construction lane does not match its owner".into(),
                ))),
            );
            return;
        }
    };
    let dae = work.compiled().compiled.dae.clone();
    pending.insert(id, work);
    if let Err(error) = live_session::construct(id, lane, dae, plan, pool.tx.clone()) {
        let _ = tx.send(ModelicaResult::worker_failure(error.clone()));
        pipeline.constructed.insert(
            id,
            Err(Arc::new(rumoca_sim::SimulationDiagnosticError::Solver(
                error,
            ))),
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn install_compile_work(
    work: CompileWork,
    stepper_result: Result<BuiltStepper<SessionHandle>, Arc<rumoca_sim::SimulationDiagnosticError>>,
    steppers: &mut HashMap<Entity, (u64, String, SessionHandle)>,
    cached_models: &mut HashMap<Entity, CachedModel>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &ModelicaResponseSender,
) {
    let _span = bevy::log::info_span!(
        "modelica_live_construction_commit",
        model = %work.model_name,
        entity = ?work.entity,
    )
    .entered();
    match stepper_result {
        Ok((
            stepper,
            live_solver_snapshot,
            variable_metadata,
            symbols,
            construction_profile,
            retained,
        )) => {
            send_preparation_phase(
                tx,
                work.entity,
                work.session_id,
                lunco_modelica_runtime::ModelicaPreparationPhase::LiveSolverConstructed(
                    construction_profile,
                ),
            );
            let CompileWork {
                execution_domain,
                entity,
                session_id,
                cancelled: _,
                model_name,
                source,
                doc_uri,
                raw_extras,
                parameter_overrides,
                initial_inputs,
                initial_state_values,
                unit,
                comp_res,
                library_gen,
                plan: _,
                intent,
            } = work;
            let (published_metadata, pending_metadata) =
                if matches!(&intent, CompileIntent::StepInit(_)) {
                    (None, Some(variable_metadata))
                } else {
                    (Some(variable_metadata), None)
                };
            let input_names = stepper.input_names().to_vec();
            let unit_hash = compile_unit_hash(&model_name, &doc_uri, &unit);
            cached_models.insert(
                entity,
                CachedModel {
                    execution_domain,
                    model_name: model_name.clone(),
                    source: Arc::from(source),
                    extra_sources: raw_extras,
                    parameter_overrides,
                    initial_inputs,
                    expected_library_revision: unit.expected_library_revision,
                    initial_state_values,
                    doc_uri,
                    compiled: comp_res.clone(),
                    unit_hash,
                    library_gen,
                    retained,
                    pending_variable_metadata: pending_metadata,
                },
            );
            steppers.insert(entity, (session_id, model_name.clone(), stepper));
            match intent {
                CompileIntent::Compile => {
                    let _ = tx.send(add_experiment_defaults(
                        ModelicaResult {
                            entity,
                            session_id,
                            new_time: 0.0,
                            outputs: Vec::new(),
                            detected_symbols: symbols,
                            error: None,
                            log_message: Some(format!("Model '{}' compiled.", model_name)),
                            is_new_model: true,
                            is_parameter_update: false,
                            is_reset: false,
                            detected_input_names: input_names,
                            compiled_model_name: Some(model_name),
                            loaded_source_root_id: None,
                            compile_diagnostics: Vec::new(),
                            live_solver_snapshot: Some(live_solver_snapshot.clone()),
                            variable_metadata: published_metadata,
                            ..Default::default()
                        },
                        &comp_res,
                    ));
                }
                CompileIntent::Reset => {
                    let mut result =
                        reset_ok(entity, session_id, symbols, input_names, "Reset complete.");
                    result.compile_diagnostics = Vec::new();
                    result.live_solver_snapshot = Some(live_solver_snapshot.clone());
                    result.variable_metadata = published_metadata;
                    let _ = tx.send(add_experiment_defaults(result, &comp_res));
                }
                CompileIntent::UpdateParameters => {
                    let _ = tx.send(add_experiment_defaults(
                        ModelicaResult {
                            entity,
                            session_id,
                            new_time: 0.0,
                            outputs: Vec::new(),
                            detected_symbols: symbols,
                            error: None,
                            log_message: Some("Parameters applied.".to_string()),
                            is_new_model: false,
                            is_parameter_update: true,
                            is_reset: false,
                            detected_input_names: input_names,
                            compile_diagnostics: Vec::new(),
                            live_solver_snapshot: Some(live_solver_snapshot),
                            variable_metadata: published_metadata,
                            ..Default::default()
                        },
                        &comp_res,
                    ));
                }
                CompileIntent::StepInit(step) => {
                    step_lane.push_front(step.into_command());
                }
            }
        }
        Err(error) => compile_work_error(tx, &work, &error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn stage_preparation_result(
    result: WorkerPreparationResult,
    pipeline: &mut LivePreparationPipeline,
    ready_source_roots: &mut HashMap<u64, SourceRootPreparationResult>,
    ready_compiler: &mut HashMap<u64, CompilerCompletion>,
    runtime_tx: &ModelicaResponseSender,
) -> bool {
    let duplicate = match result {
        WorkerPreparationResult::Live(result) => pipeline
            .constructed
            .insert(
                result.id,
                result
                    .result
                    .map(ConstructedPreparation::Participant)
                    .map_err(Arc::new),
            )
            .is_some(),
        WorkerPreparationResult::Initialization { id, result } => pipeline
            .constructed
            .insert(
                id,
                result
                    .map(ConstructedPreparation::Initialization)
                    .map_err(Arc::new),
            )
            .is_some(),
        WorkerPreparationResult::SourceRoot(result) => {
            ready_source_roots.insert(result.id, result).is_some()
        }
        WorkerPreparationResult::Compiler(result) => {
            let id = match &result {
                CompilerCompletion::Batch { id, .. }
                | CompilerCompletion::Interface { id, .. }
                | CompilerCompletion::Compile { id, .. }
                | CompilerCompletion::SourceRoot { id, .. }
                | CompilerCompletion::SourceRootUnload { id, .. } => *id,
            };
            ready_compiler.insert(id, result).is_some()
        }
        WorkerPreparationResult::Failed(error) => {
            let _ = runtime_tx.send(ModelicaResult::worker_failure(error));
            return false;
        }
    };
    if duplicate {
        let _ = runtime_tx.send(ModelicaResult::worker_failure(
            "duplicate preparation completion id".to_owned(),
        ));
        return false;
    }
    true
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_ready_session_constructions(
    pipeline: &mut LivePreparationPipeline,
    pending: &mut HashMap<u64, PreparationWork>,
    current_sessions: &HashMap<Entity, u64>,
    library_gen: u64,
    steppers: &mut HashMap<Entity, (u64, String, SessionHandle)>,
    cached_models: &mut HashMap<Entity, CachedModel>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &ModelicaResponseSender,
    preparation_requests: &mut PreparationRequests,
) {
    while let Some((id, result)) = pipeline.order.pop_constructed(&mut pipeline.constructed) {
        let Some(work) = pending.remove(&id) else {
            let _ = tx.send(ModelicaResult::worker_failure(format!(
                "live construction {id} has no admitted compile work"
            )));
            return;
        };
        if !work.current(current_sessions, library_gen, preparation_requests, tx) {
            continue;
        }
        match work {
            PreparationWork::Participant(work) => {
                let result = result.and_then(|result| match result {
                    ConstructedPreparation::Participant(result) => Ok(result),
                    ConstructedPreparation::Initialization(_) => {
                        Err(Arc::new(rumoca_sim::SimulationDiagnosticError::Solver(
                            "initialization result reached a participant owner".into(),
                        )))
                    }
                });
                install_compile_work(work, result, steppers, cached_models, step_lane, tx);
            }
            PreparationWork::Initialization(work) => {
                let result = result
                    .map_err(|error| error.to_string())
                    .and_then(|result| match result {
                        ConstructedPreparation::Initialization(result) => Ok(result),
                        ConstructedPreparation::Participant(_) => {
                            Err("participant result reached an initialization owner".into())
                        }
                    });
                preparation_requests.finish(
                    work.request_id,
                    result.map(ModelicaPreparationOutcome::Initialization),
                );
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_ready_source_roots(
    order: &mut VecDeque<u64>,
    ready: &mut HashMap<u64, SourceRootPreparationResult>,
    compiler: &mut CompilerActor,
    compiler_order: &mut VecDeque<u64>,
    pending_solve_preparations: usize,
    solve_preparation_pool: &ModelicaWorkerPools,
    pending_installs: &mut HashSet<u64>,
    latest_source_root_operations: &HashMap<String, u64>,
    tx: &ModelicaResponseSender,
) {
    loop {
        if !solve_preparation_pool
            .can_admit(compiler_order.len() + pending_solve_preparations + order.len())
        {
            return;
        }
        let Some(result) = pop_ready_in_order(order, ready) else {
            return;
        };
        let SourceRootPreparationResult {
            root_id,
            source_root_operation_id,
            prepared,
            ..
        } = result;
        if latest_source_root_operations.get(&root_id) != Some(&source_root_operation_id) {
            log::debug!(
                "[modelica-runtime] discarded stale source-root preparation `{root_id}` operation={source_root_operation_id}"
            );
            continue;
        }
        match compiler.submit_source_root(prepared, source_root_operation_id) {
            Ok(operation_id) => {
                compiler_order.push_back(operation_id);
                pending_installs.insert(operation_id);
            }
            Err((error, _prepared)) => {
                let _ = tx.send(ModelicaResult {
                    loaded_source_root_id: Some(root_id),
                    source_root_operation_id: Some(source_root_operation_id),
                    error: Some(error),
                    ..Default::default()
                });
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn admit_worker_command(
    command: ModelicaCommand,
    compile_lane: &mut VecDeque<ModelicaCommand>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    compiler: &mut CompilerActor,
    compiler_order: &mut VecDeque<u64>,
    pending_source_root_installs: &mut HashSet<u64>,
    latest_source_root_operations: &mut HashMap<String, u64>,
    tx: &ModelicaResponseSender,
    preparation_requests: &mut PreparationRequests,
) {
    match command {
        command @ (ModelicaCommand::Initialize { request_id, .. }
        | ModelicaCommand::PrepareInterface { request_id, .. }) => {
            preparation_requests.admit(request_id);
            enqueue_command(command, compile_lane, step_lane, tx);
        }
        ModelicaCommand::CancelPreparation { request_id } => {
            preparation_requests
                .finish(request_id, Err("preparation cancelled by its owner".into()));
        }
        ModelicaCommand::LoadSourceRoot {
            id,
            operation_id,
            payload,
        } => {
            if !record_source_root_operation(latest_source_root_operations, &id, operation_id) {
                log::debug!(
                    "[modelica-runtime] discarded stale source-root load `{id}` operation={operation_id}"
                );
                return;
            }
            enqueue_command(
                ModelicaCommand::LoadSourceRoot {
                    id,
                    operation_id,
                    payload,
                },
                compile_lane,
                step_lane,
                tx,
            );
        }
        ModelicaCommand::UnloadSourceRoot { id, operation_id } => {
            if !record_source_root_operation(latest_source_root_operations, &id, operation_id) {
                log::debug!(
                    "[modelica-runtime] discarded stale source-root unload `{id}` operation={operation_id}"
                );
                return;
            }
            match compiler.submit_source_root_unload(id.clone(), operation_id) {
                Ok(actor_operation_id) => {
                    compiler_order.push_back(actor_operation_id);
                    pending_source_root_installs.insert(actor_operation_id);
                }
                Err(error) => {
                    log::error!("[modelica-runtime] failed to queue unload for `{id}`: {error}");
                    retire_source_root_operation(latest_source_root_operations, &id, operation_id);
                    let _ = tx.send(ModelicaResult {
                        unloaded_source_root_id: Some(id),
                        source_root_operation_id: Some(operation_id),
                        error: Some(format!(
                            "failed to queue Modelica source-root unload: {error}"
                        )),
                        ..Default::default()
                    });
                }
            }
        }
        command => enqueue_command(command, compile_lane, step_lane, tx),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_ready_compiler_completions(
    order: &mut VecDeque<u64>,
    ready: &mut HashMap<u64, CompilerCompletion>,
    pending_installs: &mut HashSet<u64>,
    pending_compiles: &mut HashMap<u64, PendingRequest>,
    current_sessions: &HashMap<Entity, u64>,
    library_gen: &mut u64,
    library_revision: &mut u64,
    solve_preparation_pool: &mut ModelicaWorkerPools,
    live_preparation_pipeline: &mut LivePreparationPipeline,
    pending_compile_works: &mut HashMap<u64, PreparationWork>,
    realtime_models: &std::collections::HashSet<Entity>,
    latest_source_root_operations: &mut HashMap<String, u64>,
    tx: &ModelicaResponseSender,
    preparation_requests: &mut PreparationRequests,
) {
    while let Some(completion) = pop_ready_in_order(order, ready) {
        match completion {
            CompilerCompletion::Interface { id, artifact } => {
                let Some(PendingRequest::Interface(pending)) = pending_compiles.remove(&id) else {
                    let _ = tx.send(ModelicaResult::worker_failure(
                        "interface completion has no matching preparation owner",
                    ));
                    continue;
                };
                let outcome = if pending.library_gen != *library_gen {
                    Err("Modelica source library changed during interface preparation".into())
                } else {
                    artifact.outcome.and_then(|compiled| {
                        let interface = compiled
                            .edit_interface
                            .as_deref()
                            .ok_or("interface preparation produced no checked declarations")?
                            .clone();
                        Ok(ModelicaPreparationOutcome::Interface(
                            ModelicaInterfaceOutcome {
                                entity: pending.entity,
                                session_id: pending.session_id,
                                doc_uri: pending.doc_uri,
                                parameter_overrides: artifact.unit.parameter_overrides,
                                authored_bindings: artifact.unit.authored_bindings,
                                interface,
                                library_revision: artifact.library_revision,
                            },
                        ))
                    })
                };
                preparation_requests.finish(pending.request_id, outcome);
            }
            CompilerCompletion::Batch { reply, outcome, .. } => {
                let _ = reply.send(outcome);
            }
            CompilerCompletion::SourceRoot {
                id,
                source_root_operation_id,
                commit,
            } => {
                pending_installs.remove(&id);
                let library_changed =
                    commit.inserted_file_count > 0 || commit.library_revision != *library_revision;
                if library_changed {
                    *library_gen = library_gen.wrapping_add(1);
                }
                *library_revision = commit.library_revision;
                log::info!(
                    "[modelica-runtime] LoadSourceRoot `{}`: {} parsed / {} inserted",
                    commit.root_id,
                    commit.parsed_file_count,
                    commit.inserted_file_count,
                );
                let _ = tx.send(ModelicaResult {
                    loaded_source_root_id: Some(commit.root_id),
                    loaded_source_root_namespaces: commit.namespaces,
                    loaded_source_root_sources: commit.sources,
                    loaded_source_root_documents: commit.documents,
                    source_root_operation_id: Some(source_root_operation_id),
                    error: commit.error,
                    ..Default::default()
                });
            }
            CompilerCompletion::SourceRootUnload {
                id,
                root_id,
                source_root_operation_id,
                removed,
                error,
                library_revision: updated_revision,
                ..
            } => {
                pending_installs.remove(&id);
                if let Some(error) = error {
                    retire_source_root_operation(
                        latest_source_root_operations,
                        &root_id,
                        source_root_operation_id,
                    );
                    log::error!(
                        "[modelica-runtime] failed to unload Modelica source root `{root_id}` operation={source_root_operation_id}: {error}"
                    );
                    let _ = tx.send(ModelicaResult {
                        unloaded_source_root_id: Some(root_id),
                        source_root_operation_id: Some(source_root_operation_id),
                        error: Some(error),
                        ..Default::default()
                    });
                    continue;
                }
                retire_source_root_operation(
                    latest_source_root_operations,
                    &root_id,
                    source_root_operation_id,
                );
                let library_changed = removed || updated_revision != *library_revision;
                if library_changed {
                    *library_gen = library_gen.wrapping_add(1);
                }
                *library_revision = updated_revision;
                log::info!(
                    "[modelica-runtime] unloaded Modelica source root `{root_id}` operation={source_root_operation_id} (removed={removed})"
                );
                let _ = tx.send(ModelicaResult {
                    unloaded_source_root_id: Some(root_id),
                    source_root_operation_id: Some(source_root_operation_id),
                    ..Default::default()
                });
            }
            CompilerCompletion::Compile { id, artifact } => {
                let Some(pending) = pending_compiles.remove(&id) else {
                    log::error!(
                        "[modelica-runtime] Rumoca compile completion {id} has no pending command"
                    );
                    continue;
                };
                let mut pending = match pending {
                    PendingRequest::Interface(pending) => {
                        preparation_requests.finish(
                            pending.request_id,
                            Err("compile completion reached an interface preparation owner".into()),
                        );
                        continue;
                    }
                    PendingRequest::Participant(pending) => pending,
                    PendingRequest::Initialization(pending) => {
                        let request_id = pending.request_id;
                        if !preparation_requests.contains(request_id) {
                            continue;
                        }
                        if pending.library_gen != *library_gen {
                            preparation_requests.finish(
                                request_id,
                                Err("source library changed during initialization compilation"
                                    .into()),
                            );
                            continue;
                        }
                        match initialization::compiled_work(pending, artifact) {
                            Ok(work) => admit_preparation(
                                PreparationWork::Initialization(work),
                                tx,
                                solve_preparation_pool,
                                live_preparation_pipeline,
                                pending_compile_works,
                            ),
                            Err(error) => preparation_requests.finish(request_id, Err(error)),
                        }
                        continue;
                    }
                };
                if pending.cancelled
                    || current_sessions.get(&pending.entity).copied() != Some(pending.session_id)
                {
                    log::debug!(
                        "[modelica-runtime] discarded stale Rumoca compile for `{}` entity={:?} session={} library_gen={}",
                        pending.model_name,
                        pending.entity,
                        pending.session_id,
                        pending.library_gen,
                    );
                    continue;
                }
                if pending.library_gen != *library_gen {
                    send_compile_artifact_error(
                        tx,
                        &pending,
                        "source library changed during participant compilation".into(),
                        Vec::new(),
                    );
                    continue;
                }
                let comp_res = match artifact.outcome {
                    Ok(compiled) => compiled,
                    Err(error) => {
                        send_compile_artifact_error(tx, &pending, error, artifact.diagnostics);
                        continue;
                    }
                };
                let unit_key = comp_res.prepared_source_key;
                pending.parameter_overrides = merge_instance_values(
                    &pending.parameter_overrides,
                    &comp_res.authored_bindings.parameter_overrides,
                );
                pending.initial_inputs = merge_instance_values(
                    &comp_res.authored_bindings.initial_inputs,
                    &pending.initial_inputs,
                );
                let plan = match live_build_plan(
                    profile_for(pending.entity, realtime_models),
                    &pending.parameter_overrides,
                    &pending.initial_inputs,
                    unit_key,
                    &comp_res.structural_overrides,
                    Some(artifact.library_revision),
                )
                .and_then(|plan| {
                    plan.with_initial_state_values(
                        &comp_res,
                        pending.initial_state_values.as_deref(),
                    )
                }) {
                    Ok(plan) => plan,
                    Err(error) => {
                        send_compile_stepper_error(
                            tx,
                            pending.entity,
                            pending.session_id,
                            &artifact.unit.source,
                            &error,
                            &pending.intent,
                        );
                        continue;
                    }
                };
                let work = CompileWork {
                    execution_domain: pending.execution_domain,
                    entity: pending.entity,
                    session_id: pending.session_id,
                    cancelled: false,
                    model_name: pending.model_name,
                    source: pending.source,
                    doc_uri: pending.doc_uri,
                    raw_extras: pending.raw_extras,
                    parameter_overrides: pending.parameter_overrides,
                    initial_inputs: pending.initial_inputs,
                    initial_state_values: pending.initial_state_values,
                    unit: artifact.unit,
                    comp_res,
                    library_gen: pending.library_gen,
                    plan,
                    intent: pending.intent,
                };
                admit_preparation(
                    PreparationWork::Participant(work),
                    tx,
                    solve_preparation_pool,
                    live_preparation_pipeline,
                    pending_compile_works,
                );
            }
        }
    }
}

use std::sync::Arc;

/// Installed source/artifact context and its immutable checked preparation plan.
/// Reset prepares the retained checked DAE with the exact admitted plan and
/// current inputs, independent of mutable compiler revisions. Source-backed
/// recompilation is reserved for new authoring or Step auto-initialization.
struct CachedModel {
    #[cfg(not(target_arch = "wasm32"))]
    execution_domain: lunco_modelica_runtime::ModelicaExecutionDomain,
    model_name: String,
    source: Arc<str>,
    /// Sibling docs the model was compiled with, raw like `source`. Replayed
    /// on a cache-invalidating recompile so Step auto-init resolves
    /// the same source closure as Compile.
    extra_sources: Vec<(String, String)>,
    /// Instance parameter values passed to Rumoca session initialization. Reapplied if a
    /// library invalidates the artifact and it must be compiled again.
    parameter_overrides: Vec<(String, f64)>,
    initial_inputs: Vec<(String, f64)>,
    expected_library_revision: Option<u64>,
    initial_state_values: Option<Vec<(String, f64)>>,
    /// The document's stable session URI (see `ModelicaCommand::Compile`'s
    /// `doc_uri`). Every cached-source recompile — Step auto-init or
    /// UpdateParameters — re-seats under this SAME key so the reused rumoca
    /// session never holds the document under two filenames.
    doc_uri: String,
    /// The compiled artifact steppers are rebuilt from (see struct docs).
    compiled: CompiledModelArtifact,
    /// [`compile_unit_hash`] of the [`CompileUnit`] `compiled` was built from.
    unit_hash: u64,
    /// Worker library generation at the time `compiled` was built.
    library_gen: u64,
    /// Exact immutable plan used by the installed session.
    retained: LiveBuildPlan,
    pending_variable_metadata:
        Option<std::collections::BTreeMap<String, lunco_modelica_runtime::ModelicaScalarMetadata>>,
}

/// Key identifying WHAT a cached artifact was compiled from: the assembled
/// [`CompileUnit`] (original primary + extras), the model name, and
/// the session URI it was seated under. Library roots are covered separately
/// by the worker's library generation — they mutate the shared session, not
/// the unit.
fn compile_unit_hash(model_name: &str, doc_uri: &str, unit: &CompileUnit) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    model_name.hash(&mut h);
    doc_uri.hash(&mut h);
    unit.source.hash(&mut h);
    for (uri, text) in &unit.extras {
        uri.hash(&mut h);
        text.hash(&mut h);
    }
    h.finish()
}

/// A strict compiled artifact and the captured participating-source identity.
/// The identity is computed before the compiler clears its user overlays and
/// travels with the immutable DAE through memory hits and participant resets.
#[derive(Clone)]
struct CompiledModelArtifact {
    compiled: Box<CompiledRuntimeModel>,
    prepared_source_key: u64,
    /// Exact numeric intent already incorporated by Rumoca instantiation.
    structural_overrides: Vec<(String, f64)>,
    /// Values admitted against the checked instance for this exact request.
    authored_bindings: lunco_modelica_compiler::AdmittedModelicaBindings,
    /// Source authority and bounds for the admitted instance.
    edit_interface: Option<Arc<lunco_modelica_ast::resolved_interface::ResolvedModelicaInterface>>,
    /// Identity of the last completed request for this structural variant.
    /// Checked disk records carry the DAE and reconstruct this live projection.
    checked_request: Option<lunco_hash::content::Cid>,
    #[cfg(not(target_arch = "wasm32"))]
    source_content: lunco_hash::content::Cid,
    /// Exact source/root context and explicitly bound parameter name set.
    binding_schema: lunco_hash::content::Cid,
}

/// Runtime consumers retain the checked DAE and the authored execution defaults.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CompiledRuntimeModel {
    dae: Arc<rumoca_compile::compile::Dae>,
    experiment_start_time: Option<f64>,
    experiment_stop_time: Option<f64>,
    experiment_tolerance: Option<f64>,
    experiment_interval: Option<f64>,
    experiment_solver: Option<String>,
}

/// Fold the exact compiler-owned source CID into the worker's bounded memo key.
fn prepared_source_key(source_content: lunco_hash::content::Cid) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source_content.to_bytes().hash(&mut h);
    h.finish()
}

impl CachedModel {
    /// Reconstruct one complete admitted source and instance-parameter context.
    fn compile_unit(&self, source: &str) -> CompileUnit {
        let mut unit = assemble_compile_unit(source, self.extra_sources.clone());
        unit.parameter_overrides = canonical_parameter_overrides(&self.parameter_overrides);
        unit.expected_library_revision = self.expected_library_revision;
        unit
    }

    fn parameter_update_unit(&self, source: &str, name: String, value: f64) -> CompileUnit {
        let mut unit = self.compile_unit(source);
        unit.parameter_overrides =
            merge_instance_values(&unit.parameter_overrides, &[(name, value)]);
        unit
    }
}

fn validate_reset_target(
    cached: Option<&CachedModel>,
    installed: Option<(u64, &str)>,
    current_session: Option<u64>,
    target: &lunco_modelica_runtime::ModelicaResetTarget,
) -> Result<(), String> {
    let cached = cached.ok_or("Reset requires a cached compiled model")?;
    if installed != Some((target.session_id, target.model_name.as_str()))
        || current_session != Some(target.session_id)
        || cached.model_name != target.model_name
    {
        return Err("Reset installed target was superseded before cached reconstruction".into());
    }
    if cached.retained.snapshot.admitted_library_revision != Some(target.library_revision) {
        return Err(
            "Reset cached source-library identity differs from the installed session".into(),
        );
    }
    Ok(())
}

/// A compiled artifact belongs to one exact source unit and source-root revision.
fn shared_compile_hash(
    model_name: &str,
    unit: &CompileUnit,
    doc_uri: &str,
    library_gen: u64,
    structural_overrides: &[(String, f64)],
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    compile_unit_hash(model_name, doc_uri, unit).hash(&mut h);
    library_gen.hash(&mut h);
    for (name, value) in structural_overrides {
        name.hash(&mut h);
        value.to_bits().hash(&mut h);
    }
    h.finish()
}

fn checked_compile_request_identity(
    source: &lunco_hash::content::Cid,
    unit: &CompileUnit,
) -> lunco_hash::content::Cid {
    let mut hash = lunco_hash::content::ContentHasher::default();
    let mut field = |value: &[u8]| {
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(value);
    };
    field(b"LunCoCheckedCompileRequest");
    field(&source.to_bytes());
    for values in [&unit.parameter_overrides, &unit.authored_bindings.values] {
        field(&(values.len() as u64).to_le_bytes());
        for (name, value) in values {
            field(name.as_bytes());
            field(&value.to_bits().to_le_bytes());
        }
    }
    for name in &unit.authored_bindings.external_inputs {
        field(name.as_bytes());
    }
    hash.finish()
}

/// Bind explicit parameters once before strict compilation. Checked DAE
/// structural guards and the bound-name schema authorize immutable reuse;
/// every tunable value remains a per-instance lowering input.
fn compile_shared(
    artifacts: &mut CompiledArtifactCache,
    compiler: &mut ModelicaCompiler,
    model_name: &str,
    unit: &CompileUnit,
    doc_uri: &str,
    library_gen: u64,
) -> Result<CompiledModelArtifact, String> {
    compiler.clear_compile_diagnostics();
    if unit
        .expected_library_revision
        .is_some_and(|revision| revision != compiler.library_revision())
    {
        return Err("Modelica source library changed after interface preparation".into());
    }
    let source_identity =
        compiler.source_unit_identity(model_name, &unit.source, doc_uri, &unit.extras)?;
    let checked_request = checked_compile_request_identity(&source_identity, unit);
    let mut force_rebuild = false;
    if let Some(mut artifact) = artifacts.checked_request(&checked_request) {
        let parameters = merge_instance_values(
            &unit.parameter_overrides,
            &artifact.authored_bindings.parameter_overrides,
        );
        let key = shared_compile_hash(model_name, unit, doc_uri, library_gen, &parameters);
        if compiled_cache_allows_reuse(doc_uri, key)? {
            if unit.project_edit_interface && artifact.edit_interface.is_none() {
                artifact.edit_interface = Some(Arc::new(compiler.interface_for_parameter_edits(
                    model_name,
                    &unit.source,
                    doc_uri,
                    &unit.extras,
                    &artifact.compiled.dae,
                    &parameters,
                )?));
                artifacts.insert(key, artifact.clone());
            }
            log::debug!("[modelica-runtime] reused checked compile request for `{model_name}`");
            return Ok(artifact);
        }
        force_rebuild = true;
    }
    let authored_bindings = compiler.admit_authored_bindings_after_source_root_admission(
        model_name,
        &unit.source,
        doc_uri,
        &unit.extras,
        &unit.authored_bindings.values,
        &unit.authored_bindings.external_inputs,
    )?;
    let parameters = canonical_parameter_overrides(&merge_instance_values(
        &unit.parameter_overrides,
        &authored_bindings.parameter_overrides,
    ));
    let mut identity = lunco_hash::content::ContentHasher::default();
    identity.update(&source_identity.to_bytes());
    for name in ModelicaCompiler::parameter_binding_names(&parameters)? {
        identity.update(&(name.len() as u64).to_le_bytes());
        identity.update(name.as_bytes());
    }
    let binding_schema = identity.finish();
    let shared_key = shared_compile_hash(model_name, unit, doc_uri, library_gen, &parameters);
    let compatible = if force_rebuild {
        None
    } else {
        artifacts.compatible_variant(&binding_schema, &parameters)
    };
    let use_compatible = compatible.is_some() && compiled_cache_allows_reuse(doc_uri, shared_key)?;
    let (key, mut artifact) = match compatible {
        Some((key, artifact)) if use_compatible => (key, artifact),
        candidate => {
            let rebuild = force_rebuild || candidate.is_some();
            let artifact = compile_bound_variant(
                artifacts,
                compiler,
                model_name,
                unit,
                doc_uri,
                library_gen,
                &parameters,
                binding_schema,
                &source_identity,
                rebuild,
            )?;
            (shared_key, artifact)
        }
    };
    ModelicaCompiler::validate_parameter_bindings(&artifact.compiled.dae, &parameters)?;
    artifact.authored_bindings = ModelicaCompiler::admit_authored_bindings_from_checked_dae(
        &artifact.compiled.dae,
        &unit.authored_bindings.values,
        &unit.authored_bindings.external_inputs,
    )?;
    artifact.edit_interface = if unit.project_edit_interface {
        Some(Arc::new(compiler.interface_for_parameter_edits(
            model_name,
            &unit.source,
            doc_uri,
            &unit.extras,
            &artifact.compiled.dae,
            &parameters,
        )?))
    } else {
        None
    };
    if artifact.compiled.dae.external_tables().is_empty() {
        artifact.checked_request = Some(checked_request);
        artifacts.insert(key, artifact.clone());
    }
    Ok(artifact)
}

fn compiled_cache_allows_reuse(doc_uri: &str, key: u64) -> Result<bool, String> {
    Ok(lunco_assets_core::cache_policy::artifact_cache_decision(
        "modelica_compiled",
        modelica_cache_scope(doc_uri),
        doc_uri,
        &format!("{key:016x}"),
        true,
        false,
    )? == lunco_assets_core::cache_policy::ArtifactCacheDecision::Reuse)
}

fn modelica_cache_scope(doc_uri: &str) -> &'static str {
    if doc_uri.starts_with("twin://") {
        "twin"
    } else {
        "application"
    }
}

/// Merge scalar values with the latter transaction taking precedence. A wired
/// input sample therefore replaces an authored unconnected input value.
fn merge_instance_values(
    defaults: &[(String, f64)],
    values: &[(String, f64)],
) -> Vec<(String, f64)> {
    let mut merged: std::collections::BTreeMap<_, _> = defaults.iter().cloned().collect();
    merged.extend(values.iter().cloned());
    merged.into_iter().collect()
}

fn compile_bound_variant(
    artifacts: &mut CompiledArtifactCache,
    compiler: &mut ModelicaCompiler,
    model_name: &str,
    unit: &CompileUnit,
    doc_uri: &str,
    library_gen: u64,
    numeric: &[(String, f64)],
    binding_schema: lunco_hash::content::Cid,
    source_identity: &lunco_hash::content::Cid,
    force_rebuild: bool,
) -> Result<CompiledModelArtifact, String> {
    let key = shared_compile_hash(model_name, unit, doc_uri, library_gen, numeric);
    log::debug!(
        "[worker] shared artifact lookup model=`{model_name}` doc=`{doc_uri}` source_bytes={} cache_entries={}",
        unit.source.len(),
        artifacts.len(),
    );
    #[cfg(not(target_arch = "wasm32"))]
    let disk_key = artifacts.disk_key(source_identity, numeric);
    #[cfg(not(target_arch = "wasm32"))]
    let mut policy_evaluated = false;
    #[cfg(not(target_arch = "wasm32"))]
    if !force_rebuild {
        if let Some(disk_key) = &disk_key {
            let started = web_time::Instant::now();
            let _span = bevy::log::info_span!("modelica_compiled_artifact_lookup").entered();
            match artifacts.load_disk(disk_key, numeric, binding_schema) {
                Ok(Some(artifact)) => {
                    let decision = lunco_assets_core::cache_policy::artifact_cache_decision(
                        "modelica_compiled",
                        modelica_cache_scope(doc_uri),
                        doc_uri,
                        &format!("{key:016x}"),
                        true,
                        false,
                    )?;
                    policy_evaluated = true;
                    if decision == lunco_assets_core::cache_policy::ArtifactCacheDecision::Reuse {
                        log::info!(
                            "[modelica-runtime] strict compiled artifact for `{model_name}`: cache=disk-hit lookup={:?}",
                            started.elapsed()
                        );
                        return Ok(artifact);
                    }
                }
                Ok(None) => {}
                Err(error) => log::warn!(
                    "[modelica-runtime] compiled artifact cache rejected for `{model_name}`: {error}"
                ),
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let policy_evaluated = false;
    if !force_rebuild && !policy_evaluated {
        // This call also gives a Twin policy a chance to observe a genuine
        // cache miss; the Rust freshness fence below still forces compilation.
        let _ = lunco_assets_core::cache_policy::artifact_cache_decision(
            "modelica_compiled",
            modelica_cache_scope(doc_uri),
            doc_uri,
            &format!("{key:016x}"),
            false,
            false,
        )?;
    }
    let compiled = compiler.compile_str_multi_after_source_root_admission(
        model_name,
        &unit.source,
        doc_uri,
        &unit.extras,
        numeric,
    )?;
    let checked_structural =
        ModelicaCompiler::structural_parameter_overrides(&compiled.dae, numeric)?;
    let structural_overrides = numeric
        .iter()
        .filter(|(name, _)| {
            checked_structural
                .iter()
                .any(|(structural, _)| structural == name)
        })
        .cloned()
        .collect();
    let source_content = compiler.compiled_source_content_identity(&compiled)?;
    let prepared_source_key = prepared_source_key(source_content);
    let artifact = CompiledModelArtifact {
        compiled: Box::new(CompiledRuntimeModel {
            dae: compiled.dae,
            experiment_start_time: compiled.experiment_start_time,
            experiment_stop_time: compiled.experiment_stop_time,
            experiment_tolerance: compiled.experiment_tolerance,
            experiment_interval: compiled.experiment_interval,
            experiment_solver: compiled.experiment_solver,
        }),
        prepared_source_key,
        structural_overrides,
        authored_bindings: Default::default(),
        edit_interface: None,
        checked_request: None,
        #[cfg(not(target_arch = "wasm32"))]
        source_content,
        binding_schema,
    };
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(disk_key) = &disk_key {
        let _span = bevy::log::info_span!("modelica_compiled_artifact_publish").entered();
        if let Err(error) = artifacts.save_disk(disk_key, &artifact) {
            log::warn!(
                "[modelica-runtime] compiled artifact publication failed for `{model_name}`: {error}"
            );
        }
    }
    Ok(artifact)
}

struct BackendCompileResult {
    #[cfg(not(target_arch = "wasm32"))]
    unit: CompileUnit,
    outcome: Result<CompiledModelArtifact, String>,
    diagnostics: Vec<lunco_doc::Diagnostic>,
    library_revision: u64,
}

#[cfg(target_arch = "wasm32")]
trait CompileBackend {
    fn library_revision(&self) -> u64;
    fn compile(
        &mut self,
        model_name: &str,
        unit: CompileUnit,
        doc_uri: &str,
        library_gen: u64,
    ) -> BackendCompileResult;
}

#[cfg(target_arch = "wasm32")]
struct InlineCompileBackend<'a> {
    compiler: &'a mut Option<ModelicaCompiler>,
    artifacts: &'a mut CompiledArtifactCache,
}

#[cfg(target_arch = "wasm32")]
impl CompileBackend for InlineCompileBackend<'_> {
    fn library_revision(&self) -> u64 {
        self.compiler
            .as_ref()
            .map(ModelicaCompiler::library_revision)
            .unwrap_or_default()
    }

    fn compile(
        &mut self,
        model_name: &str,
        unit: CompileUnit,
        doc_uri: &str,
        library_gen: u64,
    ) -> BackendCompileResult {
        let compiler = self.compiler.get_or_insert_with(ModelicaCompiler::new);
        let outcome = compile_shared(
            self.artifacts,
            compiler,
            model_name,
            &unit,
            doc_uri,
            library_gen,
        );
        let diagnostics = if outcome.is_err() {
            compiler.take_compile_diagnostics(model_name, doc_uri)
        } else {
            Vec::new()
        };
        let library_revision = compiler.library_revision();
        compiler.clear_user_documents();
        BackendCompileResult {
            outcome,
            diagnostics,
            library_revision,
        }
    }
}

/// Whether a cached artifact built at (`cached_hash`, `cached_gen`) may be
/// reused for the unit currently hashing to `hash` under `library_gen`.
/// Factored out of [`rebuild_from_cache`] so the invalidation rule is
/// testable without a compiler.
fn artifact_still_valid(cached_hash: u64, cached_gen: u64, hash: u64, library_gen: u64) -> bool {
    cached_hash == hash && cached_gen == library_gen
}

/// One rebuild-from-cache pass: the compiled artifact for `entity`'s CACHED
/// source set, plus everything the caller needs to seat a fresh stepper.
#[cfg(target_arch = "wasm32")]
struct CacheRebuild {
    source: Arc<str>,
    /// Revision of the compiler-owned source roots used by this artifact.
    library_revision: Option<u64>,
    /// The instance values that must be supplied to Rumoca when the cached DAE
    /// is lowered into a fresh live stepper.
    parameter_overrides: Vec<(String, f64)>,
    initial_inputs: Vec<(String, f64)>,
    initial_state_values: Option<Vec<(String, f64)>>,
    /// `Ok` = artifact to build the stepper from; `Err` = rumoca's formatted
    /// compile summary.
    outcome: Result<CompiledModelArtifact, String>,
    compile_diagnostics: Vec<lunco_doc::Diagnostic>,
}

/// Produce the compiled artifact for a wasm entity's cached source set,
/// reusing [`CachedModel::compiled`] when its source and library generation
/// still match, and refreshing the cache entry after a recompile.
///
/// Returns `None` when the entity has no cached model at all.
#[cfg(target_arch = "wasm32")]
fn rebuild_from_cache(
    cached_models: &mut HashMap<Entity, CachedModel>,
    backend: &mut impl CompileBackend,
    entity: Entity,
    library_gen: u64,
) -> Option<CacheRebuild> {
    let (
        model_name,
        doc_uri,
        source,
        extras,
        parameter_overrides,
        initial_inputs,
        initial_state_values,
        cached_hash,
        cached_gen,
    ) = {
        let c = cached_models.get(&entity)?;
        (
            c.model_name.clone(),
            c.doc_uri.clone(),
            Arc::clone(&c.source),
            c.extra_sources.clone(),
            c.parameter_overrides.clone(),
            c.initial_inputs.clone(),
            c.initial_state_values.clone(),
            c.unit_hash,
            c.library_gen,
        )
    };
    let mut unit = assemble_compile_unit(&source, extras);
    unit.expected_library_revision = cached_models
        .get(&entity)
        .and_then(|cached| cached.expected_library_revision);
    unit.parameter_overrides = canonical_parameter_overrides(&parameter_overrides);
    let diagnostic_source = Arc::clone(&source);
    let hash = compile_unit_hash(&model_name, &doc_uri, &unit);
    if artifact_still_valid(cached_hash, cached_gen, hash, library_gen) {
        let library_revision = Some(backend.library_revision());
        let compiled = cached_models
            .get(&entity)
            .expect("checked above")
            .compiled
            .clone();
        return Some(CacheRebuild {
            source: diagnostic_source,
            library_revision,
            parameter_overrides,
            initial_inputs,
            initial_state_values,
            outcome: Ok(compiled),
            compile_diagnostics: Vec::new(),
        });
    }
    let result = backend.compile(&model_name, unit, &doc_uri, library_gen);
    if let Ok(comp_res) = &result.outcome {
        if let Some(c) = cached_models.get_mut(&entity) {
            c.compiled = comp_res.clone();
            c.unit_hash = hash;
            c.library_gen = library_gen;
        }
    }
    Some(CacheRebuild {
        source: diagnostic_source,
        library_revision: Some(result.library_revision),
        parameter_overrides,
        initial_inputs,
        initial_state_values,
        outcome: result.outcome,
        compile_diagnostics: result.diagnostics,
    })
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeCachedRebuild {
    execution_domain: lunco_modelica_runtime::ModelicaExecutionDomain,
    model_name: String,
    source: Arc<str>,
    raw_extras: Vec<(String, String)>,
    doc_uri: String,
    parameter_overrides: Vec<(String, f64)>,
    initial_inputs: Vec<(String, f64)>,
    initial_state_values: Option<Vec<(String, f64)>>,
    compiled: CompiledModelArtifact,
    unit_hash: u64,
    cached_library_gen: u64,
    unit_key: u64,
    unit: CompileUnit,
    library_revision: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeCachedRebuild {
    fn artifact_is_valid(&self, library_gen: u64) -> bool {
        artifact_still_valid(
            self.unit_hash,
            self.cached_library_gen,
            compile_unit_hash(&self.model_name, &self.doc_uri, &self.unit),
            library_gen,
        )
    }

    fn into_pending_compile(
        self,
        entity: Entity,
        session_id: u64,
        library_gen: u64,
        intent: CompileIntent,
    ) -> (PendingCompile, CompileUnit) {
        (
            PendingCompile {
                execution_domain: self.execution_domain,
                entity,
                session_id,
                cancelled: false,
                model_name: self.model_name,
                source: self.source.to_string(),
                doc_uri: self.doc_uri,
                raw_extras: self.raw_extras,
                parameter_overrides: self.parameter_overrides,
                initial_inputs: self.initial_inputs,
                initial_state_values: self.initial_state_values,
                library_gen,
                intent,
            },
            self.unit,
        )
    }

    fn into_compile_work(
        self,
        entity: Entity,
        session_id: u64,
        library_gen: u64,
        profile: solver::RuntimeProfile,
        intent: CompileIntent,
    ) -> Result<CompileWork, rumoca_sim::SimulationDiagnosticError> {
        let plan = live_build_plan(
            profile,
            &self.parameter_overrides,
            &self.initial_inputs,
            self.unit_key,
            &self.compiled.structural_overrides,
            Some(self.library_revision),
        )?
        .with_initial_state_values(&self.compiled, self.initial_state_values.as_deref())?;
        Ok(CompileWork {
            execution_domain: self.execution_domain,
            entity,
            session_id,
            cancelled: false,
            model_name: self.model_name,
            source: self.source.to_string(),
            doc_uri: self.doc_uri,
            raw_extras: self.raw_extras,
            parameter_overrides: self.parameter_overrides,
            initial_inputs: self.initial_inputs,
            initial_state_values: self.initial_state_values,
            unit: self.unit,
            comp_res: self.compiled,
            library_gen,
            plan,
            intent,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn compiled_cached_rebuild(
    cached_models: &HashMap<Entity, CachedModel>,
    entity: Entity,
    library_revision: u64,
) -> Option<NativeCachedRebuild> {
    let cached = cached_models.get(&entity)?;
    let unit = cached.compile_unit(&cached.source);
    let unit_key = cached.compiled.prepared_source_key;
    Some(NativeCachedRebuild {
        execution_domain: cached.execution_domain,
        model_name: cached.model_name.clone(),
        source: Arc::clone(&cached.source),
        raw_extras: cached.extra_sources.clone(),
        doc_uri: cached.doc_uri.clone(),
        parameter_overrides: cached.parameter_overrides.clone(),
        initial_inputs: cached.initial_inputs.clone(),
        initial_state_values: cached.initial_state_values.clone(),
        compiled: cached.compiled.clone(),
        unit_hash: cached.unit_hash,
        cached_library_gen: cached.library_gen,
        unit_key,
        unit,
        library_revision,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn submit_pending_compile(
    pending: PendingCompile,
    mut unit: CompileUnit,
    compiler: &mut CompilerActor,
    compiler_order: &mut VecDeque<u64>,
    pending_compiles: &mut HashMap<u64, PendingRequest>,
    tx: &ModelicaResponseSender,
) {
    unit.parameter_overrides = canonical_parameter_overrides(&pending.parameter_overrides);
    send_preparation_phase(
        tx,
        pending.entity,
        pending.session_id,
        lunco_modelica_runtime::ModelicaPreparationPhase::QueuedForCompilation,
    );
    match compiler.submit_compile(
        pending.model_name.clone(),
        unit,
        pending.doc_uri.clone(),
        pending.library_gen,
    ) {
        Ok(operation_id) => {
            compiler_order.push_back(operation_id);
            pending_compiles.insert(operation_id, PendingRequest::Participant(pending));
        }
        Err(error) => {
            send_compile_artifact_error(tx, &pending, error, Vec::new());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn submit_cached_solve_preparation(
    cached: NativeCachedRebuild,
    entity: Entity,
    session_id: u64,
    library_gen: u64,
    profile: solver::RuntimeProfile,
    intent: CompileIntent,
    solve_preparation_pool: &mut ModelicaWorkerPools,
    live_preparation_pipeline: &mut LivePreparationPipeline,
    pending_compile_works: &mut HashMap<u64, PreparationWork>,
    tx: &ModelicaResponseSender,
) {
    let source = Arc::clone(&cached.source);
    let failure_intent = intent.clone();
    let work = match cached.into_compile_work(entity, session_id, library_gen, profile, intent) {
        Ok(work) => work,
        Err(error) => {
            send_compile_stepper_error(tx, entity, session_id, &source, &error, &failure_intent);
            return;
        }
    };
    admit_preparation(
        PreparationWork::Participant(work),
        tx,
        solve_preparation_pool,
        live_preparation_pipeline,
        pending_compile_works,
    );
}

/// Project the admitted live observation without hiding solver diagnostics.
fn collect_stepper_observables(
    stepper: &LiveStepper,
) -> Result<Vec<(VarName, f64)>, rumoca_sim::SimulationDiagnosticError> {
    let mut values = stepper.observables()?;
    values.retain(|(name, _)| name.as_str() != "time");
    Ok(values)
}

/// Admit causal inputs, integrate, and read the complete checked observation
/// before the worker publishes any part of the transaction.
fn advance_checked_step(
    stepper: &mut LiveStepper,
    inputs: &[(String, f64)],
    stop_time: f64,
) -> Result<Vec<(VarName, f64)>, rumoca_sim::SimulationDiagnosticError> {
    {
        let _span =
            bevy::log::info_span!("modelica_admit_step_inputs", count = inputs.len()).entered();
        let inputs: Vec<_> = inputs
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect();
        stepper.set_inputs(&inputs)?;
    }
    {
        let _span = bevy::log::info_span!("modelica_integrate_macro_step").entered();
        stepper.advance_to(stop_time)?;
    }
    validate_step_completion(stepper.time(), stop_time)
        .map_err(rumoca_sim::SimulationDiagnosticError::Solver)?;
    let _span = bevy::log::info_span!("modelica_collect_step_observables").entered();
    collect_stepper_observables(stepper)
}

fn capture_initial_inputs(
    stepper: &LiveStepper,
) -> Result<Vec<(String, f64)>, rumoca_sim::SimulationDiagnosticError> {
    let values = stepper.observables()?;
    stepper
        .input_names()
        .iter()
        .map(|name| {
            let value = values
                .iter()
                .find(|(candidate, _)| candidate.as_str() == name.as_str())
                .map(|(_, value)| *value)
                .ok_or_else(|| {
                    rumoca_sim::SimulationDiagnosticError::Solver(format!(
                        "admitted input '{name}' has no runtime observation"
                    ))
                })?;
            if !value.is_finite() {
                return Err(rumoca_sim::SimulationDiagnosticError::Solver(format!(
                    "admitted input '{name}' is not finite"
                )));
            }
            Ok((name.clone(), value))
        })
        .collect()
}

fn merge_initial_inputs(initial: &mut Vec<(String, f64)>, values: &[(String, f64)]) {
    let mut merged: std::collections::BTreeMap<_, _> = initial.drain(..).collect();
    merged.extend(values.iter().cloned());
    initial.extend(merged);
}

/// Fixed solver tolerance on the LIVE path. Explicit, and deliberately NOT the
/// model's `experiment(Tolerance=…)` annotation nor the batch runner's default —
/// see [`live_stepper_options`] and the runtime solver capability contract.
const LIVE_TOL: f64 = 1e-6;

/// Numerical step setting supplied to the selected backend. Fixed prediction
/// integrates on this lattice; adaptive integration owns its accepted steps.
const LIVE_INTERNAL_STEP_SECS: f64 = lunco_core_runtime::SECS_PER_TICK / 3.0;

const COMMUNICATION_TIME_ROUNDOFF: f64 = 32.0 * f64::EPSILON;

/// Compare finite communication timestamps at floating point roundoff scale.
#[inline]
fn communication_times_close(a: f64, b: f64) -> bool {
    a.is_finite()
        && b.is_finite()
        && (a - b).abs() <= COMMUNICATION_TIME_ROUNDOFF * a.abs().max(b.abs()).max(1.0)
}

/// Validate the worker-side FMI-CS transaction envelope before mutating solver
/// state. The master owns the interval and the worker must honor that interval.
fn validate_step_request(
    actual_start: f64,
    start_time: f64,
    stop_time: f64,
    dt: f64,
) -> Result<(), String> {
    // FMI 3 computes the endpoint with one addition. Subtracting a large
    // absolute start from stop loses precision relative to the short interval.
    let expected_stop = start_time + dt;
    if !start_time.is_finite()
        || !stop_time.is_finite()
        || !dt.is_finite()
        || dt <= 0.0
        || dt > MAX_MACRO_STEP_DT
        || stop_time <= start_time
        || expected_stop <= start_time
        || !communication_times_close(stop_time, expected_stop)
        || !communication_times_close(actual_start, start_time)
    {
        return Err(format!(
            "invalid Modelica communication transaction: solver at {actual_start:.12}, requested [{start_time:.12}, {stop_time:.12}] with dt={dt:.12}"
        ));
    }
    Ok(())
}

#[inline]
fn validate_step_completion(actual_end: f64, stop_time: f64) -> Result<(), String> {
    if communication_times_close(actual_end, stop_time) {
        Ok(())
    } else {
        Err(format!(
            "Modelica participant stopped at {actual_end:.12}, before or after requested communication point {stop_time:.12}"
        ))
    }
}

/// Model-vs-world lag past which the co-sim worker is visibly waiting on the
/// coupling barrier. Surfaced as a rate-limited `warn!` + [`CosimLag`]. Every
/// participant in the shared simulation holds the deterministic fixed clock
/// while its result is pending; the worker itself remains off-thread so the UI
/// and render/update schedule stay responsive.
const LAG_WARN_SECS: f64 = 0.25;

/// Fixed ticks between two lag warnings (5 s at 60 Hz) — the warn is on the
/// per-tick hot path, so it must never become a per-frame spam source.
const LAG_WARN_COOLDOWN_TICKS: u32 = 300;

/// **The co-simulation lag diagnostic** (A3).
///
/// Every fixed tick, `spawn_modelica_requests` measures `|model.current_time −
/// world_sim_secs|` for every live model and records the worst offender here.
/// Before this existed, NOTHING compared the model's own clock to the world's —
/// the model could run at half speed forever and no surface reported it.
///
/// `worst_secs` is the distance between the next communication point and the
/// model's last completed state. During an in-flight step it describes the
/// amount of simulation the participant is waiting to process; it is not
/// permission to apply stale state because the shared simulation holds until
/// the result lands.
#[derive(Resource, Default, Debug, Clone)]
pub struct CosimLag {
    /// Worst `|model_time − world_time|` seen on the last fixed tick, seconds.
    pub worst_secs: f64,
    /// The model entity that owned `worst_secs`.
    pub worst_entity: Option<Entity>,
    /// Live (unpaused, compiled) models measured on the last tick.
    pub models: usize,
    /// Ticks remaining before another `warn!` is allowed.
    cooldown: u32,
}

/// Helper to build a ModelicaResult with defaults.
fn result_ok(entity: Entity, session_id: u64) -> ModelicaResult {
    ModelicaResult {
        entity,
        session_id,
        ..Default::default()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn send_preparation_phase(
    tx: &ModelicaResponseSender,
    entity: Entity,
    session_id: u64,
    phase: lunco_modelica_runtime::ModelicaPreparationPhase,
) {
    let _ = tx.send(ModelicaResult {
        entity,
        session_id,
        preparation_phase: Some(phase),
        ..Default::default()
    });
}

fn add_experiment_defaults(
    mut result: ModelicaResult,
    comp_res: &CompiledModelArtifact,
) -> ModelicaResult {
    result.checked_interface = comp_res.edit_interface.clone();
    result.detected_parameter_names = comp_res.compiled.dae.inspect(|view| {
        view.variables()
            .filter(|(_, variable)| {
                variable.role() == rumoca_ir_dae::VariableRole::Parameter
                    && matches!(
                        variable.value_type().scalar_type(),
                        rumoca_ir_dae::ScalarType::Real
                            | rumoca_ir_dae::ScalarType::Integer
                            | rumoca_ir_dae::ScalarType::Boolean
                            | rumoca_ir_dae::ScalarType::Enumeration
                    )
            })
            .flat_map(|(_, variable)| variable.scalar_names())
            .collect()
    });
    result.experiment_start_time = comp_res.compiled.experiment_start_time;
    result.experiment_stop_time = comp_res.compiled.experiment_stop_time;
    result.experiment_tolerance = comp_res.compiled.experiment_tolerance;
    result.experiment_interval = comp_res.compiled.experiment_interval;
    result.experiment_solver = comp_res.compiled.experiment_solver.clone();
    result
}

fn step_result_ok(entity: Entity, session_id: u64, step_id: u64) -> ModelicaResult {
    ModelicaResult {
        entity,
        session_id,
        step_id: Some(step_id),
        ..Default::default()
    }
}

/// A successful `Reset` result (`is_reset`, `new_time = 0`, no error).
/// CQ-110: the native and wasm Reset arms built this byte-identically —
/// one constructor keeps the two `#[cfg]` twins from drifting. Pass the
/// refreshed `symbols`/`input_names` (empty for the no-cached-model case)
/// and the user-facing `log` line.
fn reset_ok(
    entity: Entity,
    session_id: u64,
    detected_symbols: Vec<(String, f64)>,
    detected_input_names: Vec<String>,
    log: &str,
) -> ModelicaResult {
    ModelicaResult {
        entity,
        session_id,
        detected_symbols,
        detected_input_names,
        log_message: Some(log.to_string()),
        is_reset: true,
        ..Default::default()
    }
}

/// Build the terminal response for a command that cannot complete. The
/// response retains the command's lifecycle shape: a Compile failure closes
/// compilation, a Step failure closes its exact transaction, and a source-root
/// failure resolves its matching load or unload operation. A placeholder-only
/// response cannot clear any of those state machines.
pub fn failed_result_for_command(
    cmd: &ModelicaCommand,
    message: impl Into<String>,
) -> lunco_modelica_runtime::ModelicaWorkerResponse {
    let message = message.into();
    let mut result = ModelicaResult {
        error: Some(message.clone()),
        log_message: Some("Modelica worker command failed".to_string()),
        ..Default::default()
    };
    match cmd {
        ModelicaCommand::Initialize { request_id, .. }
        | ModelicaCommand::PrepareInterface { request_id, .. }
        | ModelicaCommand::CancelPreparation { request_id } => {
            return lunco_modelica_runtime::ModelicaPreparationResult {
                request_id: *request_id,
                outcome: Err(message),
            }
            .into();
        }
        ModelicaCommand::Step {
            entity,
            session_id,
            step_id,
            ..
        } => {
            result.entity = *entity;
            result.session_id = *session_id;
            result.step_id = Some(*step_id);
        }
        ModelicaCommand::Compile {
            entity, session_id, ..
        } => {
            result.entity = *entity;
            result.session_id = *session_id;
            result.is_new_model = true;
        }
        ModelicaCommand::UpdateParameters {
            entity, session_id, ..
        } => {
            result.entity = *entity;
            result.session_id = *session_id;
            result.is_parameter_update = true;
        }
        ModelicaCommand::Reset {
            entity, session_id, ..
        } => {
            result.entity = *entity;
            result.session_id = *session_id;
            result.is_reset = true;
        }
        ModelicaCommand::LoadSourceRoot {
            id, operation_id, ..
        } => {
            result.loaded_source_root_id = Some(id.clone());
            result.source_root_operation_id = Some(*operation_id);
        }
        ModelicaCommand::UnloadSourceRoot { id, operation_id } => {
            result.unloaded_source_root_id = Some(id.clone());
            result.source_root_operation_id = Some(*operation_id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        ModelicaCommand::CompileBatch { .. } => {}
        ModelicaCommand::Despawn { .. } => {}
    }
    result.into()
}

fn record_source_root_operation(
    latest: &mut HashMap<String, u64>,
    root_id: &str,
    operation_id: u64,
) -> bool {
    if latest
        .get(root_id)
        .is_some_and(|current| *current >= operation_id)
    {
        return false;
    }
    latest.insert(root_id.to_owned(), operation_id);
    true
}

fn retire_source_root_operation(
    latest: &mut HashMap<String, u64>,
    root_id: &str,
    operation_id: u64,
) -> bool {
    if latest.get(root_id) != Some(&operation_id) {
        return false;
    }
    latest.remove(root_id);
    true
}

/// Build the terminal response for a command that panicked inside the solver
/// worker. Panics are still reported with the shared command lifecycle shape;
/// the worker transport decides whether the worker can continue afterward.
pub fn panic_result_for_command(
    cmd: &ModelicaCommand,
    message: &str,
) -> lunco_modelica_runtime::ModelicaWorkerResponse {
    failed_result_for_command(cmd, format!("Modelica worker panic: {message}"))
}

/// Immutable source set admitted to the compiler. Input initialization stays
/// with each parsed declaration and is resolved by Rumoca, including libraries.
#[derive(Clone)]
struct CompileUnit {
    source: String,
    extras: Vec<(String, String)>,
    parameter_overrides: Vec<(String, f64)>,
    authored_bindings: lunco_modelica_runtime::ModelicaAuthoredBindings,
    expected_library_revision: Option<u64>,
    /// Live actors and interface requests expose checked source editing facts.
    project_edit_interface: bool,
}

fn assemble_compile_unit(source: &str, extra_sources: Vec<(String, String)>) -> CompileUnit {
    CompileUnit {
        source: source.to_owned(),
        extras: extra_sources,
        parameter_overrides: Vec::new(),
        authored_bindings: Default::default(),
        expected_library_revision: None,
        project_edit_interface: true,
    }
}

/// The native command/result owner, with per-entity source metadata and the
/// two-lane scheduler. Persistent solver workers own thread-affine sessions;
/// a separate compiler actor owns the mutable Rumoca session and DAE cache.
///
/// **Native only.** It is spawned on a real `std::thread` (see
/// `ModelicaExecutionPlugin::build`) and persists cache entries through the storage
/// boundary. The browser dispatches the *same* commands through
/// [`process_worker_command`] in the `lunica_worker` Web Worker bundle with the
/// source carried in the message.
#[cfg(not(target_arch = "wasm32"))]
pub fn modelica_worker(
    rx: Receiver<ModelicaCommand>,
    tx: ModelicaResponseSender,
    disk_limits: ModelicaArtifactDiskLimits,
    cache_limits: ModelicaCacheLimits,
    worker_limits: ModelicaWorkerLimits,
) {
    let capacities = match cache_limits.validate() {
        Ok(capacities) => capacities,
        Err(error) => {
            let _ = tx.send(ModelicaResult::worker_failure(error));
            return;
        }
    };
    let mut preparation_requests = PreparationRequests::new(tx.clone());
    let mut steppers: HashMap<Entity, (u64, String, SessionHandle)> = HashMap::default();
    let mut current_sessions: HashMap<Entity, u64> = HashMap::default();
    // Which models declared the realtime promise, from `Compile`. Half of the
    // solver-selection input, kept per entity because every later rebuild — Reset,
    // parameter update, Step
    // auto-init — must resolve the SAME solver as the original compile did.
    let mut realtime_models: std::collections::HashSet<Entity> = Default::default();
    // Compiled-artifact cache per entity (M3) — Reset and Step auto-init
    // rebuild steppers from `CachedModel::compiled` without recompiling.
    let mut cached_models: HashMap<Entity, CachedModel> = HashMap::default();
    // Source-root reads and mutable compiler work stay off the owner thread;
    // Rumoca creates each session on its thread-affine execution lane.
    let mut solve_preparation_pool = match ModelicaWorkerPools::new(worker_limits) {
        Ok(pool) => pool,
        Err(error) => {
            let _ = tx.send(ModelicaResult::worker_failure(error));
            return;
        }
    };
    let mut compiler = match CompilerActor::new(
        solve_preparation_pool.tx.clone(),
        capacities.compiled_artifacts,
        Some(disk_limits),
    ) {
        Ok(compiler) => compiler,
        Err(error) => {
            let _ = tx.send(ModelicaResult::worker_failure(error));
            return;
        }
    };
    // M3: cached compiled artifacts are valid only for the library set they
    // were compiled against — every LoadSourceRoot bumps this and thereby
    // invalidates all of them (see `CachedModel::library_gen`).
    let mut library_gen: u64 = 0;
    let mut library_revision: u64 = 0;
    // M8: the two scheduling lanes — see `enqueue_command` for the contract.
    let mut compile_lane: VecDeque<ModelicaCommand> = VecDeque::new();
    let mut step_lane: VecDeque<ModelicaCommand> = VecDeque::new();
    let mut pending_compile_works: HashMap<u64, PreparationWork> = HashMap::new();
    let mut pending_compiles: HashMap<u64, PendingRequest> = HashMap::new();
    let mut compiler_order = VecDeque::new();
    let mut ready_compiler_completions = HashMap::new();
    let mut pending_source_root_installs = HashSet::new();
    let mut latest_source_root_operations = HashMap::<String, u64>::new();
    let mut live_preparation_pipeline = LivePreparationPipeline::new();
    let mut source_root_preparation_order = VecDeque::new();
    let mut ready_source_root_preparations = HashMap::new();

    loop {
        // Block only when idle; otherwise just soak up whatever has arrived
        // since the last command, so Steps that landed during a long compile
        // are scheduled ahead of older queued compiles.
        if compile_lane.is_empty()
            && step_lane.is_empty()
            && !live_preparation_pipeline.has_ready()
            && ready_source_root_preparations.is_empty()
            && ready_compiler_completions.is_empty()
        {
            if pending_compile_works.is_empty()
                && pending_compiles.is_empty()
                && source_root_preparation_order.is_empty()
                && compiler_order.is_empty()
            {
                match rx.recv() {
                    Ok(cmd) => admit_worker_command(
                        cmd,
                        &mut compile_lane,
                        &mut step_lane,
                        &mut compiler,
                        &mut compiler_order,
                        &mut pending_source_root_installs,
                        &mut latest_source_root_operations,
                        &tx,
                        &mut preparation_requests,
                    ),
                    Err(_) => return,
                }
            } else {
                // A live scene keeps sending Step commands while a solve is
                // preparing. Preparation completion must win this wait: an
                // unbiased select can repeatedly choose the hot command
                // channel and leave a finished model uncommitted forever.
                crossbeam_channel::select_biased! {
                    recv(solve_preparation_pool.rx) -> message => match message {
                        Ok(result) => { if !stage_preparation_result(
                            result,
                            &mut live_preparation_pipeline,
                            &mut ready_source_root_preparations,
                            &mut ready_compiler_completions,
                         &tx) { return; } },
                        Err(_) => return,
                    },
                    recv(rx) -> message => match message {
                        Ok(cmd) => admit_worker_command(
                            cmd,
                            &mut compile_lane,
                            &mut step_lane,
                            &mut compiler,
                            &mut compiler_order,
                            &mut pending_source_root_installs,
                            &mut latest_source_root_operations,
                            &tx,
                            &mut preparation_requests,
                        ),
                        Err(_) => return,
                    },
                }
            }
        }
        // Completion is a lifecycle event, not background work. Process
        // completions already staged by the blocking select before accepting
        // more commands; otherwise a continuously-fed Step channel can keep
        // the worker in the command-drain loop and strand a finished solver.
        while let Ok(result) = solve_preparation_pool.rx.try_recv() {
            if !stage_preparation_result(
                result,
                &mut live_preparation_pipeline,
                &mut ready_source_root_preparations,
                &mut ready_compiler_completions,
                &tx,
            ) {
                return;
            };
        }
        dispatch_ready_source_roots(
            &mut source_root_preparation_order,
            &mut ready_source_root_preparations,
            &mut compiler,
            &mut compiler_order,
            pending_compile_works.len(),
            &solve_preparation_pool,
            &mut pending_source_root_installs,
            &latest_source_root_operations,
            &tx,
        );
        commit_ready_compiler_completions(
            &mut compiler_order,
            &mut ready_compiler_completions,
            &mut pending_source_root_installs,
            &mut pending_compiles,
            &current_sessions,
            &mut library_gen,
            &mut library_revision,
            &mut solve_preparation_pool,
            &mut live_preparation_pipeline,
            &mut pending_compile_works,
            &realtime_models,
            &mut latest_source_root_operations,
            &tx,
            &mut preparation_requests,
        );
        commit_ready_session_constructions(
            &mut live_preparation_pipeline,
            &mut pending_compile_works,
            &current_sessions,
            library_gen,
            &mut steppers,
            &mut cached_models,
            &mut step_lane,
            &tx,
            &mut preparation_requests,
        );

        // Bound command intake so a hot Step producer cannot starve either
        // preparation completions or the scheduler's own fairness points.
        const MAX_COMMANDS_PER_ROUND: usize = 64;
        for _ in 0..MAX_COMMANDS_PER_ROUND {
            let Ok(cmd) = rx.try_recv() else { break };
            admit_worker_command(
                cmd,
                &mut compile_lane,
                &mut step_lane,
                &mut compiler,
                &mut compiler_order,
                &mut pending_source_root_installs,
                &mut latest_source_root_operations,
                &tx,
                &mut preparation_requests,
            );
        }

        // A result can arrive while the bounded command batch is being
        // admitted. Drain and commit it before selecting the next work round.
        while let Ok(result) = solve_preparation_pool.rx.try_recv() {
            if !stage_preparation_result(
                result,
                &mut live_preparation_pipeline,
                &mut ready_source_root_preparations,
                &mut ready_compiler_completions,
                &tx,
            ) {
                return;
            };
        }
        dispatch_ready_source_roots(
            &mut source_root_preparation_order,
            &mut ready_source_root_preparations,
            &mut compiler,
            &mut compiler_order,
            pending_compile_works.len(),
            &solve_preparation_pool,
            &mut pending_source_root_installs,
            &latest_source_root_operations,
            &tx,
        );
        commit_ready_compiler_completions(
            &mut compiler_order,
            &mut ready_compiler_completions,
            &mut pending_source_root_installs,
            &mut pending_compiles,
            &current_sessions,
            &mut library_gen,
            &mut library_revision,
            &mut solve_preparation_pool,
            &mut live_preparation_pipeline,
            &mut pending_compile_works,
            &realtime_models,
            &mut latest_source_root_operations,
            &tx,
            &mut preparation_requests,
        );
        commit_ready_session_constructions(
            &mut live_preparation_pipeline,
            &mut pending_compile_works,
            &current_sessions,
            library_gen,
            &mut steppers,
            &mut cached_models,
            &mut step_lane,
            &tx,
            &mut preparation_requests,
        );

        // One scheduling round: every runnable Step, then one compile-lane
        // command. A compile's pure DAE lowering is submitted to the bounded
        // preparation pool, so the worker can continue compiling the next
        // source while that job runs. In-flight preparation is part of the
        // lifecycle state: its entity cannot be stepped, rebuilt, or replaced
        // until the result is committed here.
        let mut pending_entities = pending_preparation_entities(&pending_compile_works);
        pending_entities.extend(
            pending_compiles
                .values()
                .filter_map(PendingRequest::participant)
                .filter(|work| !work.cancelled)
                .map(|work| work.entity),
        );
        let source_roots_pending =
            !source_root_preparation_order.is_empty() || !pending_source_root_installs.is_empty();
        if source_roots_pending || !compiler_order.is_empty() {
            for command in &step_lane {
                let ModelicaCommand::Step {
                    entity,
                    session_id,
                    model_name,
                    ..
                } = command
                else {
                    continue;
                };
                let needs_init =
                    steppers
                        .get(entity)
                        .is_none_or(|(stepper_session, stepper_name, _)| {
                            *stepper_session < *session_id || stepper_name != model_name
                        });
                let cache_needs_compile = cached_models.get(entity).is_some_and(|cached| {
                    cached.model_name == *model_name && cached.library_gen != library_gen
                });
                let cached_model_matches = cached_models
                    .get(entity)
                    .is_some_and(|cached| cached.model_name == *model_name);
                let needs_async_rebuild = needs_init || cache_needs_compile;
                let pipeline_load = compiler_order.len()
                    + pending_compile_works.len()
                    + source_root_preparation_order.len();
                let pipeline_has_capacity = solve_preparation_pool.can_admit(pipeline_load);
                if needs_init
                    && cached_model_matches
                    && (source_roots_pending || (needs_async_rebuild && !pipeline_has_capacity))
                {
                    pending_entities.insert(*entity);
                }
            }
        }
        let mut to_process = take_runnable_steps(&mut step_lane, &pending_entities);
        if let Some(cmd) = take_runnable_compile_command(
            &mut compile_lane,
            &pending_entities,
            !pending_compile_works.is_empty() || !pending_compiles.is_empty(),
            !source_root_preparation_order.is_empty() || !pending_source_root_installs.is_empty(),
            solve_preparation_pool.can_admit(
                compiler_order.len()
                    + pending_compile_works.len()
                    + source_root_preparation_order.len(),
            ),
        ) {
            to_process.push(cmd);
            promote_unblocked_steps(&mut compile_lane, &mut step_lane);
        }

        // A queued command can be correctly blocked by an in-flight
        // preparation. Wait for either its result or a new command instead of
        // busy-spinning the worker thread while the pool is doing the work.
        if to_process.is_empty()
            && (!pending_compile_works.is_empty()
                || !pending_compiles.is_empty()
                || !source_root_preparation_order.is_empty()
                || !compiler_order.is_empty()
                || !compile_lane.is_empty()
                || !step_lane.is_empty())
        {
            // Preparation results are lifecycle completions, not optional
            // background work. Prioritize them over the continuously-fed Step
            // channel so a finished participant always reaches its ordered construction commit.
            crossbeam_channel::select_biased! {
                recv(solve_preparation_pool.rx) -> message => match message {
                    Ok(result) => { if !stage_preparation_result(
                        result,
                        &mut live_preparation_pipeline,
                        &mut ready_source_root_preparations,
                        &mut ready_compiler_completions,
                     &tx) { return; } },
                    Err(_) => return,
                },
                recv(rx) -> message => match message {
                    Ok(cmd) => admit_worker_command(
                        cmd,
                        &mut compile_lane,
                        &mut step_lane,
                        &mut compiler,
                        &mut compiler_order,
                        &mut pending_source_root_installs,
                        &mut latest_source_root_operations,
                        &tx,
                        &mut preparation_requests,
                    ),
                    Err(_) => return,
                },
            }
            continue;
        }

        // Independent participants already have their complete causal inputs.
        // Dispatch one transaction per entity before admitting any result; each
        // owner keeps its solver serial, while this loop retains command order.
        let mut dispatched_entities: HashSet<Entity> = HashSet::default();
        for command in &mut to_process {
            let ModelicaCommand::Step {
                entity,
                session_id,
                step_id,
                start_time,
                stop_time,
                model_name,
                inputs,
                dt,
            } = command
            else {
                continue;
            };
            if !dispatched_entities.insert(*entity)
                || *session_id < *current_sessions.get(entity).unwrap_or(&0)
            {
                continue;
            }
            let Some((owner_session, owner_name, stepper)) = steppers.get_mut(entity) else {
                continue;
            };
            if owner_session != session_id
                || owner_name != model_name
                || stepper.has_pending_step()
                || validate_step_request(stepper.time(), *start_time, *stop_time, *dt).is_err()
            {
                continue;
            }
            if let Err(error) = stepper.start_step(
                *step_id,
                *start_time,
                *stop_time,
                *dt,
                std::mem::take(inputs),
            ) {
                let _ = tx.send(ModelicaResult::worker_failure(error.to_string()));
                return;
            }
        }
        let round_size = to_process.len();
        for (round_index, cmd) in to_process.into_iter().enumerate() {
            let is_step = matches!(&cmd, ModelicaCommand::Step { .. });
            let worker_backlog_count = is_step.then(|| {
                let remaining_round = round_size.saturating_sub(round_index + 1);
                remaining_round
                    .saturating_add(step_lane.len())
                    .saturating_add(compile_lane.len())
                    .saturating_add(rx.len())
                    .saturating_add(pending_compile_works.len())
                    .saturating_add(pending_compiles.len())
                    .saturating_add(source_root_preparation_order.len())
                    .saturating_add(pending_source_root_installs.len()) as u64
            });
            let tx_inner = tx.clone();
            let panic_entity = match &cmd {
                ModelicaCommand::Step { entity, .. }
                | ModelicaCommand::Compile { entity, .. }
                | ModelicaCommand::UpdateParameters { entity, .. }
                | ModelicaCommand::Reset { entity, .. }
                | ModelicaCommand::Despawn { entity } => Some(*entity),
                ModelicaCommand::LoadSourceRoot { .. }
                | ModelicaCommand::UnloadSourceRoot { .. }
                | ModelicaCommand::CompileBatch { .. }
                | ModelicaCommand::PrepareInterface { .. }
                | ModelicaCommand::Initialize { .. }
                | ModelicaCommand::CancelPreparation { .. } => None,
            };
            let batch_failure_reply = match &cmd {
                ModelicaCommand::CompileBatch { reply, .. } => Some(reply.clone()),
                _ => None,
            };
            let panic_result = panic_result_for_command(
                &cmd,
                "the affected Modelica command was aborted; see the worker log",
            );
            // Instrumentation for the "sometimes stuck" class of bugs:
            // when the worker hangs (usually inside a pathological
            // rumoca compile on a malformed model), the main-thread
            // UI sees no progress and no log breadcrumb. These bracket
            // logs let us see exactly which command + model was
            // in-flight and how long it actually took, so a stall is
            // visible in `RUST_LOG=info` output instead of silent.
            let cmd_label = command_label(&cmd);
            let cmd_started = web_time::Instant::now();
            // Lifecycle traffic is normal during a scene swap (one Compile per
            // scene-owned model), so it belongs in debug alongside Step. Errors
            // still surface through their result and diagnostics below.
            log::debug!("[worker] begin: {}", cmd_label);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match cmd {
                    ModelicaCommand::PrepareInterface {
                        request_id,
                        entity,
                        session_id,
                        model_name,
                        source,
                        doc_uri,
                        extra_sources,
                        parameter_overrides,
                        authored_bindings,
                    } => {
                        if !preparation_requests.contains(request_id) {
                            return;
                        }
                        let pending = initialization::PendingInterface {
                            request_id,
                            entity,
                            session_id,
                            doc_uri: doc_uri.clone(),
                            library_gen,
                        };
                        let mut unit = assemble_compile_unit(&source, extra_sources);
                        unit.parameter_overrides =
                            canonical_parameter_overrides(&parameter_overrides);
                        unit.authored_bindings = authored_bindings;
                        match compiler.submit_interface(model_name, unit, doc_uri, library_gen) {
                            Ok(id) => {
                                compiler_order.push_back(id);
                                pending_compiles.insert(id, PendingRequest::Interface(pending));
                            }
                            Err(error) => preparation_requests.finish(request_id, Err(error)),
                        }
                    }
                    ModelicaCommand::Initialize {
                        request_id,
                        model_name,
                        source,
                        doc_uri,
                        extra_sources,
                        parameter_overrides,
                        authored_bindings,
                        initial_inputs,
                        probes,
                        expected_library_revision,
                    } => {
                        if !preparation_requests.contains(request_id) {
                            return;
                        }
                        let mut unit = assemble_compile_unit(&source, extra_sources);
                        unit.parameter_overrides =
                            canonical_parameter_overrides(&parameter_overrides);
                        unit.authored_bindings = authored_bindings;
                        unit.expected_library_revision = expected_library_revision;
                        unit.project_edit_interface = false;
                        let pending = initialization::PendingInitialization {
                            request_id,
                            parameter_overrides,
                            initial_inputs,
                            probes,
                            library_gen,
                        };
                        match compiler.submit_compile(model_name, unit, doc_uri, library_gen) {
                            Ok(operation_id) => {
                                compiler_order.push_back(operation_id);
                                pending_compiles
                                    .insert(operation_id, PendingRequest::Initialization(pending));
                            }
                            Err(error) => preparation_requests.finish(request_id, Err(error)),
                        }
                    }
                    ModelicaCommand::CancelPreparation { request_id } => {
                        preparation_requests
                            .finish(request_id, Err("preparation cancelled by its owner".into()));
                    }
                    ModelicaCommand::CompileBatch {
                        model_name,
                        source,
                        doc_uri,
                        extra_sources,
                        cancel,
                        reply,
                    } => {
                        match compiler.submit_batch(
                            model_name,
                            assemble_compile_unit(&source, extra_sources),
                            doc_uri,
                            library_gen,
                            cancel,
                            reply.clone(),
                        ) {
                            Ok(id) => compiler_order.push_back(id),
                            Err(error) => {
                                let _ = reply.send(Err(error));
                            }
                        }
                    }
                    ModelicaCommand::Reset {
                        entity,
                        session_id,
                        target,
                        initial_inputs,
                    } => {
                        if let Err(error) = validate_reset_target(
                            cached_models.get(&entity),
                            steppers
                                .get(&entity)
                                .map(|(session, name, _)| (*session, name.as_str())),
                            current_sessions.get(&entity).copied(),
                            &target,
                        ) {
                            let mut result = reset_ok(
                                entity,
                                session_id,
                                Vec::new(),
                                Vec::new(),
                                "Reset rejected.",
                            );
                            result.error = Some(error);
                            let _ = tx_inner.send(result);
                            return;
                        }
                        let cached = cached_models.get(&entity).expect("validated reset cache");
                        let captured = match steppers
                            .get(&entity)
                            .expect("validated reset session")
                            .2
                            .capture_initial_inputs()
                        {
                            Ok(inputs) => inputs,
                            Err(error) => {
                                send_compile_stepper_error(
                                    &tx_inner,
                                    entity,
                                    session_id,
                                    &cached.source,
                                    &error,
                                    &CompileIntent::Reset,
                                );
                                return;
                            }
                        };
                        let inputs = merge_instance_values(&captured, &initial_inputs);
                        let work = PreparationWork::Participant(CompileWork {
                            execution_domain: cached.execution_domain,
                            entity,
                            session_id,
                            cancelled: false,
                            model_name: cached.model_name.clone(),
                            source: cached.source.to_string(),
                            doc_uri: cached.doc_uri.clone(),
                            raw_extras: cached.extra_sources.clone(),
                            parameter_overrides: cached.parameter_overrides.clone(),
                            initial_inputs: inputs.clone(),
                            initial_state_values: cached.initial_state_values.clone(),
                            unit: cached.compile_unit(&cached.source),
                            comp_res: cached.compiled.clone(),
                            library_gen: cached.library_gen,
                            plan: cached.retained.with_initial_inputs(&inputs),
                            intent: CompileIntent::Reset,
                        });
                        current_sessions.insert(entity, session_id);
                        admit_preparation(
                            work,
                            &tx_inner,
                            &mut solve_preparation_pool,
                            &mut live_preparation_pipeline,
                            &mut pending_compile_works,
                        );
                    }
                    ModelicaCommand::UpdateParameters {
                        entity,
                        session_id,
                        model_name,
                        source,
                        parameter_name,
                        parameter_value,
                        initial_inputs,
                    } => {
                        if session_id < *current_sessions.get(&entity).unwrap_or(&0) {
                            let _ = tx_inner.send(result_ok(entity, session_id));
                            return;
                        }
                        current_sessions.insert(entity, session_id);

                        if let Some((_, _, stepper)) = steppers.get(&entity) {
                            match stepper.capture_initial_inputs() {
                                Ok(inputs) => {
                                    if let Some(cached) = cached_models.get_mut(&entity) {
                                        cached.initial_inputs = inputs;
                                    }
                                }
                                Err(error) => {
                                    let mut result = result_ok(entity, session_id);
                                    result.is_parameter_update = true;
                                    result.error = Some(error.to_string());
                                    let _ = tx_inner.send(result);
                                    return;
                                }
                            }
                        }

                        // Re-seat under the SAME session URI the model was first
                        // compiled with. The actor owns the mutable Rumoca session;
                        // the command owner records this continuation and remains
                        // available for other entities while it compiles.
                        let Some(cached) = cached_models.get(&entity) else {
                            let mut result = result_ok(entity, session_id);
                            result.is_parameter_update = true;
                            result.error = Some(
                                "Parameter update requires a cached compiled model".to_owned(),
                            );
                            let _ = tx_inner.send(result);
                            return;
                        };
                        let unit =
                            cached.parameter_update_unit(&source, parameter_name, parameter_value);
                        let execution_domain = cached.execution_domain;
                        let doc_uri = cached.doc_uri.clone();
                        let pending = PendingCompile {
                            execution_domain,
                            entity,
                            session_id,
                            cancelled: false,
                            model_name,
                            source: source.clone(),
                            doc_uri,
                            raw_extras: unit.extras.clone(),
                            parameter_overrides: unit.parameter_overrides.clone(),
                            initial_inputs: merge_instance_values(
                                &cached.initial_inputs,
                                &initial_inputs,
                            ),
                            initial_state_values: cached.initial_state_values.clone(),
                            library_gen,
                            intent: CompileIntent::UpdateParameters,
                        };
                        submit_pending_compile(
                            pending,
                            unit,
                            &mut compiler,
                            &mut compiler_order,
                            &mut pending_compiles,
                            &tx_inner,
                        );
                    }
                    ModelicaCommand::Compile {
                        execution_domain,
                        entity,
                        session_id,
                        model_name,
                        source,
                        doc_uri,
                        extra_sources,
                        parameter_overrides,
                        authored_bindings,
                        expected_library_revision,
                        initial_inputs,
                        initial_state_values,
                        realtime_safe,
                    } => {
                        current_sessions.insert(entity, session_id);
                        // Record the declared promise for THIS model, so every
                        // later rebuild (Reset, parameter update, Step auto-init)
                        // resolves the same solver class the first compile did.
                        if realtime_safe {
                            realtime_models.insert(entity);
                        } else {
                            realtime_models.remove(&entity);
                        }

                        // Keep the raw sibling docs for cache-invalidating
                        // rebuilds, but submit the immutable source set to the
                        // single-owner Rumoca actor so this worker can keep
                        // servicing Steps for other entities.
                        let raw_extras = extra_sources.clone();
                        let mut unit = assemble_compile_unit(&source, extra_sources);
                        unit.parameter_overrides =
                            canonical_parameter_overrides(&parameter_overrides);
                        unit.authored_bindings = authored_bindings;
                        unit.expected_library_revision = expected_library_revision;
                        let pending = PendingCompile {
                            execution_domain,
                            entity,
                            session_id,
                            cancelled: false,
                            model_name: model_name.clone(),
                            source,
                            doc_uri: doc_uri.clone(),
                            raw_extras,
                            parameter_overrides,
                            initial_inputs,
                            initial_state_values,
                            library_gen,
                            intent: CompileIntent::Compile,
                        };
                        match compiler.submit_compile(
                            model_name.clone(),
                            unit,
                            doc_uri,
                            library_gen,
                        ) {
                            Ok(operation_id) => {
                                compiler_order.push_back(operation_id);
                                pending_compiles
                                    .insert(operation_id, PendingRequest::Participant(pending));
                            }
                            Err(error) => {
                                let mut result = result_ok(entity, session_id);
                                result.error = Some(format!("Compiler Error: {error}"));
                                result.is_new_model = true;
                                let _ = tx_inner.send(result);
                            }
                        }
                    }
                    ModelicaCommand::Step {
                        entity,
                        session_id,
                        step_id,
                        start_time,
                        stop_time,
                        model_name,
                        inputs,
                        dt,
                    } => {
                        if session_id < *current_sessions.get(&entity).unwrap_or(&0) {
                            let _ = tx_inner.send(step_result_ok(entity, session_id, step_id));
                            return;
                        }
                        current_sessions.insert(entity, session_id);

                        let needs_init = match steppers.get(&entity) {
                            Some((s_id, s_name, _)) => *s_id < session_id || s_name != &model_name,
                            None => true,
                        };

                        if needs_init {
                            // Reuse the cached DAE when its source and library
                            // generation match. A missing solve model is loaded or
                            // lowered asynchronously; only an invalidated library
                            // generation needs another Rumoca compile. Failures are
                            // returned with the actual cause.
                            let cached_name_matches = cached_models
                                .get(&entity)
                                .is_some_and(|c| c.model_name == model_name);
                            if cached_name_matches {
                                if let Some(mut cached) = compiled_cached_rebuild(
                                    &cached_models,
                                    entity,
                                    library_revision,
                                ) {
                                    merge_initial_inputs(&mut cached.initial_inputs, &inputs);
                                    if !cached.artifact_is_valid(library_gen) {
                                        let pipeline_load = compiler_order.len()
                                            + pending_compile_works.len()
                                            + source_root_preparation_order.len();
                                        if !solve_preparation_pool.can_admit(pipeline_load) {
                                            step_lane.push_front(ModelicaCommand::Step {
                                                entity,
                                                session_id,
                                                step_id,
                                                start_time,
                                                stop_time,
                                                model_name,
                                                inputs,
                                                dt,
                                            });
                                            return;
                                        }
                                        let step = StepRequest {
                                            entity,
                                            session_id,
                                            step_id,
                                            start_time,
                                            stop_time,
                                            model_name: model_name.clone(),
                                            inputs,
                                            dt,
                                        };
                                        let (pending, unit) = cached.into_pending_compile(
                                            entity,
                                            session_id,
                                            library_gen,
                                            CompileIntent::StepInit(step),
                                        );
                                        submit_pending_compile(
                                            pending,
                                            unit,
                                            &mut compiler,
                                            &mut compiler_order,
                                            &mut pending_compiles,
                                            &tx_inner,
                                        );
                                        return;
                                    }
                                    let step = StepRequest {
                                        entity,
                                        session_id,
                                        step_id,
                                        start_time,
                                        stop_time,
                                        model_name,
                                        inputs,
                                        dt,
                                    };
                                    submit_cached_solve_preparation(
                                        cached,
                                        entity,
                                        session_id,
                                        library_gen,
                                        profile_for(entity, &realtime_models),
                                        CompileIntent::StepInit(step),
                                        &mut solve_preparation_pool,
                                        &mut live_preparation_pipeline,
                                        &mut pending_compile_works,
                                        &tx_inner,
                                    );
                                    return;
                                }
                            }
                        }

                        if let Some((_, _, stepper)) = steppers.get(&entity) {
                            if let Err(error) =
                                validate_step_request(stepper.time(), start_time, stop_time, dt)
                            {
                                let mut result = step_result_ok(entity, session_id, step_id);
                                result.error = Some(error);
                                let _ = tx_inner.send(result);
                                steppers.remove(&entity);
                                return;
                            }
                        }

                        let live_solver_snapshot = cached_models
                            .get(&entity)
                            .map(|cached| cached.retained.snapshot.clone());

                        if let Some((s_id, _, stepper)) = steppers.get_mut(&entity) {
                            if *s_id == session_id {
                                let step_outcome =
                                    stepper.advance_checked(step_id, inputs, dt, stop_time);
                                let worker_step_duration_ns = stepper.last_step_duration_ns();
                                if let Err(e) = &step_outcome {
                                    let mut r = step_result_ok(entity, session_id, step_id);
                                    r.new_time = stepper.time();
                                    r.step_id = Some(step_id);
                                    r.worker_step_duration_ns = worker_step_duration_ns;
                                    r.worker_backlog_count = worker_backlog_count;
                                    // Runtime solver blow-up: `SimulationDiagnosticError`
                                    // Display is human-readable (the `Solver` variant
                                    // carries no source span, so it stays unlocated).
                                    r.error = Some(format!("Solver Error: {e}"));
                                    let _ = tx_inner.send(r);
                                    steppers.remove(&entity);
                                } else {
                                    // The checked session observation includes
                                    // reconstructed algebraics, outputs, and inputs.
                                    let outputs = step_outcome
                                        .expect("successful step outcome checked above");
                                    let new_time = stepper.time();
                                    let _ = tx_inner.send(ModelicaResult {
                                        entity,
                                        session_id,
                                        step_id: Some(step_id),
                                        new_time,
                                        outputs,
                                        error: None,
                                        log_message: None,
                                        is_new_model: false,
                                        detected_symbols: Vec::new(),
                                        is_parameter_update: false,
                                        is_reset: false,
                                        detected_input_names: Vec::new(),
                                        worker_step_duration_ns,
                                        worker_backlog_count,
                                        live_solver_snapshot,
                                        variable_metadata: cached_models.get_mut(&entity).and_then(
                                            |cached| cached.pending_variable_metadata.take(),
                                        ),
                                        ..Default::default()
                                    });
                                }
                            } else {
                                let _ = tx_inner.send(step_result_ok(entity, session_id, step_id));
                            }
                        } else {
                            let mut r = step_result_ok(entity, session_id, step_id);
                            r.error = Some(
                                "No compiled model. Click Compile (or Run will compile + start)."
                                    .to_string(),
                            );
                            let _ = tx_inner.send(r);
                        }
                    }
                    ModelicaCommand::Despawn { entity } => {
                        steppers.remove(&entity);
                        cached_models.remove(&entity);
                        for work in pending_compile_works
                            .values_mut()
                            .filter_map(PreparationWork::participant_mut)
                        {
                            if work.entity == entity {
                                work.cancelled = true;
                            }
                        }
                        for work in pending_compiles
                            .values_mut()
                            .filter_map(PendingRequest::participant_mut)
                        {
                            if work.entity == entity {
                                work.cancelled = true;
                            }
                        }
                    }
                    ModelicaCommand::LoadSourceRoot {
                        id,
                        operation_id,
                        payload,
                    } => {
                        if latest_source_root_operations.get(&id) == Some(&operation_id) {
                            let preparation_id = solve_preparation_pool.submit_source_root(
                                id,
                                operation_id,
                                payload,
                            );
                            source_root_preparation_order.push_back(preparation_id);
                        } else {
                            log::debug!(
                                "[modelica-runtime] discarded superseded source-root load `{id}` operation={operation_id}"
                            );
                        }
                    }
                    ModelicaCommand::UnloadSourceRoot { id, operation_id } => {
                        latest_source_root_operations.insert(id.clone(), operation_id);
                        match compiler.submit_source_root_unload(id.clone(), operation_id) {
                            Ok(operation) => {
                                compiler_order.push_back(operation);
                                pending_source_root_installs.insert(operation);
                            }
                            Err(error) => log::error!(
                                "[modelica-runtime] failed to queue unload for `{id}`: {error}"
                            ),
                        }
                    }
                }
            }));

            let elapsed = cmd_started.elapsed();
            // Flag anything slow enough that a user would perceive it
            // as "stuck" at WARN so it shows up even without verbose
            // logging. The 2s threshold is well above a typical source library
            // compile (<500ms) but below "waited through it" (>5s).
            if elapsed > std::time::Duration::from_secs(2) {
                log::warn!(
                    "[worker] end: {} took {:?} (slow — possible stall)",
                    cmd_label,
                    elapsed
                );
            } else {
                log::debug!("[worker] end: {} took {:?}", cmd_label, elapsed);
            }

            if result.is_err() {
                if let Some(reply) = batch_failure_reply {
                    let _ =
                        reply.send(Err("Modelica batch preparation command panicked".to_owned()));
                }
                if let Some(entity) = panic_entity {
                    steppers.remove(&entity);
                    cached_models.remove(&entity);
                    current_sessions.remove(&entity);
                    realtime_models.remove(&entity);
                }
                match panic_result {
                    ModelicaWorkerResponse::Preparation(result) => {
                        preparation_requests.finish(result.request_id, result.outcome)
                    }
                    response => {
                        let _ = tx.send(response);
                    }
                }
            }
        }
    }
}

/// One-line identifier for a `ModelicaCommand`, used in worker
/// instrumentation logs. Includes the model name where available so
/// a stall can be pinned to a specific source.
#[cfg(not(target_arch = "wasm32"))]
fn command_label(cmd: &ModelicaCommand) -> String {
    match cmd {
        ModelicaCommand::PrepareInterface {
            request_id,
            model_name,
            ..
        } => {
            format!("PrepareInterface model={model_name} request={request_id:?}")
        }
        ModelicaCommand::Initialize {
            request_id,
            model_name,
            ..
        } => {
            format!("Initialize model={model_name} request={request_id:?}")
        }
        ModelicaCommand::CancelPreparation { request_id } => {
            format!("CancelPreparation request={request_id:?}")
        }
        ModelicaCommand::Step {
            model_name, entity, ..
        } => {
            format!("Step model={model_name} entity={entity:?}")
        }
        ModelicaCommand::Compile {
            model_name, entity, ..
        } => {
            format!("Compile model={model_name} entity={entity:?}")
        }
        ModelicaCommand::UpdateParameters {
            model_name, entity, ..
        } => {
            format!("UpdateParameters model={model_name} entity={entity:?}")
        }
        ModelicaCommand::Reset { entity, .. } => format!("Reset entity={entity:?}"),
        ModelicaCommand::Despawn { entity } => format!("Despawn entity={entity:?}"),
        ModelicaCommand::LoadSourceRoot { id, .. } => format!("LoadSourceRoot id={id}"),
        ModelicaCommand::UnloadSourceRoot { id, .. } => format!("UnloadSourceRoot id={id}"),
        ModelicaCommand::CompileBatch { model_name, .. } => {
            format!("CompileBatch model={model_name}")
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cmd_entity(cmd: &ModelicaCommand) -> Option<Entity> {
    match cmd {
        ModelicaCommand::Step { entity, .. }
        | ModelicaCommand::Compile { entity, .. }
        | ModelicaCommand::UpdateParameters { entity, .. }
        | ModelicaCommand::Reset { entity, .. }
        | ModelicaCommand::Despawn { entity } => Some(*entity),
        ModelicaCommand::LoadSourceRoot { .. }
        | ModelicaCommand::UnloadSourceRoot { .. }
        | ModelicaCommand::CompileBatch { .. }
        | ModelicaCommand::PrepareInterface { .. }
        | ModelicaCommand::Initialize { .. }
        | ModelicaCommand::CancelPreparation { .. } => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cmd_session(cmd: &ModelicaCommand) -> Option<u64> {
    match cmd {
        ModelicaCommand::Step { session_id, .. }
        | ModelicaCommand::Compile { session_id, .. }
        | ModelicaCommand::UpdateParameters { session_id, .. }
        | ModelicaCommand::Reset { session_id, .. } => Some(*session_id),
        ModelicaCommand::Despawn { .. }
        | ModelicaCommand::LoadSourceRoot { .. }
        | ModelicaCommand::UnloadSourceRoot { .. }
        | ModelicaCommand::CompileBatch { .. }
        | ModelicaCommand::PrepareInterface { .. }
        | ModelicaCommand::Initialize { .. }
        | ModelicaCommand::CancelPreparation { .. } => None,
    }
}

/// Whether consecutive commands replace the same entity's idempotent setpoint.
///
/// Squashing prevents "back-pressure" lag when the UI sends rapid updates
/// (e.g., dragging a parameter slider). Only the latest value is processed —
/// the dropped command is acked with a synthetic success (`result_ok`).
///
/// **`Step` is NOT squashable** (A5). Squashing is only sound for commands that
/// are *idempotent setpoints*: `UpdateParameters` for the same scalar parameter
/// (the last value wins) and `Compile` (the last source wins). Distinct parameter
/// deltas must remain ordered so each override is retained. A `Step` is an **integration**, not a setpoint: collapsing two
/// `Step`s deletes `dt` of model time from the co-simulation and then reports
/// SUCCESS for the step that never ran, so the model silently falls behind the
/// world clock with nothing to show for it.
///
/// If back-pressure on `Step` is ever genuinely needed, coalesce by **summing
/// the `dt`s** — never by dropping one.
#[cfg(not(target_arch = "wasm32"))]
fn is_squashable(last: &ModelicaCommand, next: &ModelicaCommand) -> bool {
    match (last, next) {
        (
            ModelicaCommand::UpdateParameters {
                entity: e1,
                parameter_name: p1,
                ..
            },
            ModelicaCommand::UpdateParameters {
                entity: e2,
                parameter_name: p2,
                ..
            },
        ) => e1 == e2 && p1 == p2,
        (
            ModelicaCommand::Compile { entity: e1, .. },
            ModelicaCommand::Compile { entity: e2, .. },
        ) => e1 == e2,
        _ => false,
    }
}

// =============================================================================
// WebAssembly Web Worker state
// =============================================================================
//
// The browser transport dispatches ordered messages inside a dedicated Web
// Worker. Each worker admits its host's immutable cache configuration before
// constructing this state; the main-thread Bevy host does not run its solver.

/// Simulation state owned by the wasm Web Worker.
/// Mirrors the local variables in `modelica_worker` on desktop.
///
/// `pub` so the off-thread worker bin (`bin/lunica_worker.rs`) can own
/// one of these directly. The fields stay private — only the type itself
/// crosses crate boundaries.
#[cfg(target_arch = "wasm32")]
pub struct ModelicaWorkerState {
    steppers: HashMap<Entity, (u64, String, LiveStepper)>,
    current_sessions: HashMap<Entity, u64>,
    cached_models: HashMap<Entity, CachedModel>,
    compiled_artifacts: CompiledArtifactCache,
    compiler: Option<ModelicaCompiler>,
    /// Models that declared the realtime promise — the same per-entity fact the
    /// native worker keeps, so wasm resolves the same solver for the same model.
    realtime_models: std::collections::HashSet<Entity>,
    /// M3: bumped on every LoadSourceRoot (and compiler reset) to invalidate
    /// cached compiled artifacts — same contract as the native worker's local.
    library_gen: u64,
    /// Latest registry operation per source-set id, fencing transported
    /// source-root messages whose owner has already closed.
    latest_source_root_operations: HashMap<String, u64>,
}

#[cfg(target_arch = "wasm32")]
impl ModelicaWorkerState {
    pub fn new(limits: ModelicaCacheLimits) -> Result<Self, String> {
        let capacities = limits.validate()?;
        Ok(Self {
            steppers: HashMap::new(),
            current_sessions: HashMap::new(),
            cached_models: HashMap::new(),
            compiled_artifacts: CompiledArtifactCache::new(capacities.compiled_artifacts),
            compiler: None,
            realtime_models: Default::default(),
            library_gen: 0,
            latest_source_root_operations: HashMap::new(),
        })
    }

    /// Drop inconsistent runtime state after a panic, retaining admitted limits.
    pub fn reset(&mut self) {
        self.steppers.clear();
        self.current_sessions.clear();
        self.cached_models.clear();
        self.compiled_artifacts.clear();
        self.compiler = None;
        self.realtime_models.clear();
        self.library_gen = 0;
        self.latest_source_root_operations.clear();
    }

    /// Lazily-built shared compiler. Same instance the regular
    /// Compile path uses, so RunFast hits the same warm caches.
    pub fn compiler(&mut self) -> &mut ModelicaCompiler {
        self.compiler.get_or_insert_with(ModelicaCompiler::new)
    }
}

/// Apply a single `ModelicaCommand` against the worker state, sending
/// any resulting `ModelicaResult` values through `send`.
///
/// Same dispatch the desktop `modelica_worker` loop runs, parameterised over
/// the result sink so the native worker and the off-thread Web Worker entry
/// (`bin/lunica_worker.rs`) can share it. Passing a closure rather than a
/// concrete `Sender` keeps this fn agnostic to whether results go to a
/// crossbeam channel, a `Vec`, or a `postMessage` queue.
///
/// `state` carries the per-entity `SimulationSession` map, DAE cache, and the lazy
/// `ModelicaCompiler`. The wasm worker bin owns one of these for the lifetime
/// of the page and reuses it across postMessage dispatches.
#[cfg(target_arch = "wasm32")]
pub fn process_worker_command<F: FnMut(ModelicaWorkerResponse)>(
    state: &mut ModelicaWorkerState,
    cmd: ModelicaCommand,
    mut send: F,
) {
    let cmd = match cmd {
        command @ ModelicaCommand::Initialize { request_id, .. } => {
            let outcome = initialization::execute_wasm(state, command)
                .map(ModelicaPreparationOutcome::Initialization);
            send(
                ModelicaPreparationResult {
                    request_id,
                    outcome,
                }
                .into(),
            );
            return;
        }
        ModelicaCommand::PrepareInterface {
            request_id,
            entity,
            session_id,
            model_name,
            source,
            doc_uri,
            extra_sources,
            parameter_overrides,
            authored_bindings,
        } => {
            let compiler = state.compiler.get_or_insert_with(ModelicaCompiler::new);
            let mut unit = assemble_compile_unit(&source, extra_sources);
            unit.parameter_overrides = canonical_parameter_overrides(&parameter_overrides);
            unit.authored_bindings = authored_bindings;
            let outcome = compile_shared(
                &mut state.compiled_artifacts,
                compiler,
                &model_name,
                &unit,
                &doc_uri,
                state.library_gen,
            )
            .and_then(|compiled| {
                let interface = compiled
                    .edit_interface
                    .as_deref()
                    .ok_or("interface preparation produced no checked declarations")?
                    .clone();
                Ok(ModelicaPreparationOutcome::Interface(
                    ModelicaInterfaceOutcome {
                        entity,
                        session_id,
                        doc_uri,
                        interface,
                        parameter_overrides: unit.parameter_overrides,
                        authored_bindings: unit.authored_bindings,
                        library_revision: compiler.library_revision(),
                    },
                ))
            });
            send(
                ModelicaPreparationResult {
                    request_id,
                    outcome,
                }
                .into(),
            );
            return;
        }
        ModelicaCommand::CancelPreparation { .. } => {
            // Web-worker dispatch is serial: the preceding initialization has
            // already returned its terminal result before cancellation runs.
            return;
        }
        command => command,
    };
    let mut send = |result: ModelicaResult| send(ModelicaWorkerResponse::Participant(result));
    let w = state;
    match cmd {
        ModelicaCommand::Initialize { .. }
        | ModelicaCommand::PrepareInterface { .. }
        | ModelicaCommand::CancelPreparation { .. } => {
            unreachable!("initialization commands dispatched above")
        }
        ModelicaCommand::Step {
            entity,
            session_id,
            step_id,
            start_time,
            stop_time,
            model_name,
            inputs,
            dt,
        } => {
            // Auto-init: rebuild from the cached compiled artifact (M3) if
            // the stepper doesn't exist — recompiles only when invalidated.
            if !w.steppers.contains_key(&entity) {
                let cached_name_matches = w
                    .cached_models
                    .get(&entity)
                    .is_some_and(|c| c.model_name == model_name);
                if cached_name_matches {
                    let rebuild = {
                        let mut backend = InlineCompileBackend {
                            compiler: &mut w.compiler,
                            artifacts: &mut w.compiled_artifacts,
                        };
                        rebuild_from_cache(
                            &mut w.cached_models,
                            &mut backend,
                            entity,
                            w.library_gen,
                        )
                    };
                    if let Some(mut rb) = rebuild {
                        merge_initial_inputs(&mut rb.initial_inputs, &inputs);
                        let comp_res = match rb.outcome {
                            Ok(comp_res) => comp_res,
                            Err(error) => {
                                let mut result = step_result_ok(entity, session_id, step_id);
                                result.error = Some(format!("Initialization failed: {error}"));
                                result.compile_diagnostics = rb.compile_diagnostics;
                                send(result);
                                return;
                            }
                        };
                        let (
                            stepper,
                            _live_solver_snapshot,
                            variable_metadata,
                            _,
                            construction_profile,
                            retained,
                        ) = match build_stepper(
                            &comp_res,
                            profile_for(entity, &w.realtime_models),
                            &rb.parameter_overrides,
                            &rb.initial_inputs,
                            rb.initial_state_values.as_deref(),
                            comp_res.prepared_source_key,
                            rb.library_revision,
                        ) {
                            Ok(built) => built,
                            Err(error) => {
                                let mut result = step_result_ok(entity, session_id, step_id);
                                result.error = Some(format!("Initialization failed: {error}"));
                                result.compile_diagnostics =
                                    diagnostics_from_sim_error(&error, &rb.source);
                                send(result);
                                return;
                            }
                        };
                        send(ModelicaResult { entity, session_id,
                            preparation_phase: Some(lunco_modelica_runtime::ModelicaPreparationPhase::LiveSolverConstructed(construction_profile)),
                            ..Default::default() });
                        if let Some(cached) = w.cached_models.get_mut(&entity) {
                            cached.retained = retained;
                            cached.pending_variable_metadata = Some(variable_metadata);
                        }
                        w.steppers
                            .insert(entity, (session_id, model_name.clone(), stepper));
                    }
                }
            }

            if let Some((_, _, stepper)) = w.steppers.get(&entity) {
                if let Err(error) = validate_step_request(stepper.time(), start_time, stop_time, dt)
                {
                    let mut result = step_result_ok(entity, session_id, step_id);
                    result.error = Some(error);
                    send(result);
                    w.steppers.remove(&entity);
                    return;
                }
            }

            let live_solver_snapshot = w
                .cached_models
                .get(&entity)
                .map(|cached| cached.retained.snapshot.clone());

            if let Some((s_id, _, stepper)) = w.steppers.get_mut(&entity) {
                if *s_id == session_id {
                    let step_started = web_time::Instant::now();
                    let step_outcome = advance_checked_step(stepper, &inputs, stop_time);
                    let worker_step_duration_ns = step_started.elapsed().as_nanos() as u64;

                    if let Err(e) = &step_outcome {
                        send(ModelicaResult {
                            entity,
                            session_id,
                            step_id: Some(step_id),
                            new_time: stepper.time(),
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(),
                            error: Some(format!("Solver Error: {e}")),
                            log_message: None,
                            is_new_model: false,
                            is_parameter_update: false,
                            is_reset: false,
                            detected_input_names: Vec::new(),
                            worker_step_duration_ns: Some(worker_step_duration_ns),
                            ..Default::default()
                        });
                        w.steppers.remove(&entity);
                    } else {
                        let outputs = step_outcome.expect("successful step outcome checked above");
                        send(ModelicaResult {
                            entity,
                            session_id,
                            step_id: Some(step_id),
                            new_time: stepper.time(),
                            outputs,
                            error: None,
                            log_message: None,
                            is_new_model: false,
                            detected_symbols: Vec::new(),
                            is_parameter_update: false,
                            is_reset: false,
                            detected_input_names: Vec::new(),
                            live_solver_snapshot,
                            variable_metadata: w
                                .cached_models
                                .get_mut(&entity)
                                .and_then(|cached| cached.pending_variable_metadata.take()),
                            worker_step_duration_ns: Some(worker_step_duration_ns),
                            ..Default::default()
                        });
                    }
                } else {
                    send(step_result_ok(entity, session_id, step_id));
                }
            } else {
                // Stepping requires a successfully installed compiled session.
                send(ModelicaResult {
                    entity,
                    session_id,
                    step_id: Some(step_id),
                    new_time: 0.0,
                    outputs: Vec::new(),
                    detected_symbols: Vec::new(),
                    error: Some(
                        "No compiled model. Click Compile (or Run will compile + start)."
                            .to_string(),
                    ),
                    log_message: None,
                    is_new_model: false,
                    is_parameter_update: false,
                    is_reset: false,
                    detected_input_names: Vec::new(),
                    ..Default::default()
                });
            }
        }
        ModelicaCommand::Compile {
            execution_domain: _,
            entity,
            session_id,
            model_name,
            source,
            doc_uri,
            extra_sources,
            parameter_overrides,
            authored_bindings,
            expected_library_revision,
            initial_inputs,
            initial_state_values,
            realtime_safe,
        } => {
            if realtime_safe {
                w.realtime_models.insert(entity);
            } else {
                w.realtime_models.remove(&entity);
            }
            w.current_sessions.insert(entity, session_id);
            // Raw sibling docs for the cache — see the native Compile arm.
            let raw_extras = extra_sources.clone();
            let mut unit = assemble_compile_unit(&source, extra_sources);
            unit.parameter_overrides = canonical_parameter_overrides(&parameter_overrides);
            unit.authored_bindings = authored_bindings;
            unit.expected_library_revision = expected_library_revision;

            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);

            let compile_outcome = compile_shared(
                &mut w.compiled_artifacts,
                compiler,
                &model_name,
                &unit,
                &doc_uri,
                w.library_gen,
            );
            match compile_outcome {
                Ok(comp_res) => {
                    let parameter_overrides = merge_instance_values(
                        &parameter_overrides,
                        &comp_res.authored_bindings.parameter_overrides,
                    );
                    let initial_inputs = merge_instance_values(
                        &comp_res.authored_bindings.initial_inputs,
                        &initial_inputs,
                    );
                    let stepper_result = build_stepper(
                        &comp_res,
                        profile_for(entity, &w.realtime_models),
                        &parameter_overrides,
                        &initial_inputs,
                        initial_state_values.as_deref(),
                        comp_res.prepared_source_key,
                        Some(compiler.library_revision()),
                    );
                    match stepper_result {
                        Ok((
                            stepper,
                            live_solver_snapshot,
                            variable_metadata,
                            symbols,
                            construction_profile,
                            retained,
                        )) => {
                            send(ModelicaResult { entity, session_id,
                                preparation_phase: Some(lunco_modelica_runtime::ModelicaPreparationPhase::LiveSolverConstructed(construction_profile)),
                                ..Default::default() });
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let unit_hash = compile_unit_hash(&model_name, &doc_uri, &unit);
                            w.cached_models.insert(
                                entity,
                                CachedModel {
                                    model_name: model_name.clone(),
                                    source: Arc::from(source.clone()),
                                    extra_sources: raw_extras,
                                    parameter_overrides,
                                    initial_inputs,
                                    expected_library_revision: unit.expected_library_revision,
                                    initial_state_values,
                                    doc_uri: doc_uri.clone(),
                                    compiled: comp_res.clone(),
                                    unit_hash,
                                    library_gen: w.library_gen,
                                    retained,
                                    pending_variable_metadata: None,
                                },
                            );

                            w.steppers
                                .insert(entity, (session_id, model_name.clone(), stepper));
                            send(add_experiment_defaults(
                                ModelicaResult {
                                    entity,
                                    session_id,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    detected_symbols: symbols,
                                    error: None,
                                    log_message: Some("Compiled successfully.".to_string()),
                                    is_new_model: true,
                                    is_parameter_update: false,
                                    is_reset: false,
                                    detected_input_names: input_names,
                                    compiled_model_name: Some(model_name.clone()),
                                    loaded_source_root_id: None,
                                    live_solver_snapshot: Some(live_solver_snapshot),
                                    variable_metadata: Some(variable_metadata),
                                    ..Default::default()
                                },
                                &comp_res,
                            ));
                        }
                        Err(e) => {
                            send(ModelicaResult {
                                entity,
                                session_id,
                                new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(),
                                error: Some(format!("Stepper Init Error: {e}")),
                                log_message: None,
                                is_new_model: true,
                                is_parameter_update: false,
                                is_reset: false,
                                detected_input_names: Vec::new(),
                                compile_diagnostics: diagnostics_from_sim_error(&e, &unit.source),
                                ..Default::default()
                            });
                        }
                    }
                }
                Err(e) => {
                    // Structured, located diagnostics so the Diagnostics
                    // panel can make compile errors click-to-source.
                    let diags = compiler.take_compile_diagnostics(&model_name, &doc_uri);
                    send(ModelicaResult {
                        entity,
                        session_id,
                        new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(),
                        error: Some(format!("Compile Error: {e}")),
                        log_message: None,
                        is_new_model: true,
                        is_parameter_update: false,
                        is_reset: false,
                        detected_input_names: Vec::new(),
                        compile_diagnostics: diags,
                        ..Default::default()
                    });
                }
            }
        }
        ModelicaCommand::Reset {
            entity,
            session_id,
            target,
            initial_inputs,
        } => {
            if let Err(error) = validate_reset_target(
                w.cached_models.get(&entity),
                w.steppers
                    .get(&entity)
                    .map(|(session, name, _)| (*session, name.as_str())),
                w.current_sessions.get(&entity).copied(),
                &target,
            ) {
                let mut result = reset_ok(
                    entity,
                    session_id,
                    Vec::new(),
                    Vec::new(),
                    "Reset rejected.",
                );
                result.error = Some(error);
                send(result);
                return;
            }
            let captured = match capture_initial_inputs(
                &w.steppers.get(&entity).expect("validated reset session").2,
            ) {
                Ok(inputs) => inputs,
                Err(error) => {
                    let mut result = reset_ok(
                        entity,
                        session_id,
                        Vec::new(),
                        Vec::new(),
                        "Reset rejected.",
                    );
                    result.error = Some(error.to_string());
                    send(result);
                    return;
                }
            };
            let inputs = merge_instance_values(&captured, &initial_inputs);
            let cached = w.cached_models.get(&entity).expect("validated reset cache");
            let comp_res = cached.compiled.clone();
            let source = Arc::clone(&cached.source);
            let model_name = cached.model_name.clone();
            let plan = cached.retained.with_initial_inputs(&inputs);
            let built = build_live_stepper(&comp_res.compiled.dae, plan);
            w.current_sessions.insert(entity, session_id);
            match built {
                Ok((
                    stepper,
                    live_solver_snapshot,
                    variable_metadata,
                    symbols,
                    construction_profile,
                    retained,
                )) => {
                    send(ModelicaResult {
                        entity,
                        session_id,
                        preparation_phase: Some(
                            lunco_modelica_runtime::ModelicaPreparationPhase::LiveSolverConstructed(
                                construction_profile,
                            ),
                        ),
                        ..Default::default()
                    });
                    let input_names = stepper.input_names().to_vec();
                    w.steppers.insert(entity, (session_id, model_name, stepper));
                    let cached = w
                        .cached_models
                        .get_mut(&entity)
                        .expect("reset retains its cache owner");
                    cached.retained = retained;
                    cached.initial_inputs = inputs;
                    cached.pending_variable_metadata = None;
                    let mut result =
                        reset_ok(entity, session_id, symbols, input_names, "Reset complete.");
                    result.live_solver_snapshot = Some(live_solver_snapshot);
                    result.variable_metadata = Some(variable_metadata);
                    send(add_experiment_defaults(result, &comp_res));
                }
                Err(error) => send(ModelicaResult {
                    entity,
                    session_id,
                    is_reset: true,
                    compile_diagnostics: diagnostics_from_sim_error(&error, &source),
                    error: Some(format!("Reset stepper error: {error}")),
                    ..Default::default()
                }),
            }
        }
        ModelicaCommand::UpdateParameters {
            entity,
            session_id,
            model_name,
            source,
            parameter_name,
            parameter_value,
            initial_inputs: current_inputs,
        } => {
            if session_id < *w.current_sessions.get(&entity).unwrap_or(&0) {
                send(result_ok(entity, session_id));
                return;
            }
            w.current_sessions.insert(entity, session_id);
            let Some(cached) = w.cached_models.get(&entity) else {
                let mut result = result_ok(entity, session_id);
                result.is_parameter_update = true;
                result.error = Some("Parameter update requires a cached compiled model".to_owned());
                send(result);
                return;
            };
            let unit = cached.parameter_update_unit(&source, parameter_name, parameter_value);
            let parameter_overrides = unit.parameter_overrides.clone();
            let doc_uri = cached.doc_uri.clone();
            let initial_state_values = cached.initial_state_values.clone();
            let cached_inputs = cached.initial_inputs.clone();
            let initial_inputs = match w.steppers.get(&entity) {
                Some((_, _, stepper)) => match capture_initial_inputs(stepper) {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        let mut result = result_ok(entity, session_id);
                        result.is_parameter_update = true;
                        result.error = Some(error.to_string());
                        send(result);
                        return;
                    }
                },
                None => cached_inputs,
            };

            let initial_inputs = merge_instance_values(&initial_inputs, &current_inputs);
            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);

            match compile_shared(
                &mut w.compiled_artifacts,
                compiler,
                &model_name,
                &unit,
                &doc_uri,
                w.library_gen,
            ) {
                Ok(comp_res) => {
                    match build_stepper(
                        &comp_res,
                        profile_for(entity, &w.realtime_models),
                        &parameter_overrides,
                        &initial_inputs,
                        initial_state_values.as_deref(),
                        comp_res.prepared_source_key,
                        Some(compiler.library_revision()),
                    ) {
                        Ok((
                            stepper,
                            live_solver_snapshot,
                            variable_metadata,
                            symbols,
                            construction_profile,
                            retained,
                        )) => {
                            send(ModelicaResult { entity, session_id,
                                preparation_phase: Some(lunco_modelica_runtime::ModelicaPreparationPhase::LiveSolverConstructed(construction_profile)),
                                ..Default::default() });
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let unit_hash = compile_unit_hash(&model_name, &doc_uri, &unit);
                            w.cached_models.insert(
                                entity,
                                CachedModel {
                                    model_name: model_name.clone(),
                                    source: Arc::from(source.clone()),
                                    extra_sources: unit.extras.clone(),
                                    parameter_overrides,
                                    initial_inputs,
                                    expected_library_revision: unit.expected_library_revision,
                                    initial_state_values,
                                    doc_uri: doc_uri.clone(),
                                    compiled: comp_res.clone(),
                                    unit_hash,
                                    library_gen: w.library_gen,
                                    retained,
                                    pending_variable_metadata: None,
                                },
                            );

                            w.steppers
                                .insert(entity, (session_id, model_name.clone(), stepper));
                            send(add_experiment_defaults(
                                ModelicaResult {
                                    entity,
                                    session_id,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    detected_symbols: symbols,
                                    error: None,
                                    log_message: Some("Parameters applied.".to_string()),
                                    is_new_model: false,
                                    is_parameter_update: true,
                                    is_reset: false,
                                    detected_input_names: input_names,
                                    compile_diagnostics: Vec::new(),
                                    live_solver_snapshot: Some(live_solver_snapshot),
                                    variable_metadata: Some(variable_metadata),
                                    ..Default::default()
                                },
                                &comp_res,
                            ));
                        }
                        Err(e) => {
                            send(ModelicaResult {
                                entity,
                                session_id,
                                new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(),
                                error: Some(format!("Stepper Init Error: {e}")),
                                log_message: None,
                                is_new_model: false,
                                is_parameter_update: true,
                                is_reset: false,
                                detected_input_names: Vec::new(),
                                compile_diagnostics: diagnostics_from_sim_error(&e, &unit.source),
                                ..Default::default()
                            });
                        }
                    }
                }
                Err(e) => {
                    send(ModelicaResult {
                        entity,
                        session_id,
                        new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(),
                        error: Some(format!("Re-compile Error: {e}")),
                        log_message: None,
                        is_new_model: false,
                        is_parameter_update: true,
                        is_reset: false,
                        detected_input_names: Vec::new(),
                        compile_diagnostics: compiler
                            .take_compile_diagnostics(&model_name, &doc_uri),
                        ..Default::default()
                    });
                }
            }
        }
        ModelicaCommand::Despawn { entity } => {
            w.steppers.remove(&entity);
            w.cached_models.remove(&entity);
        }
        ModelicaCommand::LoadSourceRoot {
            id,
            operation_id,
            payload,
        } => {
            if !record_source_root_operation(
                &mut w.latest_source_root_operations,
                &id,
                operation_id,
            ) {
                log::debug!(
                    "[modelica-worker] discarded stale LoadSourceRoot `{id}` operation={operation_id}"
                );
                return;
            }
            // Wasm path: matches the native handler. The worker thread merges the
            // library into its session after the host has supplied source text.
            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            let t0 = web_time::Instant::now();
            let prepared = prepare_source_root_payload(id.clone(), payload);
            let documents = prepared.source_snapshots();
            let report = compiler.install_source_root(prepared);
            if report.diagnostics.is_empty() && report.inserted_file_count > 0 {
                w.library_gen += 1;
                w.compiled_artifacts.clear();
            }
            log::info!(
                "[modelica-worker] LoadSourceRoot `{}`: {} parsed / {} \
                 inserted in {:.2}s",
                id,
                report.parsed_file_count,
                report.inserted_file_count,
                t0.elapsed().as_secs_f64(),
            );
            let err = if report.diagnostics.is_empty() {
                None
            } else {
                Some(report.diagnostics.join("; "))
            };
            let namespaces = if err.is_none() {
                compiler.source_root_namespaces(&id)
            } else {
                Vec::new()
            };
            send(ModelicaResult {
                loaded_source_root_documents: if err.is_none() { documents } else { Vec::new() },
                loaded_source_root_sources: if err.is_none() {
                    compiler.source_root_source_facts(&id)
                } else {
                    Vec::new()
                },
                loaded_source_root_id: Some(id),
                loaded_source_root_namespaces: namespaces,
                source_root_operation_id: Some(operation_id),
                error: err,
                ..Default::default()
            });
        }
        ModelicaCommand::UnloadSourceRoot { id, operation_id } => {
            if !record_source_root_operation(
                &mut w.latest_source_root_operations,
                &id,
                operation_id,
            ) {
                log::debug!(
                    "[modelica-worker] discarded stale UnloadSourceRoot `{id}` operation={operation_id}"
                );
                return;
            }
            if let Some(compiler) = w.compiler.as_mut() {
                let removed = compiler.remove_source_root(&id);
                if removed {
                    w.library_gen = w.library_gen.wrapping_add(1);
                    w.compiled_artifacts.clear();
                }
                log::info!(
                    "[modelica-worker] unloaded Modelica source root `{id}` (removed={removed})"
                );
            }
            retire_source_root_operation(&mut w.latest_source_root_operations, &id, operation_id);
            send(ModelicaResult {
                unloaded_source_root_id: Some(id),
                source_root_operation_id: Some(operation_id),
                ..Default::default()
            });
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod worker_budget_tests {
    use super::*;

    #[test]
    fn parameter_update_without_a_cached_identity_is_terminal() {
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let one = WorkerThreadBudget {
            max_worker_threads: 1,
            reserved_cpu_threads: 0,
        };
        let owner = std::thread::spawn(move || {
            modelica_worker(
                command_rx,
                ModelicaResponseSender::new(result_tx, crossbeam_channel::unbounded().0),
                ModelicaArtifactDiskLimits::default(),
                ModelicaCacheLimits::default(),
                ModelicaWorkerLimits {
                    preparation: one,
                    live_execution: one,
                    document_execution: one,
                },
            )
        });
        let entity = Entity::from_raw_u32(7).unwrap();
        command_tx
            .send(ModelicaCommand::UpdateParameters {
                entity,
                session_id: 1,
                model_name: "Uncompiled".to_owned(),
                source: "model Uncompiled end Uncompiled;".to_owned(),
                parameter_name: "gain".into(),
                parameter_value: 1.0,
                initial_inputs: Vec::new(),
            })
            .unwrap();
        let result = result_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(result.entity, entity);
        assert_eq!(result.session_id, 1);
        assert!(result.is_parameter_update);
        assert_eq!(
            result.error.as_deref(),
            Some("Parameter update requires a cached compiled model")
        );
        assert!(result.worker_failure.is_none());
        drop(command_tx);
        assert!(owner.join().is_ok());
        assert!(result_rx.try_recv().is_err());
    }

    #[test]
    fn parameter_updates_retain_sibling_sources_and_distinct_instance_overrides() {
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let one = WorkerThreadBudget {
            max_worker_threads: 1,
            reserved_cpu_threads: 0,
        };
        let owner = std::thread::spawn(move || {
            modelica_worker(
                command_rx,
                ModelicaResponseSender::new(result_tx, crossbeam_channel::unbounded().0),
                ModelicaArtifactDiskLimits::default(),
                ModelicaCacheLimits::default(),
                ModelicaWorkerLimits {
                    preparation: one,
                    live_execution: one,
                    document_execution: one,
                },
            )
        });
        let entity = Entity::from_raw_u32(9).unwrap();
        let source = |gain| {
            format!(
                "model ParameterProbe parameter Real gain={gain}; parameter Real keep=1; ParameterHelper helper; output Real value; equation value=gain*keep*helper.value; end ParameterProbe;"
            )
        };
        let terminal = || loop {
            let result = result_rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap();
            if result.preparation_phase.is_none() {
                break result;
            }
        };
        command_tx.send(ModelicaCommand::Compile {
            entity, session_id: 1,
            execution_domain: lunco_modelica_runtime::ModelicaExecutionDomain::Document,
            model_name: "ParameterProbe".into(), source: source(1), realtime_safe: false,
            doc_uri: "ParameterProbe.mo".into(),
            extra_sources: vec![("ParameterHelper.mo".into(), "model ParameterHelper output Real value; equation value=2; end ParameterHelper;".into())],
            expected_library_revision: None,
            parameter_overrides: vec![("keep".into(), 5.0)],
            authored_bindings: Default::default(), initial_inputs: Vec::new(), initial_state_values: None,
        }).unwrap();
        let initial = terminal();
        assert!(initial.error.is_none(), "{:?}", initial.error);
        assert_eq!(
            initial
                .detected_symbols
                .iter()
                .find(|(name, _)| name == "value")
                .unwrap()
                .1,
            10.0
        );
        for (session_id, gain) in [(2, 3), (3, 4)] {
            command_tx
                .send(ModelicaCommand::UpdateParameters {
                    entity,
                    session_id,
                    model_name: "ParameterProbe".into(),
                    source: source(gain),
                    parameter_name: "gain".into(),
                    parameter_value: f64::from(gain),
                    initial_inputs: Vec::new(),
                })
                .unwrap();
            let result = terminal();
            assert!(result.error.is_none(), "{:?}", result.error);
            assert!(result.is_parameter_update);
            assert_eq!(
                result
                    .detected_symbols
                    .iter()
                    .find(|(name, _)| name == "value")
                    .unwrap()
                    .1,
                f64::from(gain) * 10.0
            );
            assert!(
                result
                    .detected_parameter_names
                    .iter()
                    .any(|name| name == "keep")
            );
            assert!(
                result
                    .live_solver_snapshot
                    .unwrap()
                    .parameter_overrides
                    .contains(&("keep".into(), 5.0))
            );
        }
        drop(command_tx);
        assert!(owner.join().is_ok());
    }

    #[test]
    fn worker_budgets_bound_all_pools_and_reject_zero_capacity() {
        let available = |count| std::num::NonZeroUsize::new(count).unwrap();
        let limits = WorkerThreadBudget::default();
        assert_eq!(limits.worker_threads(available(32)).unwrap(), 6);
        assert_eq!(limits.worker_threads(available(4)).unwrap(), 2);
        assert_eq!(limits.worker_threads(available(1)).unwrap(), 1);
        let custom = ModelicaWorkerLimits {
            preparation: WorkerThreadBudget {
                max_worker_threads: 3,
                reserved_cpu_threads: 0,
            },
            live_execution: WorkerThreadBudget {
                max_worker_threads: 2,
                reserved_cpu_threads: 0,
            },
            document_execution: WorkerThreadBudget {
                max_worker_threads: 1,
                reserved_cpu_threads: 0,
            },
        };
        let pool = ModelicaWorkerPools::new(custom).unwrap();
        let count = pool.pool.worker_count();
        assert!((1..=3).contains(&count));
        assert!((1..=2).contains(&pool.live_pool.worker_count()));
        assert_eq!(pool.document_pool.worker_count(), 1);
        assert!(pool.can_admit(count + 1));
        assert!(!pool.can_admit(count + 2));

        for domain in 0..3 {
            let mut limits = ModelicaWorkerLimits::default();
            match domain {
                0 => limits.preparation.max_worker_threads = 0,
                1 => limits.live_execution.max_worker_threads = 0,
                2 => limits.document_execution.max_worker_threads = 0,
                _ => unreachable!(),
            }
            let (_command_tx, command_rx) = crossbeam_channel::unbounded();
            let (result_tx, result_rx) = crossbeam_channel::unbounded();
            modelica_worker(
                command_rx,
                ModelicaResponseSender::new(result_tx, crossbeam_channel::unbounded().0),
                ModelicaArtifactDiskLimits::default(),
                ModelicaCacheLimits::default(),
                limits,
            );
            let result = result_rx.try_recv().unwrap();
            assert!(
                result
                    .worker_failure
                    .as_deref()
                    .unwrap()
                    .contains("worker capacity must be positive")
            );
            assert!(result_rx.try_recv().is_err());
        }
    }
}

#[cfg(test)]
mod macro_step_tests {
    use super::*;

    /// Every communication transaction is distinct; setpoint commands coalesce.
    #[test]
    fn step_is_not_squashable() {
        let e = Entity::PLACEHOLDER;
        let step = |dt: f64| ModelicaCommand::Step {
            entity: e,
            session_id: 7,
            step_id: 1,
            start_time: 0.0,
            stop_time: dt,
            model_name: "M".into(),
            inputs: Vec::new(),
            dt,
        };
        assert!(
            !is_squashable(&step(0.016), &step(0.016)),
            "two Steps collapsing silently deletes simulated time"
        );

        let params = || ModelicaCommand::UpdateParameters {
            entity: e,
            session_id: 7,
            model_name: "M".into(),
            source: String::new(),
            parameter_name: "gain".into(),
            parameter_value: 1.0,
            initial_inputs: Vec::new(),
        };
        assert!(
            is_squashable(&params(), &params()),
            "UpdateParameters is an idempotent setpoint — it SHOULD squash"
        );
    }

    #[test]
    fn communication_schedule_is_derived_from_its_integer_endpoint() {
        let mut model = ModelicaModel {
            current_time: 3.25,
            communication_tick: 195,
            ..Default::default()
        };

        assert_eq!(
            model.validated_communication_period_secs().unwrap(),
            lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS
        );
        model.reset_communication_schedule().unwrap();
        assert_eq!(
            model.next_communication_time,
            lunco_core_runtime::SimTick(196).seconds()
        );
    }

    #[test]
    fn invalid_authored_communication_schedule_is_not_defaulted() {
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            0.0,
            LIVE_INTERNAL_STEP_SECS * 0.5,
            LIVE_INTERNAL_STEP_SECS,
        ] {
            let mut model = ModelicaModel {
                communication_period_secs: invalid,
                ..Default::default()
            };

            let error = model
                .validated_communication_period_secs()
                .expect_err("invalid authored schedule must be terminal");
            assert!(error.contains("invalid Modelica communication period"));
            assert!(model.reset_communication_schedule().is_err());
            assert_eq!(
                model.next_communication_time,
                lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS
            );
        }
    }

    #[test]
    fn communication_schedule_must_be_representable_and_bounded() {
        assert!(
            lunco_modelica_runtime::validate_communication_period_secs(
                lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS
            )
            .is_ok()
        );
        assert!(
            lunco_modelica_runtime::validate_communication_period_secs(
                lunco_core_runtime::SECS_PER_TICK
            )
            .is_ok()
        );
        assert!(
            lunco_modelica_runtime::validate_communication_period_secs(
                3.0 * lunco_core_runtime::SECS_PER_TICK
            )
            .is_ok()
        );
        assert!(lunco_modelica_runtime::validate_communication_period_secs(0.13).is_err());
        assert!(
            lunco_modelica_runtime::validate_communication_period_secs(
                MAX_MACRO_STEP_DT + LIVE_INTERNAL_STEP_SECS
            )
            .is_err()
        );
    }

    #[test]
    fn panic_results_preserve_command_lifecycle_identity() {
        let entity = Entity::from_raw_u32(7).expect("valid test entity");
        let compile = panic_result_for_command(
            &ModelicaCommand::Compile {
                execution_domain: lunco_modelica_runtime::ModelicaExecutionDomain::Scene,
                entity,
                session_id: 4,
                model_name: "Balloon".into(),
                source: String::new(),
                realtime_safe: false,
                doc_uri: "balloon.mo".into(),
                extra_sources: Vec::new(),
                parameter_overrides: Vec::new(),
                authored_bindings: Default::default(),
                expected_library_revision: None,
                initial_inputs: Vec::new(),
                initial_state_values: None,
            },
            "panic",
        );
        let lunco_modelica_runtime::ModelicaWorkerResponse::Participant(compile) = compile else {
            panic!("actor compile must retain participant identity");
        };
        assert_eq!(compile.entity, entity);
        assert_eq!(compile.session_id, 4);
        assert!(compile.is_new_model);
        assert!(!compile.is_parameter_update);

        let step = panic_result_for_command(
            &ModelicaCommand::Step {
                entity,
                session_id: 4,
                step_id: 19,
                start_time: 0.0,
                stop_time: 0.1,
                model_name: "Balloon".into(),
                inputs: Vec::new(),
                dt: 0.1,
            },
            "panic",
        );
        let lunco_modelica_runtime::ModelicaWorkerResponse::Participant(step) = step else {
            panic!("actor step must retain participant identity");
        };
        assert_eq!(step.step_id, Some(19));
        assert!(!step.is_new_model);
        assert!(step.error.is_some());

        let root = panic_result_for_command(
            &ModelicaCommand::LoadSourceRoot {
                id: "Modelica".into(),
                operation_id: 1,
                payload: LoadSourceRootPayload::InMemory {
                    label: "test".into(),
                    files: Vec::new(),
                },
            },
            "panic",
        );
        let lunco_modelica_runtime::ModelicaWorkerResponse::Participant(root) = root else {
            panic!("source-root reply must reach the source-root consumer");
        };
        assert_eq!(root.loaded_source_root_id.as_deref(), Some("Modelica"));

        let unload = panic_result_for_command(
            &ModelicaCommand::UnloadSourceRoot {
                id: "twin:17:scenario:modelica:0".into(),
                operation_id: 2,
            },
            "panic",
        );
        let lunco_modelica_runtime::ModelicaWorkerResponse::Participant(unload) = unload else {
            panic!("source-root reply must reach the source-root consumer");
        };
        assert_eq!(
            unload.unloaded_source_root_id.as_deref(),
            Some("twin:17:scenario:modelica:0")
        );
        assert_eq!(unload.source_root_operation_id, Some(2));
        assert!(unload.error.is_some());

        let request_id = lunco_modelica_runtime::ModelicaPreparationId {
            scene_generation: 8,
            sequence: 3,
        };
        let response = panic_result_for_command(
            &ModelicaCommand::Initialize {
                request_id,
                model_name: "InitialProbe".into(),
                source: String::new(),
                doc_uri: "initial-probe.mo".into(),
                extra_sources: Vec::new(),
                parameter_overrides: Vec::new(),
                authored_bindings: Default::default(),
                expected_library_revision: None,
                initial_inputs: Vec::new(),
                probes: Vec::new(),
            },
            "initialization failed",
        );
        let lunco_modelica_runtime::ModelicaWorkerResponse::Preparation(response) = response else {
            panic!("initialization failure must not publish an actor result");
        };
        assert_eq!(response.request_id, request_id);
        assert!(response.outcome.is_err());
        let response = failed_result_for_command(
            &ModelicaCommand::PrepareInterface {
                request_id,
                entity,
                session_id: 7,
                model_name: "InterfaceProbe".into(),
                source: String::new(),
                doc_uri: "interface-probe.mo".into(),
                extra_sources: Vec::new(),
                parameter_overrides: Vec::new(),
                authored_bindings: Default::default(),
            },
            "interface failed",
        );
        let ModelicaWorkerResponse::Preparation(response) = response else {
            panic!("interface failure must reach the preparation owner");
        };
        assert_eq!(response.request_id, request_id);
        assert!(response.outcome.is_err());
    }

    #[test]
    fn source_root_operation_fence_accepts_only_newer_operations() {
        let mut latest = HashMap::new();
        assert!(record_source_root_operation(
            &mut latest,
            "twin:17:scenario",
            4
        ));
        assert!(!record_source_root_operation(
            &mut latest,
            "twin:17:scenario",
            4
        ));
        assert!(!record_source_root_operation(
            &mut latest,
            "twin:17:scenario",
            3
        ));
        assert!(record_source_root_operation(
            &mut latest,
            "twin:17:scenario",
            5
        ));
        assert!(record_source_root_operation(
            &mut latest,
            "application:controls",
            1
        ));
        assert_eq!(latest.get("twin:17:scenario"), Some(&5));
        assert!(!retire_source_root_operation(
            &mut latest,
            "twin:17:scenario",
            4
        ));
        assert_eq!(latest.get("twin:17:scenario"), Some(&5));
        assert!(retire_source_root_operation(
            &mut latest,
            "twin:17:scenario",
            5
        ));
        assert!(!latest.contains_key("twin:17:scenario"));
        assert!(latest.contains_key("application:controls"));
    }

    #[test]
    fn resolves_only_omitted_period_to_the_documented_default() {
        assert_eq!(
            lunco_modelica_runtime::resolve_communication_period_secs(false, None).unwrap(),
            lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS
        );
        assert!(lunco_modelica_runtime::resolve_communication_period_secs(true, None).is_err());
        assert!(
            lunco_modelica_runtime::resolve_communication_period_secs(true, Some(0.13)).is_err()
        );
    }

    #[test]
    fn communication_transaction_requires_matching_solver_endpoint() {
        assert!(validate_step_request(0.1, 0.1, 0.2, 0.1).is_ok());
        assert!(validate_step_request(0.1001, 0.1, 0.2, 0.1).is_err());
        assert!(validate_step_request(0.1, 0.1, 0.2, 0.2).is_err());
        assert!(validate_step_completion(0.2, 0.2).is_ok());
        assert!(validate_step_completion(0.199, 0.2).is_err());
    }

    #[test]
    fn communication_transaction_preserves_large_time_addition() {
        let start = 1e9;
        let dt = lunco_core_runtime::SECS_PER_TICK;
        let stop = start + dt;
        assert!(validate_step_request(start, start, stop, dt).is_ok());
        assert!(validate_step_completion(stop, stop).is_ok());
        assert!(validate_step_completion(stop.next_up(), stop).is_ok());
    }

    #[test]
    fn communication_transaction_rejects_non_finite_and_stale_clocks() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(validate_step_request(invalid, 0.0, 0.1, 0.1).is_err());
            assert!(validate_step_completion(invalid, 0.1).is_err());
            assert!(validate_step_completion(0.1, invalid).is_err());
        }
        let start = 1e9;
        let dt = lunco_core_runtime::SECS_PER_TICK;
        let stop = start + dt;
        assert!(validate_step_request(start - 1.0, start, stop, dt).is_err());
        assert!(validate_step_completion(stop - 1.0, stop).is_err());
        assert!(validate_step_completion(stop - dt, stop).is_err());
        assert!(validate_step_request(start, start, stop + 1.0, dt).is_err());
        assert!(validate_step_request(0.0, 0.0, 0.2, 0.2).is_err());
        let unresolved_start: f64 = 1e20;
        assert!(
            validate_step_request(
                unresolved_start,
                unresolved_start,
                unresolved_start.next_up(),
                dt
            )
            .is_err()
        );
    }
}

// ===========================================================================
// ===========================================================================
// M3 — cached-artifact invalidation key
// ===========================================================================
#[cfg(test)]
mod artifact_cache_tests {
    use super::*;

    fn hash_of(model: &str, uri: &str, source: &str, extras: Vec<(String, String)>) -> u64 {
        let unit = assemble_compile_unit(source, extras);
        compile_unit_hash(model, uri, &unit)
    }

    fn shared_hash_of(model: &str, source: &str, uri: &str, extras: Vec<(String, String)>) -> u64 {
        let unit = assemble_compile_unit(source, extras);
        shared_compile_hash(model, &unit, uri, 4, &[])
    }

    fn prepared_hash_of(
        model: &str,
        source: &str,
        uri: &str,
        extras: Vec<(String, String)>,
    ) -> u64 {
        let unit = assemble_compile_unit(source, extras);
        let mut compiler = ModelicaCompiler::new();
        let compiled = compiler
            .compile_str_multi(model, &unit.source, uri, &unit.extras)
            .expect("inline strict compilation succeeds");
        prepared_source_key(
            compiler
                .compiled_source_content_identity(&compiled)
                .unwrap(),
        )
    }

    /// The hash keys the whole assembled CompileUnit: primary source, extras,
    /// model name, and session URI each independently invalidate.
    #[test]
    fn unit_hash_covers_the_whole_source_set() {
        let base = hash_of("M", "doc.mo", "model M end M;", Vec::new());
        assert_eq!(
            base,
            hash_of("M", "doc.mo", "model M end M;", Vec::new()),
            "hash must be deterministic"
        );
        assert_ne!(
            base,
            hash_of("M", "doc.mo", "model M Real x; end M;", Vec::new())
        );
        assert_ne!(base, hash_of("M2", "doc.mo", "model M end M;", Vec::new()));
        assert_ne!(base, hash_of("M", "other.mo", "model M end M;", Vec::new()));
        assert_ne!(
            base,
            hash_of(
                "M",
                "doc.mo",
                "model M end M;",
                vec![("sib.mo".into(), "package P end P;".into())]
            )
        );
    }

    /// Artifact reuse requires BOTH an unchanged unit and no LoadSourceRoot
    /// since the compile (the library generation).
    #[test]
    fn artifact_validity_requires_hash_and_generation() {
        assert!(artifact_still_valid(7, 3, 7, 3));
        assert!(!artifact_still_valid(7, 3, 8, 3), "source set changed");
        assert!(
            !artifact_still_valid(7, 3, 7, 4),
            "a source root loaded since"
        );
    }

    /// Compiled admission preserves its exact target, source, and document identity.
    #[test]
    fn compiled_key_matches_exact_source_unit() {
        assert_eq!(
            shared_hash_of("M", "model M end M;", "doc.mo", Vec::new()),
            shared_hash_of("M", "model M end M;", "doc.mo", Vec::new())
        );
        assert_ne!(
            shared_hash_of("M", "model M end M;", "doc.mo", Vec::new()),
            shared_hash_of("M2", "model M end M;", "doc.mo", Vec::new())
        );
        assert_ne!(
            shared_hash_of("M", "model M end M;", "doc.mo", Vec::new()),
            shared_hash_of("M", "model M Real x; end M;", "doc.mo", Vec::new())
        );
    }

    #[test]
    fn exact_generated_source_identity_preserves_declared_output_names() {
        let first = shared_hash_of(
            "First__1_System",
            "model First__1_System output Real First__1_System_signal=1; end First__1_System;",
            "generated/First__1_System.mo",
            Vec::new(),
        );
        let second = shared_hash_of(
            "Second__2_System",
            "model Second__2_System output Real Second__2_System_signal=1; end Second__2_System;",
            "generated/Second__2_System.mo",
            Vec::new(),
        );
        assert_ne!(
            first, second,
            "distinct qualified outputs require distinct compiled artifacts"
        );
    }

    #[test]
    fn prepared_closure_key_excludes_unrelated_siblings_and_retains_dependencies_and_starts() {
        let source = "model Probe Dependency part; end Probe;";
        let dependency = "model Dependency parameter Real gain=1; Real x(start=1); equation der(x)=-gain*x; end Dependency;";
        let extras = |irrelevant: &str, relevant: &str| {
            vec![
                ("dependency.mo".into(), relevant.into()),
                ("unrelated.mo".into(), irrelevant.into()),
            ]
        };
        let first = prepared_hash_of(
            "Probe",
            source,
            "a.mo",
            extras(
                "model Ignored Real y; equation y=1; end Ignored;",
                dependency,
            ),
        );
        let second = prepared_hash_of(
            "Probe",
            source,
            "b.mo",
            extras(
                "model RenamedIgnored Real y; equation y=2; end RenamedIgnored;",
                dependency,
            ),
        );
        assert_eq!(
            first, second,
            "unrelated sibling edits and document IDs do not change solver identity"
        );
        for changed in [
            dependency.replace("start=1", "start=2"),
            dependency.replace("gain=1", "gain=2"),
        ] {
            assert_ne!(
                first,
                prepared_hash_of(
                    "Probe",
                    source,
                    "a.mo",
                    extras("model Ignored end Ignored;", &changed)
                ),
                "participating starts and parameters are part of solver identity"
            );
        }
    }

    #[test]
    fn prepared_closure_key_preserves_exact_generated_target_and_source() {
        let source = "model Assembly__101_System parameter Real gain=101; Real x(start=1); equation der(x)=-gain*x; end Assembly__101_System;";
        let first = prepared_hash_of(
            "Assembly__101_System",
            source,
            "generated/Assembly__101_System.mo",
            Vec::new(),
        );
        let renamed = source.replace("Assembly__101_System", "Assembly__202_System");
        assert_ne!(
            first,
            prepared_hash_of(
                "Assembly__202_System",
                &renamed,
                "generated/Assembly__202_System.mo",
                Vec::new()
            )
        );
        assert_ne!(
            first,
            prepared_hash_of(
                "Assembly__101_System",
                &source.replace("gain=101", "gain=202"),
                "generated/Assembly__101_System.mo",
                Vec::new()
            )
        );
        assert_eq!(
            first,
            prepared_hash_of(
                "Assembly__101_System",
                source,
                "another-document.mo",
                Vec::new()
            )
        );
    }

    #[test]
    fn precompile_rejection_cannot_reuse_prior_compiler_diagnostics() {
        let mut compiler = ModelicaCompiler::new();
        let mut artifacts = CompiledArtifactCache::new(std::num::NonZeroUsize::new(2).unwrap());
        let mut unit = assemble_compile_unit(
            "model Broken Real x; equation x=missing; end Broken;",
            Vec::new(),
        );
        assert!(
            compile_shared(
                &mut artifacts,
                &mut compiler,
                "Broken",
                &unit,
                "broken.mo",
                0
            )
            .is_err()
        );
        unit.expected_library_revision = Some(compiler.library_revision().wrapping_add(1));
        let error = compile_shared(
            &mut artifacts,
            &mut compiler,
            "Broken",
            &unit,
            "broken.mo",
            0,
        )
        .err()
        .unwrap();
        assert!(error.contains("source library changed"));
        assert!(
            compiler
                .take_compile_diagnostics("Broken", "broken.mo")
                .is_empty()
        );
    }

    #[test]
    fn structural_bindings_compile_before_tunable_lowering_and_keep_cache_variants() {
        let source = "model Probe parameter Integer n; parameter Real capacity=8 annotation(Evaluate=true); parameter Real gain=2; Real x[n](each start=0, each fixed=true); equation der(x)=fill(capacity*gain,n); end Probe;";
        let mut unit = assemble_compile_unit(source, Vec::new());
        unit.parameter_overrides = canonical_parameter_overrides(&[
            ("n".into(), 3.0),
            ("capacity".into(), 20.0),
            ("gain".into(), 3.0),
        ]);
        let mut compiler = ModelicaCompiler::new();
        let mut artifacts = CompiledArtifactCache::new(std::num::NonZeroUsize::new(8).unwrap());
        let first =
            compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "probe.mo", 0).unwrap();
        let count = |artifact: &CompiledModelArtifact| {
            artifact.compiled.dae.inspect(|view| {
                view.variables()
                    .find(|(_, variable)| variable.name().as_str() == "x")
                    .unwrap()
                    .1
                    .scalar_count()
            })
        };
        assert_eq!(count(&first), 3);
        let plan = live_build_plan(
            solver::RuntimeProfile {
                live: true,
                predicted: false,
            },
            &unit.parameter_overrides,
            &[],
            first.prepared_source_key,
            &first.structural_overrides,
            Some(0),
        )
        .unwrap();
        assert_eq!(plan.options.param_overrides, vec![("gain".into(), 3.0)]);
        compiler.clear_user_documents();
        let original = unit.parameter_overrides.clone();
        unit.parameter_overrides
            .iter_mut()
            .find(|(name, _)| name == "gain")
            .unwrap()
            .1 = 4.0;
        let tuned =
            compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "probe.mo", 0).unwrap();
        assert!(Arc::ptr_eq(&first.compiled.dae, &tuned.compiled.dae));
        unit.parameter_overrides.retain(|(name, _)| name != "gain");
        let authored =
            compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "probe.mo", 0).unwrap();
        assert!(
            !Arc::ptr_eq(&first.compiled.dae, &authored.compiled.dae),
            "omitting a bound name restores its authored binding graph"
        );
        unit.parameter_overrides = canonical_parameter_overrides(&[
            ("n".into(), 4.0),
            ("capacity".into(), 40.0),
            ("gain".into(), 3.0),
        ]);
        let second =
            compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "probe.mo", 0).unwrap();
        assert_eq!(count(&second), 4);
        assert!(!Arc::ptr_eq(&first.compiled.dae, &second.compiled.dae));
        unit.parameter_overrides = original;
        let reused =
            compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "probe.mo", 0).unwrap();
        assert!(Arc::ptr_eq(&first.compiled.dae, &reused.compiled.dae));
        for invalid in [1.5, f64::NAN] {
            unit.parameter_overrides = vec![("n".into(), invalid)];
            assert!(
                compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "probe.mo", 0)
                    .is_err()
            );
        }
    }

    #[test]
    fn reused_nested_parameter_initialization_uses_current_instance() {
        let source = r#"
model Navigation
 parameter Real initial_y=0;
 output Real y;
initial equation
 y=initial_y;
equation
 der(y)=0;
end Navigation;
model Probe
 parameter Real initial_y=60;
 Navigation navigation(initial_y=initial_y);
end Probe;
"#;
        let mut compiler = ModelicaCompiler::new();
        let mut artifacts = CompiledArtifactCache::new(std::num::NonZeroUsize::new(4).unwrap());
        let mut first_dae = None;
        for expected in [60.0, 1.0, 2.0, -4.0, 60.0] {
            let mut unit = assemble_compile_unit(source, Vec::new());
            unit.parameter_overrides = vec![("initial_y".into(), expected)];
            let artifact = compile_shared(
                &mut artifacts,
                &mut compiler,
                "Probe",
                &unit,
                "nested-init.mo",
                0,
            )
            .unwrap();
            if let Some(first) = &first_dae {
                assert!(
                    Arc::ptr_eq(first, &artifact.compiled.dae),
                    "tunable values must reuse the same compiled model"
                );
            } else {
                first_dae = Some(Arc::clone(&artifact.compiled.dae));
            }
            let plan = live_build_plan(
                solver::RuntimeProfile {
                    live: true,
                    predicted: false,
                },
                &unit.parameter_overrides,
                &[],
                artifact.prepared_source_key,
                &artifact.structural_overrides,
                None,
            )
            .unwrap();
            let stepper = lunco_modelica_solver::simulation_session::live(
                &artifact.compiled.dae,
                plan.options.clone(),
            )
            .unwrap();
            let values = stepper.selected_values(&["navigation.y".into()]).unwrap();
            assert!(
                (values[0].unwrap() - expected).abs() < 1e-8,
                "expected {expected}, actual {}",
                values[0].unwrap()
            );
        }
    }

    #[test]
    fn coupled_array_modifiers_initialize_with_qualified_indexed_parameter_overrides() {
        let source = r#"
model Element
  input Real u[2]={1,2};
  parameter Real p[2]={1,2};
  output Real sample=0.5*u[1]+0.1*u[2]+p[2];
end Element;
model CoupledArrays
  Element p0(u = {p1.sample, 7.0});
  Element p1(u = {p0.sample, 9.0});
  output Real first=p0.sample;
  output Real second=p1.sample;
  output Real first_parameter=p0.p[2];
  output Real second_parameter=p1.p[2];
  output Real untouched_parameter=p0.p[1];
end CoupledArrays;
"#;
        let mut unit = assemble_compile_unit(source, Vec::new());
        unit.parameter_overrides =
            canonical_parameter_overrides(&[("p0.p[2]".into(), 3.0), ("p1.p[2]".into(), 5.0)]);
        let mut compiler = ModelicaCompiler::new();
        let mut artifacts = CompiledArtifactCache::new(std::num::NonZeroUsize::new(2).unwrap());
        let revision = compiler.library_revision();
        let artifact = compile_shared(
            &mut artifacts,
            &mut compiler,
            "CoupledArrays",
            &unit,
            "coupled-arrays.mo",
            revision,
        )
        .expect("whole-array feedback modifiers and indexed parameter overlays compile together");
        let plan = live_build_plan(
            solver::RuntimeProfile {
                live: true,
                predicted: false,
            },
            &unit.parameter_overrides,
            &[],
            artifact.prepared_source_key,
            &artifact.structural_overrides,
            Some(revision),
        )
        .expect("checked indexed parameters enter the shared preparation plan");
        assert_eq!(plan.options.param_overrides, unit.parameter_overrides);
        let names = [
            "first",
            "second",
            "first_parameter",
            "second_parameter",
            "untouched_parameter",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let stepper = lunco_modelica_solver::simulation_session::live(
            &artifact.compiled.dae,
            plan.options.clone(),
        )
        .expect("qualified indexed values are applied during session construction");
        let values = stepper
            .selected_values(&names)
            .expect("coupled array feedback initializes through the public session");
        assert_eq!(values.len(), names.len());
        for (name, (actual, expected)) in
            names
                .iter()
                .zip(values.iter().zip([133.0 / 15.0, 31.0 / 3.0, 3.0, 5.0, 1.0]))
        {
            let actual = actual.expect("declared output is present");
            assert!(
                (actual - expected).abs() < 1e-8,
                "{name}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn prepared_closure_key_remains_with_shared_artifact_after_overlay_release() {
        let mut unit = assemble_compile_unit(
            "model Probe parameter Integer count=2; final parameter Real fixedGain=4; protected parameter Real hidden=7; public Real x(start=1); equation der(x)=-x; end Probe;",
            Vec::new(),
        );
        let mut compiler = ModelicaCompiler::new();
        let mut artifacts = CompiledArtifactCache::new(std::num::NonZeroUsize::new(2).unwrap());
        unit.project_edit_interface = false;
        let numerical =
            compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "first.mo", 0)
                .expect("numerical compile admits the checked DAE");
        assert!(numerical.edit_interface.is_none());
        compiler.clear_user_documents();

        unit.project_edit_interface = true;
        let first = compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "first.mo", 0)
            .expect("live compile admits the edit interface on the retained artifact");
        assert!(Arc::ptr_eq(&numerical.compiled.dae, &first.compiled.dae));
        compiler.clear_user_documents();
        let reused = compile_shared(&mut artifacts, &mut compiler, "Probe", &unit, "first.mo", 0)
            .expect("shared artifact retains captured identity without seated overlays");
        assert_eq!(first.prepared_source_key, reused.prepared_source_key);
        assert!(std::sync::Arc::ptr_eq(
            &first.compiled.dae,
            &reused.compiled.dae
        ));
        assert!(Arc::ptr_eq(
            first.edit_interface.as_ref().unwrap(),
            reused.edit_interface.as_ref().unwrap()
        ));
        assert_eq!(artifacts.len(), 1);
        for artifact in [&first, &reused] {
            let interface = artifact
                .edit_interface
                .as_ref()
                .expect("checked source authority is attached after every cache lookup");
            assert_eq!(
                interface.declarations["count"]
                    .parameter_edit_source(3.0)
                    .unwrap(),
                "3"
            );
            assert!(
                interface.declarations["count"]
                    .parameter_edit_source(1.5)
                    .unwrap_err()
                    .contains("Integer")
            );
            for name in ["fixedGain", "hidden"] {
                assert!(
                    interface.declarations[name]
                        .parameter_edit_source(9.0)
                        .unwrap_err()
                        .contains("public, non-final")
                );
            }
        }
    }

    #[test]
    fn parameter_override_cache_keys_are_order_independent() {
        let first = canonical_parameter_overrides(&[("zeta".into(), 2.0), ("alpha".into(), 1.0)]);
        let second = canonical_parameter_overrides(&[("alpha".into(), 1.0), ("zeta".into(), 2.0)]);
        assert_eq!(first, second);
        assert_eq!(first[0].0, "alpha");
        assert_eq!(first[1].0, "zeta");
    }
}

#[cfg(test)]
mod live_solver_snapshot_tests {
    use super::*;

    #[test]
    fn reset_inputs_preserve_the_admitted_solver_and_source_plan() {
        let original = live_build_plan(
            solver::RuntimeProfile {
                live: true,
                predicted: false,
            },
            &[("gain".into(), 6.0)],
            &[("u".into(), 1.0)],
            17,
            &[],
            Some(9),
        )
        .unwrap();
        let reset = original.with_initial_inputs(&[("u".into(), 8.0)]);
        assert_eq!(reset.snapshot.admitted_library_revision, Some(9));
        assert_eq!(reset.snapshot.solver_id, original.snapshot.solver_id);
        assert_eq!(reset.snapshot.parameters, original.snapshot.parameters);
        assert_eq!(
            reset.snapshot.parameter_overrides,
            original.snapshot.parameter_overrides
        );
        assert_eq!(
            reset.options.param_overrides,
            original.options.param_overrides
        );
        assert_eq!(reset.options.initial_inputs, vec![("u".into(), 8.0)]);
        assert_eq!(original.options.initial_inputs, vec![("u".into(), 1.0)]);
        assert_eq!(reset.snapshot.initial_inputs, reset.options.initial_inputs);
    }

    #[test]
    fn snapshot_matches_the_resolved_live_plan_and_ordered_overrides() {
        let profile = solver::RuntimeProfile {
            live: true,
            predicted: true,
        };
        let plan = live_build_plan(
            profile,
            &[("zeta".to_owned(), 2.0), ("alpha".to_owned(), 1.0)],
            &[],
            17,
            &[],
            Some(9),
        )
        .expect("built-in solvers include a deterministic fixed-step live backend");

        let expected_solver_id = live_stepper_options(profile).unwrap().0.id.to_string();
        assert_eq!(plan.snapshot.solver_id, expected_solver_id);
        assert_eq!(plan.snapshot.profile.live, profile.live);
        assert_eq!(plan.snapshot.profile.predicted, profile.predicted);
        assert!(plan.snapshot.capabilities.usable_live);
        assert!(plan.snapshot.capabilities.fixed_step);
        assert!(plan.snapshot.capabilities.deterministic);
        assert_eq!(plan.snapshot.parameters.atol, LIVE_TOL);
        assert_eq!(plan.snapshot.parameters.rtol, LIVE_TOL);
        assert_eq!(plan.snapshot.parameters.h0, Some(LIVE_INTERNAL_STEP_SECS));
        assert_eq!(plan.snapshot.parameters.t_start, 0.0);
        assert_eq!(plan.snapshot.parameters.t_end, LIVE_INTERNAL_STEP_SECS);
        assert_eq!(
            plan.snapshot.parameter_overrides,
            vec![("alpha".to_owned(), 1.0), ("zeta".to_owned(), 2.0)]
        );
        assert_eq!(
            plan.options.param_overrides,
            plan.snapshot.parameter_overrides
        );
    }
}

// ===========================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod checked_step_tests {
    use super::*;

    #[test]
    fn adaptive_communication_interval_lands_on_its_requested_stop() {
        let source = "model ClockProbe Real x(start=0); equation der(x)=1; end ClockProbe;";
        let compiled = ModelicaCompiler::new()
            .compile_str("ClockProbe", source, "ClockProbe.mo")
            .unwrap();
        let plan = live_build_plan(
            solver::RuntimeProfile {
                live: true,
                predicted: false,
            },
            &[],
            &[],
            0,
            &[],
            Some(0),
        )
        .unwrap();
        let mut stepper =
            lunco_modelica_solver::simulation_session::live(&compiled.dae, plan.options.clone())
                .unwrap();
        for stop_time in [0.007, 0.014, 0.025] {
            let values = advance_checked_step(&mut stepper, &[], stop_time)
                .expect("an adaptive session must honor the requested communication point");
            assert!((stepper.time() - stop_time).abs() < 1e-12);
            assert!(
                (values
                    .iter()
                    .find(|(name, _)| name.as_str() == "x")
                    .unwrap()
                    .1
                    - stop_time)
                    .abs()
                    < 1e-12
            );
        }
    }

    #[test]
    fn rejected_worker_inputs_return_diagnostics_without_advancing() {
        let source = "model Probe input Real admitted=1; output Real observed; equation observed=admitted; end Probe;";
        let compiled = ModelicaCompiler::new()
            .compile_str("Probe", source, "Probe.mo")
            .unwrap();
        for (name, value) in [("unadmitted", 99.0), ("admitted", f64::NAN)] {
            let plan = live_build_plan(
                solver::RuntimeProfile {
                    live: true,
                    predicted: false,
                },
                &[],
                &[("admitted".into(), 1.0)],
                0,
                &[],
                Some(0),
            )
            .unwrap();
            let mut stepper = lunco_modelica_solver::simulation_session::live(
                &compiled.dae,
                plan.options.clone(),
            )
            .unwrap();
            let error = advance_checked_step(
                &mut stepper,
                &[(name.to_owned(), value)],
                LIVE_INTERNAL_STEP_SECS,
            )
            .unwrap_err();
            assert!(
                error.to_string().to_ascii_lowercase().contains("input"),
                "{error}"
            );
            assert_eq!(stepper.time(), 0.0);
        }
    }
}
