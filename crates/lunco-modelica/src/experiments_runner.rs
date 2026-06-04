//! Experiment runner — modelica/rumoca binding for the
//! [`lunco_experiments::ExperimentRunner`] trait.
//!
//! See `docs/architecture/25-experiments.md` for the design.
//!
//! ## v1 limitations
//!
//! - Overrides go through source-string mutation (rumoca has no public
//!   `compile_with_modifications` yet). Only top-level literal
//!   `parameter` declarations are supported. See
//!   [`apply_overrides_to_source`].
//! - Reflatten on every override change. Cache keyed by
//!   (source_hash, override_set) is a future optimization. See TODO.
//! - One in-flight Fast Run per runner instance. Native enforcement
//!   matches wasm worker serialization.
//!
//! ## TODO(rumoca)
//! Replace string injection with `rumoca-compile::compile_with_modifications`
//! once upstream exposes `ClassModification` on the public API.
//! See `rumoca-ir-ast::visitor`.
//!
//! ## Compile-once parameter sweeps
//! Overrides are applied at the *DAE* level, not by reflattening per run.
//! `run_inner` compiles the input-substituted source ONCE, caches the
//! resulting `Dae` keyed by source hash (`dae_cache`), and for each sweep
//! point rebinds the target parameters' `start` to literals via
//! [`apply_overrides_to_dae`]. This relies on rumoca's
//! `preserve_overridable_param_starts` fold (commit 6a849ac) keeping computed
//! derived params symbolic so they recompute at `SimStepper::new` time. If an
//! override can't be set at the DAE level (non-top-level param, array/enum
//! value), it falls back to the legacy string-injection recompile.

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
    pub solver: Option<String>,
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
    /// Compile-once cache: `dae_cache_key(source, after_inputs)` → compiled
    /// DAE. A parameter sweep reuses one rumoca compile and applies overrides
    /// at the DAE level. Cleared when a model's source actually changes.
    dae_cache: HashMap<u64, Arc<Dae>>,
}

impl Default for RunnerState {
    fn default() -> Self {
        Self {
            sources: BTreeMap::new(),
            defaults: BTreeMap::new(),
            max_parallel: default_max_parallel(),
            in_flight: HashSet::new(),
            pending: VecDeque::new(),
            dae_cache: HashMap::new(),
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
            // already folds in the source hash; this only bounds memory.)
            let changed = s
                .sources
                .get(&model_ref)
                .map(|old| old.source != source.source || old.extras != source.extras)
                .unwrap_or(true);
            s.sources.insert(model_ref, source);
            if changed {
                s.dae_cache.clear();
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
            let mut s = self.state.lock().unwrap();
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
            tolerance: d.tolerance,
            solver: d.solver.clone(),
            h0: None,
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
    let mut slot = wasm_forwarders().lock().unwrap();
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
        let mut slot = wasm_forwarders().lock().unwrap();
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
/// Key for the compile-once DAE cache. Folds in model identity, extra sources,
/// and the input-substituted source — but NOT overrides (those are applied to
/// the cached DAE), so a sweep that varies only overrides hits one cache entry.
fn dae_cache_key(src: &ModelSource, after_inputs: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.model_name.hash(&mut h);
    src.filename.hash(&mut h);
    after_inputs.hash(&mut h);
    for (name, body) in &src.extras {
        name.hash(&mut h);
        body.hash(&mut h);
    }
    h.finish()
}

/// Apply parameter overrides directly to a compiled DAE by rebinding each
/// target parameter's `start` to a literal. Relies on rumoca's
/// `preserve_overridable_param_starts` fold keeping computed dependents
/// symbolic, so overriding a base (e.g. `Isp`) recomputes its dependents
/// (e.g. `massRatio`) at `SimStepper::new` via `build_params`. Returns `Err`
/// (→ caller recompiles with string-injected source) when a target isn't a
/// top-level DAE parameter or the value isn't a scalar literal.
fn apply_overrides_to_dae(
    dae: &mut Dae,
    overrides: &BTreeMap<ParamPath, ParamValue>,
) -> Result<(), String> {
    for (path, value) in overrides {
        let key = DaeVarName::new(path.0.clone());
        let var = dae
            .variables
            .parameters
            .get_mut(&key)
            .ok_or_else(|| format!("'{}' is not a top-level DAE parameter", path.0))?;
        let lit = match value {
            ParamValue::Real(x) => DaeLiteral::Real(*x),
            ParamValue::Int(x) => DaeLiteral::Integer(*x),
            ParamValue::Bool(b) => DaeLiteral::Boolean(*b),
            ParamValue::String(s) => DaeLiteral::String(s.clone()),
            _ => return Err(format!("override for '{}' is not a scalar literal", path.0)),
        };
        var.start = Some(DaeExpression::Literal { value: lit, span: DaeSpan::default() });
    }
    Ok(())
}

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

    // Apply input substitutions first (input → parameter), then
    // parameter overrides on the rewritten source.
    let after_inputs = match apply_inputs_to_source(&source.source, &inputs) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(RunUpdate::Failed {
                error: format!("input substitution failed: {e}"),
                partial: None,
            });
            return;
        }
    };
    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Compile-once: compile the input-substituted source (NO overrides baked
    // in) a single time and cache the resulting DAE keyed by source hash. A
    // parameter sweep recompiles zero times after the first run.
    let key = dae_cache_key(&source, &after_inputs);
    let cached = state.lock().ok().and_then(|s| s.dae_cache.get(&key).cloned());
    let base_dae: Arc<Dae> = match cached {
        Some(d) => d,
        None => {
            let mut compiler = crate::ModelicaCompiler::new();
            match compiler.compile_str_multi(
                &source.model_name,
                &after_inputs,
                &source.filename,
                &source.extras,
            ) {
                Ok(d) => {
                    let dae = d.dae.clone();
                    if let Ok(mut s) = state.lock() {
                        s.dae_cache.insert(key, dae.clone());
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

    // Apply parameter overrides at the DAE level (the compile-once fast path).
    // Fall back to a string-injected recompile if any override can't be set
    // there (non-top-level param, or non-scalar value).
    let run_dae: Arc<Dae> = if overrides.is_empty() {
        base_dae
    } else {
        let mut d = (*base_dae).clone();
        match apply_overrides_to_dae(&mut d, &overrides) {
            Ok(()) => Arc::new(d),
            Err(reason) => {
                bevy::log::debug!(
                    "[runner] DAE override fast-path unavailable ({reason}); recompiling with injected source"
                );
                let injected = match apply_overrides_to_source(&after_inputs, &overrides) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(RunUpdate::Failed {
                            error: format!("override application failed: {e}"),
                            partial: None,
                        });
                        return;
                    }
                };
                let mut compiler = crate::ModelicaCompiler::new();
                match compiler.compile_str_multi(
                    &source.model_name,
                    &injected,
                    &source.filename,
                    &source.extras,
                ) {
                    Ok(d) => d.dae,
                    Err(e) => {
                        let _ = tx.send(RunUpdate::Failed {
                            error: format!("compile failed: {e}"),
                            partial: None,
                        });
                        return;
                    }
                }
            }
        }
    };

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Build sim options from bounds.
    //
    // Defaults chosen to match what our solver stack can actually
    // deliver (FD Jacobian + scalar atol can't honor OMC's 1e-6
    // convention without burning retry budgets on noise) and what
    // works across stiff multi-day horizons:
    //   * tolerance default = 1e-4 (was 1e-6; OMC honors 1e-6 because
    //     it has analytical Jacobian + vector atol — we don't yet)
    //   * solver default = TR-BDF2 (was BDF via Default trait; BDF
    //     dies at the second sunrise louver crossing for stiff
    //     thermal models — TR-BDF2 handles events robustly across
    //     multi-month horizons).
    // Background: docs/numeric-experiments/2026-05-28-lunar-thermal.md
    let mut stepper_opts = rumoca_sim::SimOptions::default();
    stepper_opts.atol = bounds.tolerance.unwrap_or(1e-4);
    stepper_opts.rtol = bounds.tolerance.unwrap_or(1e-4);
    let solver_name = bounds.solver.as_deref().unwrap_or("tr_bdf2");
    // Map the bounds string to (family, tableau). `parse_request` selects the
    // solver *family* (Auto / implicit-BDF / explicit-RK); `DiffsolMethod`
    // selects the implicit tableau (ESDIRK34 / TR-BDF2) on the BDF-family path.
    // Implicit tableau names like "tr_bdf2" now resolve to `SimSolverMode::Bdf`
    // + the matching `DiffsolMethod`. Unknown strings fall back to BDF (matches
    // OMC's default).
    let (mode, _label) =
        rumoca_sim::SimSolverMode::parse_request(Some(solver_name));
    stepper_opts.solver_mode = mode;
    stepper_opts.diffsol_method =
        rumoca_sim::DiffsolMethod::from_external_name(solver_name).unwrap_or_default();
    // `SimOptions.dt` is the solver's initial step (h0) on the diffsol path.
    stepper_opts.dt = bounds.h0;

    let mut stepper = match rumoca_sim::SimStepper::new(&run_dae, stepper_opts) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(RunUpdate::Failed {
                error: format!("stepper init failed: {e:?}"),
                partial: None,
            });
            return;
        }
    };

    bevy::log::info!(
        "[runner] simulate begin: t={}..{} dt={:?}",
        bounds.t_start,
        bounds.t_end,
        bounds.dt
    );

    let t_end = bounds.t_end;
    // Output sample spacing (`Interval` in the Modelica `experiment`
    // annotation). The Modelica spec defaults `Interval` to 0, which is a
    // sentinel for "unspecified" — tools then derive it from a default
    // `numberOfIntervals` of 500 over [StartTime, StopTime]. So:
    //   * dt missing OR <= 0 (the spec's 0 sentinel)  → span / 500.
    //   * an explicit positive Interval               → honoured as given.
    // The old `unwrap_or(0.01)` ignored the spec entirely and pinned 10 ms
    // regardless of horizon, so a 1-year run (t_end=3.15e7) tried to emit
    // ~3.15 BILLION samples and appeared to hang forever.
    //
    // SAMPLE_CAP is a non-spec safety backstop: even an explicit (or
    // derived) interval never emits more than this many points — clamp dt
    // up and warn. Guards against a pathological hand-authored `Interval`
    // that would exhaust memory; well-formed models never hit it.
    const NUM_INTERVALS: f64 = 500.0;
    const SAMPLE_CAP: f64 = 200_000.0;
    let span = (t_end - bounds.t_start).max(0.0);
    let mut step_dt = match bounds.dt {
        Some(dt) if dt > 0.0 => dt,
        _ if span > 0.0 => span / NUM_INTERVALS,
        _ => 0.01, // degenerate zero-length span; emit a couple of points
    };
    if span > 0.0 && step_dt > 0.0 && span / step_dt > SAMPLE_CAP {
        let capped = span / SAMPLE_CAP;
        bevy::log::warn!(
            "[runner] Interval={step_dt}s over span={span}s would emit {:.0} \
             samples (>{SAMPLE_CAP:.0}); clamping to Interval={capped}s",
            span / step_dt
        );
        step_dt = capped;
    }
    let mut last_progress_emit = web_time::Instant::now();

    // Get variable names from the initial state.
    let names: Vec<String> = stepper.state().values.keys()
        .filter(|n| *n != "time")
        .cloned()
        .collect();

    let mut all_times: Vec<f64> = Vec::new();
    let mut all_series: Vec<Vec<f64>> = vec![Vec::new(); names.len()];
    let mut last_emit_idx = 0;

    // Simulation loop.
    while stepper.time() < t_end {
        if cancel.load(Ordering::SeqCst) {
            let _ = tx.send(RunUpdate::Cancelled);
            return;
        }

        if let Err(e) = stepper.step(step_dt) {
            bevy::log::warn!("[runner] simulate err: {e:?}");
            // Pack whatever we have so far into partial result.
            let mut series = BTreeMap::new();
            for (i, name) in names.iter().enumerate() {
                series.insert(name.clone(), all_series[i].clone());
            }
            let partial = RunResult {
                times: all_times.clone(),
                series,
                meta: RunMeta {
                    wall_time_ms: t_wall.elapsed().as_millis() as u64,
                    sample_count: all_times.len(),
                    notes: Some(format!("failed during step: {e:?}")),
                },
            };
            let _ = tx.send(RunUpdate::Failed {
                error: format!("simulate failed: {e:?}"),
                partial: Some(partial),
            });
            return;
        }

        let t = stepper.time();
        all_times.push(t);
        let current_values = stepper.state().values;
        for (i, name) in names.iter().enumerate() {
            let val = current_values.get(name).copied().unwrap_or(0.0);
            all_series[i].push(val);
        }

        // Throttle progress updates to ~10 Hz to avoid flooding the
        // main thread and incurring excessive serialization overhead.
        if last_progress_emit.elapsed().as_millis() > 100 {
            let delta_times = all_times[last_emit_idx..].to_vec();
            let mut delta_series = BTreeMap::new();
            for (i, name) in names.iter().enumerate() {
                delta_series.insert(name.clone(), all_series[i][last_emit_idx..].to_vec());
            }

            let delta = RunResult {
                times: delta_times,
                series: delta_series,
                meta: RunMeta {
                    wall_time_ms: t_wall.elapsed().as_millis() as u64,
                    sample_count: all_times.len() - last_emit_idx,
                    notes: None,
                },
            };

            let _ = tx.send(RunUpdate::Progress {
                t_current: t,
                delta: Some(delta),
            });
            last_progress_emit = web_time::Instant::now();
            last_emit_idx = all_times.len();
        }
    }

    bevy::log::info!(
        "[runner] simulate ok: {} samples, {} vars, {:.2}s wall",
        all_times.len(),
        names.len(),
        t_wall.elapsed().as_secs_f64(),
    );

    let mut final_series = BTreeMap::new();
    for (i, name) in names.iter().enumerate() {
        final_series.insert(name.clone(), all_series[i].clone());
    }

    let n_samples = all_times.len();
    let result = RunResult {
        times: all_times,
        series: final_series,
        meta: RunMeta {
            wall_time_ms: t_wall.elapsed().as_millis() as u64,
            sample_count: n_samples,
            notes: None,
        },
    };
    let _ = tx.send(RunUpdate::Completed(result));
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
    /// Whether v1's string-injection override path can mutate this
    /// declaration. False for non-literal RHS (expression bindings),
    /// for arrays / records, and for params not found at the top level.
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

/// Find top-level `input <Type> <name>;` declarations in a model
/// source. Used by the Setup dialog to render an Inputs section, and
/// by the runner to substitute values into the source pre-compile.
pub fn detect_top_level_inputs(source: &str) -> Vec<DetectedInput> {
    let re = regex::Regex::new(
        r"(?m)\binput\b\s+([A-Za-z_][A-Za-z0-9_.\[\]]*)\s+([A-Za-z_][A-Za-z0-9_]*)\b",
    )
    .expect("regex");
    let mut out = Vec::new();
    for cap in re.captures_iter(source) {
        let type_name = cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        let name = cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        out.push(DetectedInput { name, type_name });
    }
    out
}

/// Substitute input declarations with parameter declarations so the
/// batch simulator sees a fixed value. Replaces
/// `input <Type> <name>` with `parameter <Type> <name> = <value>`.
/// Only fires for inputs the user actually set; unset inputs stay
/// as `input` and the simulator defaults them to 0.
pub fn apply_inputs_to_source(
    source: &str,
    inputs: &BTreeMap<ParamPath, ParamValue>,
) -> Result<String, String> {
    if inputs.is_empty() {
        return Ok(source.to_string());
    }
    let mut out = source.to_string();
    for (path, value) in inputs {
        let leaf = crate::ast_extract::short_name(&path.0);
        let escaped = regex::escape(leaf);
        let re = regex::Regex::new(&format!(
            r"(?m)\binput\b(\s+[A-Za-z_][A-Za-z0-9_.\[\]]*)\s+{}\b\s*;",
            escaped
        ))
        .map_err(|e| format!("input regex: {e}"))?;
        let lit = format_literal(value);
        let replaced = re
            .replace(
                &out,
                format!("parameter$1 {} = {};", leaf, lit).as_str(),
            )
            .into_owned();
        if replaced == out {
            return Err(format!(
                "input '{leaf}' not found at the top level of the model source"
            ));
        }
        out = replaced;
    }
    Ok(out)
}

/// Detect top-level `parameter` declarations in a Modelica source
/// string. v1: scans the textual source only (no AST traversal),
/// matches the same patterns `replace_param_literal` accepts. Used by
/// the override editor UI.
///
/// Inherited parameters (declared in a base class extends) are NOT
/// detected — they don't appear in the model's own source. Surface
/// them as unsupported in v2 once we lift detection to the AST.
pub fn detect_top_level_literal_parameters(source: &str) -> Vec<DetectedParam> {
    // Pattern: `parameter [final] [each] <Type> <name> [(modifiers)] [= <rhs>] ;`
    // We capture name, type, optional rhs literal. To stay robust
    // against modifier clauses, the regex is loose and a follow-up
    // scan classifies the rhs literal vs expression.
    let re = regex::Regex::new(
        r"(?m)\bparameter\b\s+(?:final\s+|each\s+)*([A-Za-z_][A-Za-z0-9_.\[\]]*)\s+([A-Za-z_][A-Za-z0-9_]*)\b",
    )
    .expect("regex");
    let mut out = Vec::new();
    for cap in re.captures_iter(source) {
        let type_name = cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        let name = cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        // Find the `=` for this declaration up to next `;` (top-level).
        let after = cap.get(0).unwrap().end();
        let tail = &source[after..];
        let mut depth: i32 = 0;
        let mut eq_pos: Option<usize> = None;
        let mut end_pos: Option<usize> = None;
        for (i, ch) in tail.char_indices() {
            match ch {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth -= 1,
                '=' if depth == 0 && eq_pos.is_none() => eq_pos = Some(i),
                ';' if depth == 0 => {
                    end_pos = Some(i);
                    break;
                }
                _ => {}
            }
        }
        // Trailing Modelica description-comment: `... "Gravity";`.
        // Capture the last quoted string before the terminating `;` so
        // the override editor can show it as hover help. (For a
        // bare String default with no comment this picks up the value
        // instead — a benign edge case; String params aren't override-
        // supportable in v1 anyway.)
        let description = end_pos.and_then(|end| {
            let decl = &tail[..end];
            let close = decl.rfind('"')?;
            let open = decl[..close].rfind('"')?;
            let s = &decl[open + 1..close];
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        });
        let (default_literal, supportable, reason) = match (eq_pos, end_pos) {
            (Some(eq), Some(end)) if end > eq + 1 => {
                // Modelica allows a trailing description string after
                // the binding: `parameter Real g = 9.81 "Gravity";`.
                // Strip it before classifying — otherwise the whole
                // `9.81 "Gravity"` blob fails `looks_like_literal` and
                // the override editor disables the row.
                let raw_rhs = tail[eq + 1..end].trim();
                let rhs = match raw_rhs.find('"') {
                    Some(q) => raw_rhs[..q].trim_end(),
                    None => raw_rhs,
                };
                if rhs.is_empty() {
                    (None, false, Some("no default value".to_string()))
                } else if looks_like_literal(rhs) {
                    (Some(rhs.to_string()), true, None)
                } else {
                    (
                        Some(rhs.to_string()),
                        false,
                        Some("complex binding — override unsupported in v1".to_string()),
                    )
                }
            }
            _ => (
                None,
                false,
                Some("no literal default — override unsupported in v1".to_string()),
            ),
        };
        out.push(DetectedParam {
            name,
            type_name,
            default_literal,
            supportable,
            reason,
            description,
        });
    }
    // Dedupe by parameter name. The scan is regex-based over the
    // whole file and picks up `parameter Real g` in every class
    // (RocketStage AND Airframe both declare `g`, for example). The
    // override path rewrites only the first occurrence in source —
    // surfacing duplicates here just produces conflicting widget ids
    // in the UI and confuses users about which row their edit
    // targets. Keep the first occurrence (matches the source order
    // the rewriter touches).
    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.name.clone()));
    out
}

fn looks_like_literal(rhs: &str) -> bool {
    let s = rhs.trim();
    if s.is_empty() {
        return false;
    }
    // Real / Int / signed numbers, scientific notation.
    if s.parse::<f64>().is_ok() {
        return true;
    }
    if s.parse::<i64>().is_ok() {
        return true;
    }
    if s == "true" || s == "false" {
        return true;
    }
    // Quoted string literal, no string-concat / function calls.
    if s.starts_with('"') && s.ends_with('"') && !s[1..s.len() - 1].contains('"') {
        return true;
    }
    // Fall through: identifier / expression / function call / array
    // literal — v1 doesn't substitute these via regex.
    false
}

/// Mutate `source` so each top-level literal `parameter` declaration
/// listed in `overrides` carries the new value.
///
/// v1 implementation: simple regex-based replacement of
/// `parameter <Type> <name> = <literal>`. Limitations:
///
/// - Only matches top-level literal RHS (numbers, true/false,
///   quoted strings). Expression-bound or inherited params are
///   silently skipped — caller filters those out at UI time so the
///   user sees them greyed.
/// - Requires the param declaration to be in the supplied source
///   (won't traverse imports). Same limitation as the UI editor.
///
/// Returns `Err` if a requested override name isn't found in the
/// source — surfaces as a Failed run with a clear message instead of
/// running with stale params silently.
pub fn apply_overrides_to_source(
    source: &str,
    overrides: &BTreeMap<ParamPath, ParamValue>,
) -> Result<String, String> {
    if overrides.is_empty() {
        return Ok(source.to_string());
    }
    let mut out = source.to_string();
    for (path, value) in overrides {
        // We look for `parameter <whitespace and type stuff> <name> = <literal>`.
        // The path may be dotted (component.subcomponent.param); v1 only
        // supports the trailing identifier in the model's own source.
        // Inherited params raise an error per spec.
        let leaf = crate::ast_extract::short_name(&path.0);
        let new_literal = format_literal(value);
        let replaced = replace_param_literal(&out, leaf, &new_literal)?;
        out = replaced;
    }
    Ok(out)
}

fn format_literal(v: &ParamValue) -> String {
    match v {
        ParamValue::Real(x) => {
            // Keep "5.0" not "5" so the parser still sees Real.
            if x.fract() == 0.0 && x.is_finite() {
                format!("{x:.1}")
            } else {
                format!("{x}")
            }
        }
        ParamValue::Int(x) => format!("{x}"),
        ParamValue::Bool(b) => if *b { "true".into() } else { "false".into() },
        ParamValue::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        ParamValue::Enum(s) => s.clone(),
        ParamValue::RealArray(xs) => {
            let inner: Vec<String> = xs.iter().map(|x| format!("{x}")).collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// Find `parameter ... <name> [(modifiers)] = <literal>;` and substitute
/// the literal.
///
/// We can't use a full Modelica parser here without re-flattening, which
/// defeats the point. So we anchor a loose regex on the declaration up to
/// the parameter name, then scan forward tracking bracket depth to find the
/// declaration's *own* `=` (the first `=` at depth 0) and the terminating
/// `;`/`,`. Tracking depth is what lets a value-modifier such as
/// `g(unit="m/s2") = 9.81` keep its `unit="m/s2"` — the `=` inside the
/// parentheses is at depth 1 and correctly skipped (a plain `[^=;]*=` regex
/// would wrongly match that inner `=` and clobber the modifier).
fn replace_param_literal(
    source: &str,
    name: &str,
    new_literal: &str,
) -> Result<String, String> {
    // Anchor on `parameter [final] [each] <type…> <name>` only — do NOT let
    // the regex consume the `=`, since the binding `=` must be located by
    // the depth-aware scan below.
    let escaped = regex::escape(name);
    let re = regex::Regex::new(&format!(
        r"(?m)\bparameter\b[^;]*?\b{}\b",
        escaped
    ))
    .map_err(|e| format!("override regex build failed: {e}"))?;
    let mat = match re.find(source) {
        Some(m) => m,
        None => {
            return Err(format!(
                "parameter '{name}' not found at the top level of the model source — \
                 inherited / expression-bound params are not supported in v1"
            ));
        }
    };
    // Scan from just after the name for the declaration's `=` and end.
    let scan_start = mat.end();
    let tail = &source[scan_start..];
    let mut depth: i32 = 0;
    let mut eq_off: Option<usize> = None;
    let mut end_off: Option<usize> = None;
    for (i, ch) in tail.char_indices() {
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            '=' if depth == 0 && eq_off.is_none() => eq_off = Some(i),
            ';' | ',' if depth == 0 => {
                end_off = Some(i);
                break;
            }
            _ => {}
        }
    }
    let end_off = end_off.unwrap_or(tail.len());
    let eq_off = match eq_off {
        Some(e) if e < end_off => e,
        // No top-level `=` before the terminator → no literal binding to
        // replace (expression-bound / no default). Same v1 limitation as
        // the UI detector flags.
        _ => {
            return Err(format!(
                "parameter '{name}' has no literal default binding — \
                 override unsupported in v1"
            ));
        }
    };
    // Replace the span after the `=` (the value) with ` <literal>`, keeping
    // everything up to and including the `=` and the terminator onward.
    let value_start = scan_start + eq_off + 1; // byte right after `=`
    let value_end = scan_start + end_off;
    let mut out = String::with_capacity(source.len() + new_literal.len() + 1);
    out.push_str(&source[..value_start]);
    out.push(' ');
    out.push_str(new_literal);
    out.push_str(&source[value_end..]);
    Ok(out)
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
    mut commands: bevy::prelude::Commands,
    mut pending: ResMut<PendingHandles>,
    mut registry: ResMut<ExperimentRegistry>,
    mut ev_progress: MessageWriter<RunProgress>,
    mut ev_completed: MessageWriter<RunCompleted>,
    mut ev_failed: MessageWriter<RunFailed>,
    mut ev_cancelled: MessageWriter<RunCancelled>,
    sources: Res<ExperimentSources>,
    mut console: Option<ResMut<crate::ui::panels::console::ConsoleLog>>,
    mut plot_states: Option<ResMut<crate::ui::panels::experiments::PlotPanelStates>>,
    active_plot: Option<Res<crate::ui::panels::experiments::ActivePlot>>,
    mut playback: ResMut<PlaybackEntities>,
    mut signals: Option<ResMut<lunco_viz::SignalRegistry>>,
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
                    // Auto-visible: a run that just completed is what
                    // the user is looking at, no checkbox needed. Mark
                    // it visible on the active plot tab only (per-plot
                    // visibility — other plot windows stay untouched,
                    // matching Dymola's per-window curve set).
                    // Also auto-pick a few variables on the very first
                    // completion so the plot has content without
                    // hunting through Telemetry. Skip parameters
                    // (constant series) — pick the first 3 dynamic
                    // signals by series-variance heuristic.
                    if let Some(states) = plot_states.as_mut() {
                        let viz = active_plot
                            .as_deref()
                            .copied()
                            .unwrap_or_default()
                            .or_default();
                        let entry = states.entry(viz);
                        entry.visible_experiments.insert(handle.run_id);
                        if entry.picked_vars.is_empty() {
                            let mut by_var: Vec<(&String, f64)> = result
                                .series
                                .iter()
                                .map(|(k, v)| {
                                    let n = v.len().max(1) as f64;
                                    let mean = v.iter().copied().sum::<f64>() / n;
                                    let var = v
                                        .iter()
                                        .map(|x| (x - mean) * (x - mean))
                                        .sum::<f64>()
                                        / n;
                                    (k, var)
                                })
                                .filter(|(_, v)| v.is_finite() && *v > 1e-12)
                                .collect();
                            by_var.sort_by(|a, b| {
                                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                            });
                            for (k, _) in by_var.into_iter().take(3) {
                                entry.picked_vars.insert(k.clone());
                            }
                        }
                    }
                    let run_name = registry
                        .get(handle.run_id)
                        .map(|e| e.name.clone())
                        .unwrap_or_else(|| "Run".into());
                    if let Some(c) = console.as_mut() {
                        c.info(format!(
                            "✓ {run_name} done: {n_samples} samples × {n_vars} vars in {wall} ms"
                        ));
                    }
                    // Publish the run's series into `SignalRegistry`
                    // under a per-doc playback entity, so canvas plot
                    // tiles bound by `PlotBinding::Doc` resolve to
                    // real (entity, path) samples without needing a
                    // live cosim entity. One entity per doc, reused
                    // across runs — drop prior signals then push the
                    // new run's data.
                    if let (Some(doc_id), Some(signals_mut)) = (
                        sources.0.get(&handle.run_id).copied(),
                        signals.as_deref_mut(),
                    ) {
                        let entity = *playback
                            .0
                            .entry(doc_id)
                            .or_insert_with(|| commands.spawn_empty().id());
                        signals_mut.drop_entity(entity);
                        for (path, samples) in &result.series {
                            let sig = lunco_viz::SignalRef {
                                entity,
                                path: path.clone(),
                            };
                            for (t, v) in result.times.iter().zip(samples.iter()) {
                                signals_mut.push_scalar(sig.clone(), *t, *v);
                            }
                        }
                    }
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
                    let run_name = registry
                        .get(handle.run_id)
                        .map(|e| e.name.clone())
                        .unwrap_or_else(|| "Fast Run".into());
                    if let Some(c) = console.as_mut() {
                        c.error(format!("⚠ {run_name} FAILED: {error}"));
                    }
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
                    let run_name = registry
                        .get(handle.run_id)
                        .map(|e| e.name.clone())
                        .unwrap_or_else(|| "Fast Run".into());
                    if let Some(c) = console.as_mut() {
                        c.info(format!("⊘ {run_name} cancelled"));
                    }
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

    #[test]
    fn override_real_literal() {
        let src = "model M\n  parameter Real m = 1.5;\nequation\nend M;\n";
        let mut ov: BTreeMap<ParamPath, ParamValue> = BTreeMap::new();
        ov.insert(ParamPath("m".into()), ParamValue::Real(2.5));
        let out = apply_overrides_to_source(src, &ov).unwrap();
        assert!(out.contains("parameter Real m = 2.5"));
        assert!(!out.contains("1.5"));
    }

    #[test]
    fn override_with_modifier() {
        let src = "model M\n  parameter Real g(unit=\"m/s2\") = 9.81;\nend M;\n";
        let mut ov: BTreeMap<ParamPath, ParamValue> = BTreeMap::new();
        ov.insert(ParamPath("g".into()), ParamValue::Real(3.71));
        let out = apply_overrides_to_source(src, &ov).unwrap();
        assert!(out.contains("3.71"));
        assert!(out.contains("unit=\"m/s2\""));
    }

    #[test]
    fn missing_param_errors() {
        let src = "model M\n  Real x;\nend M;\n";
        let mut ov: BTreeMap<ParamPath, ParamValue> = BTreeMap::new();
        ov.insert(ParamPath("nope".into()), ParamValue::Real(1.0));
        assert!(apply_overrides_to_source(src, &ov).is_err());
    }

    #[test]
    fn integer_and_bool_format() {
        assert_eq!(format_literal(&ParamValue::Int(7)), "7");
        assert_eq!(format_literal(&ParamValue::Bool(true)), "true");
        assert_eq!(format_literal(&ParamValue::Real(3.0)), "3.0");
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
}
