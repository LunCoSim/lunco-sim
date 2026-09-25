//! Off-thread Modelica simulation worker + Bevy bridge.
//!
//! `modelica_worker` runs on its own OS thread (it owns a
//! `!Send` `SimulationSession`, so it can't live on the Bevy main loop). The
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
use lunco_modelica_ast::ast_extract::{InputDefaultIssue, strip_input_defaults_with_report};
use lunco_modelica_compiler::{ModelicaCompiler, PreparedSourceRoot};
use lunco_modelica_runtime::{
    CompileRequested, InFlightModelicaStep, LoadSourceRootPayload, MAX_MACRO_STEP_DT,
    ModelicaChannels, ModelicaCommand, ModelicaModel, ModelicaNotice, ModelicaResult, NoticeLevel,
    SimSampleBatch, SimSampleStream,
};
use lunco_modelica_solver::simulation_session::LiveStepper;
use lunco_signal::{SimSnapshot, SimStream};

#[cfg(not(target_arch = "wasm32"))]
const PREPARED_SOLVE_CACHE_VERSION: u32 = 4;

mod cache;
use cache::{PreparedSolveCache, PreparedSolveKey};
mod bridge;
mod compile;
pub use bridge::{
    handle_modelica_responses, on_remove_modelica, reconcile_modelica_preparation_progress,
    request_modelica_compiles, spawn_modelica_requests,
};
pub use compile::dispatch_modelica_compile_requests;
#[cfg(not(target_arch = "wasm32"))]
mod compiler_actor;
#[cfg(not(target_arch = "wasm32"))]
mod scheduling;
#[cfg(not(target_arch = "wasm32"))]
use compiler_actor::{CompilerActor, CompilerCompletion};
#[cfg(not(target_arch = "wasm32"))]
use scheduling::{
    enqueue_command, pending_preparation_entities, pop_ready_in_order, promote_unblocked_steps,
    take_runnable_compile_command, take_runnable_steps,
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

/// Solver options for the **LIVE** (co-simulated) path.
///
/// WHAT IS LIVE-SPECIFIC, and it is only this: a **fixed macro/micro step
/// ladder** ([`LIVE_MICRO_DT`] via [`micro_steps_for`], so the stop-time sequence
/// is an integer function of `dt`), `h0` pinned to that micro-step, and an
/// explicit fixed tolerance ([`LIVE_TOL`]) rather than the model's
/// `experiment(Tolerance=…)`, which is an offline accuracy knob and must not
/// reach the realtime loop.
///
/// WHAT IS NOT LIVE-SPECIFIC: **which solver**. Hardcoding a family here would
/// bypass the resolver the batch path uses, and the two would disagree silently.
/// See `lunco_experiments::solver`.
///
/// So the family comes from [`solver::resolve`], the same call the batch path
/// makes, from where the model runs: stepped inside the frame loop, and whether
/// it drives a client-predicted body. A live model resolves to a backend that
/// declares `usable_live`; a predicted one is admitted only by a backend that
/// is both fixed-step and deterministic.
fn live_stepper_options(
    profile: solver::RuntimeProfile,
) -> Result<(solver::SolverSpec, rumoca_sim::SimOptions), solver::SolverError> {
    lunco_modelica_solver::solver_backends::ensure_builtin_solvers();

    let spec = solver::resolve(&solver::SolverRequest {
        profile,
        // The live path takes no authored override: an `experiment(...)`
        // annotation is an offline knob, and the same reasoning that keeps its
        // tolerance out of this loop keeps its solver out.
        authored: None,
    })?;

    let options = lunco_modelica_solver::solver_backends::rumoca_options(
        &spec,
        &solver::SolverParams {
            atol: LIVE_TOL,
            rtol: LIVE_TOL,
            // `h0` is the initial/maximum internal step: pinned to the micro-step
            // so the integrator's first internal step matches what it is asked for.
            h0: Some(LIVE_MICRO_DT),
            // The live stepper is driven by `step(dt)` calls, never by `t_end`;
            // the window only feeds defaults, so it is wide enough that no
            // realistic session reaches it.
            t_start: 0.0,
            t_end: f64::from(u32::MAX),
        },
    )?;
    Ok((spec, options))
}

/// Build a `SimulationSession` for the LIVE path from a freshly-compiled model.
///
/// **Single source of truth** for live stepper construction across the worker —
/// every site routes through here instead of copy-pasting the `SimOptions` setup
/// + `SimulationSession::new` call (there were ~9 such copies).
///
/// Stepping POLICY (fixed micro-step ladder, fixed tolerance) is live-specific;
/// the SOLVER is resolved from the model's own requirements — see
/// [`live_stepper_options`]. The model's `experiment(Tolerance=…)` annotation is
/// deliberately ignored here: it is an offline-accuracy knob and must not reach
/// into the realtime coupling loop.
///
/// A solver that cannot serve the model is an error HERE, at construction, not a
/// per-step `WARN` on a model that has already been accepted and is silently
/// producing nothing. That silence is what let broken islands ship.
/// The realtime half of the solver request for one model.
///
/// A model absent from the set is NOT predicted. The live path still requires a
/// frame-loop-usable backend; offline/batch paths are where adaptive implicit
/// solvers are selected.
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

struct LiveBuildPlan {
    spec: solver::SolverSpec,
    options: rumoca_sim::SimOptions,
    key: PreparedSolveKey,
    #[cfg(not(target_arch = "wasm32"))]
    source_key: u64,
    #[cfg(not(target_arch = "wasm32"))]
    persistent_library_revision: Option<u64>,
    #[cfg(not(target_arch = "wasm32"))]
    override_key: Vec<(String, u64)>,
}

fn live_build_plan(
    profile: solver::RuntimeProfile,
    parameter_overrides: &[(String, f64)],
    source_key: u64,
    library_revision: Option<u64>,
    _prepared: &PreparedSolveCache,
) -> Result<LiveBuildPlan, rumoca_sim::SimulationDiagnosticError> {
    let parameter_overrides = canonical_parameter_overrides(parameter_overrides);
    let (spec, mut options) = live_stepper_options(profile).map_err(|e| {
        rumoca_sim::SimulationDiagnosticError::Solver(format!("solver selection failed: {e}"))
    })?;
    // Parameter overrides must enter Rumoca's lowering boundary. That is where
    // parameter dependents and initial-equation states are recomputed. Mutating
    // the DAE after compilation leaves the initialization vector stale.
    options.param_overrides = parameter_overrides.clone();
    #[cfg(not(target_arch = "wasm32"))]
    let override_key = parameter_overrides
        .iter()
        .map(|(name, value)| (name.clone(), value.to_bits()))
        .collect();
    let library_revision_value = library_revision.unwrap_or_default();
    let key = PreparedSolveCache::key(
        source_key,
        library_revision_value,
        &spec,
        &parameter_overrides,
    );
    Ok(LiveBuildPlan {
        spec,
        options,
        key,
        #[cfg(not(target_arch = "wasm32"))]
        source_key,
        #[cfg(not(target_arch = "wasm32"))]
        persistent_library_revision: _prepared.persistent_library_revision(library_revision),
        #[cfg(not(target_arch = "wasm32"))]
        override_key,
    })
}

fn build_stepper(
    _comp_res: &rumoca_compile::compile::DaeCompilationResult,
    profile: solver::RuntimeProfile,
    parameter_overrides: &[(String, f64)],
    source_key: u64,
    library_revision: Option<u64>,
    prepared: &mut PreparedSolveCache,
) -> Result<LiveStepper, rumoca_sim::SimulationDiagnosticError> {
    let plan = live_build_plan(
        profile,
        parameter_overrides,
        source_key,
        library_revision,
        prepared,
    )?;
    if !prepared.models.contains_key(&plan.key) {
        #[cfg(not(target_arch = "wasm32"))]
        return Err(rumoca_sim::SimulationDiagnosticError::Solver(
            "native solver model reached stepper construction before asynchronous preparation committed"
                .to_owned(),
        ));

        #[cfg(target_arch = "wasm32")]
        {
            let lower_started = web_time::Instant::now();
            let model = lunco_modelica_solver::simulation_session::lower_for_live(
                &_comp_res.dae,
                &plan.options,
            )?;
            bevy::log::info!(
                "[modelica-runtime] prepared solver IR for `{}`: lower={:?} cache=miss",
                plan.spec.id,
                lower_started.elapsed(),
            );
            prepared.models.insert(plan.key.clone(), model);
        }
    } else {
        bevy::log::info!(
            "[modelica-runtime] reused prepared solver IR for `{}`: cache=hit",
            plan.spec.id,
        );
    }
    let model = prepared
        .models
        .get(&plan.key)
        .expect("prepared solver model inserted or found above");
    lunco_modelica_solver::simulation_session::live_from_solve_model(
        model,
        &plan.spec,
        plan.options,
    )
}

#[cfg(not(target_arch = "wasm32"))]
/// Owns bounded off-thread preparation for immutable solve models and source roots.
/// Disk-cache access and DAE lowering stay here; the Modelica command owner only
/// commits a completed model and creates the thread-affine live stepper.
struct SolvePreparationPool {
    pool: rayon::ThreadPool,
    tx: Sender<WorkerPreparationResult>,
    rx: Receiver<WorkerPreparationResult>,
    next_id: u64,
}

fn prepare_source_root_payload(id: String, payload: LoadSourceRootPayload) -> PreparedSourceRoot {
    let (label, files, diagnostics) = match payload {
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
            match lunco_assets_runtime::models::package_files_live(&root) {
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
impl SolvePreparationPool {
    fn new() -> Self {
        // Solve lowering is memory-heavy and each request walks a complete
        // structural graph. A small dedicated pool avoids turning parallel
        // startup into memory-bandwidth contention while reserving two logical
        // CPUs for Bevy and the OS.
        let threads = std::thread::available_parallelism()
            .map(|count| count.get().saturating_sub(2).clamp(1, 4))
            .unwrap_or(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .stack_size(16 * 1024 * 1024)
            .thread_name(|index| format!("modelica-solve-{index}"))
            .build()
            .expect("Modelica solve-preparation pool must be constructible");
        bevy::log::info!(
            "[modelica-runtime] solve preparation pool ready: {threads} worker thread(s)"
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            pool,
            tx,
            rx,
            next_id: 0,
        }
    }

    fn submit(&mut self, work: &CompileWork) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let dae = work.comp_res.dae.clone();
        let options = work.plan.options.clone();
        let model_name = work.model_name.clone();
        let disk_cache = work.plan.persistent_library_revision.map(|revision| {
            (
                work.plan.source_key,
                revision,
                work.plan.override_key.clone(),
            )
        });
        let tx = self.tx.clone();
        self.pool.spawn(move || {
            let cached = disk_cache.as_ref().and_then(
                |(source_key, revision, overrides)| {
                    PreparedSolveCache::load_disk(*source_key, *revision, overrides)
                },
            );
            let disk_hit = cached.is_some();
            let result = if let Some(model) = cached {
                Ok(model)
            } else {
                let lower_started = web_time::Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    lunco_modelica_solver::simulation_session::lower_for_live(&dae, &options)
                }))
                .unwrap_or_else(|payload| {
                    let message = payload
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown payload");
                    Err(rumoca_sim::SimulationDiagnosticError::Solver(format!(
                        "parallel solve lowering panicked: {message}"
                    )))
                });
                if result.is_ok() {
                    log::debug!(
                        "[modelica-runtime] parallel solve lowering finished for `{model_name}` in {:?}",
                        lower_started.elapsed(),
                    );
                }
                result
            };
            if !disk_hit {
                if let (Ok(model), Some((source_key, revision, overrides))) =
                    (&result, disk_cache.as_ref())
                {
                    PreparedSolveCache::save_disk(*source_key, *revision, overrides, model);
                }
            } else {
                log::debug!(
                    "[modelica-runtime] loaded prepared solver IR for `{model_name}`: cache=disk-hit"
                );
            }
            if tx
                .send(WorkerPreparationResult::Solve(SolvePreparationResult {
                    id,
                    result,
                }))
                .is_err()
            {
                log::error!(
                    "[modelica-runtime] solve preparation result dropped id={id} for `{model_name}`"
                );
            }
        });
        id
    }

    fn submit_source_root(&mut self, id: String, payload: LoadSourceRootPayload) -> u64 {
        let operation_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
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
    Solve(SolvePreparationResult),
    SourceRoot(SourceRootPreparationResult),
    Compiler(CompilerCompletion),
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingCompile {
    entity: Entity,
    session_id: u64,
    cancelled: bool,
    model_name: String,
    source: String,
    doc_uri: String,
    raw_extras: Vec<(String, String)>,
    parameter_overrides: Vec<(String, f64)>,
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
struct SolvePreparationResult {
    id: u64,
    result: Result<rumoca_ir_solve::SolveModel, rumoca_sim::SimulationDiagnosticError>,
}

#[cfg(not(target_arch = "wasm32"))]
struct SourceRootPreparationResult {
    id: u64,
    prepared: PreparedSourceRoot,
}

#[cfg(not(target_arch = "wasm32"))]
struct CompileWork {
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
    unit: CompileUnit,
    comp_res: Box<rumoca_compile::compile::DaeCompilationResult>,
    unit_key: u64,
    library_gen: u64,
    library_revision: u64,
    plan: LiveBuildPlan,
    intent: CompileIntent,
}

#[cfg(not(target_arch = "wasm32"))]
fn compile_work_error(
    tx: &Sender<ModelicaResult>,
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
    tx: &Sender<ModelicaResult>,
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
    tx: &Sender<ModelicaResult>,
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
fn finish_compile_work(
    work: CompileWork,
    steppers: &mut HashMap<Entity, (u64, String, LiveStepper)>,
    cached_models: &mut HashMap<Entity, CachedModel>,
    realtime_models: &std::collections::HashSet<Entity>,
    prepared_solve_cache: &mut PreparedSolveCache,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &Sender<ModelicaResult>,
) {
    let stepper_result = build_stepper(
        &work.comp_res,
        profile_for(work.entity, realtime_models),
        &work.parameter_overrides,
        work.unit_key,
        Some(work.library_revision),
        prepared_solve_cache,
    );
    match stepper_result {
        Ok(mut stepper) => {
            let CompileWork {
                entity,
                session_id,
                cancelled: _,
                model_name,
                source,
                doc_uri,
                raw_extras,
                parameter_overrides,
                unit,
                comp_res,
                unit_key: _,
                library_gen,
                library_revision: _,
                plan: _,
                intent,
            } = work;
            let defaults_context = match &intent {
                CompileIntent::Reset => "Reset",
                CompileIntent::StepInit(_) => "Init",
                _ => "Compile",
            };
            apply_input_defaults_validated(&mut stepper, &unit.input_defaults, defaults_context);
            let input_names = stepper.input_names().to_vec();
            let symbols = collect_stepper_observables(&stepper);
            let unit_hash = compile_unit_hash(&model_name, &doc_uri, &unit);
            cached_models.insert(
                entity,
                CachedModel {
                    model_name: model_name.clone(),
                    source: Arc::from(source),
                    extra_sources: raw_extras,
                    parameter_overrides,
                    doc_uri,
                    compiled: comp_res.clone(),
                    unit_hash,
                    library_gen,
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
                            compile_diagnostics: unit.default_diagnostics,
                            ..Default::default()
                        },
                        &comp_res,
                    ));
                }
                CompileIntent::Reset => {
                    let mut result =
                        reset_ok(entity, session_id, symbols, input_names, "Reset complete.");
                    result.compile_diagnostics = unit.default_diagnostics;
                    let _ = tx.send(result);
                }
                CompileIntent::UpdateParameters => {
                    let _ = tx.send(ModelicaResult {
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
                        compile_diagnostics: unit.default_diagnostics,
                        ..Default::default()
                    });
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
fn complete_preparation(
    preparation: SolvePreparationResult,
    pending_compile_works: &mut HashMap<u64, CompileWork>,
    current_sessions: &HashMap<Entity, u64>,
    library_gen: u64,
    prepared_solve_cache: &mut PreparedSolveCache,
    steppers: &mut HashMap<Entity, (u64, String, LiveStepper)>,
    cached_models: &mut HashMap<Entity, CachedModel>,
    realtime_models: &std::collections::HashSet<Entity>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &Sender<ModelicaResult>,
) {
    let preparation_id = preparation.id;
    let Some(work) = pending_compile_works.remove(&preparation_id) else {
        bevy::log::error!(
            "[modelica-runtime] solve preparation result {preparation_id} has no pending compile work"
        );
        return;
    };
    if work.cancelled {
        bevy::log::debug!(
            "[modelica-runtime] discarded cancelled solve preparation {preparation_id} for `{}`",
            work.model_name
        );
        return;
    }
    // A newer Compile or Despawn owns this entity now. The worker still drains
    // the finished job, but its result must not install an obsolete stepper.
    if current_sessions.get(&work.entity).copied() != Some(work.session_id)
        || work.library_gen != library_gen
    {
        bevy::log::warn!(
            "[modelica-runtime] discarded stale solve preparation {preparation_id} for `{}`: \
             entity={:?} work_session={} current_session={:?} work_library_gen={} current_library_gen={}",
            work.model_name,
            work.entity,
            work.session_id,
            current_sessions.get(&work.entity),
            work.library_gen,
            library_gen,
        );
        return;
    }
    match preparation.result {
        Ok(model) => {
            prepared_solve_cache
                .models
                .insert(work.plan.key.clone(), model);
            let entity = work.entity;
            let session_id = work.session_id;
            let model_name = work.model_name.clone();
            let intent = work.intent.clone();
            let commit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                finish_compile_work(
                    work,
                    steppers,
                    cached_models,
                    realtime_models,
                    prepared_solve_cache,
                    step_lane,
                    tx,
                );
            }));
            if let Err(payload) = commit {
                let message = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown panic payload");
                bevy::log::error!(
                    "[modelica-runtime] solve preparation commit panicked for `{model_name}` \
                     (entity={entity:?}, session={session_id}): {message}"
                );
                let mut result = result_for_compile_intent(entity, session_id, &intent);
                result.error = Some(format!(
                    "Stepper preparation commit panicked for `{model_name}`: {message}"
                ));
                let _ = tx.send(result);
            }
        }
        Err(error) => compile_work_error(tx, &work, &error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn stage_preparation_result(
    result: WorkerPreparationResult,
    ready_solves: &mut HashMap<u64, SolvePreparationResult>,
    ready_source_roots: &mut HashMap<u64, SourceRootPreparationResult>,
    ready_compiler: &mut HashMap<u64, CompilerCompletion>,
) {
    let duplicate = match result {
        WorkerPreparationResult::Solve(result) => ready_solves.insert(result.id, result).is_some(),
        WorkerPreparationResult::SourceRoot(result) => {
            ready_source_roots.insert(result.id, result).is_some()
        }
        WorkerPreparationResult::Compiler(result) => {
            let id = match &result {
                CompilerCompletion::Compile { id, .. }
                | CompilerCompletion::SourceRoot { id, .. } => *id,
            };
            ready_compiler.insert(id, result).is_some()
        }
    };
    if duplicate {
        bevy::log::error!("[modelica-runtime] duplicate preparation completion id");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_ready_solve_preparations(
    order: &mut VecDeque<u64>,
    ready: &mut HashMap<u64, SolvePreparationResult>,
    pending_compile_works: &mut HashMap<u64, CompileWork>,
    current_sessions: &HashMap<Entity, u64>,
    library_gen: u64,
    prepared_solve_cache: &mut PreparedSolveCache,
    steppers: &mut HashMap<Entity, (u64, String, LiveStepper)>,
    cached_models: &mut HashMap<Entity, CachedModel>,
    realtime_models: &std::collections::HashSet<Entity>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &Sender<ModelicaResult>,
) {
    loop {
        let Some(result) = pop_ready_in_order(order, ready) else {
            return;
        };
        complete_preparation(
            result,
            pending_compile_works,
            current_sessions,
            library_gen,
            prepared_solve_cache,
            steppers,
            cached_models,
            realtime_models,
            step_lane,
            tx,
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_ready_source_roots(
    order: &mut VecDeque<u64>,
    ready: &mut HashMap<u64, SourceRootPreparationResult>,
    compiler: &mut CompilerActor,
    compiler_order: &mut VecDeque<u64>,
    pending_solve_preparations: usize,
    pending_installs: &mut HashSet<u64>,
    tx: &Sender<ModelicaResult>,
) {
    loop {
        if !compiler.can_submit(compiler_order.len() + pending_solve_preparations) {
            return;
        }
        let Some(result) = pop_ready_in_order(order, ready) else {
            return;
        };
        let root_id = result.prepared.source_set_id().to_owned();
        match compiler.submit_source_root(result.prepared) {
            Ok(operation_id) => {
                compiler_order.push_back(operation_id);
                pending_installs.insert(operation_id);
            }
            Err((error, _prepared)) => {
                let _ = tx.send(ModelicaResult {
                    loaded_source_root_id: Some(root_id),
                    error: Some(error),
                    ..Default::default()
                });
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_ready_compiler_completions(
    order: &mut VecDeque<u64>,
    ready: &mut HashMap<u64, CompilerCompletion>,
    pending_installs: &mut HashSet<u64>,
    pending_compiles: &mut HashMap<u64, PendingCompile>,
    current_sessions: &HashMap<Entity, u64>,
    library_gen: &mut u64,
    library_defaults: &mut HashMap<String, f64>,
    library_revision: &mut u64,
    prepared_solve_cache: &mut PreparedSolveCache,
    solve_preparation_pool: &mut SolvePreparationPool,
    solve_preparation_order: &mut VecDeque<u64>,
    pending_compile_works: &mut HashMap<u64, CompileWork>,
    steppers: &mut HashMap<Entity, (u64, String, LiveStepper)>,
    cached_models: &mut HashMap<Entity, CachedModel>,
    realtime_models: &std::collections::HashSet<Entity>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &Sender<ModelicaResult>,
) {
    while let Some(completion) = pop_ready_in_order(order, ready) {
        match completion {
            CompilerCompletion::SourceRoot { id, commit } => {
                pending_installs.remove(&id);
                let library_changed =
                    commit.inserted_file_count > 0 || commit.library_revision != *library_revision;
                if library_changed {
                    *library_gen = library_gen.wrapping_add(1);
                    prepared_solve_cache.disable_persistent();
                }
                *library_defaults = commit.library_defaults;
                *library_revision = commit.library_revision;
                log::info!(
                    "[modelica-runtime] LoadSourceRoot `{}`: {} parsed / {} inserted",
                    commit.root_id,
                    commit.parsed_file_count,
                    commit.inserted_file_count,
                );
                let _ = tx.send(ModelicaResult {
                    loaded_source_root_id: Some(commit.root_id),
                    error: commit.error,
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
                if pending.cancelled
                    || current_sessions.get(&pending.entity).copied() != Some(pending.session_id)
                    || pending.library_gen != *library_gen
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
                let comp_res = match artifact.outcome {
                    Ok(compiled) => compiled,
                    Err(error) => {
                        send_compile_artifact_error(tx, &pending, error, artifact.diagnostics);
                        continue;
                    }
                };
                let unit_key = prepared_unit_hash(
                    &pending.model_name,
                    &pending.doc_uri,
                    &artifact.unit,
                    pending.library_gen,
                );
                let plan = match live_build_plan(
                    profile_for(pending.entity, realtime_models),
                    &pending.parameter_overrides,
                    unit_key,
                    Some(artifact.library_revision),
                    prepared_solve_cache,
                ) {
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
                    entity: pending.entity,
                    session_id: pending.session_id,
                    cancelled: false,
                    model_name: pending.model_name,
                    source: pending.source,
                    doc_uri: pending.doc_uri,
                    raw_extras: pending.raw_extras,
                    parameter_overrides: pending.parameter_overrides,
                    unit: artifact.unit,
                    comp_res,
                    unit_key,
                    library_gen: pending.library_gen,
                    library_revision: artifact.library_revision,
                    plan,
                    intent: pending.intent,
                };
                if prepared_solve_cache.models.contains_key(&work.plan.key) {
                    finish_compile_work(
                        work,
                        steppers,
                        cached_models,
                        realtime_models,
                        prepared_solve_cache,
                        step_lane,
                        tx,
                    );
                } else {
                    let job_id = solve_preparation_pool.submit(&work);
                    pending_compile_works.insert(job_id, work);
                    solve_preparation_order.push_back(job_id);
                }
            }
        }
    }
}

use std::sync::Arc;

/// Cached compilation result per entity.
///
/// This holds the compiled artifact, not just the source. Rumoca's
/// `DaeCompilationResult` is `Clone` and carries the DAE behind an `Arc`; Reset
/// and Step auto-init can rebuild from it without touching the compiler while
/// the library generation matches. If solve IR is absent, disk cache access
/// and lowering run through the asynchronous preparation pool.
///
/// The artifact is valid only for what it was built from: `unit_hash` keys the
/// assembled [`CompileUnit`] (stripped primary + extras + model name + session
/// URI), and `library_gen` records the worker's library generation (bumped on
/// every `LoadSourceRoot`). Native rebuilds validate both before reusing the
/// artifact; wasm rebuilds refresh it through [`rebuild_from_cache`].
struct CachedModel {
    model_name: String,
    source: Arc<str>,
    /// Sibling docs the model was compiled with, raw like `source`. Replayed
    /// on a cache-invalidating recompile so Reset / Step auto-init resolve the
    /// same cross-doc references the original Compile did (the source-only
    /// cache silently dropped these and recompiled the primary alone).
    extra_sources: Vec<(String, String)>,
    /// Instance parameter values applied to the cached DAE. Reapplied if a
    /// library invalidates the artifact and it must be compiled again.
    parameter_overrides: Vec<(String, f64)>,
    /// The document's stable session URI (see `ModelicaCommand::Compile`'s
    /// `doc_uri`). Every cached-source recompile — Reset, Step auto-init,
    /// UpdateParameters — re-seats under this SAME key so the reused rumoca
    /// session never holds the document under two filenames.
    doc_uri: String,
    /// The compiled artifact steppers are rebuilt from (see struct docs).
    compiled: Box<rumoca_compile::compile::DaeCompilationResult>,
    /// [`compile_unit_hash`] of the [`CompileUnit`] `compiled` was built from.
    unit_hash: u64,
    /// Worker library generation at the time `compiled` was built.
    library_gen: u64,
}

/// Key identifying WHAT a cached artifact was compiled from: the assembled
/// [`CompileUnit`] (stripped primary + stripped extras), the model name, and
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

/// Stable cross-process identity for the solve-IR cache. It uses the same
/// structural source identity as the in-process artifact cache and includes
/// the worker library generation; unlike a Bevy entity or a Rumoca source id
/// it is identical in a fresh recorder process.
fn prepared_unit_hash(
    model_name: &str,
    doc_uri: &str,
    unit: &CompileUnit,
    library_gen: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    shared_source_hash(model_name, unit, doc_uri).hash(&mut h);
    library_gen.hash(&mut h);
    h.finish()
}

const GENERATED_MODEL_MARKER: &str = "__LUNCO_GENERATED_MODEL__";
const GENERATED_INSTANCE_MARKER: &str = "__LUNCO_GENERATED_INSTANCE__";

/// Whether this is a generated USD wrapper whose root name is instance-local.
fn is_generated_structural_unit(model_name: &str, unit: &CompileUnit, doc_uri: &str) -> bool {
    doc_uri.starts_with("generated://") && unit.source.contains(model_name)
}

/// Remove only identity emitted by the generated USD wrapper.  The generated
/// class name is instance-qualified for diagnostics, while the network title
/// also contains a numeric runtime-root suffix.  Both identify the USD copy,
/// not its equations; ordinary numeric Modelica literals remain part of the
/// structural key.
fn generated_structural_source(model_name: &str, source: &str) -> String {
    let mut normalized = source.replace(model_name, GENERATED_MODEL_MARKER);
    let Some(class_stem) = model_name.strip_suffix("_System") else {
        return normalized;
    };
    let Some((_, instance_id)) = class_stem.rsplit_once("__") else {
        return normalized;
    };
    if instance_id.is_empty() || !instance_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return normalized;
    }
    // The generated policy puts the runtime-root suffix in the network-title
    // graphic. Restrict the replacement to that authored identity line so a
    // legitimate equation literal equal to the root id remains structural.
    let mut title_normalized = String::with_capacity(normalized.len());
    for line in normalized.split_inclusive('\n') {
        if line.contains(" network") {
            title_normalized.push_str(&line.replace(instance_id, GENERATED_INSTANCE_MARKER));
        } else {
            title_normalized.push_str(line);
        }
    }
    normalized = title_normalized;
    normalized
}

/// Hash the source identity used by the cross-entity artifact and prepared
/// solve-IR caches. Document URIs are attribution keys, not equation identity;
/// generated instance names and their numeric network-title suffix are
/// similarly excluded while all authored source text and sibling URIs remain
/// part of the key.
fn shared_source_hash(model_name: &str, unit: &CompileUnit, doc_uri: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if is_generated_structural_unit(model_name, unit, doc_uri) {
        generated_structural_source(model_name, &unit.source).hash(&mut h);
    } else {
        model_name.hash(&mut h);
        unit.source.hash(&mut h);
    }
    for (uri, text) in &unit.extras {
        uri.hash(&mut h);
        text.hash(&mut h);
    }
    h.finish()
}

/// Stable key for a compiled DAE that can be shared by multiple scene
/// participants.  The document URI is intentionally absent: it identifies
/// the authoring document, not the Modelica equations.  Two USD instances
/// with the same assembled equations have the same compile artifact, even
/// when generated wrappers carry different instance-qualified names; their
/// parameter bindings and live steppers are still created independently below.
fn shared_compile_hash(
    model_name: &str,
    unit: &CompileUnit,
    doc_uri: &str,
    library_gen: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Generated USD network wrappers carry a per-instance identity in their
    // Modelica class name so diagnostics and document URIs stay attributable.
    // That identity is not part of the equations, however: hashing it here
    // defeats the cross-entity DAE cache and serially recompiles every copy of
    // one rover. Keep authored documents keyed by their declared model name,
    // while generated wrappers use the same structural source key after
    // removing only their generated root identifier. Live steppers remain
    // entity-local; only the immutable compiled DAE is shared.
    shared_source_hash(model_name, unit, doc_uri).hash(&mut h);
    library_gen.hash(&mut h);
    h.finish()
}

/// Compile once per assembled Modelica unit, even when several USD instances
/// request the same source in the same scene.  The returned DAE is cloned
/// cheaply (rumoca stores the large graph behind an `Arc`); parameter
/// overrides and solver selection remain per-instance operations.
fn compile_shared(
    artifacts: &mut HashMap<u64, Box<rumoca_compile::compile::DaeCompilationResult>>,
    compiler: &mut ModelicaCompiler,
    model_name: &str,
    unit: &CompileUnit,
    doc_uri: &str,
    library_gen: u64,
) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
    let key = shared_compile_hash(model_name, unit, doc_uri, library_gen);
    let generated = is_generated_structural_unit(model_name, unit, doc_uri);
    log::debug!(
        "[worker] shared artifact lookup model=`{model_name}` doc=`{doc_uri}` generated={generated} source_bytes={} cache_entries={}",
        unit.source.len(),
        artifacts.len(),
    );
    if let Some(compiled) = artifacts.get(&key) {
        log::debug!("[worker] shared Modelica artifact hit for `{model_name}` (key={key})");
        return Ok(compiled.clone());
    }
    let outcome = if unit.extras.is_empty() {
        compiler.compile_str_after_source_root_admission(model_name, &unit.source, doc_uri)
    } else {
        compiler.compile_str_multi_after_source_root_admission(
            model_name,
            &unit.source,
            doc_uri,
            &unit.extras,
        )
    };
    if let Ok(compiled) = &outcome {
        artifacts.insert(key, compiled.clone());
    }
    outcome
}

struct BackendCompileResult {
    unit: CompileUnit,
    outcome: Result<Box<rumoca_compile::compile::DaeCompilationResult>, String>,
    diagnostics: Vec<lunco_doc::Diagnostic>,
    library_revision: u64,
}

#[cfg(target_arch = "wasm32")]
trait CompileBackend {
    fn library_input_defaults(&self) -> &HashMap<String, f64>;
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
    artifacts: &'a mut HashMap<u64, Box<rumoca_compile::compile::DaeCompilationResult>>,
}

#[cfg(target_arch = "wasm32")]
impl CompileBackend for InlineCompileBackend<'_> {
    fn library_input_defaults(&self) -> &HashMap<String, f64> {
        self.compiler
            .as_ref()
            .map(ModelicaCompiler::library_input_defaults)
            .unwrap_or_else(|| empty_library_input_defaults())
    }

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
            compiler.compile_diagnostics(model_name, doc_uri)
        } else {
            Vec::new()
        };
        BackendCompileResult {
            unit,
            outcome,
            diagnostics,
            library_revision: compiler.library_revision(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn empty_library_input_defaults() -> &'static HashMap<String, f64> {
    static DEFAULTS: std::sync::OnceLock<HashMap<String, f64>> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(HashMap::new)
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
    model_name: String,
    /// Revision of the compiler-owned source roots used by this artifact.
    library_revision: Option<u64>,
    /// The instance values that must be supplied to Rumoca when the cached DAE
    /// is lowered into a fresh live stepper.
    parameter_overrides: Vec<(String, f64)>,
    /// Stable source-set identity used by the cross-process solve-IR cache.
    unit_key: u64,
    /// Assembled from the cached source set — carries the `input_defaults`
    /// to re-seed and the stripped primary for error diagnostics.
    unit: CompileUnit,
    /// `Ok` = artifact to build the stepper from; `Err` = rumoca's formatted
    /// compile summary.
    outcome: Result<Box<rumoca_compile::compile::DaeCompilationResult>, String>,
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
    let (model_name, doc_uri, source, extras, parameter_overrides, cached_hash, cached_gen) = {
        let c = cached_models.get(&entity)?;
        (
            c.model_name.clone(),
            c.doc_uri.clone(),
            Arc::clone(&c.source),
            c.extra_sources.clone(),
            c.parameter_overrides.clone(),
            c.unit_hash,
            c.library_gen,
        )
    };
    let mut unit = assemble_compile_unit(&source, extras);
    let hash = compile_unit_hash(&model_name, &doc_uri, &unit);
    let unit_key = prepared_unit_hash(&model_name, &doc_uri, &unit, library_gen);
    // Library defaults are folded in AFTER hashing on purpose: the hash keys the
    // source set, and `library_gen` already invalidates the artifact when the
    // seated libraries change. Both the reuse and the recompile path below need
    // the merged map, so it happens before either returns.
    unit.merge_library_defaults(backend.library_input_defaults());
    if artifact_still_valid(cached_hash, cached_gen, hash, library_gen) {
        let library_revision = Some(backend.library_revision());
        let compiled = cached_models
            .get(&entity)
            .expect("checked above")
            .compiled
            .clone();
        return Some(CacheRebuild {
            model_name,
            library_revision,
            parameter_overrides,
            unit_key,
            unit,
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
        model_name,
        library_revision: Some(result.library_revision),
        parameter_overrides,
        unit_key,
        unit: result.unit,
        outcome: result.outcome,
        compile_diagnostics: result.diagnostics,
    })
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeCachedRebuild {
    model_name: String,
    source: Arc<str>,
    raw_extras: Vec<(String, String)>,
    doc_uri: String,
    parameter_overrides: Vec<(String, f64)>,
    compiled: Box<rumoca_compile::compile::DaeCompilationResult>,
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
                entity,
                session_id,
                cancelled: false,
                model_name: self.model_name,
                source: self.source.to_string(),
                doc_uri: self.doc_uri,
                raw_extras: self.raw_extras,
                parameter_overrides: self.parameter_overrides,
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
        prepared: &PreparedSolveCache,
    ) -> Result<CompileWork, rumoca_sim::SimulationDiagnosticError> {
        let plan = live_build_plan(
            profile,
            &self.parameter_overrides,
            self.unit_key,
            Some(self.library_revision),
            prepared,
        )?;
        Ok(CompileWork {
            entity,
            session_id,
            cancelled: false,
            model_name: self.model_name,
            source: self.source.to_string(),
            doc_uri: self.doc_uri,
            raw_extras: self.raw_extras,
            parameter_overrides: self.parameter_overrides,
            unit: self.unit,
            comp_res: self.compiled,
            unit_key: self.unit_key,
            library_gen,
            library_revision: self.library_revision,
            plan,
            intent,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn native_cached_rebuild(
    cached_models: &HashMap<Entity, CachedModel>,
    entity: Entity,
    library_gen: u64,
    library_defaults: &HashMap<String, f64>,
    library_revision: u64,
) -> Option<NativeCachedRebuild> {
    let cached = cached_models.get(&entity)?;
    let mut unit = assemble_compile_unit(&cached.source, cached.extra_sources.clone());
    let unit_key = prepared_unit_hash(&cached.model_name, &cached.doc_uri, &unit, library_gen);
    unit.merge_library_defaults(library_defaults);
    Some(NativeCachedRebuild {
        model_name: cached.model_name.clone(),
        source: Arc::clone(&cached.source),
        raw_extras: cached.extra_sources.clone(),
        doc_uri: cached.doc_uri.clone(),
        parameter_overrides: cached.parameter_overrides.clone(),
        compiled: cached.compiled.clone(),
        unit_hash: cached.unit_hash,
        cached_library_gen: cached.library_gen,
        unit_key,
        unit,
        library_revision,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn cached_solve_is_prepared(
    cached_models: &HashMap<Entity, CachedModel>,
    entity: Entity,
    library_gen: u64,
    library_defaults: &HashMap<String, f64>,
    library_revision: u64,
    profile: solver::RuntimeProfile,
    prepared_solve_cache: &PreparedSolveCache,
) -> bool {
    let Some(cached) = native_cached_rebuild(
        cached_models,
        entity,
        library_gen,
        library_defaults,
        library_revision,
    ) else {
        return false;
    };
    if !cached.artifact_is_valid(library_gen) {
        return false;
    }
    live_build_plan(
        profile,
        &cached.parameter_overrides,
        cached.unit_key,
        Some(cached.library_revision),
        prepared_solve_cache,
    )
    .map(|plan| prepared_solve_cache.models.contains_key(&plan.key))
    // A selection error is reported when the queued Step reaches its owner
    // boundary; it does not need async preparation capacity.
    .unwrap_or(true)
}

#[cfg(not(target_arch = "wasm32"))]
fn submit_pending_compile(
    pending: PendingCompile,
    unit: CompileUnit,
    compiler: &mut CompilerActor,
    compiler_order: &mut VecDeque<u64>,
    pending_compiles: &mut HashMap<u64, PendingCompile>,
    tx: &Sender<ModelicaResult>,
) {
    match compiler.submit_compile(
        pending.model_name.clone(),
        unit,
        pending.doc_uri.clone(),
        pending.library_gen,
    ) {
        Ok(operation_id) => {
            compiler_order.push_back(operation_id);
            pending_compiles.insert(operation_id, pending);
        }
        Err((error, unit)) => {
            send_compile_artifact_error(tx, &pending, error, unit.default_diagnostics);
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
    solve_preparation_pool: &mut SolvePreparationPool,
    solve_preparation_order: &mut VecDeque<u64>,
    pending_compile_works: &mut HashMap<u64, CompileWork>,
    steppers: &mut HashMap<Entity, (u64, String, LiveStepper)>,
    cached_models: &mut HashMap<Entity, CachedModel>,
    realtime_models: &std::collections::HashSet<Entity>,
    prepared_solve_cache: &mut PreparedSolveCache,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &Sender<ModelicaResult>,
) {
    let source = Arc::clone(&cached.source);
    let failure_intent = intent.clone();
    let work = match cached.into_compile_work(
        entity,
        session_id,
        library_gen,
        profile,
        intent,
        prepared_solve_cache,
    ) {
        Ok(work) => work,
        Err(error) => {
            send_compile_stepper_error(tx, entity, session_id, &source, &error, &failure_intent);
            return;
        }
    };
    if prepared_solve_cache.models.contains_key(&work.plan.key) {
        finish_compile_work(
            work,
            steppers,
            cached_models,
            realtime_models,
            prepared_solve_cache,
            step_lane,
            tx,
        );
    } else {
        let job_id = solve_preparation_pool.submit(&work);
        pending_compile_works.insert(job_id, work);
        solve_preparation_order.push_back(job_id);
    }
}

/// Collect every readable variable from the stepper — states, inputs, and
/// (on rumoca `main`) algebraic / output reconstructions via
/// `EliminationResult`. Non-finite values are dropped so the UI never
/// plots NaN. Filtering out parameters / inputs happens downstream in
/// [`handle_modelica_responses`]; we report everything here so the UI has
/// the full picture and decides what goes into `model.variables`.
pub(crate) trait ObservableStepper {
    fn observable_state(
        &self,
    ) -> Result<rumoca_sim::SessionState, rumoca_sim::SimulationDiagnosticError>;
}

impl ObservableStepper for LiveStepper {
    fn observable_state(
        &self,
    ) -> Result<rumoca_sim::SessionState, rumoca_sim::SimulationDiagnosticError> {
        self.state()
    }
}

impl ObservableStepper for rumoca_sim::SimulationSession {
    fn observable_state(
        &self,
    ) -> Result<rumoca_sim::SessionState, rumoca_sim::SimulationDiagnosticError> {
        self.state()
    }
}

pub(crate) fn collect_stepper_observables<S: ObservableStepper>(stepper: &S) -> Vec<(String, f64)> {
    let Ok(state) = stepper.observable_state() else {
        return Vec::new();
    };
    state
        .values
        .into_iter()
        .filter(|(name, val)| val.is_finite() && name != "time")
        .collect()
}

/// Fixed solver tolerance on the LIVE path. Explicit, and deliberately NOT the
/// model's `experiment(Tolerance=…)` annotation nor the batch runner's default —
/// see [`live_stepper_options`] and the runtime solver capability contract.
const LIVE_TOL: f64 = 1e-6;

/// The LIVE path's **micro-step**: the one and only step size handed to the
/// solver. Three micro-steps per fixed tick (60 Hz ⇒ 180 Hz solver rate).
///
/// Derived from [`lunco_core_runtime::SECS_PER_TICK`], so the model's stop-time lattice
/// is a pure function of the FIXED-STEP clock — never of the render frame, GPU
/// load, or window focus (A3).
const LIVE_MICRO_DT: f64 = lunco_core_runtime::SECS_PER_TICK / 3.0;

const COMMUNICATION_EPS: f64 = 1e-9;
const COMMUNICATION_TIME_EPS: f64 = 1e-8;

/// Hard cap on micro-steps integrated inside ONE `Step` command.
///
/// A large deficit can still occur after an intentional pause or a rate change,
/// so the requested macro step is capped at this many micro-steps (~0.178 s).
/// A worker stall is handled differently: the fixed-step coupling barrier holds
/// physics and does not advance `target_time` while a step is in flight. The
/// clamp therefore bounds an explicit authored time jump, not accumulated worker
/// debt.
const MAX_MICRO_STEPS_PER_MACRO: u32 = 32;

/// Below this deficit we don't dispatch a `Step` at all — the model is already
/// at the communication point (within half a micro-step) and a sub-micro-step
/// `dt` would just round to a full micro-step and overshoot.
const MIN_MACRO_STEP_DT: f64 = LIVE_MICRO_DT * 0.5;

/// How many [`LIVE_MICRO_DT`] micro-steps a macro step of `dt` seconds becomes.
///
/// Integer, monotone, and clamped to [`MAX_MICRO_STEPS_PER_MACRO`] — the same
/// on every peer, for every `dt`. Round-to-nearest (rather than floor) keeps the
/// model's clock centred on the world's: a residual of at most half a micro-step
/// is carried into the next tick's deficit and cancels there.
fn micro_steps_for(dt: f64) -> u32 {
    if dt.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return 0;
    }
    let n = (dt / LIVE_MICRO_DT).round();
    (n as u32).clamp(1, MAX_MICRO_STEPS_PER_MACRO)
}

/// Integrate one macro step: `micro_steps_for(dt)` fixed micro-steps.
///
/// The ONE integration loop for the live path — native and wasm workers both
/// call it, so the two `#[cfg]` twins cannot drift on step policy.
/// Advances the model's own clock by exactly `micro_steps_for(dt) *
/// LIVE_MICRO_DT`; the caller reads `stepper.time()` for the truth and the Bevy
/// side reconciles any residual against the world clock next tick.
fn integrate_macro_step(
    stepper: &mut LiveStepper,
    dt: f64,
) -> Result<(), rumoca_sim::SimulationDiagnosticError> {
    for _ in 0..micro_steps_for(dt) {
        stepper.step(LIVE_MICRO_DT)?;
    }
    Ok(())
}

/// Validate the worker-side FMI-CS transaction envelope before mutating solver
/// state. The master owns the interval; a worker must not silently reinterpret
/// a request as "whatever time happens to be next".
#[inline]
fn communication_times_close(a: f64, b: f64) -> bool {
    (a - b).abs() <= COMMUNICATION_TIME_EPS * a.abs().max(b.abs()).max(1.0)
}

fn validate_step_request(
    actual_start: f64,
    start_time: f64,
    stop_time: f64,
    dt: f64,
) -> Result<(), String> {
    if !start_time.is_finite()
        || !stop_time.is_finite()
        || !dt.is_finite()
        || dt <= 0.0
        || stop_time <= start_time
        || !communication_times_close(stop_time - start_time, dt)
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

fn add_experiment_defaults(
    mut result: ModelicaResult,
    comp_res: &rumoca_compile::compile::DaeCompilationResult,
) -> ModelicaResult {
    result.experiment_start_time = comp_res.experiment_start_time;
    result.experiment_stop_time = comp_res.experiment_stop_time;
    result.experiment_tolerance = comp_res.experiment_tolerance;
    result.experiment_interval = comp_res.experiment_interval;
    result.experiment_solver = comp_res.experiment_solver.clone();
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
/// failure resolves the root load. A placeholder-only response cannot clear
/// any of those state machines.
pub fn failed_result_for_command(
    cmd: &ModelicaCommand,
    message: impl Into<String>,
) -> ModelicaResult {
    let mut result = ModelicaResult {
        error: Some(message.into()),
        log_message: Some("Modelica worker command failed".to_string()),
        ..Default::default()
    };
    match cmd {
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
        ModelicaCommand::LoadSourceRoot { id, .. } => {
            result.loaded_source_root_id = Some(id.clone());
        }
        ModelicaCommand::Despawn { .. } => {}
    }
    result
}

/// Build the terminal response for a command that panicked inside the solver
/// worker. Panics are still reported with the shared command lifecycle shape;
/// the worker transport decides whether the worker can continue afterward.
pub fn panic_result_for_command(cmd: &ModelicaCommand, message: &str) -> ModelicaResult {
    failed_result_for_command(cmd, format!("Modelica worker panic: {message}"))
}

/// Where a captured default was declared, which decides how its leaf name is
/// matched against the compiled model's runtime input slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultOrigin {
    /// Declared in the PRIMARY document. The compile target's own components
    /// flatten to UNQUALIFIED slot names, so match exactly first; fall back to
    /// instance-qualified slots for a default declared in a nested class of the
    /// primary, which flattens as `<instance>.<leaf>`.
    Primary,
    /// Declared in a sibling document or a seated library member. Such a class
    /// is only ever reached by INSTANTIATION, so its inputs can only appear as
    /// `<instance path>.<leaf>` — an exact unqualified hit would be some OTHER
    /// class's slot that merely shares the leaf name, so qualified matches only.
    Instanced,
}

/// One captured `input` default plus the matching rule its origin implies.
///
/// Deliberately ONE map for all origins rather than a second "library defaults"
/// / "extras defaults" map beside it: the seeding rule is the only thing that
/// differs, so it travels as data on the value.
#[derive(Debug, Clone, Copy)]
struct InputDefault {
    value: f64,
    origin: DefaultOrigin,
}

/// Which of the compiled model's runtime input slots a captured default applies
/// to — see [`DefaultOrigin`]. Multiple hits are correct and expected: two
/// instances of the same library class share the class's authored default.
fn resolve_default_slots(known: &[String], name: &str, origin: DefaultOrigin) -> Vec<String> {
    if origin == DefaultOrigin::Primary && known.iter().any(|k| k == name) {
        return vec![name.to_string()];
    }
    let suffix = format!(".{name}");
    known
        .iter()
        .filter(|k| k.ends_with(&suffix))
        .cloned()
        .collect()
}

/// Apply parsed input defaults to a stepper at init time, logging any
/// mismatch between the rumoca-detected names and the stepper's actual
/// input slots. The mismatch case is a rumoca-vs-flatten disagreement —
/// rare, but silent failure here would mean a user-set default never
/// reaches the simulator. Logged once per init, not per-call.
///
/// This is the ONE re-seed mechanism: every source of stripped defaults
/// (primary document, sibling docs, seated library members) arrives here in the
/// same map and is resolved by [`resolve_default_slots`].
fn apply_input_defaults_validated(
    stepper: &mut LiveStepper,
    input_defaults: &HashMap<String, InputDefault>,
    ctx: &str,
) {
    if input_defaults.is_empty() {
        return;
    }
    let known: Vec<String> = stepper.input_names().to_vec();
    let mut to_set: Vec<(String, f64)> = Vec::new();
    // Only a PRIMARY default that matches nothing is a signal. An `Instanced`
    // default that matches nothing just means the library class it came from is
    // not instantiated by this model — the common case, and not a problem.
    let mut unknown: Vec<&str> = Vec::new();
    for (name, def) in input_defaults {
        let slots = resolve_default_slots(&known, name, def.origin);
        if slots.is_empty() {
            if def.origin == DefaultOrigin::Primary {
                unknown.push(name.as_str());
            }
            continue;
        }
        for slot in slots {
            to_set.push((slot, def.value));
        }
    }
    if !unknown.is_empty() {
        // ALL of them missing is categorically worse than some of them: the model
        // exposes NO runtime slot at all, so every wire into it is rejected and it
        // runs on its declared defaults for the whole session — a simulation that
        // completes, publishes plausible numbers, and simulates nothing. That is
        // the expensive failure (it renders as usable footage), so it is an ERROR
        // and it names the two causes worth checking.
        if known.is_empty() {
            bevy::log::error!(
                "[{ctx}] the compiled model exposes NO runtime inputs at all, but the \
                 source declares {}: {:?}. Every wired value into this model will be \
                 DISCARDED and it will run on its declared defaults. rumoca demotes a \
                 bound `input Real x = <default>` to an algebraic, so this means the \
                 source reaching the compiler was NOT stripped — check that it entered \
                 through `seat_user_source` / `seat_library_files`.",
                unknown.len(),
                unknown,
            );
        } else {
            bevy::log::warn!(
                "[{ctx}] {} parsed input default(s) not in stepper.input_names(): {:?} (known: {:?})",
                unknown.len(),
                unknown,
                known,
            );
        }
    }
    for (name, val) in to_set {
        if let Err(e) = stepper.set_input(&name, val) {
            bevy::log::warn!("[{ctx}] set_input({name}) failed: {e:?}");
        }
    }
}

/// The complete source set one rumoca compile receives — primary plus any
/// sibling docs — with the bound-`input` workaround applied to EVERY member.
///
/// All worker compile paths (Compile, Reset, Step auto-init, UpdateParameters;
/// native and inline) assemble their sources through
/// [`assemble_compile_unit`], so no path can hand rumoca an unstripped string:
/// rumoca demotes a bound `input Real x = <default>` to an algebraic, which
/// deletes the runtime slot and silently drops every wire into it (see
/// `strip_input_defaults`). The compiler applies the same strip again at its
/// own `seat_user_source` chokepoint; the strip is a length-preserving no-op
/// on already-stripped text, so the two layers compose.
#[derive(Clone)]
struct CompileUnit {
    /// Primary source with input bindings blanked (length-preserving, so
    /// diagnostic byte offsets still index the editor's original buffer).
    source: String,
    /// Extra sibling docs, each stripped like the primary.
    extras: Vec<(String, String)>,
    /// Numeric input defaults captured from EVERY member of the source set —
    /// primary, sibling docs, and (folded in by
    /// [`CompileUnit::merge_library_defaults`]) the seated library members —
    /// re-seeded into the fresh stepper via [`apply_input_defaults_validated`].
    ///
    /// One map, not one per origin: the origin only changes how the leaf name is
    /// matched against the flattened slots, so it rides on the value.
    input_defaults: HashMap<String, InputDefault>,
    /// One diagnostic per default that could NOT be carried across the strip —
    /// a non-literal binding (`= 2*3.14/T`), a leaf-name collision between two
    /// scopes, or a source the strip could not parse at all. Each one means an
    /// input that starts at 0.0 (or is folded to a constant) unless wired, which
    /// must never be silent. Attached to the compile result's
    /// `compile_diagnostics`.
    default_diagnostics: Vec<lunco_doc::Diagnostic>,
}

fn assemble_compile_unit(source: &str, extra_sources: Vec<(String, String)>) -> CompileUnit {
    let (stripped_source, primary_defaults, primary_issues) =
        strip_input_defaults_with_report(source);
    log_parse_failures("the primary document", &primary_issues);
    // The primary document is the only one with an editor buffer behind it, so
    // it is the only one whose diagnostics can be located for click-to-source.
    let mut default_diagnostics: Vec<lunco_doc::Diagnostic> = primary_issues
        .iter()
        .map(|issue| located_default_diagnostic(source, issue))
        .collect();
    let mut input_defaults: HashMap<String, InputDefault> = primary_defaults
        .into_iter()
        .map(|(name, value)| {
            (
                name,
                InputDefault {
                    value,
                    origin: DefaultOrigin::Primary,
                },
            )
        })
        .collect();
    let extras = extra_sources
        .into_iter()
        .map(|(uri, text)| {
            let (stripped, defaults, issues) = strip_input_defaults_with_report(&text);
            log_parse_failures(&uri, &issues);
            // Message-only, since click-to-source targets the primary document.
            for issue in &issues {
                default_diagnostics.push(lunco_doc::Diagnostic::warning(
                    format!("{} (in {uri})", default_issue_message(issue)),
                    None,
                    None,
                ));
            }
            // An extra's numeric defaults ARE seeded. They used to be dropped
            // because "their inputs flatten under instance-qualified names the
            // leaf keys can't address" — true of the KEY, but the fix is to
            // resolve the leaf against the qualified slots
            // (`resolve_default_slots`), not to throw the authored value away
            // and let the slot start at 0.0.
            merge_instanced_defaults(
                &mut input_defaults,
                defaults,
                &uri,
                &mut default_diagnostics,
            );
            (uri, stripped)
        })
        .collect();
    CompileUnit {
        source: stripped_source,
        extras,
        input_defaults,
        default_diagnostics,
    }
}

impl CompileUnit {
    /// Fold the seated libraries' captured `input` defaults into this unit.
    ///
    /// This is the C7 seam: `ModelicaCompiler::load_source_root_in_memory`
    /// strips every library member, so without this the bound `input`s in the
    /// `within LunCo.*` members reach the stepper as runtime slots sitting at
    /// 0.0 instead of at their authored defaults. Seeded as
    /// [`DefaultOrigin::Instanced`] — a library class is only reached by
    /// instantiation.
    fn merge_library_defaults(&mut self, library: &HashMap<String, f64>) {
        if library.is_empty() {
            return;
        }
        merge_instanced_defaults(
            &mut self.input_defaults,
            library.iter().map(|(k, v)| (k.clone(), *v)),
            "a seated library member",
            &mut self.default_diagnostics,
        );
    }
}

/// Fold non-primary defaults into the unit's ONE defaults map.
///
/// The primary document wins any leaf-name clash (its slot is the unqualified
/// one and its value is the one the user is editing), and a clash between two
/// non-primary sources keeps the first. Either way the loser is NAMED rather
/// than silently overwritten.
fn merge_instanced_defaults(
    into: &mut HashMap<String, InputDefault>,
    defaults: impl IntoIterator<Item = (String, f64)>,
    origin_label: &str,
    diagnostics: &mut Vec<lunco_doc::Diagnostic>,
) {
    for (name, value) in defaults {
        // `.copied()` so the map is not borrowed across the arms — the `None`
        // arm inserts into it.
        match into.get(&name).copied() {
            None => {
                into.insert(
                    name,
                    InputDefault {
                        value,
                        origin: DefaultOrigin::Instanced,
                    },
                );
            }
            // The same number from two places costs nothing.
            Some(existing) if existing.value == value => {}
            Some(existing) => {
                let held = match existing.origin {
                    DefaultOrigin::Primary => "the primary document",
                    DefaultOrigin::Instanced => "another member of the source set",
                };
                let held_value = existing.value;
                diagnostics.push(lunco_doc::Diagnostic::warning(
                    format!(
                        "input default `{name}` = {value} in {origin_label} clashes with \
                         {held_value} from {held}. The defaults map is keyed by the leaf \
                         component name (that is what `set_input` addresses), so only one can \
                         be seeded — {held_value} is used. Rename one if they are different \
                         signals."
                    ),
                    None,
                    None,
                ));
            }
        }
    }
}

/// A source the strip could not parse reaches rumoca UNSTRIPPED, so every bound
/// `input` in it is folded to a constant and every wire into those inputs is
/// discarded for the whole session. The diagnostic for it is only a warning (so
/// a compile rumoca accepts is not falsely reported as failed), so the log
/// carries the weight — same reasoning as the `NO runtime inputs at all` error
/// in [`apply_input_defaults_validated`].
fn log_parse_failures(label: &str, issues: &[InputDefaultIssue]) {
    if issues
        .iter()
        .any(|i| matches!(i, InputDefaultIssue::ParseFailed))
    {
        bevy::log::error!(
            "[compile] the bound-`input` strip could not parse {label} — it goes to rumoca \
             UNSTRIPPED, so every `input x = <default>` in it is demoted to a constant, those \
             runtime slots do not exist, and wired values into them are DISCARDED for the whole \
             session."
        );
    }
}

/// The compile-result diagnostic for one [`InputDefaultIssue`], located against
/// the primary document's buffer where the issue carries an offset.
fn located_default_diagnostic(source: &str, issue: &InputDefaultIssue) -> lunco_doc::Diagnostic {
    match issue {
        InputDefaultIssue::Unresolvable { byte_offset, .. } => {
            let (line, col) = lunco_modelica_document::document::core::byte_offset_to_line_col(
                source,
                *byte_offset,
            );
            lunco_doc::Diagnostic::warning(default_issue_message(issue), Some(line), Some(col))
        }
        // Warning severity ON PURPOSE even though this is the worst of the
        // three: rumoca drives its own parse and may compile the file fine, and
        // an Error diagnostic would then make a SUCCESSFUL compile read as
        // failed (`DocDiagnostics::error_message` picks the first Error). The
        // loudness goes to the log instead — see `log_parse_failures`.
        InputDefaultIssue::ParseFailed => {
            lunco_doc::Diagnostic::warning(default_issue_message(issue), None, None)
        }
        InputDefaultIssue::Collision { .. } => {
            lunco_doc::Diagnostic::warning(default_issue_message(issue), None, None)
        }
    }
}

fn default_issue_message(issue: &InputDefaultIssue) -> String {
    match issue {
        InputDefaultIssue::Unresolvable { name, binding, .. } => format!(
            "input `{name} = {binding}`: the default is an expression, not a literal — the \
             binding is stripped so `{name}` stays a runtime input slot, but its default \
             cannot be captured and the slot starts at 0.0 unless wired. Precompute the \
             value or move the expression to a `parameter`."
        ),
        InputDefaultIssue::Collision {
            name,
            kept_scope,
            kept,
            dropped_scope,
            dropped,
        } => format!(
            "input `{name}` is declared with default {kept} in `{kept_scope}` and {dropped} in \
             `{dropped_scope}`. Defaults are keyed by the leaf component name (that is what \
             `set_input` addresses), so only {kept} is seeded and `{dropped_scope}.{name}` starts \
             at {kept} instead of {dropped}. Rename one of them."
        ),
        InputDefaultIssue::ParseFailed => {
            "the bound-`input` strip could not parse this source, so it reaches rumoca \
             UNSTRIPPED: every `input x = <default>` in it is demoted to a constant, the model \
             loses those runtime input slots, and wired values into them are DISCARDED. Fix the \
             syntax error — rumoca may compile the file anyway, in which case this is the only \
             warning you get."
                .to_string()
        }
    }
}

/// `set_input` with the dedup-warn the hot Step path uses: a rejected input
/// means the compiled model exposes no such runtime slot, so the wired value
/// is silently discarded forever — warn ONCE per (entity, name).
#[cfg(not(target_arch = "wasm32"))]
fn set_input_or_warn(
    stepper: &mut LiveStepper,
    rejected_inputs: &mut std::collections::HashSet<(Entity, String)>,
    entity: Entity,
    name: &str,
    val: f64,
) {
    if stepper.set_input(name, val).is_err() && rejected_inputs.insert((entity, name.to_string())) {
        warn!(
            "[modelica] {entity:?} rejected input '{name}' — the \
             compiled model exposes no such runtime slot, so the \
             wired value is DISCARDED and the model keeps its \
             declared default forever. Usual cause: the `.mo` \
             declares `input Real {name} = <default>`, which \
             rumoca demotes to an algebraic (see \
             `strip_input_defaults`)."
        );
    }
}

/// The background worker that owns !Send live steppers, per-entity source
/// metadata, and the two-lane command scheduler. A separate native actor owns
/// the mutable Rumoca session and shared compiled-artifact cache.
///
/// **Native only.** It is spawned on a real `std::thread` (see
/// `ModelicaPlugin::build`) and persists cache entries through the storage
/// boundary. The browser dispatches the *same* commands through
/// [`process_worker_command`] in the `lunica_worker` Web Worker bundle with the
/// source carried in the message.
#[cfg(not(target_arch = "wasm32"))]
pub fn modelica_worker(rx: Receiver<ModelicaCommand>, tx: Sender<ModelicaResult>) {
    let mut steppers: HashMap<Entity, (u64, String, LiveStepper)> = HashMap::default();
    let mut current_sessions: HashMap<Entity, u64> = HashMap::default();
    // Which models declared the realtime promise, from `Compile`. Half of the
    // solver-selection input, kept per entity because every later rebuild — Reset,
    // parameter update, Step
    // auto-init — must resolve the SAME solver as the original compile did.
    let mut realtime_models: std::collections::HashSet<Entity> = Default::default();
    // Inputs the SOLVER rejected, deduped per (entity, name). `set_input` used to
    // be `let _ =` on this per-tick path, which made the single most damaging
    // failure mode in the whole co-sim silent: rumoca demotes a bound `input` to
    // an algebraic, so an input that never became a runtime slot fails here on
    // EVERY tick while the model quietly keeps its declared default forever.
    let mut rejected_inputs: std::collections::HashSet<(Entity, String)> = Default::default();
    // Compiled-artifact cache per entity (M3) — Reset and Step auto-init
    // rebuild steppers from `CachedModel::compiled` without recompiling.
    let mut cached_models: HashMap<Entity, CachedModel> = HashMap::default();
    // DAE compilation and solve-IR preparation are separate caches. The latter
    // is keyed by the structural source revision, solver, and authored
    // overrides so two USD instances do not lower identical networks twice
    // during scene startup.
    let mut prepared_solve_cache = PreparedSolveCache::new();
    // Immutable DAE lowering is dispatched to this bounded pool; the native
    // Rumoca session itself is owned by a separate single-thread actor.
    let mut solve_preparation_pool = SolvePreparationPool::new();
    let mut compiler = CompilerActor::new(solve_preparation_pool.tx.clone());
    // Lock-free publish stream per entity (Phase A of the multi-sim
    // refactor — see `sim_stream.rs`). The UI side holds a clone of
    // the same `Arc<ArcSwap<SimSnapshot>>`; every successful Step
    // publishes a new snapshot so plots render without locking or
    // involving the main thread in per-sample work.
    let mut sim_streams: HashMap<Entity, SimStream> = HashMap::default();
    // M3: cached compiled artifacts are valid only for the library set they
    // were compiled against — every LoadSourceRoot bumps this and thereby
    // invalidates all of them (see `CachedModel::library_gen`).
    let mut library_gen: u64 = 0;
    let mut library_revision: u64 = 0;
    let mut library_defaults: HashMap<String, f64> = HashMap::new();
    // M8: the two scheduling lanes — see `enqueue_command` for the contract.
    let mut compile_lane: VecDeque<ModelicaCommand> = VecDeque::new();
    let mut step_lane: VecDeque<ModelicaCommand> = VecDeque::new();
    let mut pending_compile_works: HashMap<u64, CompileWork> = HashMap::new();
    let mut pending_compiles: HashMap<u64, PendingCompile> = HashMap::new();
    let mut compiler_order = VecDeque::new();
    let mut ready_compiler_completions = HashMap::new();
    let mut pending_source_root_installs = HashSet::new();
    let mut solve_preparation_order = VecDeque::new();
    let mut ready_solve_preparations = HashMap::new();
    let mut source_root_preparation_order = VecDeque::new();
    let mut ready_source_root_preparations = HashMap::new();

    loop {
        // Block only when idle; otherwise just soak up whatever has arrived
        // since the last command, so Steps that landed during a long compile
        // are scheduled ahead of older queued compiles.
        if compile_lane.is_empty() && step_lane.is_empty() {
            if pending_compile_works.is_empty()
                && pending_compiles.is_empty()
                && source_root_preparation_order.is_empty()
                && compiler_order.is_empty()
            {
                match rx.recv() {
                    Ok(cmd) => enqueue_command(cmd, &mut compile_lane, &mut step_lane, &tx),
                    Err(_) => return,
                }
            } else {
                // A live scene keeps sending Step commands while a solve is
                // preparing. Preparation completion must win this wait: an
                // unbiased select can repeatedly choose the hot command
                // channel and leave a finished model uncommitted forever.
                crossbeam_channel::select_biased! {
                    recv(solve_preparation_pool.rx) -> message => match message {
                        Ok(result) => stage_preparation_result(
                            result,
                            &mut ready_solve_preparations,
                            &mut ready_source_root_preparations,
                            &mut ready_compiler_completions,
                        ),
                        Err(_) => return,
                    },
                    recv(rx) -> message => match message {
                        Ok(cmd) => enqueue_command(cmd, &mut compile_lane, &mut step_lane, &tx),
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
            stage_preparation_result(
                result,
                &mut ready_solve_preparations,
                &mut ready_source_root_preparations,
                &mut ready_compiler_completions,
            );
        }
        dispatch_ready_source_roots(
            &mut source_root_preparation_order,
            &mut ready_source_root_preparations,
            &mut compiler,
            &mut compiler_order,
            pending_compile_works.len(),
            &mut pending_source_root_installs,
            &tx,
        );
        commit_ready_compiler_completions(
            &mut compiler_order,
            &mut ready_compiler_completions,
            &mut pending_source_root_installs,
            &mut pending_compiles,
            &current_sessions,
            &mut library_gen,
            &mut library_defaults,
            &mut library_revision,
            &mut prepared_solve_cache,
            &mut solve_preparation_pool,
            &mut solve_preparation_order,
            &mut pending_compile_works,
            &mut steppers,
            &mut cached_models,
            &realtime_models,
            &mut step_lane,
            &tx,
        );
        commit_ready_solve_preparations(
            &mut solve_preparation_order,
            &mut ready_solve_preparations,
            &mut pending_compile_works,
            &current_sessions,
            library_gen,
            &mut prepared_solve_cache,
            &mut steppers,
            &mut cached_models,
            &realtime_models,
            &mut step_lane,
            &tx,
        );

        // Bound command intake so a hot Step producer cannot starve either
        // preparation completions or the scheduler's own fairness points.
        const MAX_COMMANDS_PER_ROUND: usize = 64;
        for _ in 0..MAX_COMMANDS_PER_ROUND {
            let Ok(cmd) = rx.try_recv() else { break };
            enqueue_command(cmd, &mut compile_lane, &mut step_lane, &tx);
        }

        // A result can arrive while the bounded command batch is being
        // admitted. Drain and commit it before selecting the next work round.
        while let Ok(result) = solve_preparation_pool.rx.try_recv() {
            stage_preparation_result(
                result,
                &mut ready_solve_preparations,
                &mut ready_source_root_preparations,
                &mut ready_compiler_completions,
            );
        }
        dispatch_ready_source_roots(
            &mut source_root_preparation_order,
            &mut ready_source_root_preparations,
            &mut compiler,
            &mut compiler_order,
            pending_compile_works.len(),
            &mut pending_source_root_installs,
            &tx,
        );
        commit_ready_compiler_completions(
            &mut compiler_order,
            &mut ready_compiler_completions,
            &mut pending_source_root_installs,
            &mut pending_compiles,
            &current_sessions,
            &mut library_gen,
            &mut library_defaults,
            &mut library_revision,
            &mut prepared_solve_cache,
            &mut solve_preparation_pool,
            &mut solve_preparation_order,
            &mut pending_compile_works,
            &mut steppers,
            &mut cached_models,
            &realtime_models,
            &mut step_lane,
            &tx,
        );
        commit_ready_solve_preparations(
            &mut solve_preparation_order,
            &mut ready_solve_preparations,
            &mut pending_compile_works,
            &current_sessions,
            library_gen,
            &mut prepared_solve_cache,
            &mut steppers,
            &mut cached_models,
            &realtime_models,
            &mut step_lane,
            &tx,
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
                let cached_solve_prepared = needs_init
                    && cached_model_matches
                    && cached_solve_is_prepared(
                        &cached_models,
                        *entity,
                        library_gen,
                        &library_defaults,
                        library_revision,
                        profile_for(*entity, &realtime_models),
                        &prepared_solve_cache,
                    );
                let needs_async_rebuild = cache_needs_compile || !cached_solve_prepared;
                let compiler_load = compiler_order.len()
                    + pending_compile_works.len()
                    + source_root_preparation_order.len();
                let compiler_has_capacity = compiler.can_submit(compiler_load);
                if needs_init
                    && cached_model_matches
                    && (source_roots_pending || (needs_async_rebuild && !compiler_has_capacity))
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
            compiler.can_submit(
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
            // channel so a finished participant always reaches `finish_compile_work`.
            crossbeam_channel::select_biased! {
                recv(solve_preparation_pool.rx) -> message => match message {
                    Ok(result) => stage_preparation_result(
                        result,
                        &mut ready_solve_preparations,
                        &mut ready_source_root_preparations,
                        &mut ready_compiler_completions,
                    ),
                    Err(_) => return,
                },
                recv(rx) -> message => match message {
                    Ok(cmd) => enqueue_command(cmd, &mut compile_lane, &mut step_lane, &tx),
                    Err(_) => return,
                },
            }
            continue;
        }

        for cmd in to_process {
            let tx_inner = tx.clone();
            let panic_entity = match &cmd {
                ModelicaCommand::Step { entity, .. }
                | ModelicaCommand::Compile { entity, .. }
                | ModelicaCommand::UpdateParameters { entity, .. }
                | ModelicaCommand::Reset { entity, .. }
                | ModelicaCommand::Despawn { entity } => Some(*entity),
                ModelicaCommand::LoadSourceRoot { .. } => None,
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
                    ModelicaCommand::Reset { entity, session_id } => {
                        current_sessions.insert(entity, session_id);

                        // Rebuild from the cached DAE. A changed library set
                        // recompiles through the Rumoca actor; a missing solve
                        // model is loaded/lowered through the preparation pool.
                        if let Some(cached) = native_cached_rebuild(
                            &cached_models,
                            entity,
                            library_gen,
                            &library_defaults,
                            library_revision,
                        ) {
                            if !cached.artifact_is_valid(library_gen) {
                                let (pending, unit) = cached.into_pending_compile(
                                    entity,
                                    session_id,
                                    library_gen,
                                    CompileIntent::Reset,
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
                            submit_cached_solve_preparation(
                                cached,
                                entity,
                                session_id,
                                library_gen,
                                profile_for(entity, &realtime_models),
                                CompileIntent::Reset,
                                &mut solve_preparation_pool,
                                &mut solve_preparation_order,
                                &mut pending_compile_works,
                                &mut steppers,
                                &mut cached_models,
                                &realtime_models,
                                &mut prepared_solve_cache,
                                &mut step_lane,
                                &tx_inner,
                            );
                        } else {
                            steppers.remove(&entity);
                            let _ = tx_inner.send(reset_ok(
                                entity,
                                session_id,
                                Vec::new(),
                                Vec::new(),
                                "Reset complete (no cached model).",
                            ));
                        }
                    }
                    ModelicaCommand::UpdateParameters {
                        entity,
                        session_id,
                        model_name,
                        source,
                    } => {
                        if session_id < *current_sessions.get(&entity).unwrap_or(&0) {
                            let _ = tx_inner.send(result_ok(entity, session_id));
                            return;
                        }
                        current_sessions.insert(entity, session_id);

                        // Re-seat under the SAME session URI the model was first
                        // compiled with. The actor owns the mutable Rumoca session;
                        // the command owner records this continuation and remains
                        // available for other entities while it compiles.
                        let doc_uri = cached_models
                            .get(&entity)
                            .map(|c| c.doc_uri.clone())
                            .unwrap_or_else(|| model_name.clone());
                        let pending = PendingCompile {
                            entity,
                            session_id,
                            cancelled: false,
                            model_name,
                            source: source.clone(),
                            doc_uri,
                            raw_extras: Vec::new(),
                            parameter_overrides: Vec::new(),
                            library_gen,
                            intent: CompileIntent::UpdateParameters,
                        };
                        let unit = assemble_compile_unit(&source, Vec::new());
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
                        entity,
                        session_id,
                        model_name,
                        source,
                        doc_uri,
                        extra_sources,
                        parameter_overrides,
                        stream,
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
                        if let Some(stream) = stream {
                            // Register the new lock-free publish target
                            // AND reset the previous snapshot so stale
                            // history from a prior compile doesn't bleed
                            // into the new model's horizon.
                            stream.store(Arc::new(SimSnapshot::empty_at_zero()));
                            sim_streams.insert(entity, stream);
                        }

                        // Keep the raw sibling docs for cache-invalidating
                        // rebuilds, but submit the immutable source set to the
                        // single-owner Rumoca actor so this worker can keep
                        // servicing Steps for other entities.
                        let raw_extras = extra_sources.clone();
                        let unit = assemble_compile_unit(&source, extra_sources);
                        let pending = PendingCompile {
                            entity,
                            session_id,
                            cancelled: false,
                            model_name: model_name.clone(),
                            source,
                            doc_uri: doc_uri.clone(),
                            raw_extras,
                            parameter_overrides,
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
                                pending_compiles.insert(operation_id, pending);
                            }
                            Err((error, unit)) => {
                                let mut result = result_ok(entity, session_id);
                                result.error = Some(format!("Compiler Error: {error}"));
                                result.compile_diagnostics = unit.default_diagnostics;
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
                                if let Some(cached) = native_cached_rebuild(
                                    &cached_models,
                                    entity,
                                    library_gen,
                                    &library_defaults,
                                    library_revision,
                                ) {
                                    if !cached.artifact_is_valid(library_gen) {
                                        let compiler_load = compiler_order.len()
                                            + pending_compile_works.len()
                                            + source_root_preparation_order.len();
                                        if !compiler.can_submit(compiler_load) {
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
                                        &mut solve_preparation_order,
                                        &mut pending_compile_works,
                                        &mut steppers,
                                        &mut cached_models,
                                        &realtime_models,
                                        &mut prepared_solve_cache,
                                        &mut step_lane,
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

                        if let Some((s_id, _, stepper)) = steppers.get_mut(&entity) {
                            if *s_id == session_id {
                                for (name, val) in inputs {
                                    set_input_or_warn(
                                        stepper,
                                        &mut rejected_inputs,
                                        entity,
                                        &name,
                                        val,
                                    );
                                }
                                // Macro step: integrate the requested `dt` — the
                                // gap between the model's clock and the world's —
                                // as a fixed ladder of micro-steps.
                                let step_err = integrate_macro_step(stepper, dt)
                                    .err()
                                    .map(|error| error.to_string())
                                    .or_else(|| {
                                        validate_step_completion(stepper.time(), stop_time).err()
                                    });
                                if let Some(e) = step_err {
                                    let mut r = step_result_ok(entity, session_id, step_id);
                                    r.new_time = stepper.time();
                                    r.step_id = Some(step_id);
                                    // Runtime solver blow-up: `SimulationDiagnosticError`
                                    // Display is human-readable (the `Solver` variant
                                    // carries no source span, so it stays unlocated).
                                    r.error = Some(format!("Solver Error: {e}"));
                                    let _ = tx_inner.send(r);
                                    steppers.remove(&entity);
                                } else {
                                    // `state()` reconstructs algebraics / outputs via
                                    // `EliminationResult` and also includes inputs, so
                                    // this single call supersedes the old two-loop
                                    // variable_names + input_names collection.
                                    let outputs = collect_stepper_observables(stepper);
                                    let new_time = stepper.time();
                                    // Publish the immutable stream projection
                                    // for same-address-space readers. The
                                    // result still carries `outputs` because
                                    // it is the transport boundary for the
                                    // main thread and wasm worker contexts.
                                    if let Some(stream) = sim_streams.get(&entity) {
                                        let prev = stream.load();
                                        let next = SimSnapshot::advance(&prev, new_time, &outputs);
                                        stream.store(Arc::new(next));
                                    }
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
                        sim_streams.remove(&entity);
                        for work in pending_compile_works.values_mut() {
                            if work.entity == entity {
                                work.cancelled = true;
                            }
                        }
                        for work in pending_compiles.values_mut() {
                            if work.entity == entity {
                                work.cancelled = true;
                            }
                        }
                    }
                    ModelicaCommand::LoadSourceRoot { id, payload } => {
                        let operation_id = solve_preparation_pool.submit_source_root(id, payload);
                        source_root_preparation_order.push_back(operation_id);
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
                if let Some(entity) = panic_entity {
                    steppers.remove(&entity);
                    cached_models.remove(&entity);
                    sim_streams.remove(&entity);
                    current_sessions.remove(&entity);
                    realtime_models.remove(&entity);
                    rejected_inputs.retain(|(candidate, _)| *candidate != entity);
                }
                let _ = tx.send(panic_result);
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
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cmd_entity(cmd: &ModelicaCommand) -> Entity {
    match cmd {
        ModelicaCommand::Step { entity, .. } => *entity,
        ModelicaCommand::Compile { entity, .. } => *entity,
        ModelicaCommand::UpdateParameters { entity, .. } => *entity,
        ModelicaCommand::Reset { entity, .. } => *entity,
        ModelicaCommand::Despawn { entity } => *entity,
        // Source-root loads aren't entity-scoped; the squash check
        // never reaches this branch (LoadSourceRoot returns false
        // from is_squashable), so the placeholder is only consulted
        // by the result-fence logic which keys on a different
        // structural shape.
        ModelicaCommand::LoadSourceRoot { .. } => Entity::PLACEHOLDER,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cmd_session(cmd: &ModelicaCommand) -> u64 {
    match cmd {
        ModelicaCommand::Step { session_id, .. } => *session_id,
        ModelicaCommand::Compile { session_id, .. } => *session_id,
        ModelicaCommand::UpdateParameters { session_id, .. } => *session_id,
        ModelicaCommand::Reset { session_id, .. } => *session_id,
        ModelicaCommand::Despawn { .. } => 0,
        ModelicaCommand::LoadSourceRoot { .. } => 0,
    }
}

/// Returns true if two consecutive commands can be squashed (same type, same entity).
///
/// Squashing prevents "back-pressure" lag when the UI sends rapid updates
/// (e.g., dragging a parameter slider). Only the latest value is processed —
/// the dropped command is acked with a synthetic success (`result_ok`).
///
/// **`Step` is NOT squashable** (A5). Squashing is only sound for commands that
/// are *idempotent setpoints*: `UpdateParameters` (the last value wins — an
/// earlier slider position has no lasting meaning) and `Compile` (the last
/// source wins). A `Step` is an **integration**, not a setpoint: collapsing two
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
            ModelicaCommand::UpdateParameters { entity: e1, .. },
            ModelicaCommand::UpdateParameters { entity: e2, .. },
        ) => e1 == e2,
        (
            ModelicaCommand::Compile { entity: e1, .. },
            ModelicaCommand::Compile { entity: e2, .. },
        ) => e1 == e2,
        _ => false,
    }
}

// =============================================================================
// WebAssembly Web Worker state (wasm32 only - no native thread support in browser)
// =============================================================================
//
// Why this exists:
//   - std::thread::spawn panics on wasm32-unknown-unknown (no OS thread support)
//   - Web Workers are not available from Rust/wasm-bindgen without additional
//     tooling (wasm-bindgen-rayon, etc.)
//   - Instead, we process one simulation command per frame in a Bevy system.
//     This keeps the UI responsive while still running full Modelica simulation.
//
// Trade-offs:
//   - One command per frame limits throughput (fine for interactive use)
//   - No back-pressure: commands pile up in the channel if the worker falls behind
//   - All state lives in a Resource, so it resets on page reload (by design)

/// Simulation state owned by the wasm Web Worker.
/// Mirrors the local variables in `modelica_worker` on desktop.
///
/// `pub` so the off-thread worker bin (`bin/lunica_worker.rs`) can own
/// one of these directly. The fields stay private — only the type itself
/// crosses crate boundaries.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct ModelicaWorkerState {
    steppers: HashMap<Entity, (u64, String, LiveStepper)>,
    sim_streams: HashMap<Entity, SimStream>,
    current_sessions: HashMap<Entity, u64>,
    cached_models: HashMap<Entity, CachedModel>,
    compiled_artifacts: HashMap<u64, Box<rumoca_compile::compile::DaeCompilationResult>>,
    prepared_solve_cache: PreparedSolveCache,
    compiler: Option<ModelicaCompiler>,
    /// Models that declared the realtime promise — the same per-entity fact the
    /// native worker keeps, so wasm resolves the same solver for the same model.
    realtime_models: std::collections::HashSet<Entity>,
    /// M3: bumped on every LoadSourceRoot (and compiler reset) to invalidate
    /// cached compiled artifacts — same contract as the native worker's local.
    library_gen: u64,
}

#[cfg(target_arch = "wasm32")]
impl ModelicaWorkerState {
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
pub fn process_worker_command<F: FnMut(ModelicaResult)>(
    state: &mut ModelicaWorkerState,
    cmd: ModelicaCommand,
    mut send: F,
) {
    let w = state;
    match cmd {
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
                    if let Some(rb) = rebuild {
                        if let Ok(comp_res) = rb.outcome {
                            if let Ok(mut s) = build_stepper(
                                &comp_res,
                                profile_for(entity, &w.realtime_models),
                                &rb.parameter_overrides,
                                rb.unit_key,
                                rb.library_revision,
                                &mut w.prepared_solve_cache,
                            ) {
                                apply_input_defaults_validated(
                                    &mut s,
                                    &rb.unit.input_defaults,
                                    "Compile",
                                );
                                for (name, val) in &inputs {
                                    let _ = s.set_input(name, *val);
                                }
                                w.steppers
                                    .insert(entity, (session_id, model_name.clone(), s));
                            }
                        }
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

            if let Some((s_id, _, stepper)) = w.steppers.get_mut(&entity) {
                if *s_id == session_id {
                    for (name, val) in &inputs {
                        let _ = stepper.set_input(name, *val);
                    }
                    // Same macro-step ladder as the native worker.
                    let step_err = integrate_macro_step(stepper, dt)
                        .err()
                        .map(|error| error.to_string())
                        .or_else(|| validate_step_completion(stepper.time(), stop_time).err());

                    if let Some(e) = step_err {
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
                            ..Default::default()
                        });
                        w.steppers.remove(&entity);
                    } else {
                        let outputs = collect_stepper_observables(stepper);
                        if let Some(stream) = w.sim_streams.get(&entity) {
                            let prev = stream.load();
                            let next = SimSnapshot::advance(&prev, stepper.time(), &outputs);
                            stream.store(Arc::new(next));
                        }
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
                            ..Default::default()
                        });
                    }
                } else {
                    send(step_result_ok(entity, session_id, step_id));
                }
            } else {
                // No stepper for this entity. The Bevy-side
                // `spawn_modelica_requests` is supposed to catch this
                // and dispatch a Compile first; if we got here the
                // user pressed Run on a never-compiled model AND the
                // auto-compile hook didn't fire (e.g. doc id is
                // missing). Surface a message that tells the user
                // what to do next instead of "Sim engine failed to
                // start." which doesn't.
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
            entity,
            session_id,
            model_name,
            source,
            doc_uri,
            extra_sources,
            parameter_overrides,
            stream,
            realtime_safe,
        } => {
            if realtime_safe {
                w.realtime_models.insert(entity);
            } else {
                w.realtime_models.remove(&entity);
            }
            if let Some(stream) = stream {
                stream.store(Arc::new(SimSnapshot::empty_at_zero()));
                w.sim_streams.insert(entity, stream);
            }
            w.current_sessions.insert(entity, session_id);
            // Raw sibling docs for the cache — see the native Compile arm.
            let raw_extras = extra_sources.clone();
            let mut unit = assemble_compile_unit(&source, extra_sources);

            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            unit.merge_library_defaults(compiler.library_input_defaults());
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
                    let stepper_result = build_stepper(
                        &comp_res,
                        profile_for(entity, &w.realtime_models),
                        &parameter_overrides,
                        prepared_unit_hash(&model_name, &doc_uri, &unit, w.library_gen),
                        Some(compiler.library_revision()),
                        &mut w.prepared_solve_cache,
                    );
                    match stepper_result {
                        Ok(mut stepper) => {
                            apply_input_defaults_validated(
                                &mut stepper,
                                &unit.input_defaults,
                                "Compile",
                            );
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            let unit_hash = compile_unit_hash(&model_name, &doc_uri, &unit);
                            w.cached_models.insert(
                                entity,
                                CachedModel {
                                    model_name: model_name.clone(),
                                    source: Arc::from(source.clone()),
                                    extra_sources: raw_extras,
                                    parameter_overrides,
                                    doc_uri: doc_uri.clone(),
                                    compiled: comp_res.clone(),
                                    unit_hash,
                                    library_gen: w.library_gen,
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
                                    // Unresolvable input defaults (non-literal bindings)
                                    // surface even on a green compile — that is exactly
                                    // when they'd otherwise run at 0.0 in silence.
                                    compile_diagnostics: unit.default_diagnostics,
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
                    let diags = compiler.compile_diagnostics(&model_name, &doc_uri);
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
        ModelicaCommand::Reset { entity, session_id } => {
            w.current_sessions.insert(entity, session_id);

            // M3: rebuild from the cached compiled artifact — instant unless a
            // LoadSourceRoot / compiler reset invalidated it.
            let rebuild = {
                let mut backend = InlineCompileBackend {
                    compiler: &mut w.compiler,
                    artifacts: &mut w.compiled_artifacts,
                };
                rebuild_from_cache(&mut w.cached_models, &mut backend, entity, w.library_gen)
            };
            if let Some(rb) = rebuild {
                match rb.outcome {
                    Ok(comp_res) => {
                        if let Ok(mut stepper) = build_stepper(
                            &comp_res,
                            profile_for(entity, &w.realtime_models),
                            &rb.parameter_overrides,
                            rb.unit_key,
                            rb.library_revision,
                            &mut w.prepared_solve_cache,
                        ) {
                            apply_input_defaults_validated(
                                &mut stepper,
                                &rb.unit.input_defaults,
                                "Compile",
                            );
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            w.steppers
                                .insert(entity, (session_id, rb.model_name.clone(), stepper));
                            if let Some(stream) = w.sim_streams.get(&entity) {
                                stream.store(Arc::new(SimSnapshot::empty_at_zero()));
                            }
                            send(reset_ok(
                                entity,
                                session_id,
                                symbols,
                                input_names,
                                "Reset complete.",
                            ));
                        } else {
                            send(ModelicaResult {
                                entity,
                                session_id,
                                new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(),
                                error: Some("Stepper init failed".to_string()),
                                log_message: None,
                                is_new_model: false,
                                is_parameter_update: false,
                                is_reset: true,
                                detected_input_names: Vec::new(),
                                ..Default::default()
                            });
                        }
                    }
                    Err(e) => {
                        send(ModelicaResult {
                            entity,
                            session_id,
                            new_time: 0.0,
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(),
                            error: Some(format!("Reset compile error: {e}")),
                            log_message: None,
                            is_new_model: false,
                            is_parameter_update: false,
                            is_reset: true,
                            detected_input_names: Vec::new(),
                            compile_diagnostics: rb.compile_diagnostics,
                            ..Default::default()
                        });
                    }
                }
            } else {
                w.steppers.remove(&entity);
                send(reset_ok(
                    entity,
                    session_id,
                    Vec::new(),
                    Vec::new(),
                    "Reset complete (no cached model).",
                ));
            }
        }
        ModelicaCommand::UpdateParameters {
            entity,
            session_id,
            model_name,
            source,
        } => {
            if session_id < *w.current_sessions.get(&entity).unwrap_or(&0) {
                send(result_ok(entity, session_id));
                return;
            }
            w.current_sessions.insert(entity, session_id);
            let mut unit = assemble_compile_unit(&source, Vec::new());

            // Re-seat under the model's original session URI (see the threaded
            // handler) so the reused session never holds it under two filenames.
            let doc_uri = w
                .cached_models
                .get(&entity)
                .map(|c| c.doc_uri.clone())
                .unwrap_or_else(|| model_name.clone());

            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            unit.merge_library_defaults(compiler.library_input_defaults());
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
                        &[],
                        prepared_unit_hash(&model_name, &doc_uri, &unit, w.library_gen),
                        Some(compiler.library_revision()),
                        &mut w.prepared_solve_cache,
                    ) {
                        Ok(mut stepper) => {
                            apply_input_defaults_validated(
                                &mut stepper,
                                &unit.input_defaults,
                                "Compile",
                            );
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            let unit_hash = compile_unit_hash(&model_name, &doc_uri, &unit);
                            w.cached_models.insert(
                                entity,
                                CachedModel {
                                    model_name: model_name.clone(),
                                    source: Arc::from(source.clone()),
                                    // Parameter substitution rewrites one doc —
                                    // compiled without extras, matching above.
                                    extra_sources: Vec::new(),
                                    parameter_overrides: Vec::new(),
                                    doc_uri: doc_uri.clone(),
                                    compiled: comp_res.clone(),
                                    unit_hash,
                                    library_gen: w.library_gen,
                                },
                            );

                            w.steppers
                                .insert(entity, (session_id, model_name.clone(), stepper));
                            send(ModelicaResult {
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
                                compile_diagnostics: unit.default_diagnostics,
                                ..Default::default()
                            });
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
                        compile_diagnostics: compiler.compile_diagnostics(&model_name, &doc_uri),
                        ..Default::default()
                    });
                }
            }
        }
        ModelicaCommand::Despawn { entity } => {
            w.steppers.remove(&entity);
            w.cached_models.remove(&entity);
            w.sim_streams.remove(&entity);
        }
        ModelicaCommand::LoadSourceRoot { id, payload } => {
            // Wasm path: matches the native handler. The worker thread merges the
            // library into its session after the host has supplied source text.
            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            let t0 = web_time::Instant::now();
            let report =
                compiler.install_source_root(prepare_source_root_payload(id.clone(), payload));
            if report.diagnostics.is_empty() && report.inserted_file_count > 0 {
                w.library_gen += 1;
                w.compiled_artifacts.clear();
                w.prepared_solve_cache.disable_persistent();
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
            send(ModelicaResult {
                loaded_source_root_id: Some(id),
                error: err,
                ..Default::default()
            });
        }
    }
}

#[cfg(test)]
mod macro_step_tests {
    use super::bridge::plan_macro_step;
    use super::*;

    /// Stand-in for the worker: integrate what the worker WOULD integrate for a
    /// requested `dt` — an integer number of fixed micro-steps — and return the
    /// model's new own-clock value. This is the same arithmetic
    /// [`integrate_macro_step`] performs, without a `SimulationSession`.
    fn worker_integrate(current_time: f64, dt: f64) -> f64 {
        current_time + micro_steps_for(dt) as f64 * LIVE_MICRO_DT
    }

    /// Drive N fixed ticks, resolving the in-flight step after `latency_ticks`
    /// ticks (0 = the worker answers within the same tick). Returns
    /// `(model_time, world_time)`.
    ///
    /// `latency_ticks` stands in for "how many fixed ticks the worker takes" —
    /// i.e. exactly the axis that used to be the RENDER FRAME. The contract is
    /// that it must not change the model's time.
    fn run_ticks(ticks: u32, latency_ticks: u32) -> (f64, f64) {
        let fixed_dt = lunco_core_runtime::SECS_PER_TICK;
        let mut model_time = 0.0_f64;
        let mut target_time = 0.0_f64;
        // (dt, ticks-remaining-until-it-lands)
        let mut in_flight: Option<(f64, u32)> = None;

        for _ in 0..ticks {
            // `handle_modelica_responses` — the result lands, model clock moves.
            if let Some((dt, 0)) = in_flight {
                model_time = worker_integrate(model_time, dt);
                in_flight = None;
            } else if let Some((dt, n)) = in_flight {
                in_flight = Some((dt, n - 1));
            }

            // `spawn_modelica_requests` — one fixed tick of world time.
            target_time += fixed_dt;
            if let Some(dt) = plan_macro_step(target_time, model_time, in_flight.is_some()) {
                in_flight = Some((dt, latency_ticks));
            }
        }
        // The world stops; let the model catch up. While the world is MOVING the
        // model is legitimately up to (latency + 1) ticks behind — that is the
        // in-flight step plus the tick that elapsed while it was in flight, and
        // it is bounded, not cumulative. The A3 contract is that the deficit is
        // never DISCARDED: once the world stops advancing, the model converges on
        // it. So drain to convergence rather than landing a single step, which is
        // what `spawn_modelica_requests` does on any tick the world is paused.
        if let Some((dt, _)) = in_flight.take() {
            model_time = worker_integrate(model_time, dt);
        }
        while let Some(dt) = plan_macro_step(target_time, model_time, false) {
            model_time = worker_integrate(model_time, dt);
        }
        (model_time, target_time)
    }

    /// **The A3 regression test.** Model time must equal world time after N
    /// ticks REGARDLESS of how long the worker (read: the render frame) takes to
    /// answer. Before the fix, a worker/frame latency of k ticks made the model
    /// run k+1× too slow, permanently.
    #[test]
    fn model_time_tracks_world_time_at_any_worker_latency() {
        const TICKS: u32 = 600; // 10 s of world time at 60 Hz
        let (_, world) = run_ticks(TICKS, 0);

        for latency in [0_u32, 1, 2, 5, 10] {
            let (model, w) = run_ticks(TICKS, latency);
            assert!(
                (w - world).abs() < 1e-9,
                "world clock must not depend on latency"
            );
            // Converged to within one micro-step (the rounding residual), NOT
            // to within a factor of (latency + 1).
            let err = (model - world).abs();
            assert!(
                err <= LIVE_MICRO_DT,
                "latency={latency}: model={model:.6} world={world:.6} err={err:.6} \
                 (> one micro-step: the model is losing time)"
            );
        }
    }

    /// The specific pre-fix failure: a worker that answers every OTHER tick used
    /// to halve the model's rate. Assert we no longer lose ~half the time.
    #[test]
    fn every_other_tick_worker_does_not_halve_model_time() {
        let (model, world) = run_ticks(600, 1);
        assert!(
            model > world * 0.99,
            "model={model:.4} world={world:.4}: model is running slow (half-rate regression)"
        );
    }

    /// A long stall (worker busy for 120 ticks — a compile) must be CAUGHT UP,
    /// not lost. The per-step clamp bounds each macro step; several ticks close
    /// the gap.
    #[test]
    fn stalled_model_catches_up_instead_of_losing_time() {
        let fixed_dt = lunco_core_runtime::SECS_PER_TICK;
        let mut model_time = 0.0_f64;
        let mut target_time = 0.0_f64;

        // 120 ticks of world time pass with the worker unavailable.
        for _ in 0..120 {
            target_time += fixed_dt;
        }
        assert!(model_time < target_time - 1.0);

        // Now the worker answers immediately, one macro step per tick.
        for _ in 0..200 {
            target_time += fixed_dt;
            if let Some(dt) = plan_macro_step(target_time, model_time, false) {
                assert!(
                    dt <= MAX_MACRO_STEP_DT + 1e-12,
                    "macro step must stay clamped: {dt}"
                );
                model_time = worker_integrate(model_time, dt);
            }
        }
        assert!(
            (model_time - target_time).abs() <= LIVE_MICRO_DT,
            "model={model_time:.4} world={target_time:.4}: the 2 s stall was never caught up"
        );
    }

    /// The deficit is clamped per step (so one long gap can't hand the solver a
    /// 10 s macro step), but never discarded.
    #[test]
    fn macro_step_is_clamped_but_deficit_survives() {
        let dt = plan_macro_step(10.0, 0.0, false).expect("a 10 s deficit must request a step");
        assert!((dt - MAX_MACRO_STEP_DT).abs() < 1e-12);
        // In flight ⇒ no second step, but the deficit is still there next tick.
        assert!(plan_macro_step(10.0, 0.0, true).is_none());
    }

    /// At the communication point, nothing is dispatched (and a sub-micro-step
    /// overshoot is absorbed rather than integrated).
    #[test]
    fn no_step_at_the_communication_point() {
        assert!(plan_macro_step(1.0, 1.0, false).is_none());
        assert!(plan_macro_step(1.0, 1.0 + LIVE_MICRO_DT, false).is_none());
        assert!(plan_macro_step(1.0 + LIVE_MICRO_DT, 1.0, false).is_some());
    }

    /// The micro-step ladder is an integer function of `dt` alone — same on
    /// every peer, clamped, and never zero for a positive `dt`.
    #[test]
    fn micro_step_ladder_is_deterministic_and_clamped() {
        assert_eq!(micro_steps_for(0.0), 0);
        assert_eq!(micro_steps_for(-1.0), 0);
        assert_eq!(micro_steps_for(LIVE_MICRO_DT), 1);
        assert_eq!(micro_steps_for(lunco_core_runtime::SECS_PER_TICK), 3);
        assert_eq!(micro_steps_for(2.0 * lunco_core_runtime::SECS_PER_TICK), 6);
        assert_eq!(micro_steps_for(1e-9), 1);
        assert_eq!(micro_steps_for(1_000.0), MAX_MICRO_STEPS_PER_MACRO);
    }

    /// **A5.** `Step` is an integration, not a setpoint: two queued `Step`s must
    /// NEVER collapse (the dropped one used to be acked with a fake success,
    /// deleting `dt` of model time). Setpoint-shaped commands still squash.
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
        };
        assert!(
            is_squashable(&params(), &params()),
            "UpdateParameters is an idempotent setpoint — it SHOULD squash"
        );
    }

    #[test]
    fn communication_schedule_default_is_valid_and_reanchors_exactly() {
        let mut model = ModelicaModel {
            current_time: 3.25,
            ..Default::default()
        };

        assert_eq!(
            model.validated_communication_period_secs().unwrap(),
            lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS
        );
        model.reset_communication_schedule().unwrap();
        assert_eq!(
            model.next_communication_time,
            3.25 + lunco_modelica_runtime::DEFAULT_COMMUNICATION_PERIOD_SECS
        );
    }

    #[test]
    fn invalid_authored_communication_schedule_is_not_defaulted() {
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            0.0,
            LIVE_MICRO_DT * 0.5,
            LIVE_MICRO_DT,
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
                MAX_MACRO_STEP_DT + LIVE_MICRO_DT
            )
            .is_err()
        );
    }

    #[test]
    fn panic_results_preserve_command_lifecycle_identity() {
        let entity = Entity::from_raw_u32(7).expect("valid test entity");
        let compile = panic_result_for_command(
            &ModelicaCommand::Compile {
                entity,
                session_id: 4,
                model_name: "Balloon".into(),
                source: String::new(),
                realtime_safe: false,
                doc_uri: "balloon.mo".into(),
                extra_sources: Vec::new(),
                parameter_overrides: Vec::new(),
                stream: None,
            },
            "panic",
        );
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
        assert_eq!(step.step_id, Some(19));
        assert!(!step.is_new_model);
        assert!(step.error.is_some());

        let root = panic_result_for_command(
            &ModelicaCommand::LoadSourceRoot {
                id: "Modelica".into(),
                payload: LoadSourceRootPayload::InMemory {
                    label: "test".into(),
                    files: Vec::new(),
                },
            },
            "panic",
        );
        assert_eq!(root.loaded_source_root_id.as_deref(), Some("Modelica"));
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
        shared_compile_hash(model, &unit, uri, 4)
    }

    fn prepared_hash_of(model: &str, source: &str, uri: &str) -> u64 {
        let unit = assemble_compile_unit(source, Vec::new());
        prepared_unit_hash(model, uri, &unit, 4)
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

    /// The cross-entity cache deliberately ignores document identity: the
    /// equations, not the USD instance URI, determine the compiled DAE.
    #[test]
    fn shared_hash_is_instance_independent() {
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
    fn generated_shared_hash_ignores_instance_root_identity() {
        let first = shared_hash_of(
            "Traverse_x2f_rocker__bogie__101_System",
            "model Traverse_x2f_rocker__bogie__101_System\n  input Real throttle;\nend Traverse_x2f_rocker__bogie__101_System;\nannotation(Documentation(info=\"rocker_bogie_101 network\"));",
            "generated://Traverse_x2f_rocker__bogie__101_System.mo",
            Vec::new(),
        );
        let second = shared_hash_of(
            "Traverse_x2f_rocker__bogie__202_System",
            "model Traverse_x2f_rocker__bogie__202_System\n  input Real throttle;\nend Traverse_x2f_rocker__bogie__202_System;\nannotation(Documentation(info=\"rocker_bogie_202 network\"));",
            "generated://Traverse_x2f_rocker__bogie__202_System.mo",
            Vec::new(),
        );
        assert_eq!(
            first, second,
            "generated instance identity must not defeat structural DAE reuse"
        );
        assert_eq!(
            prepared_hash_of(
                "Traverse_x2f_rocker__bogie__101_System",
                "model Traverse_x2f_rocker__bogie__101_System\n  input Real throttle;\nend Traverse_x2f_rocker__bogie__101_System;\nannotation(Documentation(info=\"rocker_bogie_101 network\"));",
                "generated://Traverse_x2f_rocker__bogie__101_System.mo",
            ),
            prepared_hash_of(
                "Traverse_x2f_rocker__bogie__202_System",
                "model Traverse_x2f_rocker__bogie__202_System\n  input Real throttle;\nend Traverse_x2f_rocker__bogie__202_System;\nannotation(Documentation(info=\"rocker_bogie_202 network\"));",
                "generated://Traverse_x2f_rocker__bogie__202_System.mo",
            ),
            "generated instance identity must not defeat persistent solve-IR reuse"
        );
        assert_ne!(
            shared_hash_of(
                "Traverse_x2f_rocker__bogie__101_System",
                "model Traverse_x2f_rocker__bogie__101_System\n  parameter Real retained_literal = 101;\nend Traverse_x2f_rocker__bogie__101_System;\nannotation(Documentation(info=\"rocker_bogie_101 network\"));",
                "generated://Traverse_x2f_rocker__bogie__101_System.mo",
                Vec::new(),
            ),
            shared_hash_of(
                "Traverse_x2f_rocker__bogie__202_System",
                "model Traverse_x2f_rocker__bogie__202_System\n  parameter Real retained_literal = 202;\nend Traverse_x2f_rocker__bogie__202_System;\nannotation(Documentation(info=\"rocker_bogie_202 network\"));",
                "generated://Traverse_x2f_rocker__bogie__202_System.mo",
                Vec::new(),
            ),
            "generated normalization must not rewrite equation literals"
        );
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

// ===========================================================================
