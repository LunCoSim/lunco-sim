//! Experiment runner — modelica/rumoca binding for the
//! [`lunco_experiments::ExperimentRunner`] trait.
//!
//! See `docs/architecture/25-experiments.md` for the design.
//!
//! ## Parameter / input injection
//!
//! Inputs and parameter overrides are applied at the **DAE level** by
//! rebinding each target variable's `start` after a single clean compile
//! (see [`apply_value_bindings_to_dae`]) — NOT by mutating source. One
//! compile is shared across a whole sweep; there is no source-rewriting
//! fallback. A target that is neither a top-level DAE parameter nor input
//! (or a non-scalar value) is a hard error rather than a silent re-compile.
//!
//! - One in-flight Fast Run per runner instance. Native enforcement
//!   matches wasm worker serialization.
//!
//! ## Compile-once parameter sweeps
//! Overrides are applied at the *DAE* level, not by reflattening per run.
//! `run_inner` compiles the source ONCE, caches the resulting `Dae` keyed by
//! a hash of the model source (`dae_cache`; see [`dae_cache_key`]), and for
//! each sweep point rebinds the target variables' `start` to literals via
//! [`apply_value_bindings_to_dae`]. This relies on rumoca's
//! `preserve_overridable_param_starts` fold (commit 6a849ac) keeping computed
//! derived params symbolic so they recompute at `SimulationSession::new` time. There
//! is **no** string-injection / source-rewriting fallback: an override that
//! can't be applied at the DAE level (non-top-level param/input, or a
//! non-scalar value) is a hard error, not a silent recompile.

use crate::lock_ext::LockExt;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use crossbeam_channel::{Sender, unbounded};
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};
use lunco_experiments::{
    Experiment, ExperimentId, ExperimentRegistry, ExperimentRunner, ModelRef, ParamPath,
    ParamValue, RunBounds, RunCancelled, RunCompleted, RunFailed, RunHandle, RunMeta,
    RunProgress, RunResult, RunStatus, RunUpdate,
};
use rumoca_compile::compile::Dae;
// Used only by the native-only DAE-override fast path (`apply_overrides_to_dae`).
// Used by `apply_value_bindings_to_dae` on BOTH platforms (native runner and
// the wasm worker both inject run values at the DAE level), so not wasm-gated.
use rumoca_compile::parsing::ir_core::{
    Expression as DaeExpression, Literal as DaeLiteral, Span as DaeSpan, VarName as DaeVarName,
};

/// Bound to the model source kept by the runner. The runner doesn't
/// own the live document state — `lunco-modelica` injects the current
/// source via [`ModelicaRunner::set_model_source`] before requesting
/// a run. ModelRef strings are the model's qualified name.
#[derive(Clone, Debug)]
pub struct ModelSource {
    pub model_name: String,
    pub source: String,
    pub filename: String,
    pub extras: Vec<(String, String)>,
}

/// Defaults from a previous compile's `experiment(...)` annotation.
/// Plumbed in from `CompilationResult.experiment_*` after the model
/// compiles successfully. UI uses these to prefill the Fast Run
/// bounds inline display.
#[derive(Clone, Debug, Default)]
pub struct ModelDefaults {
    pub t_start: Option<f64>,
    pub t_end: Option<f64>,
    pub tolerance: Option<f64>,
    pub interval: Option<f64>,
    /// Modelica `NumberOfIntervals` — the count alternative to `interval`.
    pub number_of_intervals: Option<f64>,
    pub solver: Option<lunco_experiments::SolverChoice>,
}

/// Platform default for the number of runs allowed to execute
/// concurrently (the "auto" setting). Both branches leave one logical core
/// for the UI/main thread and clamp low; the user can override via
/// `experiments.max_parallel`.
///
/// Native: `available_parallelism() - 1`. Wasm: `hardwareConcurrency - 1`,
/// clamped tighter because each pooled worker is a full second wasm instance
/// carrying its own copy of the (large) MSL bundle — so concurrency there
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
    model_ref: ModelRef,
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

/// Native + wasm-shared runner state. Stores the latest model source +
/// annotation defaults the UI provides, so `run_fast` can recompile
/// without round-tripping through the editor.
struct RunnerState {
    sources: BTreeMap<ModelRef, ModelSource>,
    defaults: BTreeMap<ModelRef, ModelDefaults>,
    /// Max concurrently-executing runs. `run_fast` starts a run
    /// immediately while `in_flight.len() < max_parallel`, else queues it.
    max_parallel: usize,
    /// Run ids currently executing (native: a live thread; wasm: dispatched
    /// to the worker). A run leaves this set on its terminal update.
    in_flight: HashSet<ExperimentId>,
    /// FIFO of runs waiting for a slot. Drained by `pump_scheduler` as
    /// in-flight runs finish.
    pending: VecDeque<QueuedJob>,
    /// Compile-once cache: `dae_cache_key(source)` → (model identity,
    /// compiled DAE). A parameter sweep reuses one rumoca compile and applies
    /// overrides at the DAE level. The key folds the model body (CQ-525), so a
    /// source edit yields a fresh key; the stored [`ModelIdent`] lets
    /// `set_model_source` evict only the edited model's entries.
    dae_cache: HashMap<u64, (ModelIdent, Arc<Dae>)>,
    /// Persistent compiler reused across runs, so MSL installs **once** for
    /// the runner (lazily, demand-driven via `ModelicaCompiler`) instead of
    /// rebuilding a fresh session per run. Behind its **own** lock, not the
    /// `state` mutex: a compile can take seconds, and holding `state` across
    /// it would stall the scheduler and every parallel run (which only need
    /// `state` briefly for cache lookups). Native only — the web build
    /// compiles on the worker, so the runner never constructs one. See
    /// [`run_inner`].
    #[cfg(not(target_arch = "wasm32"))]
    compiler: Arc<Mutex<crate::ModelicaCompiler>>,
}

/// `(model_name, filename)` identity used to scope DAE-cache invalidation
/// to a single model rather than clearing the whole cache (CQ-525).
type ModelIdent = (String, String);

impl Default for RunnerState {
    fn default() -> Self {
        Self {
            sources: BTreeMap::new(),
            defaults: BTreeMap::new(),
            max_parallel: default_max_parallel(),
            in_flight: HashSet::new(),
            pending: VecDeque::new(),
            dae_cache: HashMap::new(),
            // Cheap: `new()` builds an empty session and installs no MSL
            // (Layer A). MSL lands on the first run that actually needs it.
            #[cfg(not(target_arch = "wasm32"))]
            compiler: Arc::new(Mutex::new(crate::ModelicaCompiler::new())),
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
        if let Ok(mut s) = self.state.lock() {
            s.max_parallel = n.max(1);
        }
        // A raised cap may free slots for already-queued runs.
        pump_scheduler(&self.state);
    }

    /// Current max-parallel cap.
    pub fn max_parallel(&self) -> usize {
        self.state.lock().map(|s| s.max_parallel).unwrap_or(1)
    }

    /// Register or update the source for a model so subsequent
    /// `run_fast` calls have something to compile. Called by the build
    /// UI on every compile-relevant edit.
    pub fn set_model_source(&self, model_ref: ModelRef, source: ModelSource) {
        if let Ok(mut s) = self.state.lock() {
            // Only invalidate the compile-once cache when the source text (or
            // extras) actually changed — dispatch re-registers the same raw
            // source on every run of a sweep, and clearing then would defeat
            // the cache. (Correctness doesn't depend on this: the cache key
            // folds in the source hash (CQ-525), so a stale entry is never
            // served; this only bounds memory.)
            let changed = s
                .sources
                .get(&model_ref)
                .map(|old| old.source != source.source || old.extras != source.extras)
                .unwrap_or(true);
            // Capture identity before `source` is moved into `sources`.
            let ident: ModelIdent = (source.model_name.clone(), source.filename.clone());
            s.sources.insert(model_ref, source);
            if changed {
                // CQ-525: evict only THIS model's cached DAEs, not the whole
                // cache — an edit to one model shouldn't force every other
                // model in a multi-model workspace to recompile.
                s.dae_cache.retain(|_, (cached_ident, _)| *cached_ident != ident);
            }
        }
    }

    /// Stash annotation defaults from a successful compile so
    /// [`ExperimentRunner::default_bounds`] can return them.
    pub fn set_model_defaults(&self, model_ref: ModelRef, defaults: ModelDefaults) {
        if let Ok(mut s) = self.state.lock() {
            s.defaults.insert(model_ref, defaults);
        }
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
    fn run_fast(&self, exp: &Experiment) -> RunHandle {
        let (tx, rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let run_id = exp.id;

        // Cancel hook: flip the per-run flag (honored at start for a still
        // -queued run, and between solver steps once running). On wasm also
        // tell the worker so an in-flight run stops promptly.
        let cancel_for_hook = cancel.clone();
        #[cfg(target_arch = "wasm32")]
        let cancel_hook: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            cancel_for_hook.store(true, Ordering::SeqCst);
            crate::worker_transport::dispatch_cancel_run(run_id);
        });
        #[cfg(not(target_arch = "wasm32"))]
        let cancel_hook: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            cancel_for_hook.store(true, Ordering::SeqCst);
        });

        // Enqueue the snapshotted job, then start as many as slots allow.
        // A queued run sits silent (no updates) until a slot frees — its
        // registry status stays `Pending`, which already reads as "queued"
        // in the panel.
        {
            let mut s = self.state.lock_or_recover();
            s.pending.push_back(QueuedJob {
                run_id,
                model_ref: exp.model_ref.clone(),
                overrides: exp.overrides.clone(),
                inputs: exp.inputs.clone(),
                bounds: exp.bounds.clone(),
                tx,
                cancel,
            });
        }
        pump_scheduler(&self.state);

        RunHandle {
            run_id,
            progress_rx: rx,
            cancel: cancel_hook,
        }
    }

    fn default_bounds(&self, model: &ModelRef) -> Option<RunBounds> {
        let s = self.state.lock().ok()?;
        let d = s.defaults.get(model)?;
        // Only report bounds when the annotation actually specified a
        // horizon. Returning a fabricated `t_end=1.0` here forced callers
        // to guess "is this a real annotation?" with a fragile
        // `t_end != 1.0` check that silently dropped a legitimate
        // `experiment(StopTime=1)`. A stop time is the one field that makes
        // an experiment annotation usable, so gate on it.
        let t_end = d.t_end?;
        Some(RunBounds {
            t_start: d.t_start.unwrap_or(0.0),
            t_end,
            // `Interval=0` sentinel handling shared with every other
            // annotation→bounds path (preserves this struct's own `solver`).
            dt: crate::sim_target::interval_to_dt(d.interval),
            n_intervals: crate::sim_target::number_of_intervals_to_n(
                d.number_of_intervals,
                crate::sim_target::interval_to_dt(d.interval),
            ),
            tolerance: d.tolerance,
            solver: d.solver.clone(),
            h0: None,
            runtime: lunco_experiments::RuntimeMode::Batch,
        })
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
            let mut s = match state.lock() {
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

/// Mark a run as no longer in flight and pump the queue so the freed slot
/// is filled. Called from the off-thread completion path (native thread
/// end; wasm forwarder on terminal update).
fn finish_run(state: &Arc<Mutex<RunnerState>>, run_id: ExperimentId) {
    if let Ok(mut s) = state.lock() {
        s.in_flight.remove(&run_id);
    }
    pump_scheduler(state);
}

/// Begin executing one already-slotted job. Native: spawn a thread running
/// `run_inner`, calling `finish_run` when it returns. The thread-per-run
/// model gives each run fresh rumoca `thread_local` caches; the scheduler
/// caps live threads at `max_parallel`.
#[cfg(not(target_arch = "wasm32"))]
fn start_job(state: Arc<Mutex<RunnerState>>, job: QueuedJob) {
    let QueuedJob {
        run_id,
        model_ref,
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
        finish_run(&state, run_id);
        return;
    }
    let state_for_thread = state.clone();
    std::thread::spawn(move || {
        run_inner(state_for_thread.clone(), model_ref, overrides, inputs, bounds, cancel, tx);
        finish_run(&state_for_thread, run_id);
    });
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
        model_ref,
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
    let source_snapshot = state
        .lock()
        .ok()
        .and_then(|s| s.sources.get(&model_ref).cloned());
    let src = match source_snapshot {
        Some(src) => src,
        None => {
            let _ = tx.send(RunUpdate::Failed {
                error: format!("no source registered for model {}", model_ref.0),
                partial: None,
            });
            finish_run(&state, run_id);
            return;
        }
    };
    // Forward worker updates into the handle's tx; the forwarder frees the
    // slot via `finish_run` when a terminal update arrives.
    let (forward_tx, forward_rx) = unbounded::<RunUpdate>();
    crate::worker_transport::register_run_sender(run_id, forward_tx);
    spawn_forwarder(run_id, forward_rx, tx, state.clone());
    let dispatched = crate::worker_transport::dispatch_run_fast(
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
    slot.push(WasmForwarder { run_id, forward_rx, tx, state });
}

#[cfg(target_arch = "wasm32")]
struct WasmForwarder {
    run_id: ExperimentId,
    forward_rx: crossbeam_channel::Receiver<RunUpdate>,
    tx: crossbeam_channel::Sender<RunUpdate>,
    state: Arc<Mutex<RunnerState>>,
}

#[cfg(target_arch = "wasm32")]
static WASM_FORWARDERS: std::sync::OnceLock<Mutex<Vec<WasmForwarder>>> =
    std::sync::OnceLock::new();

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
/// there is no source-rewriting fallback. Both the native runner and the wasm
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
///   the old `input X` → `parameter X = v` source rewrite, with no rumoca
///   change required.
///
/// Returns `Err` when a target is neither a parameter nor an input, or the
/// value isn't a scalar literal — a hard error (no silent source-rewrite
/// fallback that would diverge the run's source from what was compiled).
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
            .ok_or_else(|| {
                format!("'{}' is not a top-level DAE parameter or input", path.0)
            })?;
        let lit = match value {
            ParamValue::Real(x) => DaeLiteral::Real(*x),
            ParamValue::Int(x) => DaeLiteral::Integer(*x),
            ParamValue::Bool(b) => DaeLiteral::Boolean(*b),
            ParamValue::String(s) => DaeLiteral::String(s.clone()),
            _ => return Err(format!("binding for '{}' is not a scalar literal", path.0)),
        };
        var.start = Some(DaeExpression::Literal { value: lit, span: DaeSpan::DUMMY });
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn run_inner(
    state: Arc<Mutex<RunnerState>>,
    model_ref: ModelRef,
    overrides: BTreeMap<ParamPath, ParamValue>,
    inputs: BTreeMap<ParamPath, ParamValue>,
    bounds: RunBounds,
    cancel: Arc<AtomicBool>,
    tx: Sender<RunUpdate>,
) {
    let t_wall = web_time::Instant::now();

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

    // Resolve model source.
    let source = match state.lock() {
        Ok(s) => match s.sources.get(&model_ref) {
            Some(src) => src.clone(),
            None => {
                let _ = tx.send(RunUpdate::Failed {
                    error: format!("no source registered for model {}", model_ref.0),
                    partial: None,
                });
                return;
            }
        },
        Err(_) => {
            let _ = tx.send(RunUpdate::Failed {
                error: "runner state poisoned".to_string(),
                partial: None,
            });
            return;
        }
    };

    // Persistent runner compiler: clone the handle out of `state` (brief
    // lock), then compile under the compiler's OWN lock so the multi-second
    // compile never holds `state` and stall the scheduler / parallel runs.
    // MSL installs once into this session (lazily) instead of per run.
    let compiler_handle = match state.lock() {
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
        .and_then(|s| s.dae_cache.get(&key).map(|(_, dae)| dae.clone()));
    let base_dae: Arc<Dae> = match cached {
        Some(d) => d,
        None => {
            let compiled = {
                let mut compiler = compiler_handle.lock().unwrap_or_else(|e| e.into_inner());
                compiler.compile_str_multi(
                    &source.model_name,
                    &source.source,
                    &source.filename,
                    &source.extras,
                )
            };
            match compiled {
                Ok(d) => {
                    let dae = d.dae.clone();
                    if let Ok(mut s) = state.lock() {
                        let ident: ModelIdent =
                            (source.model_name.clone(), source.filename.clone());
                        s.dae_cache.insert(key, (ident, dae.clone()));
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
    // Solver options (tolerance / family / initial step) — the SINGLE source
    // (`stepper_options_from_bounds`) both runtimes derive from.
    let stepper_opts = stepper_options_from_bounds(bounds);
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
            let mut batch_opts = stepper_opts.clone();
            let output_dt = crate::sim_target::resolve_step_dt(
                bounds.t_start,
                bounds.t_end,
                bounds.dt,
                bounds.n_intervals,
            );
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
            let mut stepper = match rumoca_sim::SimulationSession::new(dae, stepper_opts) {
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
/// The whole solve is one blocking call, so cancellation is honoured at the
/// boundaries (before the solve, and before emitting the result) rather than
/// mid-step; for the streamable/pausable behaviour use
/// [`RuntimeMode::Interactive`](lunco_experiments::RuntimeMode::Interactive).
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

    let result = match rumoca_sim::simulate_with_diagnostics(dae, opts) {
        Ok(r) => r,
        Err(e) => {
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
    let keep = batch_keep_indices(&result.times, opts.t_start, opts.t_end, opts.dt);
    let raw_samples = result.times.len();
    let (kept_times, gather): (Vec<f64>, Option<Vec<usize>>) = match &keep {
        Some(idx) => (idx.iter().map(|&i| result.times[i]).collect(), Some(idx.clone())),
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
            (Some(idx), Some(col)) => idx.iter().map(|&j| col.get(j).copied().unwrap_or(f64::NAN)).collect(),
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
/// first and last sample. Returns `None` (no decimation) when there's no usable
/// `dt`, or when the trajectory is already at/under the grid size (the
/// well-behaved smooth-model case — leave it byte-for-byte untouched).
fn batch_keep_indices(times: &[f64], t_start: f64, t_end: f64, dt: Option<f64>) -> Option<Vec<usize>> {
    let dt = dt?;
    if !(dt > 0.0) || times.len() < 2 {
        return None;
    }
    let span = (t_end - t_start).abs();
    if !(span > 0.0) {
        return None;
    }
    // +1 for the inclusive endpoint; small slack so we never over-decimate a
    // result that already sits on (or just above) the grid.
    let grid_n = (span / dt).round() as usize + 1;
    if times.len() <= grid_n.saturating_mul(2) {
        return None;
    }
    let mut keep: Vec<usize> = Vec::with_capacity(grid_n + 1);
    let mut cursor = 0usize;
    let n = times.len();
    for k in 0..=grid_n {
        let target = t_start + (k as f64) * dt;
        // Advance while the next sample is closer to `target` than the current.
        while cursor + 1 < n
            && (times[cursor + 1] - target).abs() <= (times[cursor] - target).abs()
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
    Some(keep)
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
/// Background: docs/numeric-experiments/2026-05-28-lunar-thermal.md
///
/// The worker's live interactive sim (`worker::build_stepper`) previously had
/// its OWN defaults (`rtol = 1e-3`, `atol = 1e-6`) with *no* solver selection,
/// silently running BDF where the batch path runs TR-BDF2. `build_stepper` now
/// delegates here, collapsing that divergence: one tolerance default, one
/// `atol`/`rtol` policy, one solver family across live + batch.
/// The ONLY place a [`SolverChoice`](lunco_experiments::SolverChoice) is
/// mapped to rumoca's two-axis selection: `SimSolverMode` (family) +
/// `DiffsolMethod` (implicit tableau on the BDF-family path). Keeping this in
/// one function is the point of the typed enum — there's no string to parse
/// twice and no `unwrap_or_default()` that could silently pick BDF.
fn solver_choice_to_rumoca(
    c: lunco_experiments::SolverChoice,
) -> (rumoca_sim::SimSolverMode, rumoca_sim::DiffsolMethod) {
    use lunco_experiments::SolverChoice as C;
    use rumoca_sim::{DiffsolMethod as D, SimSolverMode as M};
    match c {
        C::Bdf => (M::Bdf, D::Bdf),
        C::Esdirk34 => (M::Bdf, D::Esdirk34),
        C::TrBdf2 => (M::Bdf, D::TrBdf2),
        C::RkLike => (M::RkLike, D::Bdf),
    }
}

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
/// share this policy (A4): an adaptive-implicit solver whose step sequence comes
/// from per-machine error estimates must not run inside the client-predicted
/// fixed-step loop. It has its own configuration in `worker::live_stepper_options`
/// (explicit family, fixed micro-step ladder, fixed tolerance). The two surfaces
/// are meant to differ here — see
/// `docs/architecture/28-modelica-realtime-physics.md` §2a.
pub const DEFAULT_TOLERANCE: f64 = 1e-6;

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
pub fn stepper_options_from_bounds(bounds: &RunBounds) -> rumoca_sim::SimOptions {
    let mut opts = rumoca_sim::SimOptions::default();
    opts.t_start = bounds.t_start;
    opts.t_end = bounds.t_end;
    opts.atol = bounds.tolerance.unwrap_or(DEFAULT_TOLERANCE);
    opts.rtol = bounds.tolerance.unwrap_or(DEFAULT_TOLERANCE);
    // Single typed source of truth: `SolverChoice` already resolved the
    // vocabulary at the parse boundary, so here we just map it to rumoca's
    // (family, tableau) pair once — no re-parsing of strings, no silent
    // unknown→BDF degradation. `None` = backend default (BDF; see the rationale
    // on `stepper_options_from_bounds` for why this is no longer TR-BDF2).
    let (mode, method) =
        solver_choice_to_rumoca(bounds.solver.unwrap_or(lunco_experiments::SolverChoice::Bdf));
    opts.solver_mode = mode;
    opts.diffsol_method = method;
    // `SimOptions.dt` is the solver's initial step (h0) on the diffsol path.
    opts.dt = bounds.h0;
    opts
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
/// [`sim_target::resolve_step_dt`](crate::sim_target::resolve_step_dt)
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
    let step_dt =
        crate::sim_target::resolve_step_dt(bounds.t_start, t_end, bounds.dt, bounds.n_intervals);

    bevy::log::info!(
        "[sim] simulate begin: t={}..{} step_dt={}",
        bounds.t_start,
        t_end,
        step_dt
    );

    // Variable names from the initial state.
    let names: Vec<String> = match stepper.state() {
        Ok(state) => state.values.keys().filter(|n| *n != "time").cloned().collect(),
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
        output_dt.min(25.0).max(span / crate::sim_target::SAMPLE_CAP)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let internal_dt = output_dt;
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
/// `set_input(name, value)`. For batch Fast Run we substitute them
/// into the source as `parameter <type> <name> = <value>` before
/// compile so the simulator sees a fixed value instead of zero.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedInput {
    pub name: String,
    pub type_name: String,
}

/// Top-level `input` declarations of `class`, read from the **parsed
/// AST** — no source regex, no reparse. Used by the Setup dialog to
/// render an Inputs section and by the runner to pin input values.
///
/// Sourced via [`crate::ast_extract::extract_typed_inputs_for_class`],
/// which classifies by the parser's `causality` and additionally
/// catches connector-typed inputs (`RealInput`/`IntegerInput`/…) that
/// the old textual `\binput\b` scan silently missed.
pub fn detect_top_level_inputs(
    class: &rumoca_compile::parsing::ast::ClassDef,
) -> Vec<DetectedInput> {
    crate::ast_extract::extract_typed_inputs_for_class(class)
        .into_iter()
        .map(|c| DetectedInput { name: c.name, type_name: c.type_name })
        .collect()
}

/// Top-level `parameter` declarations of `class`, read from the
/// **parsed AST** — no source regex, no reparse. Drives the override
/// editor's table.
///
/// Sourced via [`crate::ast_extract::extract_typed_parameters_for_class`]
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
    crate::ast_extract::extract_typed_parameters_for_class(class)
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
                reason: is_array
                    .then(|| "array/record parameters can't be overridden".to_string()),
                description: {
                    let d = c.description.trim();
                    if d.is_empty() { None } else { Some(d.to_string()) }
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
    /// parameter overrides because they get a different source-rewrite
    /// (`input X y` → `parameter X y = value`).
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
pub struct PendingHandles(pub Vec<RunHandle>);

/// Map experiment id → originating DocumentId. Lets queries that
/// know a doc (e.g. the RunStatus API) discover which experiments
/// belong to it. Run-failure surfacing lives on `RunStatus::Failed`
/// in the registry — we no longer write run errors into
/// `CompileStates`, which is reserved for compile/Step errors on
/// the doc itself.
#[derive(Resource, Default)]
pub struct ExperimentSources(
    pub std::collections::HashMap<ExperimentId, lunco_doc::DocumentId>,
);

/// Per-document playback entity: holds the latest completed run's
/// time-series in `SignalRegistry` so canvas plot tiles can resolve
/// `(entity, path)` lookups without a live cosim entity.
///
/// One playback entity per doc; refilled in place each time a run
/// finishes (drop old signals, push new ones). When live cosim is
/// active for the same doc, the live entity wins in
/// `doc_to_entity` — playback is the no-live-cosim fallback.
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
    mut ev_progress: MessageWriter<RunProgress>,
    mut ev_completed: MessageWriter<RunCompleted>,
    mut ev_failed: MessageWriter<RunFailed>,
    mut ev_cancelled: MessageWriter<RunCancelled>,
) {
    let mut keep: Vec<RunHandle> = Vec::with_capacity(pending.0.len());
    for handle in pending.0.drain(..) {
        let mut terminal = false;
        while let Ok(update) = handle.progress_rx.try_recv() {
            match update {
                RunUpdate::Progress { t_current, delta } => {
                    registry.set_status(handle.run_id, RunStatus::Running { t_current });
                    if let Some(d) = delta {
                        registry.merge_result(handle.run_id, d);
                    }
                    ev_progress.write(RunProgress {
                        experiment_id: handle.run_id,
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
                    registry.set_status(
                        handle.run_id,
                        RunStatus::Done { wall_time_ms: wall },
                    );
                    ev_completed.write(RunCompleted {
                        experiment_id: handle.run_id,
                    });
                    terminal = true;
                }
                RunUpdate::Failed { error, partial } => {
                    bevy::log::warn!(
                        "[experiments] run {:?} failed: {error}",
                        handle.run_id
                    );
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
                        error,
                    });
                    terminal = true;
                }
                RunUpdate::Cancelled => {
                    registry.set_status(handle.run_id, RunStatus::Cancelled);
                    ev_cancelled.write(RunCancelled {
                        experiment_id: handle.run_id,
                    });
                    terminal = true;
                }
            }
        }
        // NOTE: do NOT drop sources.0[run_id] when a run goes terminal.
        // Completed runs must stay resolvable by `doc` (GetExperimentResult
        // / ListRuns `doc` filter, CompileStatus.latest_run). The mapping is
        // cleared in lockstep with the registry by DeleteExperiment instead;
        // dropping it here made every finished run unreachable by doc.
        if !terminal {
            keep.push(handle);
        }
    }
    pending.0 = keep;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Native memory probe (ignored — run explicitly):
    /// ```text
    /// cargo test -p lunco-modelica mem_probe_rover -- --ignored --nocapture
    /// ```
    /// Compiles `RoverThermalModular` and steps `RoverThermalSystem` at its
    /// full `Tolerance=1e-6 / StopTime=5.1e6 s` annotation, sampling peak RSS
    /// (`/proc/self/status`). This isolates the *solver's* native memory (no
    /// Bevy/render/MSL-in-worker) so we can compare it against the wasm 4 GiB
    /// linear-memory ceiling that traps the same run in-browser. A background
    /// sampler hard-exits after a wall cap so a never-returning giant
    /// `step()` can't hang the test; `VmHWM` (kernel peak RSS) is the headline
    /// number.
    #[test]
    #[ignore]
    fn mem_probe_rover_full() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        fn read_kb(key: &str) -> u64 {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with(key))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|n| n.parse::<u64>().ok())
                })
                .unwrap_or(0)
        }
        let mb = |kb: u64| kb as f64 / 1024.0;

        if lunco_assets::msl_source_root_path().is_none() {
            eprintln!("[memprobe] SKIP: MSL source root not available locally");
            return;
        }
        let path = "/home/rod/Documents/models/RoverThermalModular.mo";
        let src = std::fs::read_to_string(path).expect("read model source");
        let t0 = Instant::now();
        // Eagerly install the FULL pre-parsed MSL bundle (the worker's path),
        // so connector types like `HeatPort_a`/`RealOutput` resolve as real
        // connectors — the lazy on-demand hook returns stubs that `connect()`
        // rejects for this connector-heavy model.
        let mut compiler = crate::ModelicaCompiler::new();
        let report = compiler.load_source_root("Modelica", &lunco_assets::msl_dir());
        println!(
            "[memprobe] MSL installed: {} docs from {}",
            report.inserted_file_count, report.source_root_path
        );
        // `name` is the CLASS to instantiate (the file is a `package
        // LunarRover` holding it).
        let compiled = compiler
            .compile_str_multi(
                "LunarRover.RoverThermalSystem",
                &src,
                "RoverThermalModular.mo",
                &[],
            )
            .expect("compile LunarRover.RoverThermalSystem");
        println!(
            "[memprobe] compiled in {:.1}s; VmRSS={:.0}MB VmHWM={:.0}MB",
            t0.elapsed().as_secs_f64(),
            mb(read_kb("VmRSS:")),
            mb(read_kb("VmHWM:"))
        );

        let bounds = RunBounds {
            t_start: 0.0,
            t_end: 5_102_784.0,
            dt: None,
            n_intervals: None,
            tolerance: Some(1e-6),
            solver: None,
            h0: None,
            runtime: lunco_experiments::RuntimeMode::Batch,
        };
        let opts = stepper_options_from_bounds(&bounds);
        let mut stepper =
            rumoca_sim::SimulationSession::new(&compiled.dae, opts).expect("build stepper");
        let step_dt =
            crate::sim_target::resolve_step_dt(0.0, bounds.t_end, bounds.dt, bounds.n_intervals);
        println!(
            "[memprobe] stepper ready; step_dt={step_dt:.1}s; VmRSS={:.0}MB",
            mb(read_kb("VmRSS:"))
        );

        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let start = Instant::now();
        let sampler = std::thread::spawn(move || {
            let cap = Duration::from_secs(90);
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                println!(
                    "[memprobe] t_wall={:4.0}s VmRSS={:6.0}MB VmHWM(peak)={:6.0}MB",
                    start.elapsed().as_secs_f64(),
                    mb(read_kb("VmRSS:")),
                    mb(read_kb("VmHWM:"))
                );
                if s2.load(Ordering::SeqCst) || start.elapsed() > cap {
                    println!(
                        "[memprobe] === PEAK VmHWM={:.0}MB (stop={} cap_hit={}) ===",
                        mb(read_kb("VmHWM:")),
                        s2.load(Ordering::SeqCst),
                        start.elapsed() > cap
                    );
                    std::process::exit(0);
                }
            }
        });

        while stepper.time() < bounds.t_end {
            if let Err(e) = stepper.step(step_dt) {
                println!("[memprobe] step err at sim_t={:.0}: {e:?}", stepper.time());
                break;
            }
            println!(
                "[memprobe] sim_t={:.0}/{:.0} VmRSS={:.0}MB",
                stepper.time(),
                bounds.t_end,
                mb(read_kb("VmRSS:"))
            );
        }
        stop.store(true, Ordering::SeqCst);
        println!(
            "[memprobe] FINAL sim_t={:.0} VmRSS={:.0}MB VmHWM(peak)={:.0}MB",
            stepper.time(),
            mb(read_kb("VmRSS:")),
            mb(read_kb("VmHWM:"))
        );
        let _ = sampler.join();
    }

    // ── Step 2: settings / cap resolution ──

    #[test]
    fn default_max_parallel_is_at_least_one() {
        assert!(default_max_parallel() >= 1);
    }

    #[test]
    fn resolved_max_parallel_honours_setting_and_falls_back() {
        assert_eq!(
            ExperimentSettings { max_parallel: Some(3) }.resolved_max_parallel(),
            3
        );
        assert_eq!(
            ExperimentSettings { max_parallel: Some(1) }.resolved_max_parallel(),
            1
        );
        // None and the 0 sentinel both fall back to the platform auto
        // default, which is always a valid (≥1) cap.
        assert!(ExperimentSettings { max_parallel: None }.resolved_max_parallel() >= 1);
        assert!(ExperimentSettings { max_parallel: Some(0) }.resolved_max_parallel() >= 1);
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

    /// Submitting more runs than `max_parallel` must NOT reject the extras
    /// (the old busy-gate behaviour) — they queue and drain as slots free,
    /// every run reaching a terminal update, and the scheduler settling
    /// back to empty. Runs target a model with no registered source, so
    /// each fails fast in `run_inner` without invoking the compiler.
    #[test]
    fn scheduler_queues_beyond_cap_and_drains_all() {
        use std::time::Duration;

        let runner = ModelicaRunner::new();
        runner.set_max_parallel(2);

        let mut reg = ExperimentRegistry::new();
        let mut handles = Vec::new();
        for _ in 0..5 {
            let exp = mint_exp(&mut reg, "NoSuchModel");
            handles.push(runner.run_fast(&exp));
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
        assert!(settled, "scheduler should drain to empty after all runs end");
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
