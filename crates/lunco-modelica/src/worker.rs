//! Off-thread Modelica simulation worker + Bevy bridge.
//!
//! `modelica_worker` runs on its own OS thread (it owns a
//! `!Send` `SimStepper`, so it can't live on the Bevy main loop). The
//! Bevy systems `spawn_modelica_requests` and
//! `handle_modelica_responses` exchange `ModelicaCommand` /
//! `ModelicaResult` messages with it via crossbeam channels.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use rumoca_sim::{SimStepper, StepperOptions};
use serde::{Serialize, Deserialize};

use lunco_assets::modelica_dir;

use crate::ast_extract::strip_input_defaults;
use crate::sim_stream::{SimSnapshot, SimStream};
use crate::ModelicaCompiler;

/// Default relative / absolute tolerances for the adaptive DAE stepper.
///
/// Split because the two knobs do different jobs: `rtol` bounds the *relative*
/// (fractional) error and dominates for large-magnitude states (temperatures,
/// positions); `atol` is the *absolute* floor that takes over near zero (small
/// fluxes, rates) where a relative bound is meaningless. Forcing them equal —
/// the previous `1e-1` for both — is wrong for models that mix wildly different
/// magnitudes. These are the conventional SUNDIALS-style defaults.
///
/// Single source of truth for every `SimStepper::new` call in this worker
/// (previously copy-pasted at ~9 sites).
///
/// TRADE-OFF: tighter than the old `1e-1` → more accurate, but the BDF
/// integrator may take smaller steps on very stiff models (radiative thermal
/// over lunar-day horizons, smooth-abs/max contact). If live sim stalls, loosen
/// `DEFAULT_RTOL` first.
///
/// NOTE: a model's own `experiment(Tolerance=…)` annotation is captured as
/// `experiment_tolerance` but is NOT yet applied here — wiring that through
/// would let models request their own solver tolerance (separate change;
/// behaviour-affecting).
const DEFAULT_RTOL: f64 = 1e-3;
const DEFAULT_ATOL: f64 = 1e-6;

/// Channels for communicating with the background simulation worker.
///
/// This resource holds the crossbeam channel endpoints that the main Bevy thread
/// uses to send commands to and receive results from the `modelica_worker` thread.
#[derive(Resource)]
pub struct ModelicaChannels {
    /// Sender for `ModelicaCommand` -> worker
    pub tx: Sender<ModelicaCommand>,
    /// Receiver for `ModelicaResult` <- worker
    pub rx: Receiver<ModelicaResult>,
    /// Receiver for `ModelicaCommand` <- UI (used by wasm32 inline worker)
    #[cfg(target_arch = "wasm32")]
    pub rx_cmd: Receiver<ModelicaCommand>,
    /// Sender for `ModelicaResult` -> UI (used by wasm32 inline worker)
    #[cfg(target_arch = "wasm32")]
    pub tx_res: Sender<ModelicaResult>,
}

/// Commands sent to the background simulation worker.
///
/// Each command targets a specific Bevy `Entity` and carries a `session_id` for
/// fencing stale results. The worker owns all `SimStepper` instances, keyed by entity.
///
/// Derives `Serialize`/`Deserialize` so the wasm Web Worker transport can ship
/// commands over `postMessage`. The `Compile.stream` field carries an
/// `Arc<ArcSwap<…>>` that can't cross a worker boundary; it's `#[serde(skip)]`
/// — wasm builds always use the `outputs`-via-result path instead of the
/// shared snapshot fast-path. Native still uses the shared snapshot
/// in-process and never touches serde here.
#[derive(Serialize, Deserialize)]
pub enum ModelicaCommand {
    /// Advance simulation by one timestep. Sent every frame from `spawn_modelica_requests`.
    Step {
        entity: Entity,
        session_id: u64,
        model_path: PathBuf,
        model_name: String,
        inputs: Vec<(String, f64)>,
        dt: f64,
    },
    /// Compile Modelica source code into a DAE and create a new SimStepper.
    ///
    /// The compiled DAE is cached per entity for instant Reset and fast stepper rebuilds.
    Compile {
        entity: Entity,
        session_id: u64,
        model_name: String,
        source: String,
        /// Sources from other open Modelica documents, as
        /// `(filename, source)` pairs. Loaded into the rumoca
        /// session before the primary `source` so cross-doc class
        /// references (e.g. an untitled `RocketStage` referencing
        /// `AnnotatedRocketStage.Tank` from a sibling untitled
        /// package) resolve. Empty when only one doc is open.
        extra_sources: Vec<(String, String)>,
        /// Lock-free snapshot handle the worker publishes into after
        /// every successful Step (Phase A of the multi-sim arch).
        /// `None` = legacy path; main thread still receives per-sample
        /// data via `ModelicaResult.outputs` and pushes it into
        /// `SignalRegistry`. When `Some`, the worker updates the
        /// stream directly and the main-thread handler can skip the
        /// per-sample push loop.
        ///
        /// Skipped by serde: the `Arc<ArcSwap<_>>` only makes sense
        /// inside one address space. On wasm (Web Worker transport)
        /// this is always serialized as `None`, forcing the legacy
        /// outputs-via-result path. Native is unaffected.
        #[serde(skip)]
        stream: Option<SimStream>,
    },
    /// Update parameter values by recompiling with modified source code.
    ///
    /// Since Modelica parameters are compile-time constants, changing them requires
    /// recompilation. This command takes the full source with substituted parameter values,
    /// creates a new stepper, and updates the cached DAE.
    UpdateParameters {
        entity: Entity,
        session_id: u64,
        model_name: String,
        source: String,
    },
    /// Reset the stepper to initial conditions using the cached DAE (instant, no recompilation).
    Reset {
        entity: Entity,
        session_id: u64,
    },
    /// Remove the stepper and cached DAE (entity despawned).
    Despawn {
        entity: Entity,
    },
    /// Load a Modelica source root into the rumoca compile session
    /// so subsequent Compile commands can resolve types from it.
    /// Sent by the main-thread pre-Compile gate
    /// (`source_roots::ensure_loaded`) when a doc references a
    /// library/package that isn't yet in the session.
    ///
    /// Worker handles by routing on `payload`:
    /// - [`LoadSourceRootPayload::Disk`] → system libraries; calls
    ///   `compiler.load_source_root(id, &root_dir)`.
    /// - [`LoadSourceRootPayload::InMemory`] → bundled examples +
    ///   single workspace files; calls
    ///   `compiler.load_source_root_in_memory(id, &label, files)`.
    ///
    /// Idempotent: rumoca dedups by id. **Blocks the worker thread**
    /// for the duration of the parse (MSL: ~10-60s cold, ~1-3s
    /// warm-bundle); other commands queue behind it.
    LoadSourceRoot {
        /// Library id, e.g. `"Modelica"` or `"AnnotatedRocketStage"`.
        id: String,
        /// What to load and how to load it.
        payload: LoadSourceRootPayload,
    },
}

/// Payload for [`ModelicaCommand::LoadSourceRoot`]. Distinguishes
/// disk-rooted libraries from in-memory sources so the worker can
/// dispatch to the right rumoca-compile API without losing the
/// source bytes on the way.
#[derive(Serialize, Deserialize)]
pub enum LoadSourceRootPayload {
    /// Disk-rooted library (MSL, third-party). `root_dir` contains
    /// `package.mo`. Loaded via
    /// `Session::load_source_root_tolerant`.
    Disk { root_dir: PathBuf },
    /// In-memory `(uri, source)` pairs. Used for bundled examples
    /// (source comes from the embedded binary via
    /// `crate::models::get_model`) and workspace files (source
    /// read from disk by the main thread). `label` shows up in
    /// rumoca diagnostics.
    InMemory {
        label: String,
        files: Vec<(String, String)>,
    },
}

use std::sync::Arc;

/// Results received from the background simulation worker.
///
/// Contains simulation outputs, detected symbols, and error information.
/// The `session_id` field is used by `handle_modelica_responses` to fence stale results.
///
/// Derives serde for the wasm Web Worker transport. All fields are plain
/// data; no special handling required.
#[derive(Serialize, Deserialize)]
pub struct ModelicaResult {
    pub entity: Entity,
    pub session_id: u64,
    pub new_time: f64,
    pub outputs: Vec<(String, f64)>,
    pub detected_symbols: Vec<(String, f64)>,
    pub error: Option<String>,
    pub log_message: Option<String>,
    pub is_new_model: bool,
    pub is_parameter_update: bool,
    pub is_reset: bool,
    /// Input variable names discovered from the model (input Real ...).
    /// These can be changed at runtime without recompilation.
    pub detected_input_names: Vec<String>,
    /// Modelica `experiment(...)` annotation values, lifted from
    /// rumoca's `CompilationResult`. Populated only on
    /// `is_new_model = true` (Compile / UpdateParameters); `None`
    /// elsewhere. Plumbed end-to-end so the Fast Run toolbar can
    /// prefill bounds from the model rather than always defaulting
    /// to 0..1. See `docs/architecture/25-experiments.md` §"Bounds
    /// from annotation".
    #[serde(default)]
    pub experiment_start_time: Option<f64>,
    #[serde(default)]
    pub experiment_stop_time: Option<f64>,
    #[serde(default)]
    pub experiment_tolerance: Option<f64>,
    #[serde(default)]
    pub experiment_interval: Option<f64>,
    #[serde(default)]
    pub experiment_solver: Option<String>,
    /// Detected name of the compiled top-level class. Lets the main
    /// thread route the `experiment_*` defaults into the runner's
    /// per-`ModelRef` cache without a second AST pass.
    #[serde(default)]
    pub compiled_model_name: Option<String>,
    /// Set when this result acknowledges a
    /// [`ModelicaCommand::LoadSourceRoot`]. The main-thread drain
    /// system uses this to transition the matching
    /// [`crate::source_roots::SourceRootRegistry`] entry from
    /// `Loading` to `Ready` (or `Failed` when `error.is_some()`).
    /// Regular Compile / Step results leave it `None`.
    #[serde(default)]
    pub loaded_source_root_id: Option<String>,
}

impl Default for ModelicaResult {
    fn default() -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
            session_id: 0,
            new_time: 0.0,
            outputs: Vec::new(),
            detected_symbols: Vec::new(),
            error: None,
            log_message: None,
            is_new_model: false,
            is_parameter_update: false,
            is_reset: false,
            detected_input_names: Vec::new(),
            experiment_start_time: None,
            experiment_stop_time: None,
            experiment_tolerance: None,
            experiment_interval: None,
            experiment_solver: None,
            compiled_model_name: None,
            loaded_source_root_id: None,
        }
    }
}

/// Cached compilation result per entity.
///
/// Stores the DAE and source hash so we can instantly rebuild a SimStepper
/// after Reset without recompiling, and detect when the Step command's
/// model_path points to stale source.
struct CachedModel {
    model_name: String,
    source: Arc<str>,
}

/// Collect every readable variable from the stepper — states, inputs, and
/// (on rumoca `main`) algebraic / output reconstructions via
/// `EliminationResult`. Non-finite values are dropped so the UI never
/// plots NaN. Filtering out parameters / inputs happens downstream in
/// [`handle_modelica_responses`]; we report everything here so the UI has
/// the full picture and decides what goes into `model.variables`.
fn collect_stepper_observables(stepper: &SimStepper) -> Vec<(String, f64)> {
    stepper
        .state()
        .values
        .into_iter()
        .filter(|(name, val)| val.is_finite() && name != "time")
        .collect()
}

/// Helper to build a ModelicaResult with defaults.
fn result_ok(entity: Entity, session_id: u64) -> ModelicaResult {
    ModelicaResult {
        entity,
        session_id,
        ..Default::default()
    }
}

/// Apply parsed input defaults to a stepper at init time, logging any
/// mismatch between the rumoca-detected names and the stepper's actual
/// input slots. The mismatch case is a rumoca-vs-flatten disagreement —
/// rare, but silent failure here would mean a user-set default never
/// reaches the simulator. Logged once per init, not per-call.
fn apply_input_defaults_validated(
    stepper: &mut SimStepper,
    input_defaults: &HashMap<String, f64>,
    ctx: &str,
) {
    if input_defaults.is_empty() {
        return;
    }
    let known: std::collections::HashSet<String> =
        stepper.input_names().iter().cloned().collect();
    let unknown: Vec<&str> = input_defaults
        .keys()
        .filter(|n| !known.contains(*n))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        bevy::log::warn!(
            "[{ctx}] {} parsed input default(s) not in stepper.input_names(): {:?} (known: {:?})",
            unknown.len(),
            unknown,
            known,
        );
    }
    for (name, val) in input_defaults {
        if !known.contains(name) {
            continue;
        }
        if let Err(e) = stepper.set_input(name, *val) {
            bevy::log::warn!("[{ctx}] set_input({name}) failed: {e:?}");
        }
    }
}

/// The background worker that owns the !Send SimSteppers and cached DAEs.
pub fn modelica_worker(rx: Receiver<ModelicaCommand>, tx: Sender<ModelicaResult>) {
    let mut steppers: HashMap<Entity, (u64, String, SimStepper)> = HashMap::default();
    let mut current_sessions: HashMap<Entity, u64> = HashMap::default();
    // DAE cache per entity — enables instant Reset and fast Step auto-init
    let mut cached_models: HashMap<Entity, CachedModel> = HashMap::default();
    // Lock-free publish stream per entity (Phase A of the multi-sim
    // refactor — see `sim_stream.rs`). The UI side holds a clone of
    // the same `Arc<ArcSwap<SimSnapshot>>`; every successful Step
    // publishes a new snapshot so plots render without locking or
    // involving the main thread in per-sample work.
    let mut sim_streams: HashMap<Entity, SimStream> = HashMap::default();
    // Lazy compiler construction. `ModelicaCompiler::new` is now
    // cheap — it creates an empty session with no MSL loaded.
    // Actual MSL files are pulled into the session on demand by
    // `compile_str` based on what each compile's reachable closure
    // references. No reason to pre-build it.
    let mut compiler: Option<ModelicaCompiler> = None;

    while let Ok(first_cmd) = rx.recv() {
        let mut pending = vec![first_cmd];
        while let Ok(cmd) = rx.try_recv() { pending.push(cmd); }

        let mut to_process = Vec::new();
        for cmd in pending {
            if let Some(last) = to_process.last_mut() {
                if is_squashable(last, &cmd) {
                    if cmd_session(last) == cmd_session(&cmd) {
                        let _ = tx.send(result_ok(cmd_entity(last), cmd_session(last)));
                        *last = cmd;
                        continue;
                    }
                }
            }
            to_process.push(cmd);
        }

        for cmd in to_process {
            let tx_inner = tx.clone();
            // Instrumentation for the "sometimes stuck" class of bugs:
            // when the worker hangs (usually inside a pathological
            // rumoca compile on a malformed model), the main-thread
            // UI sees no progress and no log breadcrumb. These bracket
            // logs let us see exactly which command + model was
            // in-flight and how long it actually took, so a stall is
            // visible in `RUST_LOG=info` output instead of silent.
            let cmd_label = command_label(&cmd);
            let cmd_started = web_time::Instant::now();
            // `Step` fires at simulation rate (~60 Hz) — log at debug to
            // avoid drowning the console. One-shot commands (Compile,
            // Reset, …) stay at info because they're rare and useful.
            let is_hot_path = matches!(cmd, ModelicaCommand::Step { .. });
            if is_hot_path {
                log::debug!("[worker] begin: {}", cmd_label);
            } else {
                log::info!("[worker] begin: {}", cmd_label);
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match cmd {
                    ModelicaCommand::Reset { entity, session_id } => {
                        current_sessions.insert(entity, session_id);

                        if let Some(cached) = cached_models.get(&entity) {
                            // Strip input defaults from cached source and set them via set_input
                            let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);

                            let mut opts = StepperOptions::default();
                            opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;
                            // Recompile stripped source to get a fresh stepper with input slots
                            let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                            match compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                                Ok(comp_res) => {
                                    match SimStepper::new(&comp_res.dae, opts) {
                                        Ok(mut stepper) => {
                                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Init");
                                            let input_names: Vec<String> = stepper.input_names().to_vec();
                                            let symbols = collect_stepper_observables(&stepper);
                                            steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                                            let _ = tx_inner.send(ModelicaResult {
                                                entity, session_id, new_time: 0.0,
                                                outputs: Vec::new(),
                                                detected_symbols: symbols, error: None,
                                                log_message: Some("Reset complete.".to_string()),
                                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                                detected_input_names: input_names,
                                                ..Default::default()
                                            });
                                        }
                                        Err(e) => {
                                            let mut r = result_ok(entity, session_id);
                                            r.error = Some(format!("Stepper Init Error: {:?}", e));
                                            r.is_reset = true;
                                            let _ = tx_inner.send(r);
                                        }
                                    }
                                }
                                Err(e) => {
                                    let mut r = result_ok(entity, session_id);
                                    r.error = Some(format!("Reset compile error: {:?}", e));
                                    r.is_reset = true;
                                    let _ = tx_inner.send(r);
                                }
                            }
                        } else {
                            steppers.remove(&entity);
                            let mut r = result_ok(entity, session_id);
                            r.is_reset = true;
                            r.log_message = Some("Reset complete (no cached model).".to_string());
                            let _ = tx_inner.send(r);
                        }
                    }
                    ModelicaCommand::UpdateParameters { entity, session_id, model_name, source } => {
                        if session_id < *current_sessions.get(&entity).unwrap_or(&0) {
                            let _ = tx_inner.send(result_ok(entity, session_id));
                            return;
                        }
                        current_sessions.insert(entity, session_id);

                        let temp_dir = modelica_dir().join(format!("{}_{}", entity.index(), entity.generation()));
                        let _ = std::fs::create_dir_all(&temp_dir);
                        let temp_path = temp_dir.join("model.mo");
                        if let Err(e) = std::fs::write(&temp_path, &source) {
                            let mut r = result_ok(entity, session_id);
                            r.error = Some(format!("IO Error: {:?}", e));
                            let _ = tx_inner.send(r);
                            return;
                        }

                        // Strip input defaults so they become real runtime slots
                        let (stripped_source, input_defaults) = strip_input_defaults(&source);

                        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                        match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                            Ok(comp_res) => {
                                let mut opts = StepperOptions::default();
                                opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;
                                match SimStepper::new(&comp_res.dae, opts) {
                                    Ok(mut stepper) => {
                                        apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                                        let input_names: Vec<String> = stepper.input_names().to_vec();
                                        let symbols = collect_stepper_observables(&stepper);
                                        cached_models.insert(entity, CachedModel {
                                            model_name: model_name.clone(),
                                            source: Arc::from(source),
                                        });
                                        steppers.insert(entity, (session_id, model_name.clone(), stepper));
                                        let _ = tx_inner.send(ModelicaResult {
                                            entity, session_id, new_time: 0.0,
                                            outputs: Vec::new(),
                                            detected_symbols: symbols, error: None,
                                            log_message: Some("Parameters applied.".to_string()),
                                            is_new_model: false, is_parameter_update: true, is_reset: false,
                                            detected_input_names: input_names,
                                            ..Default::default()
                                        });
                                    }
                                    Err(e) => {
                                        let mut r = result_ok(entity, session_id);
                                        r.error = Some(format!("Stepper Init Error: {:?}", e));
                                        r.is_parameter_update = true;
                                        let _ = tx_inner.send(r);
                                    }
                                }
                            }
                            Err(e) => {
                                let mut r = result_ok(entity, session_id);
                                r.error = Some(format!("Re-compile Error: {:?}", e));
                                r.is_parameter_update = true;
                                let _ = tx_inner.send(r);
                            }
                        }
                    }
                    ModelicaCommand::Compile { entity, session_id, model_name, source, extra_sources, stream } => {
                        current_sessions.insert(entity, session_id);
                        if let Some(stream) = stream {
                            // Register the new lock-free publish target
                            // AND reset the previous snapshot so stale
                            // history from a prior compile doesn't bleed
                            // into the new model's horizon.
                            stream.store(Arc::new(SimSnapshot::empty_at_zero()));
                            sim_streams.insert(entity, stream);
                        }

                        // Strip input defaults so they become real runtime slots
                        let (stripped_source, input_defaults) = strip_input_defaults(&source);

                        // Loud breadcrumbs around the two opaque-and-slow
                        // steps (MSL preload + rumoca compile). Without
                        // these, the worker silently disappears for the
                        // duration — the rumoca log macros may or may
                        // not route through the workbench's tracing sink
                        // depending on Bevy's tracing-subscriber config.
                        // `bevy::log::info!` always reaches stdout.
                        let was_first_compile = compiler.is_none();
                        if was_first_compile {
                            bevy::log::info!(
                                "[worker] first-time compiler init — loading MSL into rumoca session (this can take ~10s on warm cache, minutes on cold `.cache/rumoca`)"
                            );
                        }
                        let t_init = web_time::Instant::now();
                        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                        if was_first_compile {
                            bevy::log::info!(
                                "[worker] compiler init done in {:.2}s",
                                t_init.elapsed().as_secs_f64(),
                            );
                        }
                        bevy::log::info!(
                            "[worker] calling compile_str for `{}` ({} bytes)",
                            model_name, stripped_source.len(),
                        );
                        let t_compile = web_time::Instant::now();
                        let _compile_outcome = if extra_sources.is_empty() {
                            compiler.compile_str(&model_name, &stripped_source, "model.mo")
                        } else {
                            compiler.compile_str_multi(&model_name, &stripped_source, "model.mo", &extra_sources)
                        };
                        bevy::log::info!(
                            "[worker] compile_str returned for `{}` in {:.2}s ({})",
                            model_name,
                            t_compile.elapsed().as_secs_f64(),
                            if _compile_outcome.is_ok() { "OK" } else { "ERR" },
                        );
                        match _compile_outcome {
                            Ok(comp_res) => {
                                // Capture experiment(...) annotation
                                // values BEFORE comp_res moves into the
                                // cache; the Fast Run toolbar reads
                                // these as bounds defaults.
                                let exp_t_start = comp_res.experiment_start_time;
                                let exp_t_end = comp_res.experiment_stop_time;
                                let exp_tol = comp_res.experiment_tolerance;
                                let exp_interval = comp_res.experiment_interval;
                                let exp_solver = comp_res.experiment_solver.clone();
                                let mut opts = StepperOptions::default();
                                opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;
                                match SimStepper::new(&comp_res.dae, opts) {
                                    Ok(mut stepper) => {
                                        // Set input defaults via set_input so they're runtime-changeable
                                        apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                                        let input_names: Vec<String> = stepper.input_names().to_vec();
                                        let symbols = collect_stepper_observables(&stepper);
                                        let temp_dir = modelica_dir().join(format!("{}_{}", entity.index(), entity.generation()));
                                        let _ = std::fs::create_dir_all(&temp_dir);
                                        let temp_path = temp_dir.join("model.mo");
                                        let _ = std::fs::write(&temp_path, &source);

                                        cached_models.insert(entity, CachedModel {
                                            model_name: model_name.clone(),
                                            source: Arc::from(source),
                                        });
                                        steppers.insert(entity, (session_id, model_name.clone(), stepper));
                                        let _ = tx_inner.send(ModelicaResult {
                                            entity, session_id, new_time: 0.0,
                                            outputs: Vec::new(),
                                            detected_symbols: symbols, error: None,
                                            log_message: Some(format!("Model '{}' compiled.", model_name)),
                                            is_new_model: true, is_parameter_update: false, is_reset: false,
                                            detected_input_names: input_names,
                                            experiment_start_time: exp_t_start,
                                            experiment_stop_time: exp_t_end,
                                            experiment_tolerance: exp_tol,
                                            experiment_interval: exp_interval,
                                            experiment_solver: exp_solver,
                                            compiled_model_name: Some(model_name.clone()),
                                            loaded_source_root_id: None,
                                        });
                                    }
                                    Err(e) => {
                                        let mut r = result_ok(entity, session_id);
                                        r.error = Some(format!("Stepper Error: {:?}", e));
                                        // Stepper init failure during
                                        // Compile IS a compile-attempt
                                        // result — the UI classifies
                                        // and transitions state on
                                        // this flag.
                                        r.is_new_model = true;
                                        let _ = tx_inner.send(r);
                                    }
                                }
                            }
                            Err(e) => {
                                let mut r = result_ok(entity, session_id);
                                r.error = Some(format!("Compiler Error: {:?}", e));
                                r.is_new_model = true;
                                let _ = tx_inner.send(r);
                            }
                        }
                    }
                    ModelicaCommand::Step { entity, session_id, model_path, model_name, inputs, dt } => {
                        if session_id < *current_sessions.get(&entity).unwrap_or(&0) {
                            let _ = tx_inner.send(result_ok(entity, session_id));
                            return;
                        }

                        let needs_init = match steppers.get(&entity) {
                            Some((s_id, s_name, _)) => *s_id < session_id || s_name != &model_name,
                            None => true,
                        };

                        if needs_init {
                            // Try cached DAE first — recompile stripped source for input slots
                            if let Some(cached) = cached_models.get(&entity) {
                                if cached.model_name == model_name {
                                    let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);
                                    let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                                    if let Ok(comp_res) = compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                                        let mut opts = StepperOptions::default();
                                        opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;
                                        if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                                            apply_input_defaults_validated(&mut s, &input_defaults, "Compile");
                                            // Then apply any user-provided input overrides
                                            for (name, val) in &inputs {
                                                let _ = s.set_input(name, *val);
                                            }
                                            steppers.insert(entity, (session_id, model_name.clone(), s));
                                        }
                                    }
                                }
                            }
                            // Fallback: compile from file on disk
                            if !steppers.contains_key(&entity) {
                                let source = std::fs::read_to_string(&model_path).unwrap_or_default();
                                let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                                match compiler.compile_str(&model_name, &source, &model_path.to_string_lossy()) {
                                    Ok(comp_res) => {
                                        let mut opts = StepperOptions::default();
                                        opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;
                                        if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                                            for (name, val) in &inputs { let _ = s.set_input(name, *val); }
                                            cached_models.insert(entity, CachedModel {
                                                model_name: model_name.clone(),
                                                source: Arc::from(std::fs::read_to_string(&model_path).unwrap_or_default()),
                                            });

                                            steppers.insert(entity, (session_id, model_name.clone(), s));
                                        }
                                    }
                                    Err(e) => {
                                        let mut r = result_ok(entity, session_id);
                                        r.error = Some(format!("Initialization Failed: {:?}", e));
                                        let _ = tx_inner.send(r);
                                        return;
                                    }
                                }
                            }
                        }

                        if let Some((s_id, _, stepper)) = steppers.get_mut(&entity) {
                            if *s_id == session_id {
                                for (name, val) in inputs { let _ = stepper.set_input(&name, val); }
                                let capped_dt = dt.min(0.033); let sub_dt = capped_dt / 3.0;
                                let mut step_err = None;
                                for _ in 0..3 { if let Err(e) = stepper.step(sub_dt) { step_err = Some(e); break; } }
                                if let Some(e) = step_err {
                                    let mut r = result_ok(entity, session_id);
                                    r.new_time = stepper.time();
                                    r.error = Some(format!("Solver Error: {:?}", e));
                                    let _ = tx_inner.send(r);
                                    steppers.remove(&entity);
                                } else {
                                    // `state()` reconstructs algebraics / outputs via
                                    // `EliminationResult` and also includes inputs, so
                                    // this single call supersedes the old two-loop
                                    // variable_names + input_names collection.
                                    let outputs = collect_stepper_observables(stepper);
                                    let new_time = stepper.time();
                                    // Phase A: also publish to the
                                    // lock-free stream so consumers that
                                    // wire into it (plots, telemetry —
                                    // see TODO arch-phase-a2) can read
                                    // without main-thread round-tripping.
                                    // We continue to ship `outputs`
                                    // through the crossbeam channel as
                                    // well until plots have migrated;
                                    // once they read from `SimStream`
                                    // exclusively, the `outputs` Vec
                                    // can be cleared here to drop the
                                    // per-sample main-thread push loop.
                                    if let Some(stream) = sim_streams.get(&entity) {
                                        let prev = stream.load();
                                        let next = SimSnapshot::advance(&prev, new_time, &outputs);
                                        stream.store(Arc::new(next));
                                    }
                                    let _ = tx_inner.send(ModelicaResult {
                                        entity, session_id, new_time,
                                        outputs, error: None, log_message: None,
                                        is_new_model: false, detected_symbols: Vec::new(),
                                        is_parameter_update: false, is_reset: false,
                                        detected_input_names: Vec::new(),
                                        ..Default::default()
                                    });
                                }
                            } else {
                                let _ = tx_inner.send(result_ok(entity, session_id));
                            }
                        } else {
                            let mut r = result_ok(entity, session_id);
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
                    }
                    ModelicaCommand::LoadSourceRoot { id, payload } => {
                        let compiler = compiler
                            .get_or_insert_with(ModelicaCompiler::new);
                        let t0 = web_time::Instant::now();
                        let report = match payload {
                            LoadSourceRootPayload::Disk { root_dir } => {
                                log::info!(
                                    "[worker] LoadSourceRoot `{}` (disk: {})",
                                    id,
                                    root_dir.display(),
                                );
                                compiler.load_source_root(&id, &root_dir)
                            }
                            LoadSourceRootPayload::InMemory { label, files } => {
                                log::info!(
                                    "[worker] LoadSourceRoot `{}` (in-memory: {}, {} file(s))",
                                    id,
                                    label,
                                    files.len(),
                                );
                                compiler.load_source_root_in_memory(&id, &label, files)
                            }
                        };
                        log::info!(
                            "[worker] LoadSourceRoot `{}` done: {} parsed / {} \
                             inserted in {:.2}s",
                            id,
                            report.parsed_file_count,
                            report.inserted_file_count,
                            t0.elapsed().as_secs_f64(),
                        );
                        // Ack back to the main thread so the registry can
                        // flip Loading → Ready (or Failed when diagnostics
                        // are non-empty).
                        let err = if report.diagnostics.is_empty() {
                            None
                        } else {
                            Some(report.diagnostics.join("; "))
                        };
                        let _ = tx_inner.send(ModelicaResult {
                            loaded_source_root_id: Some(id),
                            error: err,
                            ..Default::default()
                        });
                    }
                }
            }));

            let elapsed = cmd_started.elapsed();
            // Flag anything slow enough that a user would perceive it
            // as "stuck" at WARN so it shows up even without verbose
            // logging. The 2s threshold is well above a typical MSL
            // compile (<500ms) but below "waited through it" (>5s).
            if elapsed > std::time::Duration::from_secs(2) {
                log::warn!(
                    "[worker] end: {} took {:?} (slow — possible stall)",
                    cmd_label,
                    elapsed
                );
            } else if is_hot_path {
                log::debug!("[worker] end: {} took {:?}", cmd_label, elapsed);
            } else {
                log::info!("[worker] end: {} took {:?}", cmd_label, elapsed);
            }

            if let Err(_) = result {
                let _ = tx.send(ModelicaResult {
                    entity: Entity::PLACEHOLDER,
                    session_id: 0, new_time: 0.0,
                    outputs: Vec::new(), detected_symbols: Vec::new(),
                    error: Some("Internal Worker Panic!".to_string()), log_message: None,
                    is_new_model: false, is_parameter_update: false, is_reset: false,
                    detected_input_names: Vec::new(),
                    ..Default::default()
                });
            }
        }
    }
}

/// One-line identifier for a `ModelicaCommand`, used in worker
/// instrumentation logs. Includes the model name where available so
/// a stall can be pinned to a specific source.
fn command_label(cmd: &ModelicaCommand) -> String {
    match cmd {
        ModelicaCommand::Step { model_name, entity, .. } => {
            format!("Step model={model_name} entity={entity:?}")
        }
        ModelicaCommand::Compile { model_name, entity, .. } => {
            format!("Compile model={model_name} entity={entity:?}")
        }
        ModelicaCommand::UpdateParameters { model_name, entity, .. } => {
            format!("UpdateParameters model={model_name} entity={entity:?}")
        }
        ModelicaCommand::Reset { entity, .. } => format!("Reset entity={entity:?}"),
        ModelicaCommand::Despawn { entity } => format!("Despawn entity={entity:?}"),
        ModelicaCommand::LoadSourceRoot { id, .. } => format!("LoadSourceRoot id={id}"),
    }
}

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
/// (e.g., dragging a parameter slider). Only the latest value is processed.
fn is_squashable(last: &ModelicaCommand, next: &ModelicaCommand) -> bool {
    match (last, next) {
        (ModelicaCommand::Step { entity: e1, .. }, ModelicaCommand::Step { entity: e2, .. }) => e1 == e2,
        (ModelicaCommand::UpdateParameters { entity: e1, .. }, ModelicaCommand::UpdateParameters { entity: e2, .. }) => e1 == e2,
        (ModelicaCommand::Compile { entity: e1, .. }, ModelicaCommand::Compile { entity: e2, .. }) => e1 == e2,
        _ => false,
    }
}

// =============================================================================
// WebAssembly Inline Worker (wasm32 only - no thread support in browser)
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

/// Inner simulation state for wasm32 inline worker.
/// Mirrors the local variables in `modelica_worker` on desktop.
///
/// `pub` so the off-thread worker bin (`bin/lunica_worker.rs`) can own
/// one of these directly. The fields stay private — only the type itself
/// crosses crate boundaries.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct InlineWorkerInner {
    steppers: HashMap<Entity, (u64, String, SimStepper)>,
    current_sessions: HashMap<Entity, u64>,
    cached_models: HashMap<Entity, CachedModel>,
    compiler: Option<ModelicaCompiler>,
}

#[cfg(target_arch = "wasm32")]
impl InlineWorkerInner {
    /// Lazily-built shared compiler. Same instance the regular
    /// Compile path uses, so RunFast hits the same warm caches.
    pub fn compiler(&mut self) -> &mut ModelicaCompiler {
        self.compiler.get_or_insert_with(ModelicaCompiler::new)
    }
}

/// Thread-safe wrapper for wasm32 inline worker state.
///
/// SAFETY: wasm32-unknown-unknown has no threads, so Send/Sync are vacuously true.
/// SimStepper internally uses Rc<RefCell<>> which is !Send, but since no threads
/// exist on this target, we can safely implement Send/Sync.
#[cfg(target_arch = "wasm32")]
#[derive(Resource, Default)]
pub(crate) struct InlineWorker {
    inner: InlineWorkerInner,
}

#[cfg(target_arch = "wasm32")]
impl InlineWorker {
    /// Drop any previously-constructed `ModelicaCompiler`. Used by the
    /// MSL drain when the in-memory bundle finishes loading: a compiler
    /// that was lazily built before MSL was available has an empty
    /// session and would yield `unresolved type reference` for every
    /// MSL ref. The next compile will re-init via
    /// `get_or_insert_with(ModelicaCompiler::new)` and pick up the
    /// global MSL source.
    pub(crate) fn reset_compiler(&mut self) {
        self.inner.compiler = None;
    }
}

// SAFETY: wasm32-unknown-unknown has no threads, so Send/Sync are vacuously true.
#[cfg(target_arch = "wasm32")]
unsafe impl Send for InlineWorker {}
#[cfg(target_arch = "wasm32")]
unsafe impl Sync for InlineWorker {}

/// Processes Modelica commands inline on wasm32 (no background thread).
///
/// Runs each frame in the Update schedule. Drains one command from the
/// channel and processes it synchronously, sending results back immediately.
#[cfg(target_arch = "wasm32")]
pub(crate) fn inline_worker_process(
    mut worker: ResMut<InlineWorker>,
    channels: Res<ModelicaChannels>,
) {
    // If the off-thread Web Worker is wired up
    // (`worker_transport::install_worker` succeeded), it owns the
    // `rx_cmd` queue: its pump system drains commands and forwards them
    // to the worker bundle. We must not also consume from the same
    // queue here or commands would race. Bail out — the worker
    // pipeline is the active one.
    if crate::worker_transport::is_worker_active() {
        return;
    }
    // Process one command per frame to avoid blocking the main thread.
    let Ok(cmd) = channels.rx_cmd.try_recv() else { return };
    let tx = channels.tx_res.clone();
    process_inline_command(&mut worker.inner, cmd, |r| {
        let _ = tx.send(r);
    });
}

/// Apply a single `ModelicaCommand` against the inline worker state, sending
/// any resulting `ModelicaResult` values through `send`.
///
/// Same dispatch the desktop `modelica_worker` loop runs, parameterised over
/// the result sink so both the in-process inline path
/// (`inline_worker_process`) and the off-thread Web Worker entry
/// (`bin/lunica_worker.rs`) can share it. Passing a closure rather than a
/// concrete `Sender` keeps this fn agnostic to whether results go to a
/// crossbeam channel, a `Vec`, or a `postMessage` queue.
///
/// `state` carries the per-entity `SimStepper` map, DAE cache, and the lazy
/// `ModelicaCompiler`. The wasm worker bin owns one of these for the lifetime
/// of the page and reuses it across postMessage dispatches.
#[cfg(target_arch = "wasm32")]
pub fn process_inline_command<F: FnMut(ModelicaResult)>(
    state: &mut InlineWorkerInner,
    cmd: ModelicaCommand,
    mut send: F,
) {
    let w = state;
    match cmd {
        ModelicaCommand::Step { entity, session_id, model_name, inputs, dt, model_path: _ } => {

            // Auto-init: compile if stepper doesn't exist
            if !w.steppers.contains_key(&entity) {
                // Try cached DAE first
                if let Some(cached) = w.cached_models.get(&entity) {
                    if cached.model_name == model_name {
                        let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);
                        let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
                        if let Ok(comp_res) = compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                            let mut opts = StepperOptions::default();
                            opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;
                            if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                                apply_input_defaults_validated(&mut s, &input_defaults, "Compile");
                                for (name, val) in &inputs { let _ = s.set_input(name, *val); }
                                w.steppers.insert(entity, (session_id, model_name.clone(), s));
                            }
                        }
                    }
                }
                // Fallback: try to compile from model_path (won't work in web)
                // In web mode, models must be pre-compiled via Compile command first
            }

            if let Some((s_id, _, stepper)) = w.steppers.get_mut(&entity) {
                if *s_id == session_id {
                    for (name, val) in &inputs { let _ = stepper.set_input(name, *val); }
                    let capped_dt = dt.min(0.033);
                    let sub_dt = capped_dt / 3.0;
                    let mut step_err = None;
                    for _ in 0..3 { if let Err(e) = stepper.step(sub_dt) { step_err = Some(e); break; } }

                    if let Some(e) = step_err {
                        send(ModelicaResult {
                            entity, session_id, new_time: stepper.time(),
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(), error: Some(format!("Solver Error: {:?}", e)),
                            log_message: None, is_new_model: false, is_parameter_update: false,
                            is_reset: false, detected_input_names: Vec::new(),
                            ..Default::default()
                        });
                        w.steppers.remove(&entity);
                    } else {
                        let outputs = collect_stepper_observables(stepper);
                        send(ModelicaResult {
                            entity, session_id, new_time: stepper.time(),
                            outputs, error: None,
                            log_message: None, is_new_model: false, detected_symbols: Vec::new(),
                            is_parameter_update: false, is_reset: false, detected_input_names: Vec::new(),
                            ..Default::default()
                        });
                    }
                } else {
                    send(result_ok(entity, session_id));
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
                    entity, session_id, new_time: 0.0,
                    outputs: Vec::new(),
                    detected_symbols: Vec::new(),
                    error: Some(
                        "No compiled model. Click Compile (or Run will compile + start)."
                            .to_string(),
                    ),
                    log_message: None, is_new_model: false, is_parameter_update: false,
                    is_reset: false, detected_input_names: Vec::new(),
                    ..Default::default()
                });
            }
        }
        ModelicaCommand::Compile { entity, session_id, model_name, source, extra_sources, stream: _stream } => {
            // NB: the wasm inline worker runs on the Bevy main thread
            // today and does not publish to a lock-free SimStream.
            // Phase A lands on desktop first; TODO(arch-phase-b) wire
            // the wasm path once the inline worker moves off-thread.
            w.current_sessions.insert(entity, session_id);
            let (stripped_source, input_defaults) = strip_input_defaults(&source);

            let mut opts = StepperOptions::default();
            opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;

            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            let compile_outcome = if extra_sources.is_empty() {
                compiler.compile_str(&model_name, &stripped_source, "model.mo")
            } else {
                compiler.compile_str_multi(&model_name, &stripped_source, "model.mo", &extra_sources)
            };
            match compile_outcome {
                Ok(comp_res) => {
                    let exp_t_start = comp_res.experiment_start_time;
                    let exp_t_end = comp_res.experiment_stop_time;
                    let exp_tol = comp_res.experiment_tolerance;
                    let exp_interval = comp_res.experiment_interval;
                    let exp_solver = comp_res.experiment_solver.clone();
                    match SimStepper::new(&comp_res.dae, opts) {
                        Ok(mut stepper) => {
                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            w.cached_models.insert(entity, CachedModel {
                                model_name: model_name.clone(), source: Arc::from(source.clone()),
                            });

                            w.steppers.insert(entity, (session_id, model_name.clone(), stepper));
                            send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Compiled successfully.".to_string()),
                                is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: input_names,
                                experiment_start_time: exp_t_start,
                                experiment_stop_time: exp_t_end,
                                experiment_tolerance: exp_tol,
                                experiment_interval: exp_interval,
                                experiment_solver: exp_solver,
                                compiled_model_name: Some(model_name.clone()),
                                loaded_source_root_id: None,
                            });
                        }
                        Err(e) => {
                            send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {:?}", e)),
                                log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: Vec::new(),
                                ..Default::default()
                            });
                        }
                    }
                }
                Err(e) => {
                    send(ModelicaResult {
                        entity, session_id, new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Compile Error: {:?}", e)),
                        log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                        detected_input_names: Vec::new(),
                        ..Default::default()
                    });
                }
            }
        }
        ModelicaCommand::Reset { entity, session_id } => {
            w.current_sessions.insert(entity, session_id);

            if let Some(cached) = w.cached_models.get(&entity) {
                let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);
                let mut opts = StepperOptions::default();
                opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;
                let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
                match compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                    Ok(comp_res) => {
                        if let Ok(mut stepper) = SimStepper::new(&comp_res.dae, opts) {
                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            w.steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                            send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Reset complete.".to_string()),
                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: input_names,
                                ..Default::default()
                            });

                                } else {
                                send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some("Stepper init failed".to_string()),
                                log_message: None, is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(),
                                ..Default::default()
                                });
                                }
                                }
                                Err(e) => {
                                send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Reset compile error: {:?}", e)),
                                log_message: None, is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(),
                                ..Default::default()
                                });
                                }
                                }
                                } else {
                                w.steppers.remove(&entity);
                                send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: None,
                                log_message: Some("Reset complete (no cached model).".to_string()),
                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(),
                                ..Default::default()
                                });
                                }

        }
        ModelicaCommand::UpdateParameters { entity, session_id, model_name, source } => {
            if session_id < *w.current_sessions.get(&entity).unwrap_or(&0) {
                send(result_ok(entity, session_id));
                return;
            }
            w.current_sessions.insert(entity, session_id);
            let (stripped_source, input_defaults) = strip_input_defaults(&source);

            let mut opts = StepperOptions::default();
            opts.atol = DEFAULT_ATOL; opts.rtol = DEFAULT_RTOL;

            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                Ok(comp_res) => {
                    match SimStepper::new(&comp_res.dae, opts) {
                        Ok(mut stepper) => {
                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            w.cached_models.insert(entity, CachedModel {
                                model_name: model_name.clone(), source: Arc::from(source.clone()),
                            });

                            w.steppers.insert(entity, (session_id, model_name.clone(), stepper));
                            send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Parameters applied.".to_string()),
                                is_new_model: false, is_parameter_update: true, is_reset: false,
                                detected_input_names: input_names,
                                ..Default::default()
                            });
                        }
                        Err(e) => {
                            send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {:?}", e)),
                                log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
                                detected_input_names: Vec::new(),
                                ..Default::default()
                            });
                        }
                    }
                }
                Err(e) => {
                    send(ModelicaResult {
                        entity, session_id, new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Re-compile Error: {:?}", e)),
                        log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
                        detected_input_names: Vec::new(),
                        ..Default::default()
                    });
                }
            }
        }
        ModelicaCommand::Despawn { entity } => {
            w.steppers.remove(&entity);
            w.cached_models.remove(&entity);
        }
        ModelicaCommand::LoadSourceRoot { id, payload } => {
            // Wasm path: matches the native handler. Worker thread
            // (whether off-main Web Worker or inline) merges the
            // library into its session. Idempotent.
            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            let t0 = web_time::Instant::now();
            let report = match payload {
                LoadSourceRootPayload::Disk { root_dir } => {
                    compiler.load_source_root(&id, &root_dir)
                }
                LoadSourceRootPayload::InMemory { label, files } => {
                    compiler.load_source_root_in_memory(&id, &label, files)
                }
            };
            log::info!(
                "[inline-worker] LoadSourceRoot `{}`: {} parsed / {} \
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

/// Component that attaches a Modelica model to an entity.
///
/// Holds the model path, name, session ID, parameters, inputs, and observable variables.
/// The `is_stepping` flag prevents duplicate Step commands while waiting for results.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct ModelicaModel {
    pub model_path: PathBuf,
    pub model_name: String,
    pub current_time: f64,
    pub last_step_time: f64,
    pub session_id: u64,
    pub paused: bool,
    /// Tunable constants (parameter Real ...)
    pub parameters: HashMap<String, f64>,
    /// Control inputs (input Real ...)
    pub inputs: HashMap<String, f64>,
    /// All other observable variables (Real soc, etc)
    pub variables: HashMap<String, f64>,
    /// Canonical id of the Modelica source document backing this entity,
    /// looked up in [`crate::ui::state::ModelicaDocumentRegistry`]. `DocumentId::default()`
    /// (`0`) means "no document assigned yet"; systems should treat it as
    /// a miss. Not reflected — ids are session-local allocations, not
    /// scene-serializable.
    #[reflect(ignore)]
    pub document: lunco_doc::DocumentId,
    /// `true` while a `Step` request is in flight to the worker.
    /// Cleared when the response arrives in
    /// [`handle_modelica_responses`]. Distinct from
    /// [`Self::is_compiling`] — a long-running compile must NOT count
    /// as a hung step (that conflation is what made the dispatcher's
    /// "worker hung?" warning spam every frame for the duration of a
    /// slow Modelica compile).
    #[reflect(ignore)]
    pub is_stepping: bool,
    /// `true` while a `Compile` request is in flight to the worker.
    /// Set by the `CompileModel` observer, cleared when a compile-
    /// shaped result (`is_new_model` / `is_parameter_update`) lands.
    /// Compiles can take seconds (occasionally minutes for MSL-heavy
    /// examples); the dispatcher uses this to suppress its
    /// step-hang warning while a compile is legitimately running.
    #[reflect(ignore)]
    pub is_compiling: bool,
    /// `true` after a successful Compile has installed a stepper for
    /// this entity in the Modelica worker. `spawn_modelica_requests`
    /// uses this to dispatch a Compile (instead of a doomed Step) when
    /// the user clicks Run on a never-compiled model. Reset to `false`
    /// when a result reports an error or a fresh Compile is in flight.
    #[reflect(ignore)]
    pub is_compiled: bool,
    /// Document `generation_owned()` at the last SUCCESSFUL compile.
    /// Compared against the document's current generation to decide
    /// staleness: `stale = !is_compiled || compiled_generation != gen`.
    /// A stale model needs a recompile before live stepping is valid.
    #[reflect(ignore)]
    pub compiled_generation: u64,
    /// Document generation captured at the moment a Compile is
    /// dispatched. Promoted to [`Self::compiled_generation`] when that
    /// compile reports success, so an edit landing mid-compile doesn't
    /// mark the just-built model as already up to date.
    #[reflect(ignore)]
    pub pending_generation: u64,
    /// Transient flag set by `RunActiveModel` when a compile-if-stale is
    /// needed before play: the post-compile success handler unpauses the
    /// model (instead of leaving it paused) and clears this. A plain
    /// Compile leaves it `false`, so compiling never auto-starts a live
    /// sim.
    #[reflect(ignore)]
    pub resume_after_compile: bool,
}

/// Sends `Step` commands for each active model.
///
/// Runs in [`FixedUpdate`] using the fixed timestep delta. All models step with
/// the same dt, matching Avian physics and wire propagation.
pub fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    time: Res<Time<Fixed>>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs_f64();

    for (entity, mut model) in q_models.iter_mut() {
        if model.is_stepping {
            continue;
        }
        if model.paused {
            continue;
        }

        // First-step path: model has been unpaused (user pressed Run)
        // but no Compile has succeeded yet — the worker has no stepper
        // and a Step would just bounce back as "Click Compile first".
        // Auto-trigger CompileModel instead. The observer flips
        // `is_stepping = true` and bumps `session_id`, so we won't
        // re-trigger on subsequent frames; on a successful result the
        // response handler sets `is_compiled = true` and unpauses.
        if !model.is_compiled {
            let doc = model.document;
            if doc != lunco_doc::DocumentId::default() {
                commands.trigger(crate::ui::commands::CompileModel {
                    doc,
                    class: if model.model_name.is_empty() {
                        None
                    } else {
                        Some(model.model_name.clone())
                    },
                    force: false,
                });
            }
            // Don't ship a Step this frame either way — let the
            // compile flow run.
            continue;
        }

        let inputs: Vec<(String, f64)> = model.inputs.iter()
            .map(|(name, val)| (name.clone(), *val))
            .collect();

        model.is_stepping = true;
        let _ = channels.tx.send(ModelicaCommand::Step {
            entity,
            session_id: model.session_id,
            model_path: model.model_path.clone(),
            model_name: model.model_name.clone(),
            inputs,
            dt,
        });
    }
}

/// System that processes results from the background worker.
///
/// Updates `ModelicaModel` components with fresh simulation outputs, handles
/// session fencing to ignore stale results, and manages `WorkbenchState` for
/// UI display. On `is_new_model`, clears old data and unpauses the simulation.
pub fn handle_modelica_responses(
    channels: Res<ModelicaChannels>,
    mut q_models: Query<&mut ModelicaModel>,
    // `workbench_state` was the home of `compilation_error` (B.3
    // phase 4, retired). Param kept in the signature in case other
    // worker paths need it; prefix `_` silences the unused warning.
    mut _workbench_state: ResMut<crate::ui::WorkbenchState>,
    // Headless callers (e.g. cosim tests) run this system without the
    // UI plugin, so the console + compile-state resources may be
    // absent. Make both optional so the core stepping path survives
    // those setups without forcing them to pull in the UI module.
    compile_states: Option<ResMut<crate::ui::CompileStates>>,
    console: Option<ResMut<crate::ui::panels::console::ConsoleLog>>,
    // Optional — a headless cosim harness may skip `LuncoVizPlugin`
    // entirely. When present, every outgoing sample is published into
    // the registry, and the default Modelica plot's bindings are
    // seeded on first compile of each entity.
    mut signals: Option<ResMut<lunco_viz::SignalRegistry>>,
    mut viz_registry: Option<ResMut<lunco_viz::VisualizationRegistry>>,
    // Optional so headless cosim tests (no UI plugin) still link.
    // When present, signal-meta description tooltips read from the
    // doc index, keeping AST as the single source of truth.
    doc_registry: Option<Res<crate::ui::ModelicaDocumentRegistry>>,
    runner_res: Option<Res<crate::ModelicaRunnerResource>>,
    source_roots: Option<ResMut<crate::source_roots::SourceRootRegistry>>,
    status_bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
) {
    let mut compile_states = compile_states;
    let mut console = console;
    let mut source_roots = source_roots;
    let mut status_bus = status_bus;
    while let Ok(result) = channels.rx.try_recv() {
        // Source-root load ack: route to the registry and short-
        // circuit before any of the sim-result handling below
        // (which keys on `result.entity` — LoadSourceRoot uses
        // `Entity::PLACEHOLDER`).
        if let Some(root_id) = result.loaded_source_root_id.as_ref() {
            let is_failure = result.error.is_some();
            if let Some(roots) = source_roots.as_deref_mut() {
                if let Some(entry) = roots.roots.get_mut(root_id) {
                    if let Some(err) = result.error.as_ref() {
                        bevy::log::warn!(
                            "[source-roots] `{}` load failed: {}",
                            root_id, err,
                        );
                        entry.state = crate::source_roots::LoadState::Failed(
                            err.clone(),
                        );
                    } else {
                        bevy::log::info!(
                            "[source-roots] `{}` is now Ready",
                            root_id,
                        );
                        entry.state = crate::source_roots::LoadState::Ready;
                    }
                }
            }
            if let Some(bus) = status_bus.as_deref_mut() {
                bus.clear_progress(crate::source_roots::STATUS_BUS_SOURCE);
                if is_failure {
                    bus.push(
                        crate::source_roots::STATUS_BUS_SOURCE,
                        lunco_workbench::status_bus::StatusLevel::Warn,
                        format!(
                            "Library `{}` load failed: {}",
                            root_id,
                            result.error.as_deref().unwrap_or(""),
                        ),
                    );
                } else {
                    bus.push(
                        crate::source_roots::STATUS_BUS_SOURCE,
                        lunco_workbench::status_bus::StatusLevel::Info,
                        format!("Library `{}` ready", root_id),
                    );
                }
            }
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
                    crate::experiments_runner::ModelDefaults {
                        t_start: result.experiment_start_time,
                        t_end: result.experiment_stop_time,
                        tolerance: result.experiment_tolerance,
                        interval: result.experiment_interval,
                        solver: result.experiment_solver.clone(),
                    },
                );
            }
        }

        if result.entity == Entity::PLACEHOLDER {
            let msg = "Simulation worker crashed and restarted.";
            warn!("{msg}");
            if let Some(c) = console.as_mut() {
                c.error(msg);
            }
            continue;
        }

        if let Ok(mut model) = q_models.get_mut(result.entity) {
            // ALWAYS check session ID before resetting is_stepping
            // Stale results must NOT reset the flag.
            if result.session_id < model.session_id { continue; }

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
                info!("[Modelica] {msg}");
                // Only forward lifecycle notes (compile / reset / param
                // update). Skip the per-Step logs so the console doesn't
                // flood at 60 Hz.
                if result.is_new_model || result.is_reset || result.is_parameter_update {
                    if let Some(c) = console.as_mut() {
                        c.info(format!("[{}] {msg}", model.model_name));
                    }
                }
            }

            // Transition compile state for this entity's document, but
            // only on compile-style results (new-model / parameter-update).
            // Step results arrive continuously and must not clobber
            // Ready/Error classifications.
            let is_compile_result = result.is_new_model || result.is_parameter_update;
            if is_compile_result && !model.document.is_unassigned() {
                let new_state = if result.error.is_some() {
                    crate::ui::CompileState::Error
                } else {
                    crate::ui::CompileState::Ready
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
                            crate::ui::CompileState::Error => {
                                warn!(
                                    "[Modelica] Compile finished with error for `{}` in {}",
                                    model.model_name, human
                                );
                                if let Some(c) = console.as_mut() {
                                    c.error(format!(
                                        "⏹ Compile FAILED: '{}' in {}",
                                        model.model_name, human
                                    ));
                                }
                            }
                            crate::ui::CompileState::Ready => {
                                info!(
                                    "[Modelica] Compile finished for `{}` in {}",
                                    model.model_name, human
                                );
                                if let Some(c) = console.as_mut() {
                                    c.info(format!(
                                        "✓ Compile finished: '{}' in {}",
                                        model.model_name, human
                                    ));
                                }
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
                    cs.set_error(model.document, err.clone());
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
                if let Some(c) = console.as_mut() {
                    c.error(format!("[{}] {prefix}: {err}", model.model_name));
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
            } else if let Some(cs) = compile_states.as_mut() {
                cs.clear_error(model.document);
            }

            if result.is_new_model {
                model.model_path = modelica_dir()
                    .join(format!("{}_{}", result.entity.index(), result.entity.generation()))
                    .join("model.mo");
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
                    model.compiled_generation = model.pending_generation;
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
                for name in &result.detected_input_names {
                    model.inputs.entry(name.clone())
                        .or_insert_with(|| *ui_inputs.get(name).unwrap_or(&0.0));
                }
                for (name, val) in ui_inputs {
                    model.inputs.entry(name).or_insert(val);
                }

                model.current_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_parameter_update {
                model.current_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_reset {
                model.current_time = 0.0;
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

            model.current_time = result.new_time;
            model.last_step_time = result.new_time;
            let time_val = result.new_time;

            // Publish every outgoing scalar sample into the global
            // `SignalRegistry`. The Graphs panel and any future
            // visualization (Avian / USD / scripts) read uniformly
            // from here — there is no longer a Modelica-specific
            // shadow history.
            if let Some(sigs) = signals.as_deref_mut() {
                // Only a fresh compile (new DAE shape, possibly new
                // signal set) clears the registry's history. Reset and
                // parameter-update both restart sim-time at 0 but the
                // signal *shape* is unchanged, and users want to keep
                // seeing the prior run's curves while they iterate —
                // wiping on every param tweak made the Graphs tab look
                // permanently empty after any Telemetry edit.
                if result.is_new_model {
                    for (name, _) in result.detected_symbols.iter().chain(result.outputs.iter()) {
                        sigs.clear_history(&lunco_viz::SignalRef::new(
                            result.entity,
                            name.clone(),
                        ));
                    }
                }
                for (name, val) in result.outputs.iter().chain(result.detected_symbols.iter()) {
                    sigs.push_scalar(
                        lunco_viz::SignalRef::new(result.entity, name.clone()),
                        time_val,
                        *val,
                    );
                }
                // Publish / refresh description metadata on compile-
                // type results so the viz inspector can show tooltips.
                // Descriptions come from the document index (canonical
                // AST projection), looked up by leaf name — same path
                // Telemetry uses.
                if result.is_new_model || result.is_parameter_update {
                    let index_ref = doc_registry
                        .as_deref()
                        .and_then(|r| r.host(model.document))
                        .map(|h| h.document().index());
                    if let Some(index) = index_ref {
                        for (name, _) in result
                            .detected_symbols
                            .iter()
                            .chain(result.outputs.iter())
                        {
                            let Some(entry) = index.find_component_by_leaf(name) else {
                                continue;
                            };
                            if entry.description.is_empty() {
                                continue;
                            }
                            sigs.update_meta(
                                lunco_viz::SignalRef::new(result.entity, name.clone()),
                                lunco_viz::SignalMeta {
                                    description: Some(entry.description.clone()),
                                    unit: None,
                                    provenance: Some("modelica".to_string()),
                                },
                            );
                        }
                    }
                }
            }

            // Auto-seed the default Modelica plot with every observable
            // from a freshly-compiled model. Preserves the pre-viz UX
            // where compiling immediately filled the graph with all
            // the model's observables. Does nothing when the user has
            // already curated the bindings.
            if result.is_new_model {
                if let Some(reg) = viz_registry.as_deref_mut() {
                    // Clear stale bindings from any prior model/entity so
                    // switching models doesn't leave old signals plotted.
                    // We deliberately do *not* auto-bind every detected
                    // observable any more — a freshly compiled model
                    // starts with an *empty* default plot. Users add
                    // signals via the Telemetry panel checkboxes.
                    // Avoids the noisy "12 lines on launch" experience
                    // that prompted users to manually un-tick
                    // everything before they could see what they
                    // cared about.
                    if let Some(cfg) = reg.get_mut(crate::ui::viz::DEFAULT_MODELICA_GRAPH) {
                        cfg.inputs.clear();
                    }
                    let _ = result.entity;
                    let _ = result.detected_symbols.len();
                    let _ = model.parameters.len();
                }
            }
        }
    }
}