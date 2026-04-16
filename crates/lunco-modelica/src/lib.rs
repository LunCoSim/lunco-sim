//! High-performance Modelica integration for Bevy.
//!
//! This crate provides a bridge between Bevy's ECS and Modelica simulation models.
//! It features:
//! - A background worker thread that owns non-Send `SimStepper` instances
//! - Command/response architecture with session ID fencing to prevent stale data
//! - Command squashing to handle rapid parameter changes without back-pressure
//! - DAE caching per entity for instant Reset and fast stepper rebuilds
//! - Real-time telemetry and plotting via egui
//!
//! ## Architecture
//!
//! The `ModelicaPlugin` spawns a background worker thread that owns all simulation
//! steppers and cached DAEs. The main Bevy thread sends `ModelicaCommand`s via a
//! crossbeam channel and receives `ModelicaResult`s back. Each entity with a
//! `ModelicaModel` component gets its own stepper instance, identified by a
//! `session_id` that increments on each recompile/reset to fence stale results.
//!
//! ## DAE Caching
//!
//! After a successful compilation, the `CompilationResult` (including the DAE) is
//! cached per entity. This enables:
//! - **Instant Reset**: Rebuilds the SimStepper from the cached DAE without recompilation
//! - **Fast Step auto-init**: If the stepper was lost, rebuilds from cached DAE instead of
//!   recompiling from the file on disk
//! - **Parameter updates**: After UpdateParameters, the modified source is written to the
//!   temp file and the new DAE replaces the old cache entry
//!
//! ## Worker Panic Recovery
//!
//! The worker wraps all simulation logic in `catch_unwind`. If a numerical instability
//! (e.g., mass=0.0 in SpringMass) causes a solver panic, the error is caught and
//! reported as "Solver Error" in the logs rather than crashing the application.

use bevy::prelude::*;
use rumoca_session::{Session, SessionConfig};
use rumoca_sim::{StepperOptions, SimStepper};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use crossbeam_channel::{unbounded, Sender, Receiver};
use std::thread;
use lunco_assets::{msl_dir, modelica_dir};

use self::ast_extract::strip_input_defaults;

/// AST-based extraction functions for Modelica source code.
///
/// Walks the full Modelica AST (via `rumoca_phase_parse`) to extract model names,
/// parameters, inputs, and other symbols. Replaces the legacy regex-based extraction.
pub mod ast_extract;

/// Modelica-to-diagram graph builder — converts AST into DiagramGraph.
pub mod diagram;


/// Simple wrapper around rumoca-session for compiling Modelica models.
///
/// Replaces the `rumoca::Compiler` API with a session-based approach.
pub struct ModelicaCompiler {
    session: Session,
}

impl ModelicaCompiler {
    /// Create a new ModelicaCompiler instance.
    pub fn new() -> Self {
        Self {
            session: Session::new(SessionConfig::default()),
        }
    }

    /// Compile Modelica source string and return DAE result.
    ///
    /// # Arguments
    /// * `model_name` - Name of the model to compile
    /// * `source` - Modelica source code
    /// * `filename` - Virtual filename for error reporting
    pub fn compile_str(&mut self, model_name: &str, source: &str, filename: &str) -> Result<rumoca_session::compile::CompilationResult, String> {
        self.session.update_document(filename, source);
        self.session.compile_model(model_name)
            .map_err(|e| format!("{:?}", e))
    }
}

pub mod ui;

/// Bundled Modelica models for web deployment.
/// Available on all targets, but primarily used for wasm builds.
pub mod models;

/// Bevy plugin for Modelica integration.
///
/// Sets up the background worker thread, channel resources, and response systems.
pub struct ModelicaPlugin;

/// Headless variant of [`ModelicaPlugin`] that skips the UI panels.
///
/// Starts the Modelica worker thread, inserts [`ModelicaChannels`], registers
/// [`ModelicaModel`], and schedules `spawn_modelica_requests` +
/// `handle_modelica_responses`. Does NOT add `ModelicaUiPlugin`, so it has
/// no dependency on `bevy_workbench` / `bevy_egui` / the renderer. Use this
/// in headless tests and any non-windowed binary.
///
/// A default [`ui::WorkbenchState`] resource is inserted (so
/// `handle_modelica_responses` can still borrow it), but the UI systems
/// never run.
pub struct ModelicaCorePlugin;

impl Plugin for ModelicaCorePlugin {
    fn build(&self, app: &mut App) {
        build_modelica_core(app);
    }
}

impl Plugin for ModelicaPlugin {
    fn build(&self, app: &mut App) {
        build_modelica_core(app);
        app.add_plugins(ui::ModelicaUiPlugin);
    }
}

fn build_modelica_core(app: &mut App) {
    let (tx_cmd, rx_cmd) = unbounded();
    let (tx_res, rx_res) = unbounded();

    // Set MSL path globally before starting worker
    let msl = msl_dir();
    if msl.exists() {
        if let Ok(abs_path) = std::fs::canonicalize(&msl) {
            std::env::set_var("MODELICAPATH", abs_path.to_string_lossy().to_string());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Desktop: spawn a background thread that owns !Send SimStepper instances.
        // The main Bevy thread communicates via crossbeam channels.
        thread::spawn(move || {
            modelica_worker(rx_cmd, tx_res);
        });
    }

    #[cfg(target_arch = "wasm32")]
    {
        // Web: wasm32-unknown-unknown has no std::thread support. Instead we use
        // an InlineWorker Resource that processes one command per frame in a Bevy
        // system. This avoids blocking the main render thread while still running
        // the full Modelica compilation + simulation pipeline.
        app.insert_resource(InlineWorker::default());
    }

    #[cfg(not(target_arch = "wasm32"))]
    app.insert_resource(ModelicaChannels { tx: tx_cmd, rx: rx_res });
    #[cfg(target_arch = "wasm32")]
    {
        // Web: needs separate rx_cmd (worker reads commands) and tx_res (worker
        // sends results). On desktop the background thread uses rx_cmd/tx_res
        // directly via move closure. On wasm32 the InlineWorker system needs
        // Res<ModelicaChannels> access to both sides.
        app.insert_resource(ModelicaChannels { tx: tx_cmd, rx: rx_res, rx_cmd, tx_res });
    }

    // handle_modelica_responses borrows WorkbenchState; insert a default so
    // the system runs in both UI and headless configurations.
    app.init_resource::<ui::WorkbenchState>();

    app.register_type::<ModelicaModel>()
        .add_systems(Update, (
            spawn_modelica_requests,
            handle_modelica_responses,
        ));

    #[cfg(target_arch = "wasm32")]
    {
        // Web: InlineWorker system processes commands synchronously each frame.
        // One command per frame to avoid stuttering the UI.
        app.add_systems(Update, inline_worker_process);
        // Web: consume file picker results each frame.
        app.add_systems(Update, ui::update_file_load_result);
    }
}

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
    }
}

/// Results received from the background simulation worker.
///
/// Contains simulation outputs, detected symbols, and error information.
/// The `session_id` field is used by `handle_modelica_responses` to fence stale results.
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
}

/// Cached compilation result per entity.
///
/// Stores the DAE and source hash so we can instantly rebuild a SimStepper
/// after Reset without recompiling, and detect when the Step command's
/// model_path points to stale source.
struct CachedModel {
    #[allow(dead_code)]
    session_id: u64,
    model_name: String,
    source: String,
    #[allow(dead_code)]
    dae: rumoca_session::compile::CompilationResult,
}

/// Helper to build a ModelicaResult with defaults.
fn result_ok(entity: Entity, session_id: u64) -> ModelicaResult {
    ModelicaResult {
        entity, session_id,
        new_time: 0.0, outputs: Vec::new(), detected_symbols: Vec::new(),
        error: None, log_message: None, is_new_model: false,
        is_parameter_update: false, is_reset: false,
        detected_input_names: Vec::new(),
    }
}

/// Collect all variable (name, value) pairs by unioning the stepper's solver
/// state variable names with every declared non-parameter/non-input/non-constant
/// variable extracted from the source AST.
///
/// Rumoca's `SimStepper::variable_names()` only lists solver-state entries
/// (typically just states after DAE reduction eliminates algebraics). Algebraic
/// variables like `netForce`, `buoyancy`, etc. can still be queried by name via
/// `SimStepper::get()`, so we augment the iteration with names parsed from the
/// source to recover those values.
fn collect_all_variables(stepper: &SimStepper, source: &str) -> Vec<(String, f64)> {
    let mut symbols: Vec<(String, f64)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for name in stepper.variable_names() {
        if seen.insert(name.clone()) {
            if let Some(val) = stepper.get(name) {
                if val.is_finite() {
                    symbols.push((name.clone(), val));
                }
            }
        }
    }
    for name in ast_extract::extract_variable_names(source) {
        if seen.insert(name.clone()) {
            if let Some(val) = stepper.get(&name) {
                if val.is_finite() {
                    symbols.push((name, val));
                }
            }
        }
    }
    symbols
}

/// The background worker that owns the !Send SimSteppers and cached DAEs.
fn modelica_worker(rx: Receiver<ModelicaCommand>, tx: Sender<ModelicaResult>) {
    let mut steppers: HashMap<Entity, (u64, String, SimStepper)> = HashMap::default();
    let mut current_sessions: HashMap<Entity, u64> = HashMap::default();
    // DAE cache per entity — enables instant Reset and fast Step auto-init
    let mut cached_models: HashMap<Entity, CachedModel> = HashMap::default();

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
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match cmd {
                    ModelicaCommand::Reset { entity, session_id } => {
                        current_sessions.insert(entity, session_id);

                        if let Some(cached) = cached_models.get(&entity) {
                            // Strip input defaults from cached source and set them via set_input
                            let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);

                            let mut opts = StepperOptions::default();
                            opts.atol = 1e-3; opts.rtol = 1e-3;
                            // Recompile stripped source to get a fresh stepper with input slots
                            let mut compiler = ModelicaCompiler::new();
                            match compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                                Ok(comp_res) => {
                                    match SimStepper::new(&comp_res.dae, opts) {
                                        Ok(mut stepper) => {
                                            for (name, val) in &input_defaults {
                                                let _ = stepper.set_input(name, *val);
                                            }
                                            let input_names: Vec<String> = stepper.input_names().to_vec();
                                            let symbols = collect_all_variables(&stepper, &cached.source);
                                            steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                                            let _ = tx_inner.send(ModelicaResult {
                                                entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                                detected_symbols: symbols, error: None,
                                                log_message: Some("Reset complete.".to_string()),
                                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                                detected_input_names: input_names,
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

                        let mut compiler = ModelicaCompiler::new();
                        match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                            Ok(comp_res) => {
                                let mut opts = StepperOptions::default();
                                opts.atol = 1e-3; opts.rtol = 1e-3;
                                match SimStepper::new(&comp_res.dae, opts) {
                                    Ok(mut stepper) => {
                                        for (name, val) in &input_defaults {
                                            let _ = stepper.set_input(name, *val);
                                        }
                                        let input_names: Vec<String> = stepper.input_names().to_vec();
                                        let symbols = collect_all_variables(&stepper, &source);
                                        cached_models.insert(entity, CachedModel {
                                            session_id,
                                            model_name: model_name.clone(),
                                            source: source.clone(),
                                            dae: comp_res,
                                        });
                                        steppers.insert(entity, (session_id, model_name.clone(), stepper));
                                        let _ = tx_inner.send(ModelicaResult {
                                            entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                            detected_symbols: symbols, error: None,
                                            log_message: Some("Parameters applied.".to_string()),
                                            is_new_model: false, is_parameter_update: true, is_reset: false,
                                            detected_input_names: input_names,
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
                    ModelicaCommand::Compile { entity, session_id, model_name, source } => {
                        current_sessions.insert(entity, session_id);

                        // Strip input defaults so they become real runtime slots
                        let (stripped_source, input_defaults) = strip_input_defaults(&source);

                        let mut compiler = ModelicaCompiler::new();
                        match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                            Ok(comp_res) => {
                                let mut opts = StepperOptions::default();
                                opts.atol = 1e-3; opts.rtol = 1e-3;
                                match SimStepper::new(&comp_res.dae, opts) {
                                    Ok(mut stepper) => {
                                        // Set input defaults via set_input so they're runtime-changeable
                                        for (name, val) in &input_defaults {
                                            let _ = stepper.set_input(name, *val);
                                        }
                                        let input_names: Vec<String> = stepper.input_names().to_vec();
                                        let symbols = collect_all_variables(&stepper, &source);
                                        let temp_dir = modelica_dir().join(format!("{}_{}", entity.index(), entity.generation()));
                                        let _ = std::fs::create_dir_all(&temp_dir);
                                        let temp_path = temp_dir.join("model.mo");
                                        let _ = std::fs::write(&temp_path, &source);

                                        cached_models.insert(entity, CachedModel {
                                            session_id,
                                            model_name: model_name.clone(),
                                            source: source.clone(),
                                            dae: comp_res,
                                        });
                                        steppers.insert(entity, (session_id, model_name.clone(), stepper));
                                        let _ = tx_inner.send(ModelicaResult {
                                            entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                            detected_symbols: symbols, error: None,
                                            log_message: Some(format!("Model '{}' compiled.", model_name)),
                                            is_new_model: true, is_parameter_update: false, is_reset: false,
                                            detected_input_names: input_names,
                                        });
                                    }
                                    Err(e) => {
                                        let mut r = result_ok(entity, session_id);
                                        r.error = Some(format!("Stepper Error: {:?}", e));
                                        let _ = tx_inner.send(r);
                                    }
                                }
                            }
                            Err(e) => {
                                let mut r = result_ok(entity, session_id);
                                r.error = Some(format!("Compiler Error: {:?}", e));
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
                                    // Use cached source directly — no file re-read needed.
                                    // File comparison was broken because model_path points to
                                    // the temp file (set after Compile), not the original source.
                                    let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);
                                    let mut compiler = ModelicaCompiler::new();
                                    if let Ok(comp_res) = compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                                        let mut opts = StepperOptions::default();
                                        opts.atol = 1e-3; opts.rtol = 1e-3;
                                        if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                                            // Set input defaults first
                                            for (name, val) in &input_defaults {
                                                let _ = s.set_input(name, *val);
                                            }
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
                                let mut compiler = ModelicaCompiler::new();
                                match compiler.compile_str(&model_name, &source, &model_path.to_string_lossy()) {
                                    Ok(comp_res) => {
                                        let mut opts = StepperOptions::default();
                                        opts.atol = 1e-3; opts.rtol = 1e-3;
                                        if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                                            for (name, val) in &inputs { let _ = s.set_input(name, *val); }
                                            cached_models.insert(entity, CachedModel {
                                                session_id,
                                                model_name: model_name.clone(),
                                                source: source.clone(),
                                                dae: comp_res,
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
                                    // Include states + algebraics (from source AST) + inputs.
                                    // `stepper.variable_names()` alone typically only gives states
                                    // after DAE reduction, so augment with source-extracted names.
                                    let source_opt: Option<String> = cached_models
                                        .get(&entity)
                                        .map(|c| c.source.clone());
                                    let mut outputs = if let Some(src) = source_opt.as_deref() {
                                        collect_all_variables(stepper, src)
                                    } else {
                                        let mut v = Vec::new();
                                        for name in stepper.variable_names() {
                                            if let Some(val) = stepper.get(name) {
                                                if val.is_finite() { v.push((name.clone(), val)); }
                                            }
                                        }
                                        v
                                    };
                                    // Also include input values so UI can plot them
                                    for name in stepper.input_names() {
                                        if let Some(val) = stepper.get(name) { if val.is_finite() { outputs.push((name.clone(), val)); } }
                                    }
                                    let _ = tx_inner.send(ModelicaResult {
                                        entity, session_id, new_time: stepper.time(), outputs, error: None, log_message: None,
                                        is_new_model: false, detected_symbols: Vec::new(),
                                        is_parameter_update: false, is_reset: false,
                                        detected_input_names: Vec::new(),
                                    });
                                }
                            } else {
                                let _ = tx_inner.send(result_ok(entity, session_id));
                            }
                        } else {
                            let mut r = result_ok(entity, session_id);
                            r.error = Some("Sim engine failed to start.".to_string());
                            let _ = tx_inner.send(r);
                        }
                    }
                    ModelicaCommand::Despawn { entity } => {
                        steppers.remove(&entity);
                        cached_models.remove(&entity);
                    }
                }
            }));

            if let Err(_) = result {
                let _ = tx.send(ModelicaResult {
                    entity: Entity::PLACEHOLDER,
                    session_id: 0, new_time: 0.0, outputs: Vec::new(), detected_symbols: Vec::new(),
                    error: Some("Internal Worker Panic!".to_string()), log_message: None,
                    is_new_model: false, is_parameter_update: false, is_reset: false,
                    detected_input_names: Vec::new(),
                });
            }
        }
    }
}

fn cmd_entity(cmd: &ModelicaCommand) -> Entity {
    match cmd {
        ModelicaCommand::Step { entity, .. } => *entity,
        ModelicaCommand::Compile { entity, .. } => *entity,
        ModelicaCommand::UpdateParameters { entity, .. } => *entity,
        ModelicaCommand::Reset { entity, .. } => *entity,
        ModelicaCommand::Despawn { entity } => *entity,
    }
}

fn cmd_session(cmd: &ModelicaCommand) -> u64 {
    match cmd {
        ModelicaCommand::Step { session_id, .. } => *session_id,
        ModelicaCommand::Compile { session_id, .. } => *session_id,
        ModelicaCommand::UpdateParameters { session_id, .. } => *session_id,
        ModelicaCommand::Reset { session_id, .. } => *session_id,
        ModelicaCommand::Despawn { .. } => 0,
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
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct InlineWorkerInner {
    steppers: HashMap<Entity, (u64, String, SimStepper)>,
    current_sessions: HashMap<Entity, u64>,
    cached_models: HashMap<Entity, CachedModel>,
}

/// Thread-safe wrapper for wasm32 inline worker state.
///
/// SAFETY: wasm32-unknown-unknown has no threads, so Send/Sync are vacuously true.
/// SimStepper internally uses Rc<RefCell<>> which is !Send, but since no threads
/// exist on this target, we can safely implement Send/Sync.
#[cfg(target_arch = "wasm32")]
#[derive(Resource, Default)]
struct InlineWorker {
    inner: InlineWorkerInner,
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
fn inline_worker_process(
    mut worker: ResMut<InlineWorker>,
    channels: Res<ModelicaChannels>,
) {
    let w = &mut worker.inner;
    // Process one command per frame to avoid blocking the main thread
    let Ok(cmd) = channels.rx_cmd.try_recv() else { return };

    match cmd {
        ModelicaCommand::Step { entity, session_id, model_name, inputs, dt, model_path: _ } => {
            let tx = &channels.tx_res;

            // Auto-init: compile if stepper doesn't exist
            if !w.steppers.contains_key(&entity) {
                // Try cached DAE first
                if let Some(cached) = w.cached_models.get(&entity) {
                    if cached.model_name == model_name {
                        let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);
                        let mut compiler = ModelicaCompiler::new();
                        if let Ok(comp_res) = compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                            let mut opts = StepperOptions::default();
                            opts.atol = 1e-3; opts.rtol = 1e-3;
                            if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                                for (name, val) in &input_defaults { let _ = s.set_input(name, *val); }
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
                        let _ = tx.send(ModelicaResult {
                            entity, session_id, new_time: stepper.time(), outputs: Vec::new(),
                            detected_symbols: Vec::new(), error: Some(format!("Solver Error: {:?}", e)),
                            log_message: None, is_new_model: false, is_parameter_update: false,
                            is_reset: false, detected_input_names: Vec::new(),
                        });
                        w.steppers.remove(&entity);
                    } else {
                        let mut outputs = Vec::new();
                        for name in stepper.variable_names() {
                            if let Some(val) = stepper.get(name) { if val.is_finite() { outputs.push((name.clone(), val)); } }
                        }
                        for name in stepper.input_names() {
                            if let Some(val) = stepper.get(name) { if val.is_finite() { outputs.push((name.clone(), val)); } }
                        }
                        let _ = tx.send(ModelicaResult {
                            entity, session_id, new_time: stepper.time(), outputs, error: None,
                            log_message: None, is_new_model: false, detected_symbols: Vec::new(),
                            is_parameter_update: false, is_reset: false, detected_input_names: Vec::new(),
                        });
                    }
                } else {
                    let _ = tx.send(result_ok(entity, session_id));
                }
            } else {
                let _ = tx.send(ModelicaResult {
                    entity, session_id, new_time: 0.0, outputs: Vec::new(),
                    detected_symbols: Vec::new(), error: Some("Sim engine failed to start.".to_string()),
                    log_message: None, is_new_model: false, is_parameter_update: false,
                    is_reset: false, detected_input_names: Vec::new(),
                });
            }
        }
        ModelicaCommand::Compile { entity, session_id, model_name, source } => {
            w.current_sessions.insert(entity, session_id);
            let (stripped_source, input_defaults) = strip_input_defaults(&source);

            let mut opts = StepperOptions::default();
            opts.atol = 1e-3; opts.rtol = 1e-3;
            let tx = &channels.tx_res;

            let mut compiler = ModelicaCompiler::new();
            match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                Ok(comp_res) => {
                    match SimStepper::new(&comp_res.dae, opts) {
                        Ok(mut stepper) => {
                            for (name, val) in &input_defaults { let _ = stepper.set_input(name, *val); }
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let mut symbols = Vec::new();
                            for name in stepper.variable_names() {
                                if let Some(val) = stepper.get(name) { symbols.push((name.clone(), val)); }
                            }
                            w.cached_models.insert(entity, CachedModel {
                                session_id, model_name: model_name.clone(), source: source.clone(),
                                dae: comp_res.clone(),
                            });
                            w.steppers.insert(entity, (session_id, model_name.clone(), stepper));
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Compiled successfully.".to_string()),
                                is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: input_names,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {:?}", e)),
                                log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: Vec::new(),
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(ModelicaResult {
                        entity, session_id, new_time: 0.0, outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Compile Error: {:?}", e)),
                        log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                        detected_input_names: Vec::new(),
                    });
                }
            }
        }
        ModelicaCommand::Reset { entity, session_id } => {
            w.current_sessions.insert(entity, session_id);
            let tx = &channels.tx_res;

            if let Some(cached) = w.cached_models.get(&entity) {
                let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);
                let mut opts = StepperOptions::default();
                opts.atol = 1e-3; opts.rtol = 1e-3;
                let mut compiler = ModelicaCompiler::new();
                match compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                    Ok(comp_res) => {
                        if let Ok(mut stepper) = SimStepper::new(&comp_res.dae, opts) {
                            for (name, val) in &input_defaults { let _ = stepper.set_input(name, *val); }
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let mut symbols = Vec::new();
                            for name in stepper.variable_names() {
                                if let Some(val) = stepper.get(name) { symbols.push((name.clone(), val)); }
                            }
                            w.steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Reset complete.".to_string()),
                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: input_names,
                            });
                        } else {
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some("Stepper init failed".to_string()),
                                log_message: None, is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(),
                            });
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(ModelicaResult {
                            entity, session_id, new_time: 0.0, outputs: Vec::new(),
                            detected_symbols: Vec::new(), error: Some(format!("Reset compile error: {:?}", e)),
                            log_message: None, is_new_model: false, is_parameter_update: false, is_reset: true,
                            detected_input_names: Vec::new(),
                        });
                    }
                }
            } else {
                w.steppers.remove(&entity);
                let _ = tx.send(ModelicaResult {
                    entity, session_id, new_time: 0.0, outputs: Vec::new(),
                    detected_symbols: Vec::new(), error: None,
                    log_message: Some("Reset complete (no cached model).".to_string()),
                    is_new_model: false, is_parameter_update: false, is_reset: true,
                    detected_input_names: Vec::new(),
                });
            }
        }
        ModelicaCommand::UpdateParameters { entity, session_id, model_name, source } => {
            if session_id < *w.current_sessions.get(&entity).unwrap_or(&0) {
                let _ = channels.tx_res.send(result_ok(entity, session_id));
                return;
            }
            w.current_sessions.insert(entity, session_id);
            let (stripped_source, input_defaults) = strip_input_defaults(&source);

            let mut opts = StepperOptions::default();
            opts.atol = 1e-3; opts.rtol = 1e-3;
            let tx = &channels.tx_res;

            let mut compiler = ModelicaCompiler::new();
            match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                Ok(comp_res) => {
                    match SimStepper::new(&comp_res.dae, opts) {
                        Ok(mut stepper) => {
                            for (name, val) in &input_defaults { let _ = stepper.set_input(name, *val); }
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let mut symbols = Vec::new();
                            for name in stepper.variable_names() {
                                if let Some(val) = stepper.get(name) { symbols.push((name.clone(), val)); }
                            }
                            w.cached_models.insert(entity, CachedModel {
                                session_id, model_name: model_name.clone(), source: source.clone(),
                                dae: comp_res,
                            });
                            w.steppers.insert(entity, (session_id, model_name.clone(), stepper));
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Parameters applied.".to_string()),
                                is_new_model: false, is_parameter_update: true, is_reset: false,
                                detected_input_names: input_names,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0, outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {:?}", e)),
                                log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
                                detected_input_names: Vec::new(),
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(ModelicaResult {
                        entity, session_id, new_time: 0.0, outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Re-compile Error: {:?}", e)),
                        log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
                        detected_input_names: Vec::new(),
                    });
                }
            }
        }
        ModelicaCommand::Despawn { entity } => {
            w.steppers.remove(&entity);
            w.cached_models.remove(&entity);
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
    #[reflect(ignore)]
    pub is_stepping: bool,
}

/// System that sends `Step` commands for each active model.
///
/// Runs every frame in the `Update` schedule. Calculates the elapsed real time
/// since the last step and caps it to prevent solver instability. Sets `is_stepping`
/// to true to prevent duplicate step commands while waiting for results.
///
/// Inputs from the UI (e.g., `current_in` in Battery) are sent with every Step command
/// via `model.inputs`, enabling real-time input injection without recompilation.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    time: Res<Time>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
) {
    let current_real_time = time.elapsed_secs_f64();

    for (entity, mut model) in q_models.iter_mut() {
        if model.is_stepping || model.paused { continue; }

        let mut inputs = Vec::new();
        for (name, val) in &model.inputs {
            inputs.push((name.clone(), *val));
        }

        let mut dt = if model.last_step_time == 0.0 || model.paused { 0.016 }
                     else { (current_real_time - model.last_step_time).max(0.001) };
        if dt > 0.1 { dt = 0.1; }
        if !model.paused { model.last_step_time = current_real_time; }

        model.is_stepping = true;
        debug!("Sending Step command for {:?} (dt={:.3})", entity, dt);
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

/// Web version of spawn_modelica_requests.
///
/// Why fixed timestep: std::time is not available on wasm32-unknown-unknown
/// (performance.now() is restricted by browsers). We use a fixed 16ms timestep
/// (~60 FPS) which is sufficient for interactive simulation.
///
/// The desktop version uses time.elapsed_secs_f64() for adaptive dt.
#[cfg(target_arch = "wasm32")]
fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
) {
    const FIXED_DT: f64 = 0.016; // ~60 FPS

    for (entity, mut model) in q_models.iter_mut() {
        if model.is_stepping || model.paused { continue; }

        let mut inputs = Vec::new();
        for (name, val) in &model.inputs {
            inputs.push((name.clone(), *val));
        }

        model.is_stepping = true;
        debug!("Sending Step command for {:?} (dt={:.3})", entity, dt);
        let _ = channels.tx.send(ModelicaCommand::Step {
            entity,
            session_id: model.session_id,
            model_path: model.model_path.clone(),
            model_name: model.model_name.clone(),
            inputs,
            dt: FIXED_DT,
        });
    }
}

/// System that processes results from the background worker.
///
/// Updates `ModelicaModel` components with fresh simulation outputs, handles
/// session fencing to ignore stale results, and manages `WorkbenchState` for
/// UI display. On `is_new_model`, clears old data and unpauses the simulation.
fn handle_modelica_responses(
    channels: Res<ModelicaChannels>,
    #[cfg(not(target_arch = "wasm32"))] time: Res<Time>,
    mut q_models: Query<&mut ModelicaModel>,
    mut workbench_state: ResMut<ui::WorkbenchState>,
) {
    while let Ok(result) = channels.rx.try_recv() {
        if result.entity == Entity::PLACEHOLDER {
            warn!("Simulation Worker crashed and restarted.");
            continue;
        }

        if let Ok(mut model) = q_models.get_mut(result.entity) {
            // ALWAYS check session ID before resetting is_stepping
            // Stale results must NOT reset the flag.
            if result.session_id < model.session_id { continue; }

            model.is_stepping = false;
            debug!("Received Step result for {:?} (new_time={:.3}, outputs={})", result.entity, result.new_time, result.outputs.len());

            // Forward log messages to console via bevy_workbench's console system
            if let Some(msg) = result.log_message {
                info!("[Modelica] {msg}");
            }

            if let Some(err) = &result.error {
                workbench_state.compilation_error = Some(err.clone());
                warn!("[Modelica] {err}");
                model.paused = true;
            } else if workbench_state.selected_entity == Some(result.entity) {
                workbench_state.compilation_error = None;
            }

            if result.is_new_model {
                workbench_state.history.remove(&result.entity);
                model.model_path = modelica_dir()
                    .join(format!("{}_{}", result.entity.index(), result.entity.generation()))
                    .join("model.mo");
                model.variables.clear();
                model.paused = false;

                // Merge input names from the worker with values the UI already extracted from source.
                // The UI extracts defaults from source code (e.g., `input Real g = 9.81` → g: 9.81),
                // which is more reliable than the worker's DAE-discovered names (which may have 0.0).
                let ui_inputs: HashMap<String, f64> = std::mem::take(&mut model.inputs);
                for name in &result.detected_input_names {
                    // Only insert if the UI didn't already provide a value
                    model.inputs.entry(name.clone())
                        .or_insert_with(|| *ui_inputs.get(name).unwrap_or(&0.0));
                }
                // Also add any UI-discovered inputs the worker missed (e.g., inputs with default values)
                for (name, val) in ui_inputs {
                    model.inputs.entry(name).or_insert(val);
                }

                if workbench_state.selected_entity == Some(result.entity) {
                    workbench_state.plotted_variables.clear();
                    for (name, _) in &result.detected_symbols {
                        if !name.ends_with("_in") && !model.parameters.contains_key(name) {
                            workbench_state.plotted_variables.insert(name.clone());
                        }
                    }
                }
                model.current_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_parameter_update {
                workbench_state.history.remove(&result.entity);
                model.current_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_reset {
                workbench_state.history.remove(&result.entity);
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
            #[cfg(not(target_arch = "wasm32"))]
            { model.last_step_time = time.elapsed_secs_f64(); }
            #[cfg(target_arch = "wasm32")]
            { model.last_step_time = result.new_time; }

            // Record history for plotted variables
            let time_val = result.new_time;
            let max_history = workbench_state.max_history;
            let plotted: Vec<String> = workbench_state.plotted_variables.iter().cloned().collect();
            let entity_history = workbench_state.history.entry(result.entity).or_insert_with(HashMap::new);

            for (name, val) in &result.outputs {
                if plotted.contains(name) {
                    let history = entity_history.entry(name.clone()).or_insert_with(|| VecDeque::with_capacity(max_history));
                    history.push_back([time_val, *val]);
                    if history.len() > max_history { history.pop_front(); }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Re-export AST extraction for public API compatibility
// ---------------------------------------------------------------------------
// These functions live in `ast_extract` but are re-exported here so external
// callers (workbench binaries, UI panels) can import from the crate root.
pub use ast_extract::{
    extract_model_name,
    extract_parameters,
    extract_inputs_with_defaults,
    extract_input_names,
    extract_variable_names,
    substitute_params_in_source,
    hash_content,
    extract_from_source,
    ModelicaSymbols,
};
// `strip_input_defaults` is already imported via `use self::ast_extract::strip_input_defaults`
// above and is available publicly through the `pub mod ast_extract` declaration.

// ---------------------------------------------------------------------------
// Re-export diagram types for public API
// ---------------------------------------------------------------------------
pub use diagram::{
    DiagramType,
    ModelicaComponentBuilder,
    list_class_names,
};

#[derive(Component, Reflect, Default)]
pub struct ModelicaInput { pub variable_name: String, pub value: f64 }

#[derive(Component, Reflect, Default)]
pub struct ModelicaOutput { pub variable_name: String, pub value: f64 }
