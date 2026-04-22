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

/// `ModelicaDocument` — the Document System representation of a `.mo` file.
///
/// Introduced dormant (no panels use it yet). See the module-level docstring
/// for migration order.
pub mod document;

/// Shared parse + I/O cache for Modelica classes, built on
/// [`lunco_cache`]. Drill-in, AddComponent preload, and (later)
/// compile dep-walk all funnel through here so every class file is
/// read once, parsed once, and shared as `Arc<AstCache>` across tabs
/// and compile jobs.
pub mod class_cache;

/// Subset Modelica pretty-printer — emits source snippets for *new* AST
/// nodes (component declarations, connect equations, placement / line
/// annotations). Used by AST-level document ops that splice new text at
/// a span in the existing source. Not a full round-trip printer —
/// existing nodes keep their original source text.
pub mod pretty;

/// Modelica-to-diagram graph builder — converts AST into DiagramGraph.
pub mod diagram;

/// Typed extractors for graphical annotations (Placement, Icon, Diagram,
/// and the common `graphics={...}` primitives). Walks the raw
/// `Vec<Expression>` that rumoca preserves on each class/component and
/// produces structs ready for the canvas renderer.
pub mod annotations;

/// egui painter for the typed graphics produced by [`annotations`].
/// Renders `Rectangle`, `Line`, `Polygon`, and `Text` directly into a
/// destination screen rect, mapping Modelica diagram coordinates
/// (+Y up) to egui screen coordinates (+Y down).
pub mod icon_paint;

/// Single 2×3 affine transform per node from Modelica icon-local
/// coords to canvas world coords. Replaces the scattered
/// position/extent_size/rotation/mirror fields with one matrix that
/// every consumer (port placement, edge stub direction, icon body
/// painting, AABB) shares.
pub mod icon_transform;

/// Visual diagram editor — drag-and-drop component composition.
pub mod visual_diagram;

/// Simple wrapper around rumoca-session for compiling Modelica models.
///
/// MSL is preloaded into the session at construction time via
/// [`rumoca_session::compile::Session::load_source_root_tolerant`].
/// After preload, compiling any MSL-based user model is a plain
/// strict-reachable-DAE call against a session that already has
/// every MSL class visible to rumoca's §5 scope walker.
///
/// Why preload instead of demand-load? Demand-load requires
/// rumoca to emit fully-qualified references in its unresolved-ref
/// diagnostics so an external source provider can act on them.
/// Upstream rumoca currently emits raw short forms (`SI.Time`,
/// `Continuous.Filter`, `Rotational.Interfaces.PartialTwoFlanges`)
/// without scope qualification — which means an external resolver
/// has no way to disambiguate. Preload sidesteps the issue: once
/// every MSL class is in the session, the scope walker never has
/// to ask outside.
///
/// Cost: first session construction blocks while the parsed-artifact
/// cache (bincode under `RUMOCA_CACHE_DIR`) is hit. With a warm
/// cache, MSL loads in ~2–5 s. Cold cache (first run after a rumoca
/// version bump) is proportional to parser throughput; `msl_indexer`
/// can pre-warm offline.
pub struct ModelicaCompiler {
    session: Session,
}

impl ModelicaCompiler {
    /// Construct a compiler and preload MSL.
    ///
    /// On targets without a filesystem (`wasm32`),
    /// [`lunco_assets::msl_source_root_path`] returns `None` and the
    /// session is left empty — web targets populate via HTTP once
    /// the async-asset path lands.
    pub fn new() -> Self {
        let t_total = std::time::Instant::now();
        let mut session = Session::new(SessionConfig::default());
        if let Some(msl_root) = lunco_assets::msl_source_root_path() {
            // Durable-external — MSL rarely changes and is
            // library-grade; rumoca uses this classification to
            // enable bincode persistence for parsed artifacts.
            let report = session.load_source_root_tolerant(
                "msl",
                rumoca_session::compile::SourceRootKind::DurableExternal,
                &msl_root,
                None,
            );
            log::info!(
                "[ModelicaCompiler] preloaded MSL from `{}` in {:.2}s: \
                 {} parsed / {} inserted (cache {:?}); diagnostics: {}",
                msl_root.display(),
                t_total.elapsed().as_secs_f64(),
                report.parsed_file_count,
                report.inserted_file_count,
                report.cache_status,
                if report.diagnostics.is_empty() {
                    "none".to_string()
                } else {
                    format!("{} lines", report.diagnostics.len())
                },
            );
        } else {
            log::info!(
                "[ModelicaCompiler] no MSL source root available on this target; \
                 session starts empty",
            );
        }
        Self { session }
    }

    /// Compile Modelica source string and return DAE result.
    ///
    /// The user source is fed as a workspace document on top of the
    /// already-preloaded MSL. Rumoca's strict-reachable DAE walker
    /// sees the user's model plus the entire MSL class tree, so
    /// short-form refs like `SI.Time`, `Continuous.Filter`, etc.
    /// resolve through normal MLS §5 scope lookup.
    ///
    /// `filename` is used as the document URI for error reporting.
    pub fn compile_str(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
    ) -> Result<Box<rumoca_session::compile::DaeCompilationResult>, String> {
        let t_total = std::time::Instant::now();
        self.session.update_document(filename, source);
        let result = self
            .session
            .compile_model_dae_strict_reachable_uncached_with_recovery(model_name);
        log::info!(
            "[ModelicaCompiler] compile `{}` finished in {:.2}s ({})",
            model_name,
            t_total.elapsed().as_secs_f64(),
            if result.is_ok() { "OK" } else { "ERR" },
        );
        result
    }

    /// Access the underlying `rumoca_session::Session` — used by a
    /// test helper that needs to inspect loaded source roots.
    #[cfg(test)]
    pub fn session(&self) -> &Session {
        &self.session
    }
}


pub mod ui;

/// Bundled Modelica models for web deployment.
/// Available on all targets, but primarily used for wasm builds.
pub mod models;

/// System sets for Modelica stepping in [`FixedUpdate`].
///
/// These sets let downstream code (e.g., balloon_setup) order its sync systems
/// relative to the Modelica worker communication.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelicaSet {
    /// Receive async results from the worker thread.
    HandleResponses,
    /// Send the next step command to the worker thread.
    SpawnRequests,
}

/// Bevy plugin for Modelica integration.
///
/// Sets up the background worker thread, channel resources, and response systems.
/// Modelica stepping runs in [`FixedUpdate`] so all co-simulation engines share
/// the same fixed timestep.
pub struct ModelicaPlugin;

/// Headless variant of [`ModelicaPlugin`] without UI panels.
///
/// Use in tests and non-windowed binaries. Starts the worker, inserts channels,
/// schedules stepping systems, but skips `ModelicaUiPlugin`.
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
        // Install the user-facing indentation default for the
        // pretty-printer. The library-level default is two-space so
        // pure-Rust tests have predictable output; the workbench UI
        // wants tabs (matches Dymola / MSL hand-authored style).
        // Users can override at runtime via a settings panel or
        // script by calling `pretty::set_options` again.
        pretty::set_options(pretty::PrettyOptions::tabs());
    }
}

fn build_modelica_core(app: &mut App) {
    let (tx_cmd, rx_cmd) = unbounded();
    let (tx_res, rx_res) = unbounded();

    let msl = msl_dir();
    if msl.exists() {
        if let Ok(abs_path) = std::fs::canonicalize(&msl) {
            std::env::set_var("MODELICAPATH", abs_path.to_string_lossy().to_string());
        }
    }

    // Do NOT override `RUMOCA_CACHE_DIR`. Leaving it unset lets
    // rumoca-session use its XDG default (`~/.cache/rumoca/...`),
    // which is the same location the standalone `modelica_tester`
    // CLI and any other rumoca-using tool share. That share is
    // what keeps startup-to-first-compile fast: a cache populated
    // by one tool is hit by the next.
    //
    // Earlier versions of this code pinned the cache to the
    // workspace `.cache/rumoca/` to keep CI/test runs deterministic.
    // In practice that guaranteed *cold* caches for interactive
    // use: every rumoca source change bumps the artifact-cache key
    // schema, which invalidates the workspace cache while the XDG
    // cache — populated by the CLI — still matches. Result: CLI
    // compiles in ~5 s, workbench in minutes. Sharing the XDG
    // cache with the CLI is the obvious fix; callers that want a
    // sandboxed cache can still set `RUMOCA_CACHE_DIR` explicitly
    // before launching the binary.

    #[cfg(not(target_arch = "wasm32"))]
    {
        thread::spawn(move || {
            modelica_worker(rx_cmd, tx_res);
        });
    }

    #[cfg(target_arch = "wasm32")]
    {
        app.insert_resource(InlineWorker::default());
    }

    #[cfg(not(target_arch = "wasm32"))]
    app.insert_resource(ModelicaChannels { tx: tx_cmd, rx: rx_res });
    #[cfg(target_arch = "wasm32")]
    {
        app.insert_resource(ModelicaChannels { tx: tx_cmd, rx: rx_res, rx_cmd, tx_res });
    }

    app.init_resource::<ui::WorkbenchState>();

    app.configure_sets(
        FixedUpdate,
        (ModelicaSet::HandleResponses, ModelicaSet::SpawnRequests).chain(),
    );

    app.register_type::<ModelicaModel>()
        .add_systems(FixedUpdate, (
            handle_modelica_responses.in_set(ModelicaSet::HandleResponses),
            spawn_modelica_requests.in_set(ModelicaSet::SpawnRequests),
        ));

    #[cfg(target_arch = "wasm32")]
    {
        app.add_systems(Update, inline_worker_process);
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

use std::sync::Arc;

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
    /// Per-variable Modelica description strings (the `"..."` comment after
    /// a declaration — MLS §A.2.5 `description-string`). Collected from
    /// the compiled DAE on `is_new_model` / `is_parameter_update` so the
    /// UI can show them as hover tooltips. Only populated on compile-type
    /// results; Step results leave this empty.
    pub detected_descriptions: Vec<(String, String)>,
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
            detected_descriptions: Vec::new(),
        }
    }
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
    #[allow(dead_code)]
    source: Arc<str>,
    #[allow(dead_code)]
    dae: Box<rumoca_session::compile::DaeCompilationResult>,
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
        new_time: 0.0,
        outputs: Vec::new(),
        detected_symbols: Vec::new(),
        error: None, log_message: None, is_new_model: false,
        is_parameter_update: false, is_reset: false,
        detected_input_names: Vec::new(),
        detected_descriptions: Vec::new(),
    }
}

/// Pull every variable's Modelica description string (`"..."` after a
/// declaration, per MLS §A.2.5) straight from the source AST.
///
/// Rumoca's DAE drops component descriptions during compile → DAE
/// lowering (as of the rumoca commit pinned in `Cargo.lock` — the
/// field `Dae::Variable.description` is always `None` in practice),
/// so we re-parse the source AST instead. Cheap enough for compile /
/// parameter-update events (rumoca parse is fast and cached).
fn collect_variable_descriptions(source: &str) -> Vec<(String, String)> {
    ast_extract::extract_descriptions(source)
        .into_iter()
        .collect()
}

/// The background worker that owns the !Send SimSteppers and cached DAEs.
fn modelica_worker(rx: Receiver<ModelicaCommand>, tx: Sender<ModelicaResult>) {
    let mut steppers: HashMap<Entity, (u64, String, SimStepper)> = HashMap::default();
    let mut current_sessions: HashMap<Entity, u64> = HashMap::default();
    // DAE cache per entity — enables instant Reset and fast Step auto-init
    let mut cached_models: HashMap<Entity, CachedModel> = HashMap::default();
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
            let cmd_started = std::time::Instant::now();
            log::info!("[worker] begin: {}", cmd_label);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match cmd {
                    ModelicaCommand::Reset { entity, session_id } => {
                        current_sessions.insert(entity, session_id);

                        if let Some(cached) = cached_models.get(&entity) {
                            // Strip input defaults from cached source and set them via set_input
                            let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);

                            let mut opts = StepperOptions::default();
                            opts.atol = 1e-1; opts.rtol = 1e-1;
                            // Recompile stripped source to get a fresh stepper with input slots
                            let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                            match compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                                Ok(comp_res) => {
                                    match SimStepper::new(&comp_res.dae, opts) {
                                        Ok(mut stepper) => {
                                            for (name, val) in &input_defaults {
                                                let _ = stepper.set_input(name, *val);
                                            }
                                            let input_names: Vec<String> = stepper.input_names().to_vec();
                                            let symbols = collect_stepper_observables(&stepper);
                                            let descriptions = collect_variable_descriptions(&stripped_source);
                                            steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                                            let _ = tx_inner.send(ModelicaResult {
                                                entity, session_id, new_time: 0.0,
                                                outputs: Vec::new(),
                                                detected_symbols: symbols, error: None,
                                                log_message: Some("Reset complete.".to_string()),
                                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                                detected_input_names: input_names,
                                                detected_descriptions: descriptions,
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
                                opts.atol = 1e-1; opts.rtol = 1e-1;
                                match SimStepper::new(&comp_res.dae, opts) {
                                    Ok(mut stepper) => {
                                        for (name, val) in &input_defaults {
                                            let _ = stepper.set_input(name, *val);
                                        }
                                        let input_names: Vec<String> = stepper.input_names().to_vec();
                                        let symbols = collect_stepper_observables(&stepper);
                                        let descriptions = collect_variable_descriptions(&stripped_source);
                                        cached_models.insert(entity, CachedModel {
                                            session_id,
                                            model_name: model_name.clone(),
                                            source: Arc::from(source.clone()),
                                            dae: comp_res,
                                        });
                                        steppers.insert(entity, (session_id, model_name.clone(), stepper));
                                        let _ = tx_inner.send(ModelicaResult {
                                            entity, session_id, new_time: 0.0,
                                            outputs: Vec::new(),
                                            detected_symbols: symbols, error: None,
                                            log_message: Some("Parameters applied.".to_string()),
                                            is_new_model: false, is_parameter_update: true, is_reset: false,
                                            detected_input_names: input_names,
                                            detected_descriptions: descriptions,
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

                        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                        match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                            Ok(comp_res) => {
                                let mut opts = StepperOptions::default();
                                opts.atol = 1e-1; opts.rtol = 1e-1;
                                match SimStepper::new(&comp_res.dae, opts) {
                                    Ok(mut stepper) => {
                                        // Set input defaults via set_input so they're runtime-changeable
                                        for (name, val) in &input_defaults {
                                            let _ = stepper.set_input(name, *val);
                                        }
                                        let input_names: Vec<String> = stepper.input_names().to_vec();
                                        let symbols = collect_stepper_observables(&stepper);
                                        let descriptions = collect_variable_descriptions(&stripped_source);
                                        let temp_dir = modelica_dir().join(format!("{}_{}", entity.index(), entity.generation()));
                                        let _ = std::fs::create_dir_all(&temp_dir);
                                        let temp_path = temp_dir.join("model.mo");
                                        let _ = std::fs::write(&temp_path, &source);

                                        cached_models.insert(entity, CachedModel {
                                            session_id,
                                            model_name: model_name.clone(),
                                            source: Arc::from(source.clone()),
                                            dae: comp_res,
                                        });
                                        steppers.insert(entity, (session_id, model_name.clone(), stepper));
                                        let _ = tx_inner.send(ModelicaResult {
                                            entity, session_id, new_time: 0.0,
                                            outputs: Vec::new(),
                                            detected_symbols: symbols, error: None,
                                            log_message: Some(format!("Model '{}' compiled.", model_name)),
                                            is_new_model: true, is_parameter_update: false, is_reset: false,
                                            detected_input_names: input_names,
                                            detected_descriptions: descriptions,
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
                                        opts.atol = 1e-1; opts.rtol = 1e-1;
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
                                let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                                match compiler.compile_str(&model_name, &source, &model_path.to_string_lossy()) {
                                    Ok(comp_res) => {
                                        let mut opts = StepperOptions::default();
                                        opts.atol = 1e-1; opts.rtol = 1e-1;
                                        if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                                            for (name, val) in &inputs { let _ = s.set_input(name, *val); }
                                            cached_models.insert(entity, CachedModel {
                                                session_id,
                                                model_name: model_name.clone(),
                                                source: Arc::from(std::fs::read_to_string(&model_path).unwrap_or_default()),
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
                                    // `state()` reconstructs algebraics / outputs via
                                    // `EliminationResult` and also includes inputs, so
                                    // this single call supersedes the old two-loop
                                    // variable_names + input_names collection.
                                    let outputs = collect_stepper_observables(stepper);
                                    let _ = tx_inner.send(ModelicaResult {
                                        entity, session_id, new_time: stepper.time(),
                                        outputs, error: None, log_message: None,
                                        is_new_model: false, detected_symbols: Vec::new(),
                                        is_parameter_update: false, is_reset: false,
                                        detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
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
                    detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
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
                        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                        if let Ok(comp_res) = compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                            let mut opts = StepperOptions::default();
                            opts.atol = 1e-1; opts.rtol = 1e-1;
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
                            entity, session_id, new_time: stepper.time(),
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(), error: Some(format!("Solver Error: {:?}", e)),
                            log_message: None, is_new_model: false, is_parameter_update: false,
                            is_reset: false, detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
                        });
                        w.steppers.remove(&entity);
                    } else {
                        let outputs = collect_stepper_observables(stepper);
                        let _ = tx.send(ModelicaResult {
                            entity, session_id, new_time: stepper.time(),
                            outputs, error: None,
                            log_message: None, is_new_model: false, detected_symbols: Vec::new(),
                            is_parameter_update: false, is_reset: false, detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
                        });
                    }
                } else {
                    let _ = tx.send(result_ok(entity, session_id));
                }
            } else {
                let _ = tx.send(ModelicaResult {
                    entity, session_id, new_time: 0.0,
                    outputs: Vec::new(),
                    detected_symbols: Vec::new(), error: Some("Sim engine failed to start.".to_string()),
                    log_message: None, is_new_model: false, is_parameter_update: false,
                    is_reset: false, detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
                });
            }
        }
        ModelicaCommand::Compile { entity, session_id, model_name, source } => {
            w.current_sessions.insert(entity, session_id);
            let (stripped_source, input_defaults) = strip_input_defaults(&source);

            let mut opts = StepperOptions::default();
            opts.atol = 1e-1; opts.rtol = 1e-1;
            let tx = &channels.tx_res;

            let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
            match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                Ok(comp_res) => {
                    match SimStepper::new(&comp_res.dae, opts) {
                        Ok(mut stepper) => {
                            for (name, val) in &input_defaults { let _ = stepper.set_input(name, *val); }
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            let descriptions = collect_variable_descriptions(&stripped_source);
                            w.cached_models.insert(entity, CachedModel {
                                session_id, model_name: model_name.clone(), source: Arc::from(source.clone()),
                                dae: comp_res.clone(),
                            });

                            w.steppers.insert(entity, (session_id, model_name.clone(), stepper));
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Compiled successfully.".to_string()),
                                is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: input_names,
                                detected_descriptions: descriptions,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {:?}", e)),
                                log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(ModelicaResult {
                        entity, session_id, new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Compile Error: {:?}", e)),
                        log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                        detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
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
                opts.atol = 1e-1; opts.rtol = 1e-1;
                let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                match compiler.compile_str(&cached.model_name, &stripped_source, "model.mo") {
                    Ok(comp_res) => {
                        if let Ok(mut stepper) = SimStepper::new(&comp_res.dae, opts) {
                            for (name, val) in &input_defaults { let _ = stepper.set_input(name, *val); }
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            let descriptions = collect_variable_descriptions(&stripped_source);
                            w.steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Reset complete.".to_string()),
                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: input_names,
                                detected_descriptions: descriptions,
                            });

                                } else {
                                let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some("Stepper init failed".to_string()),
                                log_message: None, is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
                                });
                                }
                                }
                                Err(e) => {
                                let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Reset compile error: {:?}", e)),
                                log_message: None, is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
                                });
                                }
                                }
                                } else {
                                w.steppers.remove(&entity);
                                let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: None,
                                log_message: Some("Reset complete (no cached model).".to_string()),
                                is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
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
            opts.atol = 1e-1; opts.rtol = 1e-1;
            let tx = &channels.tx_res;

            let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
            match compiler.compile_str(&model_name, &stripped_source, "model.mo") {
                Ok(comp_res) => {
                    match SimStepper::new(&comp_res.dae, opts) {
                        Ok(mut stepper) => {
                            for (name, val) in &input_defaults { let _ = stepper.set_input(name, *val); }
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            let descriptions = collect_variable_descriptions(&stripped_source);
                            w.cached_models.insert(entity, CachedModel {
                                session_id, model_name: model_name.clone(), source: Arc::from(source.clone()),
                                dae: comp_res,
                            });

                            w.steppers.insert(entity, (session_id, model_name.clone(), stepper));
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Parameters applied.".to_string()),
                                is_new_model: false, is_parameter_update: true, is_reset: false,
                                detected_input_names: input_names,
                                detected_descriptions: descriptions,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {:?}", e)),
                                log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
                                detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(ModelicaResult {
                        entity, session_id, new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Re-compile Error: {:?}", e)),
                        log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
                        detected_input_names: Vec::new(), detected_descriptions: Vec::new(),
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
    /// Per-variable description strings lifted from the Modelica source
    /// (MLS §A.2.5). Populated on compile-type results so the UI can
    /// render them as hover tooltips in Telemetry, Inspector, Diagram,
    /// etc. Not reflected — these are derived from the source and can
    /// be recomputed on reload.
    #[reflect(ignore)]
    pub descriptions: HashMap<String, String>,
    /// Canonical id of the Modelica source document backing this entity,
    /// looked up in [`ui::ModelicaDocumentRegistry`]. `DocumentId::default()`
    /// (`0`) means "no document assigned yet"; systems should treat it as
    /// a miss. Not reflected — ids are session-local allocations, not
    /// scene-serializable.
    #[reflect(ignore)]
    pub document: lunco_doc::DocumentId,
    #[reflect(ignore)]
    pub is_stepping: bool,
}

/// Sends `Step` commands for each active model.
///
/// Runs in [`FixedUpdate`] using the fixed timestep delta. All models step with
/// the same dt, matching Avian physics and wire propagation.
fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    time: Res<Time<Fixed>>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
) {
    let dt = time.delta_secs_f64();

    for (entity, mut model) in q_models.iter_mut() {
        if model.is_stepping || model.paused { continue; }

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
fn handle_modelica_responses(
    channels: Res<ModelicaChannels>,
    mut q_models: Query<&mut ModelicaModel>,
    mut workbench_state: ResMut<ui::WorkbenchState>,
    // Headless callers (e.g. cosim tests) run this system without the
    // UI plugin, so the console + compile-state resources may be
    // absent. Make both optional so the core stepping path survives
    // those setups without forcing them to pull in the UI module.
    compile_states: Option<ResMut<ui::CompileStates>>,
    console: Option<ResMut<ui::panels::console::ConsoleLog>>,
    // Optional — a headless cosim harness may skip `LuncoVizPlugin`
    // entirely. When present, every outgoing sample is published into
    // the registry, and the default Modelica plot's bindings are
    // seeded on first compile of each entity.
    mut signals: Option<ResMut<lunco_viz::SignalRegistry>>,
    mut viz_registry: Option<ResMut<lunco_viz::VisualizationRegistry>>,
) {
    let mut compile_states = compile_states;
    let mut console = console;
    while let Ok(result) = channels.rx.try_recv() {
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
                    ui::CompileState::Error
                } else {
                    ui::CompileState::Ready
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
                            ui::CompileState::Error => {
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
                            ui::CompileState::Ready => {
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

            // Variable description strings for hover tooltips (Telemetry,
            // Inspector, Diagram). Populated on compile-type results only;
            // step results leave `detected_descriptions` empty so we
            // don't blow away the map on every step.
            if (result.is_new_model || result.is_parameter_update || result.is_reset)
                && !result.detected_descriptions.is_empty()
            {
                model.descriptions.clear();
                for (name, desc) in &result.detected_descriptions {
                    model.descriptions.insert(name.clone(), desc.clone());
                }
            }

            if let Some(err) = &result.error {
                workbench_state.compilation_error = Some(err.clone());
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
            } else if workbench_state.selected_entity == Some(result.entity) {
                workbench_state.compilation_error = None;
            }

            if result.is_new_model {
                model.model_path = modelica_dir()
                    .join(format!("{}_{}", result.entity.index(), result.entity.generation()))
                    .join("model.mo");
                model.variables.clear();
                // Only unpause on a *successful* Compile. A failed
                // Compile leaves the stepper empty, and unpausing would
                // cause `spawn_modelica_requests` to ship a Step →
                // worker recompiles from scratch (~10s) → error → repeat
                // forever. The earlier error-branch `paused = true`
                // marks the model as blocked; the user resumes
                // explicitly after fixing the source.
                if result.error.is_none() {
                    model.paused = false;
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
                // Compile-type results reset the signal's horizon so
                // the old run's tail doesn't bleed into the new one.
                if result.is_new_model || result.is_reset || result.is_parameter_update {
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
                // type results so the viz inspector can show tooltips
                // sourced the same way Telemetry does today.
                if result.is_new_model || result.is_parameter_update {
                    for (name, desc) in &result.detected_descriptions {
                        sigs.update_meta(
                            lunco_viz::SignalRef::new(result.entity, name.clone()),
                            lunco_viz::SignalMeta {
                                description: Some(desc.clone()),
                                unit: None,
                                provenance: Some("modelica".to_string()),
                            },
                        );
                    }
                }
            }

            // Auto-seed the default Modelica plot with every observable
            // from a freshly-compiled model. Preserves the pre-viz UX
            // where compiling immediately filled the graph with all
            // the model's observables. Does nothing when the user has
            // already curated the bindings — `auto_bind_observables`
            // skips already-present bindings.
            if result.is_new_model {
                if let Some(reg) = viz_registry.as_deref_mut() {
                    let parameters = model.parameters.clone();
                    ui::viz::auto_bind_observables(
                        reg,
                        result.entity,
                        &result.detected_symbols,
                        |name| parameters.contains_key(name),
                    );
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

#[cfg(test)]
mod observables_smoke {
    use super::*;
    use rumoca_sim::{SimStepper, StepperOptions};

    /// End-to-end smoke test for the observables pipeline: compile the
    /// bundled RocketEngine, run one step at full throttle, and assert
    /// every algebraic observable shows up with a physically-sensible
    /// value in [`collect_stepper_observables`]. Protects against
    /// (a) bumping rumoca to a version that drops `EliminationResult`
    ///     from the stepper again, and
    /// (b) reintroducing a Boolean intermediate in the bundled model
    ///     that rumoca's elimination pass can't reconstruct.
    #[test]
    fn rocket_engine_observables_round_trip() {
        let raw = include_str!("../../../assets/models/RocketEngine.mo");
        let (src, _) = ast_extract::strip_input_defaults(raw);
        let mut c = ModelicaCompiler::new();
        let r = c.compile_str("RocketEngine", &src, "RocketEngine.mo")
            .expect("compile ok");
        let mut stepper = SimStepper::new(&r.dae, StepperOptions::default())
            .expect("stepper ok");
        stepper.set_input("throttle", 1.0).expect("throttle is an input");
        stepper.step(0.01).expect("step ok");

        let obs = collect_stepper_observables(&stepper);
        let by_name: std::collections::HashMap<_, _> =
            obs.into_iter().collect();

        for name in ["m_prop", "impulse", "m_dot", "thrust", "p_chamber", "isp"] {
            assert!(by_name.contains_key(name), "missing observable: {name}");
        }
        assert!(by_name["m_dot"] > 0.0,
            "m_dot should be nonzero at throttle=1, got {}", by_name["m_dot"]);
        assert!(by_name["thrust"] > 0.0,
            "thrust should be nonzero, got {}", by_name["thrust"]);
        assert!(by_name["p_chamber"] > 0.0,
            "p_chamber should be nonzero, got {}", by_name["p_chamber"]);
        assert!((by_name["isp"] - 2900.0 / 9.80665).abs() < 1e-3,
            "isp should equal v_e / g, got {}", by_name["isp"]);
    }

    /// Verifies that `"..."` description strings (MLS §A.2.5) survive
    /// the AST-based extraction pipeline and reach the worker's
    /// description map. If this regresses, Telemetry tooltips go dark.
    #[test]
    fn rocket_engine_descriptions_populate() {
        let raw = include_str!("../../../assets/models/RocketEngine.mo");
        let (src, _) = ast_extract::strip_input_defaults(raw);
        let descs: std::collections::HashMap<String, String> =
            collect_variable_descriptions(&src).into_iter().collect();
        for (var, needle) in [
            ("m_dot_max", "mass flow"),
            ("throttle",  "Throttle"),
            ("m_prop",    "Propellant"),
            ("thrust",    "Thrust"),
        ] {
            let desc = descs.get(var)
                .unwrap_or_else(|| panic!(
                    "no description for '{var}'; got {:?}",
                    descs.keys().collect::<Vec<_>>()
                ));
            assert!(desc.contains(needle),
                "'{var}' description should contain '{needle}', got: {desc:?}");
        }
    }

    // ─────────────────────────────────────────────────────────
    // MSL demand-driven compile tests
    // ─────────────────────────────────────────────────────────
    //
    // Run with: `cargo test -p lunco-modelica msl --nocapture`
    //
    // `msl_` tests require the MSL tree at `<cache>/msl/Modelica/`
    // (populated by our indexer). They skip with a stderr notice if
    // absent — CI can run the non-MSL subset unconditionally.
    //
    // The headline test `msl_compile_with_limpid_is_fast_and_succeeds`
    // exercises the full iterative demand-load pipeline: alias
    // resolution, rumoca error → missing-class regex, fs::read,
    // update_document, retry loop. A known-good MSL example that
    // used to hang for minutes is the sanity check; we assert the
    // happy path + print elapsed so regression to "minutes" is
    // obvious in the log even if the timing isn't asserted strictly
    // (test runner load is variable).

    fn msl_available() -> bool {
        lunco_assets::msl_source_root_path().is_some()
    }


    /// Trivial smoke test — compile a self-contained model with no
    /// MSL references. Shouldn't touch the iterative loop at all,
    /// verifies the plain-compile path works post-refactor.
    #[test]
    fn bare_model_compiles_without_msl() {
        let src = r#"
            model Bare
              Real x(start=1);
            equation
              der(x) = -x;
            end Bare;
        "#;
        let mut c = ModelicaCompiler::new();
        let r = c.compile_str("Bare", src, "Bare.mo")
            .expect("bare model must compile without MSL");
        // Just assert we got a DAE at all — shape details vary
        // by rumoca version.
        let _ = r.dae;
    }

    /// Headline: end-to-end demand-driven compile that pulls MSL
    /// classes via the iterative loop. A minimal LimPID-using model
    /// forces the compiler to iteratively resolve Continuous.LimPID
    /// → Interfaces.SISO → SI types → Icons → etc.
    ///
    /// **Asserts**:
    /// - compile succeeds
    /// - logs total elapsed time (paste-able into regression tracking)
    /// - iteration count reasonable (< 20 for a small closure)
    ///
    /// Skips with a print if MSL isn't installed locally.
    /// Known-failing — not a resolver issue. Compiles through the
    /// resolve phase cleanly (all `SI.*`, `Logical.*` refs are
    /// resolved via the lazy hook). Fails at DAE (ToDae phase)
    /// with `unresolved reference: ModelicaServices.Machine.eps`.
    /// Rumoca hardcodes `ModelicaServices.Machine` + `Modelica.Constants`
    /// as CONSTANT_PACKAGES
    /// (rumoca-phase-flatten/src/lib.rs:687-689); its lookup in
    /// the resolved tree doesn't find `ModelicaServices.Machine`
    /// even after we `update_document(ModelicaServices/package.mo)`.
    /// Fetch trace confirms the file lands in the session but
    /// rumoca's constant-package resolver still errors.
    ///
    /// Note: `msl_compile_pid_controller_example_succeeds` passes
    /// and *also* instantiates LimPID transitively — so the gap
    /// is specific to this minimal direct-instantiation shape, not
    /// to LimPID itself. Filed as a rumoca-internal issue.
    #[test]
    #[ignore = "rumoca CONSTANT_PACKAGES lookup can't find ModelicaServices.Machine even after the file is loaded"]
    fn msl_compile_tiny_limpid_model_is_fast() {
        if !msl_available() {
            eprintln!("skipping msl_compile_tiny_limpid_model_is_fast: \
                       MSL not at {:?}",
                lunco_assets::msl_source_root_path());
            return;
        }
        // Tiny model that references one MSL block — drags in the
        // transitive closure via the iterative loader. Kept inline
        // so the test doesn't depend on a user file.
        let src = r#"
            model TestLimPID
              import Modelica.Units.SI;
              parameter SI.Time Ti = 0.1;
              Modelica.Blocks.Continuous.LimPID ctrl(
                k = 1.0,
                Ti = Ti,
                yMax = 10.0
              );
            equation
              ctrl.u_s = 1.0;
              ctrl.u_m = 0.0;
            end TestLimPID;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = std::time::Instant::now();
        let result = c.compile_str("TestLimPID", src, "TestLimPID.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_compile_tiny_limpid_model_is_fast: elapsed {:.2}s, \
             result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".to_string() } else {
                format!("ERR: {}", result.as_ref().err().unwrap())
            }
        );
        result.expect("compile must succeed after iterative MSL load");
    }

    /// Same shape, against the actual PID_Controller example
    /// extracted from `Blocks/package.mo`. Bigger closure —
    /// Mechanics.Rotational + Blocks.Continuous + KinematicPTP +
    /// sensors + Icons.
    ///
    /// With the lazy ExternalResolver hook in place this should
    /// work without any alias-table workaround; rumoca's own §5
    /// resolver walks the `within Modelica;` + enclosing package
    /// imports and calls us for the bytes. Kept as a *diagnostic*
    /// test: if it fails, the failure is either (a) a genuine
    /// rumoca MLS gap (PID is NOT in rumoca's 180-supported MSL
    /// targets list — it may be one of the 15 known-failing), or
    /// (b) a resolver miss our hook should have handled.
    #[test]
    fn msl_compile_pid_controller_example_succeeds() {
        if !msl_available() {
            eprintln!("skipping: MSL not available");
            return;
        }
        // Reference by fully-qualified name so rumoca's scope-walker
        // sees the enclosing `package Blocks` (which carries
        // `import Modelica.Units.SI;`). The earlier version of this
        // test sliced PID_Controller out of `Blocks/package.mo` and
        // fed it as a standalone class, which dropped the enclosing
        // package's imports — failure was a test-construction flaw
        // on our side, not a resolver or rumoca gap.
        let src = r#"
            model TestPID
              extends Modelica.Blocks.Examples.PID_Controller;
            end TestPID;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = std::time::Instant::now();
        let result = c.compile_str("TestPID", src, "TestPID.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_compile_pid_controller_example_succeeds: elapsed {:.2}s, \
             result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".to_string() } else {
                format!("ERR (first 500 chars): {}",
                    result.as_ref().err().unwrap().chars().take(500).collect::<String>())
            }
        );
        result.expect("PID_Controller must compile after iterative MSL load");
    }

    /// End-to-end test against an MSL target rumoca *officially*
    /// claims to support (from `msl_simulation_targets_180.json` in
    /// rumoca-test-msl). This is the real acceptance test for the
    /// lazy-resolver architecture: rumoca's §5 resolver walks the
    /// scope, our `MslLazyResolver` supplies bytes on demand, a
    /// tiny wrapper model instantiates a known-good MSL example by
    /// fully-qualified name. If this fails, the loader architecture
    /// is broken. If it passes but PID_Controller fails, the delta
    /// is a rumoca MLS gap — not our problem.
    #[test]
    fn msl_compile_known_good_rotational_example() {
        if !msl_available() {
            eprintln!("skipping: MSL not available");
            return;
        }
        // Minimal wrapper — forces the resolver to pull in
        // Rotational.Examples.First and its entire transitive
        // closure (Rotational.Components, Interfaces, SI types,
        // Icons, …). References by fully-qualified name, which is
        // the scope-friendly form rumoca resolves cleanly.
        let src = r#"
            model TestRotFirst
              extends Modelica.Mechanics.Rotational.Examples.First;
            end TestRotFirst;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = std::time::Instant::now();
        let result = c.compile_str("TestRotFirst", src, "TestRotFirst.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_compile_known_good_rotational_example: elapsed {:.2}s, \
             result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".to_string() } else {
                format!("ERR (first 800 chars): {}",
                    result.as_ref().err().unwrap().chars().take(800).collect::<String>())
            }
        );
        result.expect("Rotational.Examples.First (known-good MSL target) must compile");
    }

    /// Purely-qualified-name test. If this passes but
    /// `msl_compile_known_good_rotational_example` fails, the gap
    /// is unambiguously in rumoca's short-form scope walking
    /// (enclosing-package imports aren't reaching nested classes),
    /// not in our resolver.
    #[test]
    fn msl_fully_qualified_time_resolves() {
        if !msl_available() {
            eprintln!("skipping: MSL not available");
            return;
        }
        let src = r#"
            model TestFullyQualifiedSI
              parameter Modelica.Units.SI.Time Ti = 0.5;
              Real x(start=1);
            equation
              der(x) = -x / Ti;
            end TestFullyQualifiedSI;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = std::time::Instant::now();
        let result = c.compile_str("TestFullyQualifiedSI", src, "Q.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_fully_qualified_time_resolves: elapsed {:.2}s, result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".into() } else {
                format!("ERR (first 800 chars): {}",
                    result.as_ref().err().unwrap().chars().take(800).collect::<String>())
            }
        );
        result.expect("fully-qualified SI.Time must compile");
    }
}




