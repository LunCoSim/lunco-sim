//! Experiment runner — modelica/rumoca binding for the
//! [`lunco_experiments::ExperimentRunner`] trait.
//!
//! See `docs/architecture/25-experiments.md` for the design.
//!
//! ## Parameter / input injection
//!
//! Inputs and parameter overrides are applied at the **DAE level** by
//! rebinding each target variable's `start` after a single clean compile
//! (see [`apply_value_bindings_to_dae`]) — the authored source is never
//! mutated for an individual run. One compile is shared across a whole
//! sweep. A target that is neither a top-level DAE parameter nor input
//! (or a non-scalar value) is a hard error rather than a silent re-compile.
//!
//! - Fast Runs use the configured bounded scheduler on native and wasm.
//!
//! ## Compile-once parameter sweeps
//! Overrides are applied at the *DAE* level, not by reflattening per run.
//! `run_inner` compiles the source ONCE, caches the resulting `Dae` keyed by
//! a hash of the model source (`dae_cache`; see [`dae_cache_key`]), and for
//! each sweep point rebinds the target variables' `start` to literals via
//! [`apply_value_bindings_to_dae`]. This relies on rumoca's
//! `preserve_overridable_param_starts` fold (commit 6a849ac) keeping computed
//! derived parameters symbolic so they recompute at `SimulationSession::new`.
//! Invalid binding targets or non-scalar values produce a terminal run error.

#[cfg(target_arch = "wasm32")]
use lunco_core_runtime::LockExt;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
use std::collections::{BTreeMap, HashSet, VecDeque};
#[cfg(not(target_arch = "wasm32"))]
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use crossbeam_channel::{Sender, TryRecvError, unbounded};
use lunco_experiments::{
    Experiment, ExperimentId, ExperimentRegistry, ExperimentRunner, ModelRef, ParamPath,
    ParamValue, RunBounds, RunCancelled, RunCompleted, RunFailed, RunHandle, RunMeta, RunProgress,
    RunResult, RunStatus, RunUpdate,
};
use lunco_settings::SettingsSection;
use rumoca_compile::compile::Dae;
use serde::{Deserialize, Serialize};
// Used by `apply_value_bindings_to_dae` on both platforms: native and wasm
// inject run values at the DAE level.
use rumoca_compile::parsing::ir_core::{
    Expression as DaeExpression, Literal as DaeLiteral, Span as DaeSpan, VarName as DaeVarName,
};

/// Immutable Modelica source and runtime owner supplied at run admission.
#[derive(Clone, Debug)]
pub struct ModelSource {
    pub model_name: String,
    pub source: String,
    pub filename: String,
    pub extras: Vec<(String, String)>,
    pub runtime: lunco_workspace::DocumentRuntimeOwner,
}

/// Platform default for the number of runs allowed to execute
/// concurrently (the "auto" setting). Both branches leave one logical core
/// for the UI/main thread and clamp low; the user can override via
/// `experiments.max_parallel`.
///
/// Native: `available_parallelism() - 1`. Wasm: `hardwareConcurrency - 1`,
/// clamped tighter because each pooled worker is a full second wasm instance
/// carrying its own copy of the (large) source library bundle — so concurrency there
/// trades real memory, not just CPU. `hardwareConcurrency` is logical cores
/// (or 0/absent when the browser hides it → fall back to 1).
fn default_max_parallel() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).clamp(1, 4))
            .unwrap_or(2)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let cores = web_sys::window()
            .map(|w| w.navigator().hardware_concurrency())
            .filter(|n| n.is_finite() && *n >= 1.0)
            .map(|n| n as usize)
            .unwrap_or(1);
        cores.saturating_sub(1).clamp(1, 4)
    }
}

/// Persisted experiment-execution settings (`settings.json` key
/// `experiments`). Owned here, the feature that consumes it.
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct ExperimentSettings {
    /// Max Fast Runs allowed to execute concurrently. `None` (or `0`) means
    /// "auto" — the platform default ([`default_max_parallel`]). A user
    /// value is clamped to at least 1. Kept conservative by default because
    /// each concurrent run holds a full DAE + result buffer and (cache-cold)
    /// a rumoca compile; raise it to use more cores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
}

impl SettingsSection for ExperimentSettings {
    const KEY: &'static str = "experiments";
}

impl ExperimentSettings {
    /// Resolve to a concrete cap: the user value (clamped ≥1) when set and
    /// non-zero, else the platform default.
    pub fn resolved_max_parallel(&self) -> usize {
        match self.max_parallel {
            Some(n) if n >= 1 => n,
            _ => default_max_parallel(),
        }
    }
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

/// Native + wasm-shared scheduling and compiled snapshot cache.
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
    /// Source content and runtime owner identify reusable compiled snapshots.
    #[cfg(not(target_arch = "wasm32"))]
    dae_cache: HashMap<u64, CachedDae>,
    /// Persistent compiler reused across runs, so source library installs **once** for
    /// the runner (on first source-root admission via `ModelicaCompiler`) instead of
    /// rebuilding a fresh session per run. Behind its **own** lock, not the
    /// `state` mutex: a compile can take seconds, and holding `state` across
    /// it would stall the scheduler and every parallel run (which only need
    /// `state` briefly for cache lookups). Native only — the web build
    /// compiles on the worker, so the runner never constructs one. See
    /// [`run_inner`].
    #[cfg(not(target_arch = "wasm32"))]
    compiler: Arc<Mutex<lunco_modelica_compiler::ModelicaCompiler>>,
}

/// Source identity whose newer compiled revision replaces an older cache entry.
#[cfg(not(target_arch = "wasm32"))]
type ModelIdent = (String, String);

#[cfg(not(target_arch = "wasm32"))]
struct CachedDae {
    model: ModelIdent,
    runtime: lunco_workspace::DocumentRuntimeOwner,
    dae: Arc<Dae>,
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_cached_dae(
    state: &mut RunnerState,
    source: &ModelSource,
    key: u64,
    dae: Arc<Dae>,
    cancel: &AtomicBool,
) {
    // Called under the state lock: owner retirement sets cancellation before
    // taking the same lock, so closed work cannot repopulate its cache.
    if cancel.load(Ordering::SeqCst) {
        return;
    }
    let model = (source.model_name.clone(), source.filename.clone());
    state
        .dae_cache
        .retain(|_, entry| entry.model != model || entry.runtime != source.runtime);
    state.dae_cache.insert(
        key,
        CachedDae {
            model,
            runtime: source.runtime.clone(),
            dae,
        },
    );
}

impl Default for RunnerState {
    fn default() -> Self {
        Self {
            max_parallel: default_max_parallel(),
            in_flight: HashSet::new(),
            pending: VecDeque::new(),
            #[cfg(not(target_arch = "wasm32"))]
            dae_cache: HashMap::new(),
            // Cheap: `new()` builds an empty session and installs no source library
            // (Layer A). source library lands on the first run that actually needs it.
            #[cfg(not(target_arch = "wasm32"))]
            compiler: Arc::new(Mutex::new(lunco_modelica_compiler::ModelicaCompiler::new())),
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
                    partial: None,
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

impl Default for ModelicaRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelicaRunner {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RunnerState::default())),
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

    /// Release this runtime owner's compiled snapshots after its runs cancel.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn retire_runtime_cache(
        &self,
        owner: &lunco_workspace::DocumentRuntimeOwner,
    ) -> Result<(), String> {
        let mut state =
            lock_runner_state(&self.state).map_err(|_| POISONED_RUNNER_STATE.to_owned())?;
        state.dae_cache.retain(|_, entry| &entry.runtime != owner);
        Ok(())
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

        if let Err(error) = lunco_modelica_core::sim_target::validate_run_bounds(&exp.bounds) {
            let _ = tx.send(RunUpdate::Failed {
                error: error.to_string(),
                partial: None,
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
                    partial: None,
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
                    partial: None,
                });
            }
        });
    if let Err(error) = spawned {
        let _ = failure_tx.send(RunUpdate::Failed {
            error: format!("experiment worker could not start: {error}"),
            partial: None,
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
        delta: None,
    });
    let src = source;
    // Forward worker updates into the handle's tx; the forwarder frees the
    // slot via `finish_run` when a terminal update arrives.
    let Some(transport) = crate::worker_run_transport() else {
        let _ = tx.send(RunUpdate::Failed {
            error: "Modelica worker run transport is not installed".to_string(),
            partial: None,
        });
        finish_run(&state, run_id);
        return;
    };
    let (forward_tx, forward_rx) = unbounded::<RunUpdate>();
    (transport.register_run_sender)(run_id, forward_tx);
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

/// Body of the run thread. Compiles, runs the simulation, posts
/// updates. All errors funnel into `RunUpdate::Failed`. Cancellation
/// observed between steps via the shared `AtomicBool`.
/// Key for the compile-once DAE cache. Folds in model identity, the model
/// **source body**, and extra sources — but NOT overrides (those are applied to
/// the cached DAE), so a sweep that varies only overrides hits one cache entry.
///
/// CQ-525: the source body MUST be hashed in. Without it, editing a model's
/// body while keeping its name/filename produced an identical key, serving a
/// stale DAE; correctness then leaned entirely on the external whole-cache
/// clear. (The old `after_inputs` param was always `""` — inputs are bound at
/// the DAE level, not baked into source — so it contributed nothing and is
/// dropped.)
#[cfg(not(target_arch = "wasm32"))]
fn dae_cache_key(src: &ModelSource) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.runtime.hash(&mut h);
    src.model_name.hash(&mut h);
    src.filename.hash(&mut h);
    src.source.hash(&mut h);
    for (name, body) in &src.extras {
        name.hash(&mut h);
        body.hash(&mut h);
    }
    h.finish()
}

/// Apply value bindings — both parameter overrides AND experiment input
/// values — directly to a compiled DAE by rebinding each target variable's
/// `start` to a literal. This is THE single place run values are injected;
/// the authored source remains unchanged. Both the native runner and the wasm
/// worker call it, so the two platforms inject identically.
///
/// A target is looked up first among DAE `parameters`, then `inputs`:
/// - **Parameters** rely on rumoca's `preserve_overridable_param_starts` fold
///   keeping computed dependents symbolic, so overriding a base (e.g. `Isp`)
///   recomputes its dependents (e.g. `massRatio`) at `SimulationSession::new` via
///   `build_params`.
/// - **Inputs**: the build seeds each input's initial value from its `start`
///   once (the batch solver never calls `set_input`), so rebinding `start`
///   here pins the input to a constant for the run — the DAE-level analog of
///   the DAE-level equivalent of pinning an input for a batch run, with no
///   source mutation or second compile required.
///
/// Returns `Err` when a target is neither a parameter nor an input, or the
/// value isn't a scalar literal — a hard error so the run cannot diverge from
/// the compiled source contract.
pub fn apply_value_bindings_to_dae(
    dae: &mut Dae,
    bindings: &BTreeMap<ParamPath, ParamValue>,
) -> Result<(), String> {
    for (path, value) in bindings {
        let key = DaeVarName::new(path.0.clone());
        let var = dae
            .variables
            .parameters
            .get_mut(&key)
            .or_else(|| dae.variables.inputs.get_mut(&key))
            .ok_or_else(|| format!("'{}' is not a top-level DAE parameter or input", path.0))?;
        let lit = match value {
            ParamValue::Real(x) => DaeLiteral::Real(*x),
            ParamValue::Int(x) => DaeLiteral::Integer(*x),
            ParamValue::Bool(b) => DaeLiteral::Boolean(*b),
            ParamValue::String(s) => DaeLiteral::String(s.clone()),
            _ => return Err(format!("binding for '{}' is not a scalar literal", path.0)),
        };
        var.start = Some(DaeExpression::Literal {
            value: lit,
            span: DaeSpan::DUMMY,
        });
    }
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
    let _cancellation = rumoca_solver::SolverCancellationGuard::install(cancel.clone());
    let t_wall = web_time::Instant::now();

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Announce that this job has left the queue and is now executing. The
    // batch path is one blocking `simulate_with_diagnostics` call that emits
    // no mid-solve `Progress`, so without this the registry status would sit
    // at `Queued` (set at dispatch) for the entire compile + solve and only
    // flip straight to `Done` — making a long-grinding run look stuck in a
    // queue. `drain_pending_handles` maps any `Progress` to
    // `RunStatus::Running`, so this single emit flips the label to "Running"
    // the moment the worker thread picks the job up (covering compile time
    // too, not just the solve). Interactive runs additionally stream their
    // own per-step `Progress` with the real `t_current`.
    let _ = tx.send(RunUpdate::Progress {
        t_current: bounds.t_start,
        delta: None,
    });

    // Persistent runner compiler: clone the handle out of `state` (brief
    // lock), then compile under the compiler's OWN lock so the multi-second
    // compile never holds `state` and stall the scheduler / parallel runs.
    // source library installs once into this session (lazily) instead of per run.
    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }
    let compiler_handle = match lock_runner_state(&state) {
        Ok(s) => s.compiler.clone(),
        Err(_) => {
            let _ = tx.send(RunUpdate::Failed {
                error: "runner state poisoned".to_string(),
                partial: None,
            });
            return;
        }
    };

    // Compile-once: compile the CLEAN model source (no inputs/overrides baked
    // in) a single time and cache the resulting DAE. Inputs and overrides are
    // applied to the DAE afterwards (see `apply_value_bindings_to_dae`), so a
    // whole parameter/input sweep shares ONE compile and recompiles zero times.
    let key = dae_cache_key(&source);
    let cached = state
        .lock()
        .ok()
        .and_then(|s| s.dae_cache.get(&key).map(|entry| entry.dae.clone()));
    let base_dae: Arc<Dae> = match cached {
        Some(d) => d,
        None => {
            let compiled = {
                let mut compiler = match compiler_handle.lock() {
                    Ok(compiler) => compiler,
                    Err(_) => {
                        let _ = tx.send(RunUpdate::Failed {
                            error: "experiment compiler is poisoned after an earlier panic"
                                .to_owned(),
                            partial: None,
                        });
                        return;
                    }
                };
                if cancel.load(Ordering::SeqCst) {
                    let _ = tx.send(RunUpdate::Cancelled);
                    return;
                }
                let result = compiler.compile_str_multi(
                    &source.model_name,
                    &source.source,
                    &source.filename,
                    &source.extras,
                );
                compiler.clear_user_documents();
                result
            };
            match compiled {
                Ok(d) => {
                    let dae = d.dae.clone();
                    if let Ok(mut s) = lock_runner_state(&state) {
                        publish_cached_dae(&mut s, &source, key, dae.clone(), &cancel);
                    }
                    dae
                }
                Err(e) => {
                    let _ = tx.send(RunUpdate::Failed {
                        error: format!("compile failed: {e}"),
                        partial: None,
                    });
                    return;
                }
            }
        }
    };

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Merge inputs + overrides into one binding set and apply at the DAE level
    // — the SINGLE injection path, no source rewriting. Inputs and parameter
    // overrides are the same operation (pin a top-level variable to a value),
    // so they share `apply_value_bindings_to_dae`. An empty set reuses the
    // cached base DAE untouched.
    let mut bindings = inputs;
    bindings.extend(overrides);
    let run_dae: Arc<Dae> = if bindings.is_empty() {
        base_dae
    } else {
        let mut d = (*base_dae).clone();
        if let Err(reason) = apply_value_bindings_to_dae(&mut d, &bindings) {
            let _ = tx.send(RunUpdate::Failed {
                error: format!("parameter/input binding failed: {reason}"),
                partial: None,
            });
            return;
        }
        Arc::new(d)
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
    let mut sink = ChannelSink { tx, cancel };
    drive_run(&run_dae, &bounds, t_wall, &mut sink);
}

/// THE single simulation entry point, shared by the native runner
/// ([`run_inner`]) and the wasm worker (`lunica_worker::run_fast_in_worker`).
///
/// Branches on [`RunBounds::runtime`](lunco_experiments::RunBounds::runtime):
///   * [`Batch`](lunco_experiments::RuntimeMode::Batch) (default) — the
///     dense-output [`run_batch_sim`] solve. The solver free-steps with its
///     own adaptive step and output is dense-interpolated, so the solver step
///     is decoupled from the output `dt`. This is why stiff long-horizon
///     models (the lunar rover thermal system, the orbital-datacenter eclipse
///     switch) complete here, where the interactive stepper — bounded by the
///     output `dt` — collapses (`BDF step: step size too small at t=0`) unless
///     `dt` is driven down to a few thousand seconds.
///   * [`Interactive`](lunco_experiments::RuntimeMode::Interactive) — the live
///     [`run_stepping_loop`], for streamable / steerable runs.
///
/// Keeping this one function (rather than each platform open-coding the match)
/// is what guarantees the worker can't fall back to a stepper-only path again:
/// the divergence that made stiff models run natively but fail in the browser.
pub fn drive_run(
    dae: &Dae,
    bounds: &RunBounds,
    started: web_time::Instant,
    sink: &mut impl RunSink,
) {
    if sink.is_cancelled() {
        sink.emit(RunUpdate::Cancelled);
        return;
    }
    let output_dt = match lunco_modelica_core::sim_target::validate_run_bounds(bounds) {
        Ok(dt) => dt,
        Err(error) => {
            sink.emit(RunUpdate::Failed {
                error: error.to_string(),
                partial: None,
            });
            return;
        }
    };
    // Solver options (tolerance / family / initial step) — the SINGLE source
    // (`stepper_options_from_bounds`) both runtimes derive from. An incapable
    // authored solver fails the run here rather than per-step later.
    let stepper_opts = match stepper_options_from_bounds(bounds) {
        Ok(opts) => opts,
        Err(err) => {
            sink.emit(RunUpdate::Failed {
                error: format!("run configuration failed: {err}"),
                partial: None,
            });
            return;
        }
    };
    match bounds.runtime {
        lunco_experiments::RuntimeMode::Batch => {
            // The batch solver reads its output grid from `opts.dt` (one column
            // per `dt` step). `stepper_options_from_bounds` leaves `opts.dt` as
            // the *initial step* (`h0`) for the interactive path; for batch we
            // instead resolve the requested output spacing — from either the
            // `Interval` (`dt`) or the `NumberOfIntervals` (`n_intervals`) knob
            // — so the run honours whichever the user chose. The solver
            // free-steps regardless, so a coarse output grid doesn't constrain
            // it (and `run_batch_sim` decimates the event-flooded result back
            // to that grid).
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
            run_batch_sim(dae, &batch_opts, started, sink);
        }
        lunco_experiments::RuntimeMode::Interactive => {
            let mut stepper =
                match lunco_modelica_solver::simulation_session::interactive(dae, stepper_opts) {
                    Ok(s) => s,
                    Err(e) => {
                        sink.emit(RunUpdate::Failed {
                            error: format!("stepper init failed: {e:?}"),
                            partial: None,
                        });
                        return;
                    }
                };
            bevy::log::info!(
                "[runner] simulate begin (interactive): t={}..{} dt={:?}",
                bounds.t_start,
                bounds.t_end,
                bounds.dt
            );
            run_stepping_loop(&mut stepper, bounds, started, sink);
        }
    }
}

/// Drive a [`Dae`] to `t_end` through the non-interactive dense-output batch
/// solver ([`rumoca_sim::simulate_with_diagnostics`]) and emit the trajectory
/// as a single [`RunUpdate::Completed`]. Unlike [`run_stepping_loop`], the
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
                partial: None,
            });
            return;
        }
    };
    let result = match rumoca_sim::simulate_with_diagnostics(dae, opts) {
        Ok(r) => r,
        Err(e) => {
            if sink.is_cancelled() {
                sink.emit(RunUpdate::Cancelled);
                return;
            }
            sink.emit(RunUpdate::Failed {
                error: format!("simulate failed: {e}"),
                partial: None,
            });
            return;
        }
    };

    if sink.is_cancelled() {
        sink.emit(RunUpdate::Cancelled);
        return;
    }

    // Decimate to the requested output grid. rumoca's batch solver builds the
    // grid from `opts.dt` correctly, but ALSO records an extra sample at every
    // root/event crossing. Models with chattering discontinuities (e.g. an
    // orbit `mod(time, period)` eclipse switch, or `if`-gated thresholds that
    // re-trigger near a boundary) flood the trajectory with millions of event
    // samples — a 2-orbit run of the orbital-datacenter model returned ~5M
    // samples for a requested 1.1k-point grid (~4 GB across 75 vars; OOMs the
    // wasm worker outright). Collapse back to the requested grid before storing
    // / sending: for each grid time keep the nearest available sample. Guarded
    // so the well-behaved exact-grid case (smooth models already return the
    // grid) is left untouched.
    let keep = match batch_keep_indices(&result.times, opts.t_start, opts.t_end, output_dt) {
        Ok(keep) => keep,
        Err(error) => {
            sink.emit(RunUpdate::Failed {
                error: error.to_string(),
                partial: None,
            });
            return;
        }
    };
    let raw_samples = result.times.len();
    let (kept_times, gather): (Vec<f64>, Option<Vec<usize>>) = match &keep {
        Some(idx) => (
            idx.iter().map(|&i| result.times[i]).collect(),
            Some(idx.clone()),
        ),
        None => (result.times.clone(), None),
    };

    // SimResult stores one column per visible variable (`data[var][t]`),
    // mirroring the `RunResult` series map keyed by variable name. Drop the
    // synthetic `time` column — `times` carries it.
    let mut series: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (i, name) in result.names.iter().enumerate() {
        if name == "time" {
            continue;
        }
        let full = result.data.get(i);
        let column: Vec<f64> = match (&gather, full) {
            (Some(idx), Some(col)) => idx
                .iter()
                .map(|&j| col.get(j).copied().unwrap_or(f64::NAN))
                .collect(),
            (None, Some(col)) => col.clone(),
            (_, None) => Vec::new(),
        };
        series.insert(name.clone(), column);
    }

    let n_samples = kept_times.len();
    if keep.is_some() {
        bevy::log::info!(
            "[sim] simulate ok (batch): {} samples (decimated from {} solver/event samples), {} vars, {:.2}s wall",
            n_samples,
            raw_samples,
            series.len(),
            started.elapsed().as_secs_f64(),
        );
    } else {
        bevy::log::info!(
            "[sim] simulate ok (batch): {} samples, {} vars, {:.2}s wall",
            n_samples,
            series.len(),
            started.elapsed().as_secs_f64(),
        );
    }
    sink.emit(RunUpdate::Completed(RunResult {
        times: kept_times,
        series,
        meta: RunMeta {
            wall_time_ms: started.elapsed().as_millis() as u64,
            sample_count: n_samples,
            notes: None,
        },
    }));
}

/// Pick the sample indices to keep so a batch trajectory matches the requested
/// output grid. rumoca emits the `opts.dt` grid PLUS an extra sample at every
/// event/root crossing; for chattering models that's millions of points. For
/// each grid time `t_start + k·dt` we keep the trajectory sample whose time is
/// nearest (advancing a single monotonic cursor — O(n)), always keeping the
/// first and last sample. Invalid grids are rejected before integer conversion
/// or allocation. Returns `None` when the trajectory is at/under the grid size (the
/// well-behaved smooth-model case — leave it byte-for-byte untouched).
fn batch_keep_indices(
    times: &[f64],
    t_start: f64,
    t_end: f64,
    dt: f64,
) -> Result<Option<Vec<usize>>, lunco_modelica_core::sim_target::RunBoundsError> {
    let dt = lunco_modelica_core::sim_target::resolve_step_dt(t_start, t_end, Some(dt), None)?;
    if times.len() < 2 {
        return Ok(None);
    }
    let span = t_end - t_start;
    // +1 for the inclusive endpoint; small slack so we never over-decimate a
    // result that already sits on (or just above) the grid.
    let grid_n = (span / dt).round() as usize + 1;
    if times.len() <= grid_n.saturating_mul(2) {
        return Ok(None);
    }
    let mut keep: Vec<usize> = Vec::with_capacity(grid_n + 1);
    let mut cursor = 0usize;
    let n = times.len();
    for k in 0..=grid_n {
        let target = t_start + (k as f64) * dt;
        // Advance while the next sample is closer to `target` than the current.
        while cursor + 1 < n && (times[cursor + 1] - target).abs() <= (times[cursor] - target).abs()
        {
            cursor += 1;
        }
        if keep.last() != Some(&cursor) {
            keep.push(cursor);
        }
    }
    // Guarantee the true final sample is present (events can land past the last
    // grid node).
    if keep.last() != Some(&(n - 1)) {
        keep.push(n - 1);
    }
    Ok(Some(keep))
}

/// Cancellation + update sink for a stepping run. This is the ONE seam
/// between the native runner and the wasm worker: both share
/// [`run_stepping_loop`] and differ only in these two methods (native =
/// crossbeam channel + `AtomicBool`; worker = `postMessage` + cancel
/// registry). Keeping the loop itself platform-agnostic is what stops the
/// two from drifting — the divergence that let a stale `unwrap_or(0.01)`
/// OOM-trap the browser while native was already fixed.
pub trait RunSink {
    /// True if the run was cancelled; the loop emits `Cancelled` and stops.
    fn is_cancelled(&mut self) -> bool;
    /// Deliver a run update (`Progress` / `Completed` / `Failed` / `Cancelled`).
    fn emit(&mut self, update: RunUpdate);
    /// Optional wall-clock backstop. When `Some(d)`, a run still stepping
    /// after `d` of wall time is aborted with a graceful `Failed` instead of
    /// being allowed to run unbounded. Returns `None` by default (native
    /// runs are unbounded); the wasm worker overrides it so a pathological
    /// in-browser run can't hog its (off-thread) worker forever. Note this
    /// fires only *between* solver steps — it can't interrupt a single
    /// runaway `step()`; that case is contained by worker-crash recovery on
    /// the main thread (`worker_transport`).
    fn wall_budget(&self) -> Option<core::time::Duration> {
        None
    }
}

/// Native [`RunSink`]: crossbeam channel + shared atomic cancel flag.
#[cfg(not(target_arch = "wasm32"))]
struct ChannelSink {
    tx: Sender<RunUpdate>,
    cancel: Arc<AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl RunSink for ChannelSink {
    fn is_cancelled(&mut self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    fn emit(&mut self, update: RunUpdate) {
        let _ = self.tx.send(update);
    }
}

/// Build the solver [`SimOptions`](rumoca_sim::SimOptions) from [`RunBounds`].
/// The SINGLE source of the tolerance / solver-family / initial-step defaults,
/// shared by native and the wasm worker so they can't drift.
///
/// Defaults follow the Modelica convention, tuned for what works across the
/// stiff multi-day horizons:
///   * tolerance default = 1e-6 (the standard Modelica / OMC / Dymola default).
///     The non-interactive batch runtime (the default) honours it on the stiff
///     thermal models; per-run editable via the experiments Setup dialog.
///   * solver default = BDF. (Earlier this was TR-BDF2, on the rationale that
///     "BDF dies at the second sunrise louver crossing" — but that predates the
///     rumoca solve-IR fix for connection flow-sum unknowns. With that fix BDF
///     now completes the full multi-lunar-cycle horizon on the stiff thermal
///     models, while the diffsol 0.13 SDIRK tableaus (TR-BDF2 / ESDIRK34) hit
///     "nonlinear solver failures (50)" within the first lunar hour on the same
///     models. BDF is the robust default again; the SDIRK tableaus stay opt-in.)
///
/// Background: docs/numeric-experiments/README.md § Known working solver configurations
///
/// The worker's live interactive sim (`lunco_modelica_worker::worker::build_stepper`) previously had
/// its OWN defaults (`rtol = 1e-3`, `atol = 1e-6`) with *no* solver selection,
/// silently running BDF where the batch path runs TR-BDF2. `build_stepper` now
/// delegates here, collapsing that divergence: one tolerance default, one
/// `atol`/`rtol` policy, one solver family across live + batch.
/// Default solver tolerance when neither the run bounds nor the model's
/// `experiment(Tolerance=…)` annotation supplies one. Applied to BOTH `atol`
/// and `rtol` (scalar). This is the standard Modelica default (`1e-6`, the same
/// value OMC/Dymola assume): with the non-interactive batch runtime as the
/// default ([`RuntimeMode::Batch`](lunco_experiments::RuntimeMode::Batch)) the
/// dense-output solver honours it across the stiff multi-cycle horizons (the
/// lunar rover thermal system completes its full two-lunar-day run at `1e-6`),
/// so there's no reason to loosen below the Modelica convention. It stays
/// per-run editable through the experiments Setup dialog (`bounds.tolerance`).
///
/// **Batch only.** The LIVE interactive/co-simulated path deliberately does NOT
/// share this policy: an adaptive-implicit solver whose step sequence comes
/// from per-machine error estimates must not run inside the client-predicted
/// fixed-step loop. It has its own configuration in
/// `lunco_modelica_worker::worker::live_stepper_options`
/// (explicit family, fixed micro-step ladder, fixed tolerance). The two surfaces
/// are meant to differ here — see
/// `docs/architecture/28-modelica-realtime-physics.md` §2a.
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

/// The ONE place `SimOptions` is built for an offline/batch run — the app, the
/// wasm worker, `modelica_run` and `modelica_tester` all come through here
/// rather than hand-rolling options, so a policy change lands everywhere at once.
///
/// Carrying `bounds.t_start/t_end` through is load-bearing on BOTH runtimes, and
/// silently so:
/// * the non-interactive batch solve (`simulate_with_diagnostics`) integrates
///   `opts.t_start..opts.t_end`. Left at the `SimOptions::default()` `0.0..1.0`,
///   a run stops at t=1 with a fine output grid that pins the solver onto
///   closely spaced stop-times and collapses ("step size too small") in the
///   first second.
/// * `SimulationSession` (rumoca ≥0.9.20) **clamps every `step`/`advance_to` at
///   `opts.t_end`**. With the default horizon an interactive run parks at t=1s
///   and quietly reports a frozen model instead of erroring — the failure mode
///   that made a 60 s rocket burn drain exactly 1 s of propellant.
///
/// Both this and the live path resolve through `lunco_experiments::solver`, which
/// is the point: two paths choosing independently is how a live model ends up on
/// a family the batch path would have refused.
///
/// Fallible on purpose. A solver that cannot be resolved or built is reported to
/// the caller, never swapped for one that happens to work: a run that silently
/// used a different integrator than the one asked for produces numbers the user
/// attributes to their choice.
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

/// Emit a `Failed` update carrying everything sampled so far as a partial
/// result. The three ways a run dies mid-loop (wall-clock budget, a solver
/// step error, an unreadable session state) all report the same shape.
fn emit_partial_failure(
    sink: &mut impl RunSink,
    names: &[String],
    all_times: &[f64],
    all_series: &[Vec<f64>],
    started: web_time::Instant,
    error: String,
    notes: String,
) {
    let series: BTreeMap<String, Vec<f64>> = names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), all_series[i].clone()))
        .collect();
    sink.emit(RunUpdate::Failed {
        error,
        partial: Some(RunResult {
            times: all_times.to_vec(),
            series,
            meta: RunMeta {
                wall_time_ms: started.elapsed().as_millis() as u64,
                sample_count: all_times.len(),
                notes: Some(notes),
            },
        }),
    });
}

/// Drive a built [`SimulationSession`](rumoca_sim::SimulationSession) to `bounds.t_end`,
/// accumulating output samples at the spacing
/// [`lunco_modelica_core::sim_target::resolve_step_dt`]
/// derives, streaming throttled `Progress` deltas, and emitting the terminal
/// `Completed` / `Failed` / `Cancelled` through `sink`.
///
/// This is the SINGLE stepping loop shared by every platform — native and the
/// wasm worker each implement only [`RunSink`], never the loop. `started` is
/// the run's wall-clock origin (used for `RunMeta::wall_time_ms`).
pub fn run_stepping_loop(
    stepper: &mut rumoca_sim::SimulationSession,
    bounds: &RunBounds,
    started: web_time::Instant,
    sink: &mut impl RunSink,
) {
    let t_end = bounds.t_end;
    let step_dt = match lunco_modelica_core::sim_target::validate_run_bounds(bounds) {
        Ok(dt) => dt,
        Err(error) => {
            sink.emit(RunUpdate::Failed {
                error: error.to_string(),
                partial: None,
            });
            return;
        }
    };

    bevy::log::info!(
        "[sim] simulate begin: t={}..{} step_dt={}",
        bounds.t_start,
        t_end,
        step_dt
    );

    // Variable names from the initial state.
    let names: Vec<String> = match stepper.state() {
        Ok(state) => state
            .values
            .keys()
            .filter(|n| *n != "time")
            .cloned()
            .collect(),
        Err(e) => {
            bevy::log::warn!("[sim] initial state read failed: {e:?}");
            sink.emit(RunUpdate::Failed {
                error: format!("simulate failed before the first sample: {e:?}"),
                partial: None,
            });
            return;
        }
    };

    let mut all_times: Vec<f64> = Vec::new();
    let mut all_series: Vec<Vec<f64>> = vec![Vec::new(); names.len()];
    let mut last_emit_idx = 0;
    let mut last_progress_emit = web_time::Instant::now();

    // Output grid spacing the user asked for (`Interval` / `numberOfIntervals`
    // via `resolve_step_dt`). RECORDED identically on native and web, so the
    // emitted sample count honours the request on both — no more wasm-only
    // fixed ~`SAMPLE_CAP` point count.
    let output_dt = step_dt;
    // Solver advance per loop iteration. On wasm we bound it to a memory-safe
    // cadence (the worker's linear memory climbs between *output* samples and
    // only stays bounded when the solver is sampled frequently — see obs:
    // step≈25s completes, 50s OOM-traps), but that cadence drives the SOLVER,
    // not what we keep: output is decimated to `output_dt` below. Native steps
    // straight to the next output point. This is the one place the two targets
    // differ, and it no longer leaks into the user-visible sample count.
    #[cfg(target_arch = "wasm32")]
    let internal_dt = {
        let span = (t_end - bounds.t_start).max(0.0);
        output_dt
            .min(25.0)
            .max(span / lunco_modelica_core::sim_target::SAMPLE_CAP)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let internal_dt = output_dt;
    if let Err(error) = lunco_modelica_core::sim_target::resolve_step_dt(
        bounds.t_start,
        t_end,
        Some(internal_dt),
        None,
    ) {
        sink.emit(RunUpdate::Failed {
            error: format!("internal solver cadence: {error}"),
            partial: None,
        });
        return;
    }
    // Next output-grid time we still owe a sample for.
    let mut next_output = bounds.t_start + output_dt;

    while stepper.time() < t_end {
        if sink.is_cancelled() {
            sink.emit(RunUpdate::Cancelled);
            return;
        }

        // Wall-clock backstop (worker-only by default). Fail gracefully
        // rather than letting a too-heavy run monopolise its worker.
        if let Some(budget) = sink.wall_budget() {
            if started.elapsed() > budget {
                bevy::log::warn!(
                    "[sim] wall-time budget {:.0}s exceeded at t={:.3}/{} — aborting run",
                    budget.as_secs_f64(),
                    stepper.time(),
                    t_end
                );
                emit_partial_failure(
                    sink,
                    &names,
                    &all_times,
                    &all_series,
                    started,
                    format!(
                        "run exceeded the {:.0}s wall-time budget at sim t={:.1}/{:.1} \
                         — model too heavy for this environment; try a shorter StopTime \
                         or a looser Tolerance",
                        budget.as_secs_f64(),
                        stepper.time(),
                        t_end
                    ),
                    "aborted: wall-time budget exceeded".to_string(),
                );
                return;
            }
        }

        if let Err(e) = stepper.step(internal_dt) {
            bevy::log::warn!("[sim] simulate err: {e:?}");
            emit_partial_failure(
                sink,
                &names,
                &all_times,
                &all_series,
                started,
                format!("simulate failed: {e:?}"),
                format!("failed during step: {e:?}"),
            );
            return;
        }

        let t = stepper.time();
        // Record on the requested output grid only. Internal sub-steps (the
        // wasm memory cadence) between grid points are dropped, so the stored
        // sample count equals span/output_dt on every target — the user's
        // `dt`/`numberOfIntervals` is honoured identically native and web.
        // Always keep the final sample once we reach `t_end`.
        let at_end = t >= t_end;
        if !(t + 0.5 * internal_dt >= next_output || at_end) {
            continue;
        }
        let current_values = match stepper.state() {
            Ok(state) => state.values,
            Err(e) => {
                bevy::log::warn!("[sim] state read err: {e:?}");
                emit_partial_failure(
                    sink,
                    &names,
                    &all_times,
                    &all_series,
                    started,
                    format!("simulate failed: {e:?}"),
                    format!("failed reading state at t={t}: {e:?}"),
                );
                return;
            }
        };
        all_times.push(t);
        for (i, name) in names.iter().enumerate() {
            // CQ-522: a missing variable is an honest gap, not 0.0 — match
            // the batch path (`f64::NAN`) so plots don't show a fabricated
            // zero where the stepper had no value.
            let val = current_values.get(name).copied().unwrap_or(f64::NAN);
            all_series[i].push(val);
        }
        // Advance the grid cursor past the sample we just stored so the next
        // grid point is in the future (skips any we overshot in one step).
        while next_output <= t {
            next_output += output_dt;
        }

        // Throttle progress to ~10 Hz to avoid flooding the consumer and
        // incurring excessive serialization overhead.
        if last_progress_emit.elapsed().as_millis() > 100 {
            let delta_times = all_times[last_emit_idx..].to_vec();
            let mut delta_series = BTreeMap::new();
            for (i, name) in names.iter().enumerate() {
                delta_series.insert(name.clone(), all_series[i][last_emit_idx..].to_vec());
            }
            sink.emit(RunUpdate::Progress {
                t_current: t,
                delta: Some(RunResult {
                    times: delta_times,
                    series: delta_series,
                    meta: RunMeta {
                        wall_time_ms: started.elapsed().as_millis() as u64,
                        sample_count: all_times.len() - last_emit_idx,
                        notes: None,
                    },
                }),
            });
            last_progress_emit = web_time::Instant::now();
            last_emit_idx = all_times.len();
        }
    }

    bevy::log::info!(
        "[sim] simulate ok: {} samples, {} vars, {:.2}s wall",
        all_times.len(),
        names.len(),
        started.elapsed().as_secs_f64(),
    );

    let mut final_series = BTreeMap::new();
    for (i, name) in names.iter().enumerate() {
        final_series.insert(name.clone(), all_series[i].clone());
    }
    let n_samples = all_times.len();
    sink.emit(RunUpdate::Completed(RunResult {
        times: all_times,
        series: final_series,
        meta: RunMeta {
            wall_time_ms: started.elapsed().as_millis() as u64,
            sample_count: n_samples,
            notes: None,
        },
    }));
}

/// One detected top-level `parameter` declaration. Used by the
/// override editor UI to render an editable row per parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedParam {
    pub name: String,
    /// The parameter's declared type, e.g. `Real`, `Integer`, `Boolean`,
    /// `String`, or a class-qualified type. Used to choose the editor
    /// widget kind.
    pub type_name: String,
    /// Default literal as it appears in the source (e.g. `1.5`,
    /// `"foo"`, `true`). `None` if the parameter has no literal
    /// default (e.g. expression-bound, inherited).
    pub default_literal: Option<String>,
    /// Whether the DAE-level value-binding override path can rebind this
    /// declaration's `start`. False for non-literal RHS (expression
    /// bindings), for arrays / records, and for params not found at the
    /// top level.
    pub supportable: bool,
    /// Reason override is unsupported, when `!supportable`. Surfaced
    /// in the editor as a tooltip.
    pub reason: Option<String>,
    /// The Modelica description-comment string (e.g. the `"Gravity"` in
    /// `parameter Real g = 9.81 "Gravity";`), if present. Shown as hover
    /// help on the parameter row in the override editor.
    pub description: Option<String>,
}

/// One detected top-level `input` declaration. Modelica `input` vars
/// have no defaults — at runtime the stepper sets them via
/// `set_input(name, value)`. For batch Fast Run the runner pins them through
/// the same compiled-DAE binding path used for parameter overrides.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedInput {
    pub name: String,
    pub type_name: String,
}

/// Top-level `input` declarations of `class`, read from the **parsed
/// AST** — no source regex, no reparse. Used by the Setup dialog to
/// render an Inputs section and by the runner to pin input values.
///
/// Sourced via [`lunco_modelica_ast::ast_extract::extract_typed_inputs_for_class`],
/// which classifies by the parser's `causality` and additionally
/// catches connector-typed inputs (`RealInput`/`IntegerInput`/…) through the
/// parser's typed class representation.
pub fn detect_top_level_inputs(
    class: &rumoca_compile::parsing::ast::ClassDef,
) -> Vec<DetectedInput> {
    lunco_modelica_ast::ast_extract::extract_typed_inputs_for_class(class)
        .into_iter()
        .map(|c| DetectedInput {
            name: c.name,
            type_name: c.type_name,
        })
        .collect()
}

/// Top-level `parameter` declarations of `class`, read from the
/// **parsed AST** — no source regex, no reparse. Drives the override
/// editor's table.
///
/// Sourced via [`lunco_modelica_ast::ast_extract::extract_typed_parameters_for_class`]
/// (filters on the parser's `variability == Parameter`), which gives the
/// declared default and the Modelica description-comment directly, so the
/// editor no longer textually re-scans the file every frame.
///
/// **Supportability** reflects the actual injection path: run values are
/// applied by [`apply_value_bindings_to_dae`], which rebinds a DAE
/// parameter's `start` to a scalar literal. Every top-level *scalar*
/// parameter is therefore overridable — default-bound or not, literal or
/// computed. Only array/record parameters (which can't be rebound to a
/// scalar) are greyed out. This is strictly more capable than the old
/// textual heuristic, which refused any parameter whose default wasn't a
/// bare literal.
///
/// Inherited parameters (declared in an `extends` base) are not yet
/// surfaced — they don't live in this class's own component list.
pub fn detect_top_level_literal_parameters(
    class: &rumoca_compile::parsing::ast::ClassDef,
) -> Vec<DetectedParam> {
    lunco_modelica_ast::ast_extract::extract_typed_parameters_for_class(class)
        .into_iter()
        .map(|c| {
            // Arrays/records can't be rebound to a scalar literal by the
            // DAE value-binding path, so grey those rows; bracket in the
            // type name is the cheap, reliable array signal.
            let is_array = c.type_name.contains('[');
            DetectedParam {
                name: c.name,
                type_name: c.type_name,
                // `default` carries the numeric literal binding (or `start`)
                // straight from the AST; non-numeric defaults show as "—"
                // but are still overridable.
                default_literal: c.default.map(|x| format!("{x}")),
                supportable: !is_array,
                reason: is_array.then(|| "array/record parameters can't be overridden".to_string()),
                description: {
                    let d = c.description.trim();
                    if d.is_empty() {
                        None
                    } else {
                        Some(d.to_string())
                    }
                },
            }
        })
        .collect()
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
    /// while the runner applies both through one DAE-level binding path.
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
    pub handle: RunHandle,
    pub origin: lunco_experiments::ExperimentOrigin,
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
#[derive(Resource, Default)]
pub struct PlaybackEntities(
    pub std::collections::HashMap<lunco_doc::DocumentId, bevy::prelude::Entity>,
);

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
                    partial: None,
                },
            };
            match update {
                RunUpdate::Progress { t_current, delta } => {
                    registry.set_status(handle.run_id, RunStatus::Running { t_current });
                    if let Some(d) = delta {
                        registry.merge_result(handle.run_id, d);
                    }
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
                        "[experiments] run {:?} done: {} samples, {} vars, {} ms",
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
                    registry.set_result(handle.run_id, result);
                    registry.set_status(handle.run_id, RunStatus::Done { wall_time_ms: wall });
                    ev_completed.write(RunCompleted {
                        experiment_id: handle.run_id,
                        origin: handle.origin.clone(),
                    });
                    terminal = true;
                }
                RunUpdate::Failed { error, partial } => {
                    bevy::log::warn!("[experiments] run {:?} failed: {error}", handle.run_id);
                    let had_partial = partial.is_some();
                    if let Some(p) = partial {
                        registry.set_result(handle.run_id, p);
                    }
                    registry.set_status(
                        handle.run_id,
                        RunStatus::Failed {
                            error: error.clone(),
                            partial: had_partial,
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

    // ── Step 2: settings / cap resolution ──

    #[test]
    fn default_max_parallel_is_at_least_one() {
        assert!(default_max_parallel() >= 1);
    }

    #[test]
    fn resolved_max_parallel_honours_setting_and_falls_back() {
        assert_eq!(
            ExperimentSettings {
                max_parallel: Some(3)
            }
            .resolved_max_parallel(),
            3
        );
        assert_eq!(
            ExperimentSettings {
                max_parallel: Some(1)
            }
            .resolved_max_parallel(),
            1
        );
        // None and the 0 sentinel both fall back to the platform auto
        // default, which is always a valid (≥1) cap.
        assert!(ExperimentSettings { max_parallel: None }.resolved_max_parallel() >= 1);
        assert!(
            ExperimentSettings {
                max_parallel: Some(0)
            }
            .resolved_max_parallel()
                >= 1
        );
    }

    #[test]
    fn set_max_parallel_floors_at_one() {
        let r = ModelicaRunner::new();
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
        }
    }

    #[test]
    fn poisoned_runner_state_rejects_queued_and_new_runs_without_rescheduling() {
        let runner = ModelicaRunner::new();
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
                matches!(handle.progress_rx.try_recv(), Ok(RunUpdate::Failed { error, partial: None }) if error == POISONED_RUNNER_STATE)
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
        let runner = ModelicaRunner::new();
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

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cache_retirement_keeps_other_owners_and_rejects_late_cancelled_publish() {
        let runner = ModelicaRunner::new();
        let twin = lunco_workspace::TwinId::new(1);
        let mut source = test_source("CacheProbe");
        let mut owned = source.clone();
        owned.runtime = lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin);
        let mut other = source.clone();
        other.runtime =
            lunco_workspace::DocumentRuntimeOwner::LocalTwin(lunco_workspace::TwinId::new(2));
        let cancel = AtomicBool::new(false);
        for input in [&source, &owned, &other] {
            publish_cached_dae(
                &mut runner.state.lock().expect("state"),
                input,
                dae_cache_key(input),
                Arc::new(Dae::default()),
                &cancel,
            );
        }
        assert_eq!(runner.state.lock().expect("state").dae_cache.len(), 3);
        cancel.store(true, Ordering::SeqCst);
        runner
            .retire_runtime_cache(&lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin))
            .expect("cache retirement");
        {
            let mut state = runner.state.lock().expect("state");
            publish_cached_dae(
                &mut state,
                &owned,
                dae_cache_key(&owned),
                Arc::new(Dae::default()),
                &cancel,
            );
            assert_eq!(state.dae_cache.len(), 2);
            assert!(!state.dae_cache.contains_key(&dae_cache_key(&owned)));
        }
        // A newer application snapshot replaces only its own model revision.
        source.source = "new application source".into();
        publish_cached_dae(
            &mut runner.state.lock().expect("state"),
            &source,
            dae_cache_key(&source),
            Arc::new(Dae::default()),
            &AtomicBool::new(false),
        );
        assert_eq!(runner.state.lock().expect("state").dae_cache.len(), 2);
    }

    /// Submitting more runs than `max_parallel` must NOT reject the extras
    /// (the old busy-gate behaviour) — they queue and drain as slots free,
    /// every run reaching a terminal update, and the scheduler settling
    /// back to empty. Explicit malformed source produces a compile rejection.
    #[test]
    fn scheduler_queues_beyond_cap_and_drains_all() {
        use std::time::Duration;

        let runner = ModelicaRunner::new();
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
        let runner = ModelicaRunner::new();
        let mut registry = ExperimentRegistry::new();
        let mut exp = mint_exp(&mut registry, "BoundsProbe");
        exp.bounds.dt = Some(1.0e-300);
        let handle = runner.run_fast(&exp, test_source(&exp.model_ref.0));
        let error = match handle
            .progress_rx
            .try_recv()
            .expect("terminal admission failure")
        {
            RunUpdate::Failed {
                error,
                partial: None,
            } => error,
            update => panic!("unexpected admission update: {update:?}"),
        };
        assert!(error.contains("output grid"));
        assert!(matches!(
            handle.progress_rx.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert_eq!(runner.queued_count(), 0);
        assert_eq!(runner.in_flight_count(), 0);

        struct Capture(Vec<RunUpdate>);
        impl RunSink for Capture {
            fn is_cancelled(&mut self) -> bool {
                false
            }
            fn emit(&mut self, update: RunUpdate) {
                self.0.push(update);
            }
        }
        let mut sink = Capture(Vec::new());
        drive_run(
            &Dae::default(),
            &exp.bounds,
            web_time::Instant::now(),
            &mut sink,
        );
        assert!(
            matches!(&sink.0[..], [RunUpdate::Failed { error, partial: None }] if error.contains("output grid"))
        );
        assert!(matches!(
            stepper_options_from_bounds(&exp.bounds),
            Err(RunConfigurationError::Bounds(_))
        ));
    }

    #[test]
    fn cancelling_queued_run_withdraws_it_before_admission() {
        let runner = ModelicaRunner::new();
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

        let runner = ModelicaRunner::new();
        runner.set_max_parallel(1);
        let mut registry = ExperimentRegistry::new();
        let panicking = mint_exp(&mut registry, "PanickingWorker");
        let successor = mint_exp(&mut registry, "Successor");
        let compiler = {
            let mut state = runner.state.lock().expect("runner state");
            state.in_flight.insert(panicking.id);
            state.compiler.clone()
        };
        let (panic_tx, panic_rx) = unbounded();
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        start_native_job(runner.state.clone(), panicking.id, panic_tx, move || {
            release_rx.recv().expect("release worker");
            let _compiler = compiler.lock().expect("compiler starts valid");
            panic!("injected experiment compiler panic");
        });
        let successor_handle = runner.run_fast(&successor, test_source(&successor.model_ref.0));
        assert_eq!(runner.queued_count(), 1);
        release_tx.send(()).expect("release worker");

        let timeout = Duration::from_secs(5);
        assert!(matches!(
            panic_rx.recv_timeout(timeout),
            Ok(RunUpdate::Failed { error, partial: None })
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
            Ok(RunUpdate::Failed { error, partial: None })
                if error.contains("compiler is poisoned")
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
                    origin: origin.clone(),
                    handle: RunHandle {
                        run_id: connected.id,
                        progress_rx: connected_rx,
                        cancel: Box::new(|| {}),
                    },
                },
                PendingRun {
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
            RunStatus::Failed { error, partial: false }
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

    /// CQ-525 regression: [`dae_cache_key`] MUST fold the model source body (and
    /// extras), not just `(model_name, filename)`. Before CQ-525 an edit that
    /// kept the model name produced an identical key, so the compile-once cache
    /// served a *stale* DAE and correctness leaned entirely on an external
    /// whole-cache clear. This pins that property so a future "simplification"
    /// of the key back to identity-only fails loudly here instead of silently
    /// resurrecting the stale-DAE bug.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dae_cache_key_folds_source_body_and_extras() {
        let base = ModelSource {
            model_name: "M".into(),
            source: "model M Real x = 1; end M;".into(),
            filename: "M.mo".into(),
            extras: vec![],
            runtime: lunco_workspace::DocumentRuntimeOwner::Application,
        };

        // Identical input → identical key (the cache must still HIT on a re-run
        // that changed nothing, e.g. a sweep varying only overrides).
        assert_eq!(
            dae_cache_key(&base),
            dae_cache_key(&base.clone()),
            "identical sources must yield the same key"
        );

        // Edited body, same name/filename → different key (the exact stale-DAE bug).
        let edited = ModelSource {
            source: "model M Real x = 2; end M;".into(),
            ..base.clone()
        };
        assert_ne!(
            dae_cache_key(&base),
            dae_cache_key(&edited),
            "editing the source body must change the DAE cache key (CQ-525)"
        );

        // A changed companion/extra source must also bust the key.
        let with_extra = ModelSource {
            extras: vec![("Lib".into(), "package Lib end Lib;".into())],
            ..base.clone()
        };
        assert_ne!(
            dae_cache_key(&base),
            dae_cache_key(&with_extra),
            "a changed extra source must change the key (CQ-525)"
        );
    }
}
