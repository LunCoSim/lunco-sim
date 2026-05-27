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
//! ## TODO(perf)
//! Override changes currently force rumoca reflatten. Measure flatten
//! time on representative models; if >100ms, add (source_hash,
//! override_set) -> Dae LRU cache here. Long-term: upstream a
//! `SimStepper::set_parameter()` to rumoca to skip reflatten.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use crossbeam_channel::{Sender, unbounded};
use lunco_experiments::{
    Experiment, ExperimentId, ExperimentRegistry, ExperimentRunner, ModelRef, ParamPath,
    ParamValue, RunBounds, RunCancelled, RunCompleted, RunFailed, RunHandle, RunMeta,
    RunProgress, RunResult, RunStatus, RunUpdate,
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

/// Native + wasm-shared runner state. Stores the latest model source +
/// annotation defaults the UI provides, so `run_fast` can recompile
/// without round-tripping through the editor.
#[derive(Default)]
struct RunnerState {
    sources: BTreeMap<ModelRef, ModelSource>,
    defaults: BTreeMap<ModelRef, ModelDefaults>,
    /// Currently-executing run id. Some(id) means a Fast Run is in
    /// flight; subsequent run_fast calls fail-fast in v1 (no internal
    /// queue — caller-side queueing handled by the panel).
    busy_with: Option<ExperimentId>,
}

/// Runner state shared between the trait wrapper and the worker
/// thread. The cancel flag is per-run, swapped on each `run_fast`.
pub struct ModelicaRunner {
    state: Arc<Mutex<RunnerState>>,
    /// Cancel flag for the *currently in-flight* run, if any.
    cancel_flag: Arc<Mutex<Option<(ExperimentId, Arc<AtomicBool>)>>>,
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
            cancel_flag: Arc::new(Mutex::new(None)),
        }
    }

    /// Register or update the source for a model so subsequent
    /// `run_fast` calls have something to compile. Called by the build
    /// UI on every compile-relevant edit.
    pub fn set_model_source(&self, model_ref: ModelRef, source: ModelSource) {
        if let Ok(mut s) = self.state.lock() {
            s.sources.insert(model_ref, source);
        }
    }

    /// Stash annotation defaults from a successful compile so
    /// [`ExperimentRunner::default_bounds`] can return them.
    pub fn set_model_defaults(&self, model_ref: ModelRef, defaults: ModelDefaults) {
        if let Ok(mut s) = self.state.lock() {
            s.defaults.insert(model_ref, defaults);
        }
    }

    /// `true` when a Fast Run is currently in flight. UI uses this to
    /// disable the Fast button.
    pub fn is_busy(&self) -> bool {
        self.state.lock().map(|s| s.busy_with.is_some()).unwrap_or(false)
    }
}

impl ExperimentRunner for ModelicaRunner {
    fn run_fast(&self, exp: &Experiment) -> RunHandle {
        let (tx, rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let run_id = exp.id;

        // Mark busy. If something else is in flight, immediately fail
        // this run with a clear message rather than queue silently —
        // the panel knows to surface "busy" before calling.
        let already_busy = {
            let mut s = self.state.lock().unwrap();
            if s.busy_with.is_some() {
                true
            } else {
                s.busy_with = Some(run_id);
                false
            }
        };
        if already_busy {
            let _ = tx.send(RunUpdate::Failed {
                error: "another Fast Run is already in flight".to_string(),
                partial: None,
            });
            return RunHandle {
                run_id,
                progress_rx: rx,
                cancel: Box::new(|| {}),
            };
        }

        // Install cancel hook for this run.
        {
            let mut slot = self.cancel_flag.lock().unwrap();
            *slot = Some((run_id, cancel.clone()));
        }

        // Snapshot inputs for the worker thread.
        let model_ref = exp.model_ref.clone();
        let overrides = exp.overrides.clone();
        let inputs = exp.inputs.clone();
        let bounds = exp.bounds.clone();
        let state = self.state.clone();
        let busy_clear = self.state.clone();

        // wasm: dispatch to the Web Worker via worker_transport.
        // Native: spawn a std::thread and run inline.
        #[cfg(target_arch = "wasm32")]
        {
            // Resolve source on the main thread (worker has no
            // editor-state access).
            let source_snapshot = state
                .lock()
                .ok()
                .and_then(|s| s.sources.get(&model_ref).cloned());
            let cancel_for_handle = run_id;
            let cancel_hook = Box::new(move || {
                crate::worker_transport::dispatch_cancel_run(cancel_for_handle);
            });
            match source_snapshot {
                Some(src) => {
                    // Forward updates from the dispatch tx (registered with
                    // worker_transport) into our local tx + clear busy on
                    // terminal.
                    let (forward_tx, forward_rx) = unbounded::<RunUpdate>();
                    crate::worker_transport::register_run_sender(run_id, forward_tx);
                    // Pump in a background task — wasm is single-threaded
                    // but the receiver is drained via spawn_local-friendly
                    // poll. v1: use a Bevy system to forward updates.
                    // Here we let the panel's drain_run_updates system
                    // consume the rx instead of bouncing through tx —
                    // expose the rx via a side channel:
                    spawn_forwarder(run_id, forward_rx, tx, busy_clear);
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
                        // Fall back: no worker — fail the run.
                        if let Ok(mut s) = state.lock() {
                            s.busy_with = None;
                        }
                    }
                }
                None => {
                    let _ = tx.send(RunUpdate::Failed {
                        error: format!("no source registered for model {}", model_ref.0),
                        partial: None,
                    });
                    if let Ok(mut s) = state.lock() {
                        s.busy_with = None;
                    }
                }
            }
            return RunHandle {
                run_id,
                progress_rx: rx,
                cancel: cancel_hook,
            };
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let cancel_for_handle = cancel.clone();
            let cancel_hook = Box::new(move || {
                cancel_for_handle.store(true, Ordering::SeqCst);
            });
            let _ = run_id;
            std::thread::spawn(move || {
                run_inner(state, model_ref, overrides, inputs, bounds, cancel, tx);
                if let Ok(mut s) = busy_clear.lock() {
                    s.busy_with = None;
                }
            });
            RunHandle {
                run_id,
                progress_rx: rx,
                cancel: cancel_hook,
            }
        }
    }

    fn default_bounds(&self, model: &ModelRef) -> Option<RunBounds> {
        let s = self.state.lock().ok()?;
        let d = s.defaults.get(model)?;
        Some(RunBounds {
            t_start: d.t_start.unwrap_or(0.0),
            t_end: d.t_end.unwrap_or(1.0),
            dt: d.interval,
            tolerance: d.tolerance,
            solver: d.solver.clone(),
        })
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
            if let Ok(mut s) = fwd.state.lock() {
                if s.busy_with == Some(fwd.run_id) {
                    s.busy_with = None;
                }
            }
        } else {
            keep.push(fwd);
        }
    }
    *slot = keep;
}

/// Body of the run thread. Compiles, runs the simulation, posts
/// updates. All errors funnel into `RunUpdate::Failed`. Cancellation
/// observed between steps via the shared `AtomicBool`.
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

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Compile.
    let mut compiler = crate::ModelicaCompiler::new();
    let compile_result =
        compiler.compile_str_multi(&source.model_name, &injected, &source.filename, &source.extras);
    let dae_result = match compile_result {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send(RunUpdate::Failed {
                error: format!("compile failed: {e}"),
                partial: None,
            });
            return;
        }
    };

    if cancel.load(Ordering::SeqCst) {
        let _ = tx.send(RunUpdate::Cancelled);
        return;
    }

    // Build sim options from bounds.
    let mut stepper_opts = rumoca_sim::StepperOptions::default();
    stepper_opts.atol = bounds.tolerance.unwrap_or(1e-6);
    stepper_opts.rtol = bounds.tolerance.unwrap_or(1e-6);

    let mut stepper = match rumoca_sim::SimStepper::new(&dae_result.dae, stepper_opts) {
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
    let step_dt = bounds.dt.unwrap_or(0.01);
    let mut last_progress_emit = std::time::Instant::now();
    
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
            last_progress_emit = std::time::Instant::now();
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

/// Find `parameter ... <name> = <literal>;` and substitute the literal.
///
/// We can't use a full Modelica parser here without re-flattening,
/// which defeats the point. The regex tolerates whitespace and
/// modifiers (`final`, `each`, attribute clauses) and stops at the
/// first `=` followed by a literal up to `;` or `,`.
fn replace_param_literal(
    source: &str,
    name: &str,
    new_literal: &str,
) -> Result<String, String> {
    // Build a regex that matches:
    //   parameter [final] [each] <type-and-modifiers> <name> [(...)] = <literal>
    // and captures the literal so we can replace just that span.
    //
    // The literal capture is anything up to the next comma or
    // semicolon at the same nesting level. To keep this simple in v1
    // we scan manually rather than relying on regex backrefs.
    let escaped = regex::escape(name);
    let re = regex::Regex::new(&format!(
        r"(?m)\bparameter\b[^;]*?\b{}\b[^=;]*=\s*",
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
    let head_end = mat.end(); // position right after `=` whitespace
    // Find end of literal: nearest top-level `;` or `,`. v1 doesn't
    // support nested arrays/records in overrides, so a naive scan is
    // fine.
    let tail = &source[head_end..];
    let mut depth: i32 = 0;
    let mut end_off = tail.len();
    for (i, ch) in tail.char_indices() {
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            ';' | ',' if depth == 0 => {
                end_off = i;
                break;
            }
            _ => {}
        }
    }
    let mut out = String::with_capacity(source.len() + new_literal.len());
    out.push_str(&source[..head_end]);
    out.push_str(new_literal);
    out.push_str(&source[head_end + end_off..]);
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
    mut sources: ResMut<ExperimentSources>,
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
        if terminal {
            sources.0.remove(&handle.run_id);
        } else {
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
}
