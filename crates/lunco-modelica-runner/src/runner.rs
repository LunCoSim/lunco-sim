//! Experiment runner — modelica/rumoca binding for the
//! [`lunco_experiments::ExperimentRunner`] trait.
//!
//! See `docs/architecture/25-experiments.md` for the design.
//!
//! Run values enter `SimOptions::param_overrides` and `initial_inputs` at
//! simulation preparation. The checked compiled DAE remains immutable and
//! shared across parameter sweeps. Rumoca validates targets, recomputes
//! parameter dependents, and initializes the model with the admitted inputs.

#[cfg(target_arch = "wasm32")]
use lunco_core_runtime::LockExt;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use crossbeam_channel::{Sender, TryRecvError, unbounded};
use lunco_experiments::{
    Experiment, ExperimentId, ExperimentRegistry, ExperimentRunner, ExperimentSettings, ModelRef,
    ParamPath, ParamValue, RunBounds, RunCancelled, RunCompleted, RunFailed, RunHandle, RunMeta,
    RunProgress, RunResult, RunStatus, RunUpdate,
};
use rumoca_compile::compile::Dae;
/// Immutable Modelica source and runtime owner supplied at run admission.
#[derive(Clone, Debug)]
pub struct ModelSource {
    pub model_name: String,
    pub source: String,
    pub filename: String,
    pub extras: Vec<(String, String)>,
    pub runtime: lunco_workspace::DocumentRuntimeOwner,
    pub result_limits: lunco_experiments::RunResultLimits,
}

/// Push the persisted `experiments.max_parallel` into the live runner.
/// Change-driven: `is_changed()` is true on the frame the section is first
/// inserted (so this also applies the on-disk value at startup) and again
/// whenever the settings UI edits it. Per the lazy-systems convention it
/// bails when the section hasn't changed.
pub fn apply_experiment_settings(
    settings: Res<ExperimentSettings>,
    runner: Res<crate::ModelicaRunnerResource>,
) {
    if !settings.is_changed() {
        return;
    }
    let n = settings.resolved_max_parallel();
    runner.0.set_max_parallel(n);
    bevy::log::info!("[experiments] max parallel runs = {n}");
}

/// A run snapshotted and waiting for (or being handed) a scheduler slot.
/// Captures everything `run_inner` / the worker dispatch needs so a
/// queued run can start later without re-touching the experiment record.
struct QueuedJob {
    run_id: ExperimentId,
    source: ModelSource,
    overrides: BTreeMap<ParamPath, ParamValue>,
    inputs: BTreeMap<ParamPath, ParamValue>,
    bounds: RunBounds,
    /// Update channel the `RunHandle` consumer drains. Created in
    /// `run_fast` so a queued handle is valid before the job starts.
    tx: Sender<RunUpdate>,
    /// Per-run cancel flag. Set true by the handle's cancel hook; checked
    /// at start (queued-cancel) and between solver steps (in-flight).
    cancel: Arc<AtomicBool>,
}

/// Native and browser run admission and bounded scheduling.
struct RunnerState {
    /// Max concurrently-executing runs. `run_fast` starts a run
    /// immediately while `in_flight.len() < max_parallel`, else queues it.
    max_parallel: usize,
    /// Run ids currently executing (native: a live thread; wasm: dispatched
    /// to the worker). A run leaves this set on its terminal update.
    in_flight: HashSet<ExperimentId>,
    /// FIFO of runs waiting for a slot. Drained by `pump_scheduler` as
    /// in-flight runs finish.
    pending: VecDeque<QueuedJob>,
    #[cfg(not(target_arch = "wasm32"))]
    compile_tx: Sender<lunco_modelica_runtime::ModelicaCommand>,
}

impl RunnerState {
    fn new(
        #[cfg(not(target_arch = "wasm32"))] compile_tx: Sender<
            lunco_modelica_runtime::ModelicaCommand,
        >,
    ) -> Self {
        Self {
            max_parallel: ExperimentSettings::default().resolved_max_parallel(),
            in_flight: HashSet::new(),
            pending: VecDeque::new(),
            #[cfg(not(target_arch = "wasm32"))]
            compile_tx,
        }
    }
}

const POISONED_RUNNER_STATE: &str = "experiment runner state is poisoned; scheduling stopped";

/// A poisoned state can only release queued work with a terminal rejection.
/// Its guard is never returned to callers and the mutex remains poisoned.
fn lock_runner_state(
    state: &Mutex<RunnerState>,
) -> Result<std::sync::MutexGuard<'_, RunnerState>, ()> {
    match state.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            for job in guard.pending.drain(..) {
                job.cancel.store(true, Ordering::SeqCst);
                let _ = job.tx.send(RunUpdate::Failed {
                    error: POISONED_RUNNER_STATE.to_owned(),
                });
            }
            Err(())
        }
    }
}

/// Runner state shared between the trait wrapper and the worker thread.
/// All concurrency bookkeeping lives in `state` so the scheduler can be
/// pumped from both `run_fast` and the off-thread completion path without
/// holding a `&ModelicaRunner`.
pub struct ModelicaRunner {
    state: Arc<Mutex<RunnerState>>,
}

#[cfg(target_arch = "wasm32")]
impl Default for ModelicaRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelicaRunner {
    pub fn new(
        #[cfg(not(target_arch = "wasm32"))] compile_tx: Sender<
            lunco_modelica_runtime::ModelicaCommand,
        >,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(RunnerState::new(
                #[cfg(not(target_arch = "wasm32"))]
                compile_tx,
            ))),
        }
    }

    /// Override the max number of concurrently-executing runs. Called by
    /// the settings wiring (step 2) / tests. Takes effect on the next
    /// scheduler pump — already-running runs are never preempted, and
    /// lowering the cap below the current in-flight count simply lets
    /// those drain before new ones start.
    pub fn set_max_parallel(&self, n: usize) {
        if let Ok(mut s) = lock_runner_state(&self.state) {
            s.max_parallel = n.max(1);
        }
        // A raised cap may free slots for already-queued runs.
        pump_scheduler(&self.state);
    }

    /// Current max-parallel cap.
    pub fn max_parallel(&self) -> usize {
        self.state.lock().map(|s| s.max_parallel).unwrap_or(1)
    }

    /// `true` when no scheduler slot is free — i.e. starting another run
    /// right now would queue rather than execute immediately. UI uses this
    /// to reflect a saturated runner. (A click while saturated now queues
    /// the run instead of being rejected; the "busy" state is advisory.)
    pub fn is_busy(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.in_flight.len() >= s.max_parallel)
            .unwrap_or(false)
    }

    /// Number of runs currently executing.
    pub fn in_flight_count(&self) -> usize {
        self.state.lock().map(|s| s.in_flight.len()).unwrap_or(0)
    }

    /// Number of runs queued and waiting for a slot.
    pub fn queued_count(&self) -> usize {
        self.state.lock().map(|s| s.pending.len()).unwrap_or(0)
    }
}

impl ExperimentRunner for ModelicaRunner {
    type Source = ModelSource;

    fn run_fast(&self, exp: &Experiment, source: ModelSource) -> RunHandle {
        let (tx, rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let run_id = exp.id;

        // Withdraw queued work immediately; executing work observes the flag
        // at its cancellation boundaries. The wasm host also receives cancel.
        let cancel_for_hook = cancel.clone();
        let state_for_hook = Arc::downgrade(&self.state);
        #[cfg(target_arch = "wasm32")]
        let cancel_hook: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            cancel_for_hook.store(true, Ordering::SeqCst);
            if state_for_hook
                .upgrade()
                .is_some_and(|state| withdraw_queued_run(&state, run_id))
            {
                return;
            }
            if let Some(transport) = crate::worker_run_transport() {
                (transport.dispatch_cancel_run)(run_id);
            }
        });
        #[cfg(not(target_arch = "wasm32"))]
        let cancel_hook: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            cancel_for_hook.store(true, Ordering::SeqCst);
            if let Some(state) = state_for_hook.upgrade() {
                withdraw_queued_run(&state, run_id);
            }
        });

        if let Err(error) = source.result_limits.validate() {
            let _ = tx.send(RunUpdate::Failed {
                error: error.to_string(),
            });
            return RunHandle {
                run_id,
                progress_rx: rx,
                cancel: cancel_hook,
            };
        }

        if let Err(error) = lunco_modelica_core::sim_target::validate_run_bounds(&exp.bounds) {
            let _ = tx.send(RunUpdate::Failed {
                error: error.to_string(),
            });
            return RunHandle {
                run_id,
                progress_rx: rx,
                cancel: cancel_hook,
            };
        }

        // Enqueue the snapshotted job, then start as many as slots allow.
        // A queued run sits silent (no updates) until a slot frees — its
        // caller marks its registry row `Queued` before admission and its
        // first progress update transitions it to `Running`.
        match lock_runner_state(&self.state) {
            Ok(mut s) => s.pending.push_back(QueuedJob {
                run_id,
                source,
                overrides: exp.overrides.clone(),
                inputs: exp.inputs.clone(),
                bounds: exp.bounds.clone(),
                tx,
                cancel,
            }),
            Err(()) => {
                let _ = tx.send(RunUpdate::Failed {
                    error: POISONED_RUNNER_STATE.to_owned(),
                });
            }
        }
        pump_scheduler(&self.state);

        RunHandle {
            run_id,
            progress_rx: rx,
            cancel: cancel_hook,
        }
    }
}

/// Start as many queued runs as there are free scheduler slots. Called
/// after enqueuing in `run_fast`, after a run finishes (`finish_run`), and
/// when the cap is raised. Each started run is moved into `in_flight` and
/// handed to the platform `start_job`. Safe to call redundantly — it's a
/// no-op when no slot is free or the queue is empty.
///
/// Locking: only the brief pop/insert critical section holds the state
/// lock; `start_job` runs outside it (it spawns a thread / posts to the
/// worker), so a finishing run re-entering via `finish_run` never deadlocks.
fn pump_scheduler(state: &Arc<Mutex<RunnerState>>) {
    loop {
        let job = {
            let mut s = match lock_runner_state(state) {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.in_flight.len() >= s.max_parallel {
                return;
            }
            match s.pending.pop_front() {
                Some(j) => {
                    s.in_flight.insert(j.run_id);
                    j
                }
                None => return,
            }
        };
        start_job(state.clone(), job);
    }
}

/// Withdraw a queued run immediately; running jobs observe their cancel flag.
fn withdraw_queued_run(state: &Arc<Mutex<RunnerState>>, run_id: ExperimentId) -> bool {
    let job = {
        let Ok(mut state) = lock_runner_state(state) else {
            return false;
        };
        let Some(index) = state.pending.iter().position(|job| job.run_id == run_id) else {
            return false;
        };
        state.pending.remove(index)
    };
    let Some(job) = job else {
        return false;
    };
    let _ = job.tx.send(RunUpdate::Cancelled);
    true
}

/// Mark a run as no longer in flight and pump the queue so the freed slot
/// is filled. Called from the off-thread completion path (native thread
/// end; wasm forwarder on terminal update).
fn finish_run(state: &Arc<Mutex<RunnerState>>, run_id: ExperimentId) {
    release_run_slot(state, run_id);
    pump_scheduler(state);
}

fn release_run_slot(state: &Arc<Mutex<RunnerState>>, run_id: ExperimentId) {
    if let Ok(mut s) = lock_runner_state(state) {
        s.in_flight.remove(&run_id);
    }
}

/// Begin executing one already-slotted job. Native: spawn a thread running
/// `run_inner`, releasing its slot on every worker outcome. The thread-per-run
/// model gives each run fresh rumoca `thread_local` caches; the scheduler
/// caps live threads at `max_parallel`.
#[cfg(not(target_arch = "wasm32"))]
fn start_job(state: Arc<Mutex<RunnerState>>, job: QueuedJob) {
    let QueuedJob {
        run_id,
        source,
        overrides,
        inputs,
        bounds,
        tx,
        cancel,
    } = job;
    // Queued-cancel: if the run was cancelled before a slot freed, finish
    // it immediately without compiling.
    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        // Admission already runs inside the scheduler pump, which continues
        // after this synchronous terminal result without recursive pumping.
        release_run_slot(&state, run_id);
        return;
    }
    let worker_tx = tx.clone();
    start_native_job(state.clone(), run_id, tx, move || {
        run_inner(state, source, overrides, inputs, bounds, cancel, worker_tx);
    });
}

/// Reclaim the admitted slot even when execution unwinds.
#[cfg(not(target_arch = "wasm32"))]
struct NativeRunCompletion {
    state: Arc<Mutex<RunnerState>>,
    run_id: ExperimentId,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for NativeRunCompletion {
    fn drop(&mut self) {
        finish_run(&self.state, self.run_id);
    }
}

/// The native worker boundary owns thread admission and unexpected panics.
#[cfg(not(target_arch = "wasm32"))]
fn start_native_job(
    state: Arc<Mutex<RunnerState>>,
    run_id: ExperimentId,
    tx: Sender<RunUpdate>,
    execute: impl FnOnce() + Send + 'static,
) {
    let worker_state = state.clone();
    let failure_tx = tx.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("modelica-experiment-{}", run_id.0))
        .spawn(move || {
            let _completion = NativeRunCompletion {
                state: worker_state,
                run_id,
            };
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(execute)) {
                let detail = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic payload");
                let _ = tx.send(RunUpdate::Failed {
                    error: format!("experiment worker panicked: {detail}"),
                });
            }
        });
    if let Err(error) = spawned {
        let _ = failure_tx.send(RunUpdate::Failed {
            error: format!("experiment worker could not start: {error}"),
        });
        // The scheduler pump that admitted this job continues after return.
        release_run_slot(&state, run_id);
    }
}

/// Begin executing one already-slotted job. Wasm: resolve the source on the
/// main thread and dispatch to the Web Worker; the forwarder relays updates
/// and calls `finish_run` on the terminal one. (Today there is a single
/// worker, so `max_parallel` is 1 and these serialize; the worker pool in
/// step 3 raises the cap.)
#[cfg(target_arch = "wasm32")]
fn start_job(state: Arc<Mutex<RunnerState>>, job: QueuedJob) {
    let QueuedJob {
        run_id,
        source,
        overrides,
        inputs,
        bounds,
        tx,
        cancel,
    } = job;
    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        finish_run(&state, run_id);
        return;
    }
    // Flip the registry status Queued → Running the moment this job leaves the
    // queue (see the native `run_inner` emit for the full rationale). The
    // off-thread worker's batch Fast Run emits no mid-solve `Progress`, so
    // without this an in-browser long run would sit at "Queued" until it
    // finished. `drain_pending_handles` maps this `Progress` to `Running`.
    let _ = tx.send(RunUpdate::Progress {
        t_current: bounds.t_start,
    });
    let src = source;
    // Forward worker updates into the handle's tx; the forwarder frees the
    // slot via `finish_run` when a terminal update arrives.
    let Some(transport) = crate::worker_run_transport() else {
        let _ = tx.send(RunUpdate::Failed {
            error: "Modelica worker run transport is not installed".to_string(),
        });
        finish_run(&state, run_id);
        return;
    };
    let (forward_tx, forward_rx) = unbounded::<RunUpdate>();
    (transport.register_run_sender)(run_id, forward_tx, src.runtime.clone());
    spawn_forwarder(run_id, forward_rx, tx, state.clone());
    let dispatched = (transport.dispatch_run_fast)(
        run_id,
        src.model_name,
        src.source,
        src.filename,
        src.extras,
        overrides,
        inputs,
        bounds,
        src.result_limits,
    );
    if !dispatched {
        // No worker installed — free the slot so the queue isn't stuck.
        finish_run(&state, run_id);
    }
}

/// wasm-only forwarder: the worker_transport demux pushes updates into
/// `forward_rx`; we relay them to the runner's `tx` (which the
/// `RunHandle` consumer drains) and clear the runner-side busy flag
/// on terminal updates. v1: polled by a Bevy system on Update tick.
#[cfg(target_arch = "wasm32")]
fn spawn_forwarder(
    run_id: ExperimentId,
    forward_rx: crossbeam_channel::Receiver<RunUpdate>,
    tx: crossbeam_channel::Sender<RunUpdate>,
    state: Arc<Mutex<RunnerState>>,
) {
    let mut slot = wasm_forwarders().lock_or_recover();
    slot.push(WasmForwarder {
        run_id,
        forward_rx,
        tx,
        state,
    });
}

#[cfg(target_arch = "wasm32")]
struct WasmForwarder {
    run_id: ExperimentId,
    forward_rx: crossbeam_channel::Receiver<RunUpdate>,
    tx: crossbeam_channel::Sender<RunUpdate>,
    state: Arc<Mutex<RunnerState>>,
}

#[cfg(target_arch = "wasm32")]
static WASM_FORWARDERS: std::sync::OnceLock<Mutex<Vec<WasmForwarder>>> = std::sync::OnceLock::new();

#[cfg(target_arch = "wasm32")]
fn wasm_forwarders() -> &'static Mutex<Vec<WasmForwarder>> {
    WASM_FORWARDERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Pump pending wasm forwarders. Call from a Bevy `Update` system on
/// wasm. Drains all queued updates; clears the runner's busy flag and
/// removes the forwarder when a terminal update arrives.
#[cfg(target_arch = "wasm32")]
pub fn pump_wasm_forwarders() {
    // Collect terminal runs while holding the forwarders lock, then release
    // it before calling `finish_run` — that path re-enters the scheduler and
    // may push a new forwarder, which would deadlock under the same lock.
    let mut terminated: Vec<(Arc<Mutex<RunnerState>>, ExperimentId)> = Vec::new();
    {
        let mut slot = wasm_forwarders().lock_or_recover();
        let mut keep = Vec::with_capacity(slot.len());
        for fwd in slot.drain(..) {
            let mut terminal = false;
            while let Ok(update) = fwd.forward_rx.try_recv() {
                let is_term = matches!(
                    update,
                    RunUpdate::Completed(_) | RunUpdate::Failed { .. } | RunUpdate::Cancelled
                );
                let _ = fwd.tx.send(update);
                if is_term {
                    terminal = true;
                }
            }
            if terminal {
                terminated.push((fwd.state.clone(), fwd.run_id));
            } else {
                keep.push(fwd);
            }
        }
        *slot = keep;
    }
    for (state, run_id) in terminated {
        finish_run(&state, run_id);
    }
}

/// Admit finite scalar run values through Rumoca's parameter and input options.
pub fn apply_run_values(
    options: &mut rumoca_sim::SimOptions,
    inputs: &BTreeMap<ParamPath, ParamValue>,
    overrides: &BTreeMap<ParamPath, ParamValue>,
) -> Result<(), String> {
    fn numeric(values: &BTreeMap<ParamPath, ParamValue>) -> Result<Vec<(String, f64)>, String> {
        values.iter().map(|(path, value)| {
            let value = match value {
                ParamValue::Real(value) => *value,
                ParamValue::Int(value) if value.unsigned_abs() <= (1_u64 << 53) => *value as f64,
                ParamValue::Bool(value) => f64::from(*value),
                _ => return Err(format!("binding for '{}' requires a finite numeric or Boolean scalar representable exactly as f64", path.0)),
            };
            if !value.is_finite() { return Err(format!("binding for '{}' must be finite", path.0)); }
            Ok((path.0.clone(), value))
        }).collect()
    }
    let inputs = numeric(inputs)?;
    let overrides = numeric(overrides)?;
    options.initial_inputs = inputs;
    options.param_overrides = overrides;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn run_inner(
    state: Arc<Mutex<RunnerState>>,
    source: ModelSource,
    overrides: BTreeMap<ParamPath, ParamValue>,
    inputs: BTreeMap<ParamPath, ParamValue>,
    bounds: RunBounds,
    cancel: Arc<AtomicBool>,
    tx: Sender<RunUpdate>,
) {
    let _cancellation = rumoca_eval_solve::EvaluationCancellationGuard::install(cancel.clone());
    let t_wall = web_time::Instant::now();

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Announce that this job has left the queue and is now executing. The
    // batch path is one blocking `simulate_fmi_component` call that emits
    // no mid-solve `Progress`, so without this the registry status would sit
    // at `Queued` (set at dispatch) for the entire compile + solve and only
    // flip straight to `Done` — making a long-grinding run look stuck in a
    // queue. `drain_pending_handles` maps any `Progress` to
    // `RunStatus::Running`, so this single emit flips the label to "Running"
    // the moment the worker thread picks the job up, including compile time.
    let _ = tx.send(RunUpdate::Progress {
        t_current: bounds.t_start,
    });

    let compile_tx = match lock_runner_state(&state) {
        Ok(state) => state.compile_tx.clone(),
        Err(_) => {
            let _ = tx.send(RunUpdate::Failed {
                error: POISONED_RUNNER_STATE.to_owned(),
            });
            return;
        }
    };
    let (reply, compiled_rx) = crossbeam_channel::bounded(1);
    if compile_tx
        .send(lunco_modelica_runtime::ModelicaCommand::CompileBatch {
            model_name: source.model_name.clone(),
            source: source.source.clone(),
            doc_uri: source.filename.clone(),
            extra_sources: source.extras.clone(),
            cancel: cancel.clone(),
            reply,
        })
        .is_err()
    {
        let _ = tx.send(RunUpdate::Failed {
            error: "Modelica compiler worker is unavailable".to_owned(),
        });
        return;
    }
    // Only this background run thread waits. The shared owner admits the request
    // after source-root commits; cancellation releases its scheduler slot promptly.
    let artifact = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = tx.send(RunUpdate::Cancelled);
            return;
        }
        match compiled_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(Ok(artifact)) => break artifact,
            Ok(Err(error)) => {
                let _ = tx.send(RunUpdate::Failed {
                    error: format!("compile failed: {error}"),
                });
                return;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                let _ = tx.send(RunUpdate::Failed {
                    error: "Modelica batch compilation reply disconnected".to_owned(),
                });
                return;
            }
        }
    };
    let base_dae = artifact.dae;
    let source_content = lunco_experiments::SourceContentIdentity::Available {
        cid: artifact.source_content,
    };
    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Drive the run through the SHARED `drive_run` — the SAME entry point the
    // wasm worker (`lunica_worker::run_fast_in_worker`) calls, so native and
    // web can NOT diverge on solver selection / sampling. The only
    // platform-specific piece is the `RunSink` impl (native = crossbeam
    // channel + atomic cancel; worker = postMessage + cancel registry).
    let mut sink = ChannelSink {
        tx,
        cancel,
        source_content,
    };
    drive_run(
        &base_dae,
        &bounds,
        &inputs,
        &overrides,
        source.result_limits,
        t_wall,
        &mut sink,
    );
}

/// Shared native and Web Worker simulation entry point over Rumoca's checked
/// batch solver, admitted output grid, run values, and retained-output budget.
pub fn drive_run(
    dae: &Dae,
    bounds: &RunBounds,
    inputs: &BTreeMap<ParamPath, ParamValue>,
    overrides: &BTreeMap<ParamPath, ParamValue>,
    limits: lunco_experiments::RunResultLimits,
    started: web_time::Instant,
    sink: &mut impl RunSink,
) {
    if sink.is_cancelled() {
        sink.emit(RunUpdate::Cancelled);
        return;
    }
    let output_budget = match limits
        .validate()
        .map_err(|error| error.to_string())
        .and_then(|()| {
            rumoca_solver::SolverOutputBudget::new(limits.max_values)
                .map_err(|error| error.to_string())
        }) {
        Ok(budget) => budget,
        Err(error) => {
            sink.emit(RunUpdate::Failed { error });
            return;
        }
    };
    let _output_budget = rumoca_solver::SolverOutputBudgetGuard::install(output_budget);
    let output_dt = match lunco_modelica_core::sim_target::validate_run_bounds(bounds) {
        Ok(dt) => dt,
        Err(error) => {
            sink.emit(RunUpdate::Failed {
                error: error.to_string(),
            });
            return;
        }
    };
    // Solver options (tolerance / family / initial step) — the SINGLE source
    // (`stepper_options_from_bounds`) both runtimes derive from. An incapable
    // authored solver fails the run here rather than per-step later.
    let mut stepper_opts = match stepper_options_from_bounds(bounds) {
        Ok(opts) => opts,
        Err(err) => {
            sink.emit(RunUpdate::Failed {
                error: format!("run configuration failed: {err}"),
            });
            return;
        }
    };
    if let Err(error) = apply_run_values(&mut stepper_opts, inputs, overrides) {
        sink.emit(RunUpdate::Failed { error });
        return;
    }
    // The requested output grid is independent of the solver's internal step.
    let mut batch_opts = stepper_opts;
    batch_opts.dt = Some(output_dt);
    bevy::log::info!(
        "[runner] simulate begin (batch): t={}..{} output_dt={} (dt={:?} n_intervals={:?})",
        bounds.t_start,
        bounds.t_end,
        output_dt,
        bounds.dt,
        bounds.n_intervals
    );
    run_batch_sim(dae, &batch_opts, output_budget, started, sink);
}

/// Retain Rumoca's checked output facts under the experiment's series identity.
fn series_metadata(
    metadata: Vec<rumoca_solver::SimVariableMeta>,
) -> std::sync::Arc<BTreeMap<String, lunco_experiments::SeriesMetadata>> {
    std::sync::Arc::new(
        metadata
            .into_iter()
            .filter(|fact| fact.name != "time")
            .map(|fact| {
                (
                    fact.name,
                    lunco_experiments::SeriesMetadata {
                        unit: fact.unit,
                        description: fact.description,
                    },
                )
            })
            .collect(),
    )
}

/// Drive a [`Dae`] to `t_end` through the non-interactive dense-output batch
/// solver ([`rumoca_sim::simulate_fmi_component`]) and emit the trajectory
/// as a single [`RunUpdate::Completed`]. The
/// solver owns its own adaptive time loop and the output samples are taken by
/// dense interpolation, so the solver step size is independent of the output
/// spacing — the robust path for stiff long-horizon runs.
///
/// Native admitted runs install their cancellation flag at the solver owner.
/// Evaluation and driver checkpoints return before the trajectory can publish.
/// An executing numerical kernel returns before its next checkpoint. Browser
/// cancellation uses the worker transport's separate lifetime boundary.
///
/// Platform-agnostic: called by both native ([`drive_run`] via [`run_inner`])
/// and the wasm worker. On wasm the whole solve runs off the main thread in
/// the Web Worker, so the single blocking call doesn't stall the UI.
fn run_batch_sim(
    dae: &Dae,
    opts: &rumoca_sim::SimOptions,
    budget: rumoca_solver::SolverOutputBudget,
    started: web_time::Instant,
    sink: &mut impl RunSink,
) {
    if sink.is_cancelled() {
        sink.emit(RunUpdate::Cancelled);
        return;
    }

    let output_dt = match lunco_modelica_core::sim_target::resolve_step_dt(
        opts.t_start,
        opts.t_end,
        opts.dt,
        None,
    ) {
        Ok(dt) => dt,
        Err(error) => {
            sink.emit(RunUpdate::Failed {
                error: error.to_string(),
            });
            return;
        }
    };
    let model = match rumoca_sim::prepare_fmi_component(dae, opts) {
        Ok(model) => model,
        Err(error) => {
            sink.emit(RunUpdate::Failed {
                error: format!("solve lowering failed: {error}"),
            });
            return;
        }
    };
    let admission = budget
        .sample_capacity(model.runtime_view().model().visible_names.len())
        .and_then(|capacity| {
            rumoca_solver::timeline::output_sample_count(
                opts.t_start,
                opts.t_end,
                output_dt,
                capacity,
            )
        });
    if let Err(error) = admission {
        sink.emit(RunUpdate::Failed {
            error: error.to_string(),
        });
        return;
    }
    let result = match rumoca_sim::simulate_fmi_component(model, opts) {
        Ok(r) => r,
        Err(e) => {
            if sink.is_cancelled() {
                sink.emit(RunUpdate::Cancelled);
                return;
            }
            sink.emit(RunUpdate::Failed {
                error: format!("simulate failed: {e}"),
            });
            return;
        }
    };

    if sink.is_cancelled() {
        sink.emit(RunUpdate::Cancelled);
        return;
    }

    if result.data.len() != result.names.len()
        || result
            .data
            .iter()
            .any(|column| column.len() != result.times.len())
    {
        sink.emit(RunUpdate::Failed {
            error: "solver trajectory columns do not match their names and time vector".into(),
        });
        return;
    }
    let variable_metadata = series_metadata(result.variable_meta);
    if result
        .names
        .iter()
        .filter(|name| name.as_str() != "time")
        .any(|name| !variable_metadata.contains_key(name))
    {
        sink.emit(RunUpdate::Failed {
            error: "solver trajectory has a series without checked metadata".into(),
        });
        return;
    }
    // SimResult stores one column per visible variable (`data[var][t]`),
    // mirroring the `RunResult` series map keyed by variable name. Drop the
    // synthetic `time` column — `times` carries it.
    let mut series: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (name, column) in result.names.into_iter().zip(result.data) {
        if name == "time" {
            continue;
        }
        series.insert(name, column);
    }

    let n_samples = result.times.len();
    bevy::log::info!(
        "[sim] simulate ok (batch): {} samples, {} vars, {:.2}s wall",
        n_samples,
        series.len(),
        started.elapsed().as_secs_f64(),
    );
    sink.emit(RunUpdate::Completed(RunResult {
        times: result.times,
        series,
        meta: RunMeta {
            variable_metadata,
            wall_time_ms: started.elapsed().as_millis() as u64,
            sample_count: n_samples,
            notes: None,
            ..Default::default()
        },
    }));
}

/// Native and browser adapters for cancellation and typed experiment updates.
pub trait RunSink {
    fn is_cancelled(&mut self) -> bool;
    fn emit(&mut self, update: RunUpdate);
}

/// Native [`RunSink`]: crossbeam channel + shared atomic cancel flag.
#[cfg(not(target_arch = "wasm32"))]
struct ChannelSink {
    tx: Sender<RunUpdate>,
    cancel: Arc<AtomicBool>,
    source_content: lunco_experiments::SourceContentIdentity,
}

#[cfg(not(target_arch = "wasm32"))]
impl RunSink for ChannelSink {
    fn is_cancelled(&mut self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    fn emit(&mut self, mut update: RunUpdate) {
        lunco_experiments::stamp_source_identity(&mut update, &self.source_content);
        let _ = self.tx.send(update);
    }
}

/// Offline runs use the Modelica tolerance default unless bounds or annotations
/// supply a value. Live prediction uses its separate fixed-step policy.
pub const DEFAULT_TOLERANCE: f64 = 1e-6;

/// Offline run configuration rejection from its bounds or solver owner.
#[derive(Debug)]
pub enum RunConfigurationError {
    Bounds(lunco_modelica_core::sim_target::RunBoundsError),
    Solver(lunco_experiments::solver::SolverError),
}

impl std::fmt::Display for RunConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bounds(error) => std::fmt::Display::fmt(error, f),
            Self::Solver(error) => std::fmt::Display::fmt(error, f),
        }
    }
}

/// Resolve the registered offline solver and translate the admitted run bounds.
/// Unknown or incapable solvers return a terminal configuration error.
pub fn stepper_options_from_bounds(
    bounds: &RunBounds,
) -> Result<rumoca_sim::SimOptions, RunConfigurationError> {
    use lunco_experiments::solver;
    lunco_modelica_core::sim_target::validate_run_bounds(bounds)
        .map_err(RunConfigurationError::Bounds)?;
    lunco_modelica_solver::solver_backends::ensure_builtin_solvers();

    let request = solver::SolverRequest {
        // Batch owns its own time loop and drives no predicted body, so the
        // adaptive implicit family is not merely allowed but wanted.
        profile: solver::RuntimeProfile {
            live: false,
            predicted: false,
        },
        authored: bounds.solver.clone(),
    };

    let params = solver::SolverParams {
        atol: bounds.tolerance.unwrap_or(DEFAULT_TOLERANCE),
        rtol: bounds.tolerance.unwrap_or(DEFAULT_TOLERANCE),
        // `h0` is the solver's initial step on the diffsol path.
        h0: bounds.h0,
        t_start: bounds.t_start,
        t_end: bounds.t_end,
    };

    let spec = solver::resolve(&request).map_err(RunConfigurationError::Solver)?;
    lunco_modelica_solver::solver_backends::rumoca_options(&spec, &params)
        .map_err(RunConfigurationError::Solver)
}

/// One resolved `parameter` declaration, including inherited members. Used by the
/// override editor UI to render an editable row per parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedParam {
    pub name: String,
    /// The parameter's declared type, e.g. `Real`, `Integer`, `Boolean`,
    /// `String`, or a class-qualified type. Displayed in the editor.
    pub type_name: String,
    /// Supported scalar type resolved from the declaration's scoped type.
    pub scalar_type: Option<lunco_modelica_ast::PredefinedComponentType>,
    /// Numeric default rendered from the declaration's binding or start.
    /// Absent when the authored value cannot be projected as a number.
    pub default_literal: Option<String>,
    /// Whether this scalar declaration can enter the numeric run-options path.
    pub supportable: bool,
    /// Reason override is unsupported, when `!supportable`. Surfaced
    /// in the editor as a tooltip.
    pub reason: Option<String>,
    /// The Modelica description-comment string (e.g. the `"Gravity"` in
    /// `parameter Real g = 9.81 "Gravity";`), if present. Shown as hover
    /// help on the parameter row in the override editor.
    pub description: Option<String>,
}

/// One resolved input row. Only supported scalars admit a text override.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedInput {
    pub name: String,
    pub type_name: String,
    /// Supported scalar type resolved from the declaration's scoped type.
    pub scalar_type: Option<lunco_modelica_ast::PredefinedComponentType>,
}

fn run_scalar_type(
    c: &lunco_modelica_ast::ast_extract::TypedComponent,
) -> Option<lunco_modelica_ast::PredefinedComponentType> {
    use lunco_modelica_ast::PredefinedComponentType as T;
    if c.has_array_dimensions {
        return None;
    }
    c.scalar_type
        .filter(|kind| matches!(kind, T::Real | T::Integer | T::Boolean))
}

/// Input rows from Rumoca's inheritance-merged, scoped declarations.
pub fn input_rows(members: &[lunco_modelica_core::engine::InheritedMember]) -> Vec<DetectedInput> {
    members
        .iter()
        .filter(|member| {
            matches!(
                member.causality,
                lunco_modelica_core::engine::InheritedCausality::Input
            )
        })
        .map(|member| DetectedInput {
            name: member.name.clone(),
            type_name: member.type_name.clone(),
            scalar_type: run_scalar_type(&member.metadata),
        })
        .collect()
}

/// Parameter rows from the same scoped declaration facts as input rows.
/// The compiler and solver make the final tunability admission decision.
pub fn parameter_rows(
    members: &[lunco_modelica_core::engine::InheritedMember],
) -> Vec<DetectedParam> {
    members
        .iter()
        .filter(|member| {
            matches!(
                member.variability,
                lunco_modelica_core::engine::InheritedVariability::Parameter
            )
        })
        .map(|member| {
            let c = &member.metadata;
            let scalar_type = run_scalar_type(c);
            DetectedParam {
                name: c.name.clone(),
                type_name: member.type_name.clone(),
                scalar_type,
                default_literal: c.default.map(|x| format!("{x}")),
                supportable: scalar_type.is_some(),
                reason: scalar_type
                    .is_none()
                    .then(|| "run overrides require numeric or Boolean scalars".to_owned()),
                description: (!c.description.trim().is_empty()).then(|| c.description.clone()),
            }
        })
        .collect()
}

/// Parse a numeric run value against its resolved primitive type.
/// Empty text omits the override; unsupported or malformed text is an error.
pub fn parse_scalar_run_value(
    kind: Option<lunco_modelica_ast::PredefinedComponentType>,
    text: &str,
) -> Result<Option<ParamValue>, String> {
    use lunco_modelica_ast::PredefinedComponentType as T;
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let kind = kind.ok_or("run overrides require numeric or Boolean scalars")?;
    let value = match kind {
        T::Real => {
            let value = text
                .parse::<f64>()
                .map_err(|_| "expected a decimal number")?;
            if !value.is_finite() {
                return Err("expected a finite decimal number".to_owned());
            }
            ParamValue::Real(value)
        }
        T::Integer => ParamValue::Int(text.parse::<i64>().map_err(|_| "expected an integer")?),
        T::Boolean => match text {
            "true" => ParamValue::Bool(true),
            "false" => ParamValue::Bool(false),
            _ => return Err("expected true or false".to_owned()),
        },
        T::String => return Err("run overrides require numeric or Boolean scalars".to_owned()),
    };
    Ok(Some(value))
}

/// Per-(document, model) parameter override + bounds draft state.
/// Edited by the override editor UI; read by `FastRunActiveModel`
/// when constructing the experiment record. Keyed by
/// `(DocumentId, ModelRef)` so two open tabs of different copies of
/// the same class don't clobber each other's setup.
///
/// Cleared per-doc on `CloseDocument` (lifecycle.rs cleanup).
#[derive(Resource, Default, Debug)]
pub struct ExperimentDrafts {
    drafts: std::collections::HashMap<(lunco_doc::DocumentId, ModelRef), ExperimentDraft>,
}

#[derive(Clone, Debug, Default)]
pub struct ExperimentDraft {
    pub overrides: BTreeMap<ParamPath, ParamValue>,
    /// User-set values for `input` variables. Stored separately from
    /// parameter overrides so the UI can preserve the two authored concepts
    /// while the runner admits them into separate parameter/input option fields.
    pub inputs: BTreeMap<ParamPath, ParamValue>,
    pub bounds_override: Option<RunBounds>,
}

impl ExperimentDrafts {
    pub fn get(&self, doc: lunco_doc::DocumentId, model: &ModelRef) -> Option<&ExperimentDraft> {
        self.drafts.get(&(doc, model.clone()))
    }
    pub fn entry(&mut self, doc: lunco_doc::DocumentId, model: ModelRef) -> &mut ExperimentDraft {
        self.drafts.entry((doc, model)).or_default()
    }
    pub fn clear(&mut self, doc: lunco_doc::DocumentId, model: &ModelRef) {
        self.drafts.remove(&(doc, model.clone()));
    }
    /// Drop every draft attached to `doc`. Called from the
    /// document-close cleanup observer.
    pub fn forget_doc(&mut self, doc: lunco_doc::DocumentId) {
        self.drafts.retain(|(d, _), _| *d != doc);
    }
    /// Re-key the `(doc, old)` draft to `(doc, new)`. Called by
    /// the class-rename observer so a class rename in the editor
    /// preserves the user's parameter / bounds / inputs setup
    /// instead of forcing a fresh draft on the new name.
    pub fn rename_model_ref(
        &mut self,
        doc: lunco_doc::DocumentId,
        old: &ModelRef,
        new: &ModelRef,
    ) -> bool {
        match self.drafts.remove(&(doc, old.clone())) {
            Some(d) => {
                self.drafts.insert((doc, new.clone()), d);
                true
            }
            None => false,
        }
    }
}

/// Bevy resource holding RunHandles for in-flight experiments.
/// Drained each Update by [`drain_pending_handles`]: terminal updates
/// get written back into the registry + emitted as Bevy messages.
#[derive(Resource, Default)]
pub struct PendingHandles(pub Vec<PendingRun>);

/// Immutable ownership envelope for an admitted asynchronous handle.
pub struct PendingRun {
    pub artifact_admission: Option<lunco_experiments::ArtifactAdmission>,
    pub handle: RunHandle,
    pub origin: lunco_experiments::ExperimentOrigin,
    pub result_limits: lunco_experiments::RunResultLimits,
}
impl std::ops::Deref for PendingRun {
    type Target = RunHandle;
    fn deref(&self) -> &RunHandle {
        &self.handle
    }
}

/// Per-document playback entity: holds the latest completed run's
/// time-series in `SignalRegistry` so canvas plot tiles can resolve
/// `(entity, path)` lookups without a live cosim entity.
///
/// One playback entity per doc; refilled in place each time a run
/// finishes (drop old signals, push new ones). When live cosim is
/// active for the same doc, the live entity wins in
/// `doc_to_entity` — playback is selected when no live cosim entity is active.
/// The map resets with the scene; publishers tag its entities `SceneOwned`.
/// Document closure releases its playback while completed run history survives.
#[derive(Resource, Default)]
pub struct PlaybackEntities(
    pub std::collections::HashMap<lunco_doc::DocumentId, bevy::prelude::Entity>,
);

impl lunco_core::SceneResource for PlaybackEntities {}

/// Bevy system: drain RunUpdate messages from each pending handle,
/// update the registry status, and emit lifecycle Bevy messages
/// (`RunProgress` / `RunCompleted` / `RunFailed` / `RunCancelled`).
/// Removes handles whose runs have terminated.
pub fn drain_pending_handles(
    mut pending: ResMut<PendingHandles>,
    mut registry: ResMut<ExperimentRegistry>,
    origins: Res<lunco_experiments::ExperimentOrigins>,
    mut ev_progress: MessageWriter<RunProgress>,
    mut ev_completed: MessageWriter<RunCompleted>,
    mut ev_failed: MessageWriter<RunFailed>,
    mut ev_cancelled: MessageWriter<RunCancelled>,
) {
    let mut keep: Vec<PendingRun> = Vec::with_capacity(pending.0.len());
    for handle in pending.0.drain(..) {
        // Owner cancellation/deletion is terminal even if a worker already
        // queued progress or completion before observing its cancel flag.
        if origins.get(&handle.run_id) != Some(&handle.origin)
            || registry
                .get(handle.run_id)
                .is_none_or(|run| run.status.is_terminal())
        {
            handle.cancel();
            continue;
        }
        let mut terminal = false;
        loop {
            let update = match handle.progress_rx.try_recv() {
                Ok(update) => update,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => RunUpdate::Failed {
                    error: "experiment worker disconnected without a terminal outcome".to_owned(),
                },
            };
            match update {
                RunUpdate::Progress { t_current } => {
                    registry.set_status(handle.run_id, RunStatus::Running { t_current });
                    ev_progress.write(RunProgress {
                        experiment_id: handle.run_id,
                        origin: handle.origin.clone(),
                        t_current,
                    });
                }
                RunUpdate::Completed(result) => {
                    let wall = result.meta.wall_time_ms;
                    let n_samples = result.times.len();
                    let n_vars = result.series.len();
                    bevy::log::info!(
                        "[experiments] run {:?} received trajectory: {} samples, {} vars, {} ms",
                        handle.run_id,
                        n_samples,
                        n_vars,
                        wall
                    );
                    // UI projection of a completed run (console line, plot
                    // auto-pick, SignalRegistry playback publish) is handled
                    // reactively by `ui::core_observers::project_completed_run`
                    // off the `RunCompleted` message below — core only writes
                    // the result + status into the registry here.
                    if let Err(error) =
                        registry.set_complete_result(handle.run_id, result, handle.result_limits)
                    {
                        let error = format!("complete result rejected: {error}");
                        bevy::log::warn!("[experiments] run {:?} {error}", handle.run_id);
                        registry.set_status(
                            handle.run_id,
                            RunStatus::Failed {
                                error: error.clone(),
                            },
                        );
                        ev_failed.write(RunFailed {
                            experiment_id: handle.run_id,
                            origin: handle.origin.clone(),
                            error,
                        });
                        terminal = true;
                        break;
                    }
                    registry.set_status(handle.run_id, RunStatus::Done { wall_time_ms: wall });
                    ev_completed.write(RunCompleted {
                        experiment_id: handle.run_id,
                        origin: handle.origin.clone(),
                        artifact_admission: handle.artifact_admission.clone(),
                    });
                    terminal = true;
                }
                RunUpdate::Failed { error } => {
                    bevy::log::warn!("[experiments] run {:?} failed: {error}", handle.run_id);
                    registry.set_status(
                        handle.run_id,
                        RunStatus::Failed {
                            error: error.clone(),
                        },
                    );
                    ev_failed.write(RunFailed {
                        experiment_id: handle.run_id,
                        origin: handle.origin.clone(),
                        error,
                    });
                    terminal = true;
                }
                RunUpdate::Cancelled => {
                    registry.set_status(handle.run_id, RunStatus::Cancelled);
                    ev_cancelled.write(RunCancelled {
                        experiment_id: handle.run_id,
                        origin: handle.origin.clone(),
                    });
                    terminal = true;
                }
            }
            if terminal {
                break;
            }
        }
        // Terminal history retains its document attribution for result queries.
        // ExperimentRemoved retires attribution on deletion or bounded eviction.
        if !terminal {
            keep.push(handle);
        }
    }
    pending.0 = keep;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_runner() -> ModelicaRunner {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = unbounded();
            std::thread::spawn(move || {
                while let Ok(command) = rx.recv() {
                    if let lunco_modelica_runtime::ModelicaCommand::CompileBatch { reply, .. } =
                        command
                    {
                        let _ = reply.send(Err("test compiler rejection".to_owned()));
                    }
                }
            });
            ModelicaRunner::new(tx)
        }
        #[cfg(target_arch = "wasm32")]
        ModelicaRunner::new()
    }

    #[test]
    fn parameter_rows_use_declared_shape_instead_of_type_spelling() {
        let source = r#"model ShapeFacts
          type 'Bracket[Scalar]' = Real;
          parameter Integer n=2;
          parameter Real literal[2]={1,2};
          parameter Real symbolic[n]=fill(1,n);
          parameter Real[2,3] matrix=ones(2,3);
          parameter 'Bracket[Scalar]' scalar=1;
          parameter String text="label";
        end ShapeFacts;"#;
        let mut engine = lunco_modelica_core::engine::ModelicaEngine::new();
        engine.install_lenient(lunco_doc::DocumentId::new(1), source);
        let members = engine
            .inherited_members_typed("ShapeFacts")
            .expect("resolved declarations");
        let rows = parameter_rows(&members);
        for name in ["literal", "symbolic", "matrix", "text"] {
            let row = rows.iter().find(|row| row.name == name).expect("parameter");
            assert!(!row.supportable, "{name} must not accept a scalar override");
            assert!(row.reason.is_some());
        }
        for name in ["n", "scalar"] {
            let row = rows.iter().find(|row| row.name == name).expect("parameter");
            assert!(row.supportable, "{name} is a scalar declaration");
            assert!(row.reason.is_none());
        }
    }

    #[test]
    fn scalar_run_text_preserves_resolved_types_and_rejects_invalid_values() {
        use lunco_modelica_ast::PredefinedComponentType as T;
        assert_eq!(
            parse_scalar_run_value(Some(T::Integer), "42").unwrap(),
            Some(ParamValue::Int(42))
        );
        assert_eq!(
            parse_scalar_run_value(Some(T::Boolean), "true").unwrap(),
            Some(ParamValue::Bool(true))
        );
        assert_eq!(
            parse_scalar_run_value(Some(T::Real), "1.5").unwrap(),
            Some(ParamValue::Real(1.5))
        );
        assert_eq!(parse_scalar_run_value(None, " ").unwrap(), None);
        for (kind, text) in [
            (None, "1"),
            (Some(T::String), "label"),
            (Some(T::Boolean), "1"),
            (Some(T::Integer), "1.5"),
            (Some(T::Real), "NaN"),
            (Some(T::Real), "inf"),
        ] {
            assert!(parse_scalar_run_value(kind, text).is_err());
        }
    }

    #[test]
    fn set_max_parallel_floors_at_one() {
        let r = test_runner();
        r.set_max_parallel(0);
        assert_eq!(r.max_parallel(), 1, "cap must never drop below 1");
        r.set_max_parallel(4);
        assert_eq!(r.max_parallel(), 4);
    }

    // ── Step 1: bounded scheduler ──

    fn mint_exp(reg: &mut ExperimentRegistry, model: &str) -> Experiment {
        let id = reg.insert_new(
            lunco_experiments::TwinId("test-twin".into()),
            ModelRef(model.into()),
            BTreeMap::new(),
            BTreeMap::new(),
            RunBounds::default(),
        );
        reg.get(id).cloned().expect("just inserted")
    }

    fn test_source(model: &str) -> ModelSource {
        ModelSource {
            model_name: model.to_owned(),
            source: "model".to_owned(),
            filename: format!("{model}.mo"),
            extras: Vec::new(),
            runtime: lunco_workspace::DocumentRuntimeOwner::Application,
            result_limits: Default::default(),
        }
    }

    #[test]
    fn poisoned_runner_state_rejects_queued_and_new_runs_without_rescheduling() {
        let runner = test_runner();
        runner.set_max_parallel(1);
        let mut registry = ExperimentRegistry::new();
        let held = mint_exp(&mut registry, "HeldSlot");
        runner
            .state
            .lock()
            .expect("state")
            .in_flight
            .insert(held.id);
        let queued = mint_exp(&mut registry, "QueuedBeforePoison");
        let queued_handle = runner.run_fast(&queued, test_source(&queued.model_ref.0));
        assert_eq!(runner.queued_count(), 1);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = runner.state.lock().expect("state starts valid");
            panic!("injected runner state panic");
        }));
        assert!(poisoned.is_err());
        let rejected = mint_exp(&mut registry, "SubmittedAfterPoison");
        let rejected_handle = runner.run_fast(&rejected, test_source(&rejected.model_ref.0));
        for handle in [&queued_handle, &rejected_handle] {
            assert!(
                matches!(handle.progress_rx.try_recv(), Ok(RunUpdate::Failed { error }) if error == POISONED_RUNNER_STATE)
            );
            assert!(matches!(
                handle.progress_rx.try_recv(),
                Err(TryRecvError::Disconnected)
            ));
        }
        pump_scheduler(&runner.state);
        release_run_slot(&runner.state, held.id);
        assert!(!withdraw_queued_run(&runner.state, queued.id));
        assert!(runner.state.is_poisoned());
        let guard = match runner.state.lock() {
            Err(poisoned) => poisoned.into_inner(),
            Ok(_) => panic!("the scheduler must remain poisoned"),
        };
        assert!(guard.pending.is_empty());
        assert_eq!(guard.in_flight, HashSet::from([held.id]));
    }

    #[test]
    fn queued_same_name_runs_keep_their_admitted_source_snapshots() {
        let runner = test_runner();
        runner.set_max_parallel(1);
        let mut registry = ExperimentRegistry::new();
        let held = mint_exp(&mut registry, "Held");
        runner
            .state
            .lock()
            .expect("state")
            .in_flight
            .insert(held.id);
        let a = mint_exp(&mut registry, "SameName");
        let b = mint_exp(&mut registry, "SameName");
        let mut first = test_source("SameName");
        first.source = "first admitted source".into();
        first.runtime =
            lunco_workspace::DocumentRuntimeOwner::LocalTwin(lunco_workspace::TwinId::new(1));
        let mut second = first.clone();
        second.source = "second admitted source".into();
        second.runtime =
            lunco_workspace::DocumentRuntimeOwner::LocalTwin(lunco_workspace::TwinId::new(2));
        let ah = runner.run_fast(&a, first);
        let bh = runner.run_fast(&b, second);
        {
            let state = runner.state.lock().expect("state");
            assert_eq!(state.pending[0].source.source, "first admitted source");
            assert_eq!(
                state.pending[0].source.runtime.local_twin(),
                Some(lunco_workspace::TwinId::new(1))
            );
            assert_eq!(state.pending[1].source.source, "second admitted source");
            assert_eq!(
                state.pending[1].source.runtime.local_twin(),
                Some(lunco_workspace::TwinId::new(2))
            );
        }
        ah.cancel();
        bh.cancel();
        assert_eq!(runner.queued_count(), 0);
    }

    /// The scheduler drains every admitted run after compiler rejection.
    #[test]
    fn scheduler_queues_beyond_cap_and_drains_all() {
        use std::time::Duration;

        let runner = test_runner();
        runner.set_max_parallel(2);

        let mut reg = ExperimentRegistry::new();
        let mut handles = Vec::new();
        for _ in 0..5 {
            let exp = mint_exp(&mut reg, "NoSuchModel");
            handles.push(runner.run_fast(&exp, test_source(&exp.model_ref.0)));
        }

        // Every submitted run must terminate (none rejected).
        let mut terminal = 0usize;
        for h in &handles {
            loop {
                match h.progress_rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(RunUpdate::Failed { .. })
                    | Ok(RunUpdate::Completed(_))
                    | Ok(RunUpdate::Cancelled) => {
                        terminal += 1;
                        break;
                    }
                    Ok(RunUpdate::Progress { .. }) => continue,
                    Err(_) => break, // timed out / disconnected
                }
            }
        }
        assert_eq!(terminal, 5, "all queued runs should run and terminate");

        // The last run's `finish_run` may lag its terminal update slightly
        // (it runs after the worker thread sends Failed). Allow it to settle.
        let mut settled = false;
        for _ in 0..100 {
            if runner.in_flight_count() == 0 && runner.queued_count() == 0 {
                settled = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            settled,
            "scheduler should drain to empty after all runs end"
        );
    }

    #[test]
    fn invalid_bounds_fail_before_scheduler_or_solver_admission() {
        let runner = test_runner();
        let mut registry = ExperimentRegistry::new();
        let mut exp = mint_exp(&mut registry, "BoundsProbe");
        exp.bounds.dt = Some(1.0e-300);
        let handle = runner.run_fast(&exp, test_source(&exp.model_ref.0));
        let error = match handle
            .progress_rx
            .try_recv()
            .expect("terminal admission failure")
        {
            RunUpdate::Failed { error } => error,
            update => panic!("unexpected admission update: {update:?}"),
        };
        assert!(error.contains("output grid"));
        assert!(matches!(
            handle.progress_rx.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert_eq!(runner.queued_count(), 0);
        assert_eq!(runner.in_flight_count(), 0);

        assert!(matches!(
            stepper_options_from_bounds(&exp.bounds),
            Err(RunConfigurationError::Bounds(_))
        ));
    }

    #[test]
    fn cancelling_queued_run_withdraws_it_before_admission() {
        let runner = test_runner();
        runner.set_max_parallel(1);
        let mut registry = ExperimentRegistry::new();
        let active = mint_exp(&mut registry, "Active");
        let queued = mint_exp(&mut registry, "Queued");
        runner
            .state
            .lock()
            .expect("runner state")
            .in_flight
            .insert(active.id);
        let handle = runner.run_fast(&queued, test_source(&queued.model_ref.0));
        assert_eq!(runner.queued_count(), 1);
        handle.cancel();
        assert_eq!(runner.queued_count(), 0);
        assert_eq!(runner.in_flight_count(), 1);
        assert!(matches!(
            handle.progress_rx.try_recv(),
            Ok(RunUpdate::Cancelled)
        ));
        assert!(matches!(
            handle.progress_rx.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn source_admission_and_publication_keep_application_and_twin_ownership_distinct() {
        let mut workspace = lunco_workspace::Workspace::new();
        let loose = lunco_doc::DocumentId::new(1);
        let retired = lunco_doc::DocumentId::new(2);
        let closed_twin = lunco_workspace::TwinId::new(1);
        for (id, local_twin) in [(loose, None), (retired, Some(closed_twin))] {
            workspace.add_document(lunco_workspace::DocumentEntry {
                id,
                kind: lunco_workspace::DocumentKindId::new("modelica"),
                origin: lunco_doc::DocumentOrigin::untitled("OwnershipProbe"),
                runtime_context: local_twin.map_or(
                    lunco_workspace::DocumentRuntimeOwner::Application,
                    lunco_workspace::DocumentRuntimeOwner::LocalTwin,
                ),
                title: "OwnershipProbe".to_owned(),
                dirty: false,
            });
        }
        let source =
            lunco_workspace::PinnedDocumentRuntimeOwner::for_document(loose, Some(&workspace))
                .expect("loose document");
        assert_eq!(
            source.runtime,
            lunco_workspace::DocumentRuntimeOwner::Application
        );
        assert!(source.is_in_active_scope(Some(&workspace), None));
        let outgoing = lunco_workspace::PinnedDocumentRuntimeOwner {
            document: retired,
            runtime: lunco_workspace::DocumentRuntimeOwner::LocalTwin(closed_twin),
        };
        assert!(!outgoing.is_in_active_scope(Some(&workspace), None));
        assert!(
            lunco_workspace::PinnedDocumentRuntimeOwner::for_document(retired, Some(&workspace))
                .is_err()
        );
        assert!(
            lunco_workspace::PinnedDocumentRuntimeOwner::for_document(
                lunco_doc::DocumentId::new(3),
                Some(&workspace)
            )
            .is_err()
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_worker_panic_releases_slot_and_admits_successor() {
        use std::time::Duration;

        let runner = test_runner();
        runner.set_max_parallel(1);
        let mut registry = ExperimentRegistry::new();
        let panicking = mint_exp(&mut registry, "PanickingWorker");
        let successor = mint_exp(&mut registry, "Successor");
        runner
            .state
            .lock()
            .expect("runner state")
            .in_flight
            .insert(panicking.id);
        let (panic_tx, panic_rx) = unbounded();
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        start_native_job(runner.state.clone(), panicking.id, panic_tx, move || {
            release_rx.recv().expect("release worker");
            panic!("injected experiment compiler panic");
        });
        let successor_handle = runner.run_fast(&successor, test_source(&successor.model_ref.0));
        assert_eq!(runner.queued_count(), 1);
        release_tx.send(()).expect("release worker");

        let timeout = Duration::from_secs(5);
        assert!(matches!(
            panic_rx.recv_timeout(timeout),
            Ok(RunUpdate::Failed { error })
                if error.contains("injected experiment compiler panic")
        ));
        assert!(matches!(
            panic_rx.recv_timeout(timeout),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected)
        ));
        assert!(matches!(
            successor_handle.progress_rx.recv_timeout(timeout),
            Ok(RunUpdate::Progress { .. })
        ));
        assert!(matches!(
            successor_handle.progress_rx.recv_timeout(timeout),
            Ok(RunUpdate::Failed { error })
                if error.contains("test compiler rejection")
        ));
        assert!(matches!(
            successor_handle.progress_rx.recv_timeout(timeout),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected)
        ));
        assert_eq!(runner.in_flight_count(), 0);
        assert_eq!(runner.queued_count(), 0);
    }

    #[test]
    fn drain_handles_fails_disconnect_but_keeps_connected_empty_channel() {
        let mut registry = ExperimentRegistry::new();
        let connected = mint_exp(&mut registry, "Connected");
        let disconnected = mint_exp(&mut registry, "Disconnected");
        let (connected_tx, connected_rx) = unbounded();
        let (disconnected_tx, disconnected_rx) = unbounded::<RunUpdate>();
        drop(disconnected_tx);
        let mut origins = lunco_experiments::ExperimentOrigins::default();
        let mut admitted = ExperimentRegistry::new();
        let origin = lunco_experiments::ExperimentOrigin::LocalDocument(
            lunco_workspace::PinnedDocumentRuntimeOwner {
                document: lunco_doc::DocumentId::new(1),
                runtime: lunco_workspace::DocumentRuntimeOwner::Application,
            },
        );
        for exp in registry.iter_all() {
            origins
                .import(&mut admitted, origin.clone(), exp.clone())
                .expect("admitted fixture");
        }
        let mut app = App::new();
        app.insert_resource(admitted)
            .insert_resource(origins)
            .insert_resource(PendingHandles(vec![
                PendingRun {
                    artifact_admission: None,
                    result_limits: Default::default(),
                    origin: origin.clone(),
                    handle: RunHandle {
                        run_id: connected.id,
                        progress_rx: connected_rx,
                        cancel: Box::new(|| {}),
                    },
                },
                PendingRun {
                    artifact_admission: None,
                    result_limits: Default::default(),
                    origin,
                    handle: RunHandle {
                        run_id: disconnected.id,
                        progress_rx: disconnected_rx,
                        cancel: Box::new(|| {}),
                    },
                },
            ]))
            .add_message::<RunProgress>()
            .add_message::<RunCompleted>()
            .add_message::<RunFailed>()
            .add_message::<RunCancelled>()
            .add_systems(Update, drain_pending_handles);
        app.update();

        let pending = app.world().resource::<PendingHandles>();
        assert_eq!(pending.0.len(), 1);
        assert_eq!(pending.0[0].run_id, connected.id);
        let failures: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<RunFailed>>()
            .drain()
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].experiment_id, disconnected.id);
        assert!(
            failures[0]
                .error
                .contains("disconnected without a terminal outcome")
        );
        assert!(matches!(
            &app.world().resource::<ExperimentRegistry>().get(disconnected.id)
                .expect("disconnected experiment").status,
            RunStatus::Failed { error }
                if error.contains("disconnected without a terminal outcome")
        ));

        connected_tx
            .send(RunUpdate::Cancelled)
            .expect("terminal update");
        drop(connected_tx);
        app.update();
        assert!(app.world().resource::<PendingHandles>().0.is_empty());
        assert!(matches!(
            &app.world()
                .resource::<ExperimentRegistry>()
                .get(connected.id)
                .expect("connected experiment")
                .status,
            RunStatus::Cancelled
        ));
        assert_eq!(
            app.world().resource::<Messages<RunFailed>>().len(),
            0,
            "disconnect after a terminal update must preserve that outcome"
        );
    }

    #[test]
    fn completion_uses_captured_limits_after_settings_change() {
        let mut registry = ExperimentRegistry::new();
        let exp = mint_exp(&mut ExperimentRegistry::new(), "CapturedLimits");
        let id = exp.id;
        let origin = lunco_experiments::ExperimentOrigin::LocalDocument(
            lunco_workspace::PinnedDocumentRuntimeOwner {
                document: lunco_doc::DocumentId::new(1),
                runtime: lunco_workspace::DocumentRuntimeOwner::Application,
            },
        );
        let mut origins = lunco_experiments::ExperimentOrigins::default();
        origins
            .import(&mut registry, origin.clone(), exp)
            .expect("admitted fixture");
        registry.set_status(id, RunStatus::Queued);
        let limits = lunco_experiments::RunResultLimits {
            max_values: 6,
            max_artifact_bytes: 1024,
        };
        let (tx, rx) = unbounded();
        tx.send(RunUpdate::Completed(RunResult {
            times: vec![0.0, 1.0, 2.0],
            series: BTreeMap::from([("x".into(), vec![1.0, 2.0, 3.0])]),
            meta: RunMeta {
                sample_count: 3,
                ..Default::default()
            },
        }))
        .expect("worker outcome");
        let mut app = App::new();
        app.insert_resource(registry)
            .insert_resource(origins)
            .insert_resource(ExperimentSettings {
                result_limits: lunco_experiments::RunResultLimits {
                    max_values: 1,
                    ..limits
                },
                ..Default::default()
            })
            .insert_resource(PendingHandles(vec![PendingRun {
                artifact_admission: None,
                origin,
                result_limits: limits,
                handle: RunHandle {
                    run_id: id,
                    progress_rx: rx,
                    cancel: Box::new(|| {}),
                },
            }]))
            .add_message::<RunProgress>()
            .add_message::<RunCompleted>()
            .add_message::<RunFailed>()
            .add_message::<RunCancelled>()
            .add_systems(Update, drain_pending_handles);
        app.update();
        assert!(matches!(
            app.world()
                .resource::<ExperimentRegistry>()
                .get(id)
                .expect("retained result")
                .status,
            RunStatus::Done { .. }
        ));
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<RunCompleted>>()
                .drain()
                .count(),
            1
        );
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<RunFailed>>()
                .drain()
                .count(),
            0
        );
    }
}
