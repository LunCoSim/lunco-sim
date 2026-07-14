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
use rumoca_sim::SimStepper;
use serde::{Serialize, Deserialize};

use lunco_assets::modelica_dir;

use crate::ast_extract::strip_input_defaults;
use crate::sim_stream::{SimSnapshot, SimStream};
use crate::ModelicaCompiler;

/// Solver options for the **LIVE** (co-simulated, client-predicted) path.
///
/// A4 — deliberately **NOT** [`crate::experiments_runner::stepper_options_from_bounds`].
/// The batch runner solves an offline experiment: it wants the most accurate
/// integrator available (diffsol BDF, adaptive-implicit, `atol = rtol = 1e-6`)
/// and it does not care that its internal step sequence is chosen from
/// floating-point error estimates. The live path is the exact opposite: it runs
/// *inside* the fixed-step physics loop that feeds forces to avian on a
/// client-predicted body, so its per-macro-step cost and its step sequence must
/// be a function of the **requested `dt` alone** — see
/// `docs/architecture/28-modelica-realtime-physics.md` §1.
///
/// So the live path gets its own configuration:
/// * **explicit family** ([`rumoca_sim::SimSolverMode::RkLike`]) — no Newton /
///   LU iteration whose *count* varies with the machine's rounding, and no
///   implicit tableau to fall back on,
/// * a **fixed macro/micro step ladder** — the caller drives the stepper at
///   [`LIVE_MICRO_DT`] micro-steps ([`micro_steps_for`]), so the stop-time
///   sequence is an integer function of `dt`, identical on every peer,
/// * `h0 = LIVE_MICRO_DT` so the integrator's first internal step matches the
///   micro-step it is asked for,
/// * an explicit, fixed tolerance ([`LIVE_TOL`]) — **not** the model's
///   `experiment(Tolerance=…)` annotation, which is an *offline accuracy* knob
///   and must not be able to change the realtime loop's behaviour.
///
/// CAVEAT (documented, not fixed here): rumoca's `RkLike` backend is an
/// *embedded* RK45 — its internal sub-step size is still error-adapted
/// (`adapt_step(h, error_norm)`), so a micro-step may internally split. rumoca
/// exposes no fixed-tableau, no-error-control stepper today. Driving it at
/// fixed micro-steps bounds the divergence to *within* one micro-step and keeps
/// the macro stop-times identical everywhere, which is as far as this layer can
/// go. Full Tier-A bit-determinism needs a fixed-step tableau upstream — see
/// TODO(A4) in `docs/architecture/28-modelica-realtime-physics.md`.
fn live_stepper_options() -> rumoca_sim::SimOptions {
    let mut opts = rumoca_sim::SimOptions {
        solver_mode: rumoca_sim::SimSolverMode::RkLike,
        ..Default::default()
    };
    opts.atol = LIVE_TOL;
    opts.rtol = LIVE_TOL;
    // `SimOptions.dt` is the initial/maximum internal step (h0).
    opts.dt = Some(LIVE_MICRO_DT);
    // The live stepper is driven by `step(dt)` calls, never by `t_end`; the
    // window is only used to derive defaults, so make it wide enough that no
    // realistic session reaches it.
    opts.t_start = 0.0;
    opts.t_end = f64::from(u32::MAX);
    opts
}

/// Build a `SimStepper` for the LIVE path from a freshly-compiled model.
///
/// **Single source of truth** for live stepper construction across the worker —
/// every site routes through here instead of copy-pasting the `SimOptions` setup
/// + `SimStepper::new` call (there were ~9 such copies).
///
/// Solver policy comes from [`live_stepper_options`] and is intentionally
/// **distinct** from the batch/Fast-Run policy (A4). The model's
/// `experiment(Tolerance=…)` annotation is deliberately ignored here: it is an
/// offline-accuracy knob and must not reach into the realtime coupling loop.
fn build_stepper(
    comp_res: &rumoca_compile::compile::DaeCompilationResult,
) -> Result<SimStepper, rumoca_sim::SimulationDiagnosticError> {
    SimStepper::new(&comp_res.dae, live_stepper_options())
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
        /// Stable session URI for the primary document (the document's
        /// canonical identity from `DocumentOrigin::session_uri` — a file
        /// path, bundled filename, or `Untitled-<id>`). The worker seats
        /// `source` under THIS key, so the interactive Run, Fast Run,
        /// Step, and parameter-update paths all key the same document
        /// identically and rumoca's merge pass never sees it registered
        /// under two filenames (the duplicate-class bug). NOT a class
        /// name: a file may declare several top-level classes.
        doc_uri: String,
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
    /// Structured, located compile diagnostics produced alongside
    /// `error` on a failed Compile (rumoca `StrictCompileReport`
    /// failures, converted to [`Diagnostic`](lunco_doc::Diagnostic)).
    /// Each entry may carry a 1-based (line, column) into the user
    /// document so the Diagnostics panel can render click-to-source
    /// rows for compile errors — the structured complement to the flat
    /// `error` summary string. Empty on success and for non-compile
    /// (solver / reset / parameter) results.
    #[serde(default)]
    pub compile_diagnostics: Vec<lunco_doc::Diagnostic>,
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
            compile_diagnostics: Vec::new(),
        }
    }
}

impl ModelicaResult {
    /// Overlay the `experiment(...)` annotation defaults lifted from a
    /// compile result onto this message. Single source of the
    /// `DaeCompilationResult` → `experiment_*` field mapping, which was
    /// copy-pasted at both worker compile sites (native + inline-worker).
    fn with_experiment(
        mut self,
        comp_res: &rumoca_compile::compile::DaeCompilationResult,
    ) -> Self {
        self.experiment_start_time = comp_res.experiment_start_time;
        self.experiment_stop_time = comp_res.experiment_stop_time;
        self.experiment_tolerance = comp_res.experiment_tolerance;
        self.experiment_interval = comp_res.experiment_interval;
        self.experiment_solver = comp_res.experiment_solver.clone();
        self
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
    /// The document's stable session URI (see `ModelicaCommand::Compile`'s
    /// `doc_uri`). Every cached-source recompile — Reset, Step auto-init,
    /// UpdateParameters — re-seats under this SAME key so the reused rumoca
    /// session never holds the document under two filenames.
    doc_uri: String,
}

/// Collect every readable variable from the stepper — states, inputs, and
/// (on rumoca `main`) algebraic / output reconstructions via
/// `EliminationResult`. Non-finite values are dropped so the UI never
/// plots NaN. Filtering out parameters / inputs happens downstream in
/// [`handle_modelica_responses`]; we report everything here so the UI has
/// the full picture and decides what goes into `model.variables`.
pub(crate) fn collect_stepper_observables(stepper: &SimStepper) -> Vec<(String, f64)> {
    stepper
        .state()
        .values
        .into_iter()
        .filter(|(name, val)| val.is_finite() && name != "time")
        .collect()
}

/// Fixed solver tolerance on the LIVE path. Explicit, and deliberately NOT the
/// model's `experiment(Tolerance=…)` annotation nor the batch runner's default —
/// see [`live_stepper_options`] (A4).
const LIVE_TOL: f64 = 1e-6;

/// The LIVE path's **micro-step**: the one and only step size handed to the
/// solver. Three micro-steps per fixed tick (60 Hz ⇒ 180 Hz solver rate).
///
/// Derived from [`lunco_core::SECS_PER_TICK`], so the model's stop-time lattice
/// is a pure function of the FIXED-STEP clock — never of the render frame, GPU
/// load, or window focus (A3).
const LIVE_MICRO_DT: f64 = lunco_core::SECS_PER_TICK / 3.0;

/// Hard cap on micro-steps integrated inside ONE `Step` command.
///
/// This is the catch-up clamp: a model that has fallen behind the world clock
/// (a long compile, a hitched frame, a `TimeTransport.rate` burst) asks for a
/// large `dt`, and we integrate at most this many micro-steps for it —
/// ~0.178 s of model time. Whatever is left over stays as lag and is caught up
/// on the following ticks (`spawn_modelica_requests` recomputes the deficit from
/// the model's OWN clock every tick, so nothing is lost). The clamp exists so a
/// 10-second stall can't hand the solver a 10-second macro step.
const MAX_MICRO_STEPS_PER_MACRO: u32 = 32;

/// Largest `dt` one `Step` command may carry (= the clamp above, in seconds).
/// `spawn_modelica_requests` clamps the requested catch-up to this; the worker
/// clamps again ([`micro_steps_for`]) so a hand-built `Step` can't blow past it.
pub const MAX_MACRO_STEP_DT: f64 = LIVE_MICRO_DT * MAX_MICRO_STEPS_PER_MACRO as f64;

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
    if !(dt > 0.0) {
        return 0;
    }
    let n = (dt / LIVE_MICRO_DT).round();
    (n as u32).clamp(1, MAX_MICRO_STEPS_PER_MACRO)
}

/// Integrate one macro step: `micro_steps_for(dt)` fixed micro-steps.
///
/// The ONE integration loop for the live path — native worker and wasm inline
/// worker both call it, so the two `#[cfg]` twins cannot drift on step policy.
/// Advances the model's own clock by exactly `micro_steps_for(dt) *
/// LIVE_MICRO_DT`; the caller reads `stepper.time()` for the truth and the Bevy
/// side reconciles any residual against the world clock next tick.
fn integrate_macro_step(
    stepper: &mut SimStepper,
    dt: f64,
) -> Result<(), rumoca_sim::SimulationDiagnosticError> {
    for _ in 0..micro_steps_for(dt) {
        stepper.step(LIVE_MICRO_DT)?;
    }
    Ok(())
}

/// Model-vs-world lag past which the co-sim coupling is no longer trustworthy
/// (the forces avian integrates this tick come from a model state this far in
/// the past). Surfaced as a rate-limited `warn!` + [`CosimLag`].
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
/// `worst_secs` is the coupling delay: the age of the model state whose outputs
/// the current tick's forces were computed from. In steady state it sits at
/// roughly one macro step (the in-flight `Step`); a sustained rise means the
/// worker cannot keep up with the fixed clock and the model is being carried by
/// the catch-up path.
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
///
/// **Native only.** It is spawned on a real `std::thread` (see
/// `ModelicaPlugin::build`) and reads/writes the model file on disk. The browser
/// has neither: wasm dispatches the *same* commands through
/// [`process_inline_command`] (inline, or in the `lunica_worker` Web Worker
/// bundle) with the source carried in the message instead of read from a path.
/// Gating it native-only is what keeps `std::fs` out of the wasm bundle rather
/// than shipping calls that always `Err` in a browser.
#[cfg(not(target_arch = "wasm32"))]
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

                            // Recompile stripped source to get a fresh stepper with input slots
                            let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                            match compiler.compile_str(&cached.model_name, &stripped_source, &cached.doc_uri) {
                                Ok(comp_res) => {
                                    match build_stepper(&comp_res) {
                                        Ok(mut stepper) => {
                                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Init");
                                            let input_names: Vec<String> = stepper.input_names().to_vec();
                                            let symbols = collect_stepper_observables(&stepper);
                                            steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                                            let _ = tx_inner.send(reset_ok(
                                                entity, session_id, symbols, input_names,
                                                "Reset complete.",
                                            ));
                                        }
                                        Err(e) => {
                                            let mut r = result_ok(entity, session_id);
                                            r.error = Some(format!("Stepper Init Error: {e}"));
                                            // rumoca-sim structured error → located
                                            // diagnostics (click-to-source for solver
                                            // lowering failures).
                                            r.compile_diagnostics =
                                                crate::diagnostics_from_sim_error(&e, &stripped_source);
                                            r.is_reset = true;
                                            let _ = tx_inner.send(r);
                                        }
                                    }
                                }
                                Err(e) => {
                                    let mut r = result_ok(entity, session_id);
                                    // `e` is rumoca's formatted compile summary string.
                                    r.error = Some(format!("Reset compile error: {e}"));
                                    r.compile_diagnostics =
                                        compiler.compile_diagnostics(&cached.model_name, &cached.doc_uri);
                                    r.is_reset = true;
                                    let _ = tx_inner.send(r);
                                }
                            }
                        } else {
                            steppers.remove(&entity);
                            let _ = tx_inner.send(reset_ok(
                                entity, session_id, Vec::new(), Vec::new(),
                                "Reset complete (no cached model).",
                            ));
                        }
                    }
                    ModelicaCommand::UpdateParameters { entity, session_id, model_name, source } => {
                        if session_id < *current_sessions.get(&entity).unwrap_or(&0) {
                            let _ = tx_inner.send(result_ok(entity, session_id));
                            return;
                        }
                        current_sessions.insert(entity, session_id);

                        // Re-seat under the SAME session URI the model was first
                        // compiled with — UpdateParameters always follows a Compile,
                        // so the entity is cached. Falling back to the model name
                        // only happens for a never-compiled entity (shouldn't occur).
                        let doc_uri = cached_models
                            .get(&entity)
                            .map(|c| c.doc_uri.clone())
                            .unwrap_or_else(|| model_name.clone());

                        // CQ-213: removed a per-UpdateParameters `model.mo` temp write.
                        // It wrote `source` to disk on every parameter update but
                        // nothing read it back — `compile_str` below compiles the
                        // in-memory `stripped_source` against `doc_uri`, and the
                        // cache stores `source` directly. Pure blocking I/O.

                        // Strip input defaults so they become real runtime slots
                        let (stripped_source, input_defaults) = strip_input_defaults(&source);

                        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                        match compiler.compile_str(&model_name, &stripped_source, &doc_uri) {
                            Ok(comp_res) => {
                                match build_stepper(&comp_res) {
                                    Ok(mut stepper) => {
                                        apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                                        let input_names: Vec<String> = stepper.input_names().to_vec();
                                        let symbols = collect_stepper_observables(&stepper);
                                        cached_models.insert(entity, CachedModel {
                                            model_name: model_name.clone(),
                                            source: Arc::from(source),
                                            doc_uri: doc_uri.clone(),
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
                                        r.error = Some(format!("Stepper Init Error: {e}"));
                                        r.compile_diagnostics =
                                            crate::diagnostics_from_sim_error(&e, &stripped_source);
                                        r.is_parameter_update = true;
                                        let _ = tx_inner.send(r);
                                    }
                                }
                            }
                            Err(e) => {
                                let mut r = result_ok(entity, session_id);
                                r.error = Some(format!("Re-compile Error: {e}"));
                                r.compile_diagnostics =
                                    compiler.compile_diagnostics(&model_name, &doc_uri);
                                r.is_parameter_update = true;
                                let _ = tx_inner.send(r);
                            }
                        }
                    }
                    ModelicaCommand::Compile { entity, session_id, model_name, source, doc_uri, extra_sources, stream } => {
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
                            compiler.compile_str(&model_name, &stripped_source, &doc_uri)
                        } else {
                            compiler.compile_str_multi(&model_name, &stripped_source, &doc_uri, &extra_sources)
                        };
                        bevy::log::info!(
                            "[worker] compile_str returned for `{}` in {:.2}s ({})",
                            model_name,
                            t_compile.elapsed().as_secs_f64(),
                            if _compile_outcome.is_ok() { "OK" } else { "ERR" },
                        );
                        match _compile_outcome {
                            Ok(comp_res) => {
                                match build_stepper(&comp_res) {
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
                                            doc_uri: doc_uri.clone(),
                                        });
                                        steppers.insert(entity, (session_id, model_name.clone(), stepper));
                                        let _ = tx_inner.send(ModelicaResult {
                                            entity, session_id, new_time: 0.0,
                                            outputs: Vec::new(),
                                            detected_symbols: symbols, error: None,
                                            log_message: Some(format!("Model '{}' compiled.", model_name)),
                                            is_new_model: true, is_parameter_update: false, is_reset: false,
                                            detected_input_names: input_names,
                                            compiled_model_name: Some(model_name.clone()),
                                            loaded_source_root_id: None,
                                            compile_diagnostics: Vec::new(),
                                            ..Default::default()
                                        }.with_experiment(&comp_res));
                                    }
                                    Err(e) => {
                                        let mut r = result_ok(entity, session_id);
                                        r.error = Some(format!("Stepper Error: {e}"));
                                        // rumoca-sim structured error → located
                                        // diagnostics so a solver-lowering failure
                                        // (e.g. an un-lowerable equation) is
                                        // click-to-source like a compile error.
                                        r.compile_diagnostics =
                                            crate::diagnostics_from_sim_error(&e, &stripped_source);
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
                                // `e` is already rumoca's formatted summary
                                // string — render it directly ({:?} would
                                // quote it and escape the newlines).
                                r.error = Some(format!("Compiler Error: {e}"));
                                // Structured, located diagnostics so the
                                // Diagnostics panel can make compile errors
                                // click-to-source (rumoca StrictCompileReport).
                                r.compile_diagnostics =
                                    compiler.compile_diagnostics(&model_name, &doc_uri);
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
                                    if let Ok(comp_res) = compiler.compile_str(&cached.model_name, &stripped_source, &cached.doc_uri) {
                                        if let Ok(mut s) = build_stepper(&comp_res) {
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
                                        if let Ok(mut s) = build_stepper(&comp_res) {
                                            for (name, val) in &inputs { let _ = s.set_input(name, *val); }
                                            cached_models.insert(entity, CachedModel {
                                                model_name: model_name.clone(),
                                                // Reuse the `source` already read from disk
                                                // for this compile — re-reading `model_path`
                                                // here was a second blocking read of bytes we
                                                // hold (CQ-213).
                                                source: Arc::from(source.clone()),
                                                doc_uri: model_path.to_string_lossy().into_owned(),
                                            });

                                            steppers.insert(entity, (session_id, model_name.clone(), s));
                                        }
                                    }
                                    Err(e) => {
                                        let mut r = result_ok(entity, session_id);
                                        // `e` is rumoca's formatted compile summary.
                                        r.error = Some(format!("Initialization Failed: {e}"));
                                        let _ = tx_inner.send(r);
                                        return;
                                    }
                                }
                            }
                        }

                        if let Some((s_id, _, stepper)) = steppers.get_mut(&entity) {
                            if *s_id == session_id {
                                for (name, val) in inputs { let _ = stepper.set_input(&name, val); }
                                // Macro step: integrate the requested `dt` — the
                                // gap between the model's clock and the world's —
                                // as a fixed ladder of micro-steps (A3/A4).
                                let step_err = integrate_macro_step(stepper, dt).err();
                                if let Some(e) = step_err {
                                    let mut r = result_ok(entity, session_id);
                                    r.new_time = stepper.time();
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
fn is_squashable(last: &ModelicaCommand, next: &ModelicaCommand) -> bool {
    match (last, next) {
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
                        if let Ok(comp_res) = compiler.compile_str(&cached.model_name, &stripped_source, &cached.doc_uri) {
                            if let Ok(mut s) = build_stepper(&comp_res) {
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
                    // Same macro-step ladder as the native worker (A3/A4).
                    let step_err = integrate_macro_step(stepper, dt).err();

                    if let Some(e) = step_err {
                        send(ModelicaResult {
                            entity, session_id, new_time: stepper.time(),
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(), error: Some(format!("Solver Error: {e}")),
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
        ModelicaCommand::Compile { entity, session_id, model_name, source, doc_uri, extra_sources, stream: _stream } => {
            // NB: the wasm inline worker runs on the Bevy main thread
            // today and does not publish to a lock-free SimStream.
            // Phase A lands on desktop first; TODO(arch-phase-b) wire
            // the wasm path once the inline worker moves off-thread.
            w.current_sessions.insert(entity, session_id);
            let (stripped_source, input_defaults) = strip_input_defaults(&source);


            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            let compile_outcome = if extra_sources.is_empty() {
                compiler.compile_str(&model_name, &stripped_source, &doc_uri)
            } else {
                compiler.compile_str_multi(&model_name, &stripped_source, &doc_uri, &extra_sources)
            };
            match compile_outcome {
                Ok(comp_res) => {
                    match build_stepper(&comp_res) {
                        Ok(mut stepper) => {
                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            w.cached_models.insert(entity, CachedModel {
                                model_name: model_name.clone(), source: Arc::from(source.clone()),
                                doc_uri: doc_uri.clone(),
                            });

                            w.steppers.insert(entity, (session_id, model_name.clone(), stepper));
                            send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: symbols, error: None,
                                log_message: Some("Compiled successfully.".to_string()),
                                is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: input_names,
                                compiled_model_name: Some(model_name.clone()),
                                loaded_source_root_id: None,
                                compile_diagnostics: Vec::new(),
                                ..Default::default()
                            }.with_experiment(&comp_res));
                        }
                        Err(e) => {
                            send(ModelicaResult {
                                entity, session_id, new_time: 0.0,
                                outputs: Vec::new(),
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {e}")),
                                log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                                detected_input_names: Vec::new(),
                                compile_diagnostics: crate::diagnostics_from_sim_error(&e, &stripped_source),
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
                        entity, session_id, new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Compile Error: {e}")),
                        log_message: None, is_new_model: true, is_parameter_update: false, is_reset: false,
                        detected_input_names: Vec::new(),
                        compile_diagnostics: diags,
                        ..Default::default()
                    });
                }
            }
        }
        ModelicaCommand::Reset { entity, session_id } => {
            w.current_sessions.insert(entity, session_id);

            if let Some(cached) = w.cached_models.get(&entity) {
                let (stripped_source, input_defaults) = strip_input_defaults(&cached.source);
                let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
                match compiler.compile_str(&cached.model_name, &stripped_source, &cached.doc_uri) {
                    Ok(comp_res) => {
                        if let Ok(mut stepper) = build_stepper(&comp_res) {
                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            w.steppers.insert(entity, (session_id, cached.model_name.clone(), stepper));
                            send(reset_ok(
                                entity, session_id, symbols, input_names,
                                "Reset complete.",
                            ));

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
                                detected_symbols: Vec::new(), error: Some(format!("Reset compile error: {e}")),
                                log_message: None, is_new_model: false, is_parameter_update: false, is_reset: true,
                                detected_input_names: Vec::new(),
                                compile_diagnostics: compiler.compile_diagnostics(&cached.model_name, &cached.doc_uri),
                                ..Default::default()
                                });
                                }
                                }
                                } else {
                                w.steppers.remove(&entity);
                                send(reset_ok(
                                entity, session_id, Vec::new(), Vec::new(),
                                "Reset complete (no cached model).",
                                ));
                                }

        }
        ModelicaCommand::UpdateParameters { entity, session_id, model_name, source } => {
            if session_id < *w.current_sessions.get(&entity).unwrap_or(&0) {
                send(result_ok(entity, session_id));
                return;
            }
            w.current_sessions.insert(entity, session_id);
            let (stripped_source, input_defaults) = strip_input_defaults(&source);

            // Re-seat under the model's original session URI (see the threaded
            // handler) so the reused session never holds it under two filenames.
            let doc_uri = w.cached_models
                .get(&entity)
                .map(|c| c.doc_uri.clone())
                .unwrap_or_else(|| model_name.clone());

            let compiler = w.compiler.get_or_insert_with(ModelicaCompiler::new);
            match compiler.compile_str(&model_name, &stripped_source, &doc_uri) {
                Ok(comp_res) => {
                    match build_stepper(&comp_res) {
                        Ok(mut stepper) => {
                            apply_input_defaults_validated(&mut stepper, &input_defaults, "Compile");
                            let input_names: Vec<String> = stepper.input_names().to_vec();
                            let symbols = collect_stepper_observables(&stepper);
                            w.cached_models.insert(entity, CachedModel {
                                model_name: model_name.clone(), source: Arc::from(source.clone()),
                                doc_uri: doc_uri.clone(),
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
                                detected_symbols: Vec::new(), error: Some(format!("Stepper Init Error: {e}")),
                                log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
                                detected_input_names: Vec::new(),
                                compile_diagnostics: crate::diagnostics_from_sim_error(&e, &stripped_source),
                                ..Default::default()
                            });
                        }
                    }
                }
                Err(e) => {
                    send(ModelicaResult {
                        entity, session_id, new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(), error: Some(format!("Re-compile Error: {e}")),
                        log_message: None, is_new_model: false, is_parameter_update: true, is_reset: false,
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
    /// The model's OWN clock — `stepper.time()` as of the last result that
    /// landed. Lags [`Self::target_time`] by at least the in-flight macro step.
    pub current_time: f64,
    /// The **world clock this model is coupled to**, in model-local seconds
    /// (0 at compile/reset). Advanced by exactly one `Time<Fixed>` delta per
    /// unpaused FIXED TICK — never per render frame (A3). The macro step
    /// requested each tick is `target_time − current_time`, so a model that
    /// missed ticks (worker busy, long compile, `rate` burst) catches the time
    /// up instead of losing it forever.
    pub target_time: f64,
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
    /// looked up in [`crate::state::ModelicaDocumentRegistry`]. `DocumentId::default()`
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

/// Tears the model down on the worker when its `ModelicaModel` component goes away, so a
/// despawned entity does not leave a `SimStepper` alive in the worker thread.
/// Registered in `lib.rs` (`.add_observer(worker::on_remove_modelica)`).
pub fn on_remove_modelica(trigger: On<Remove, ModelicaModel>, channels: Res<ModelicaChannels>) {
    let entity = trigger.entity;
    let _ = channels.tx.send(ModelicaCommand::Despawn { entity });
    info!("[modelica] observer: sent Despawn to Modelica for entity {:?}", entity);
}

/// Decide this tick's macro step for one model.
///
/// **The macro-step contract** (A3), factored out as a pure function so it is
/// testable without a worker thread or an `App`:
///
/// * `target_time` — the world clock (model-local), advanced one fixed delta per
///   fixed tick by the caller. NEVER a render-frame quantity.
/// * `current_time` — the model's own clock, from the last worker result.
/// * `in_flight` — a `Step` is already out at the worker for this model.
///
/// Returns the `dt` to request, or `None` for "nothing to do this tick".
///
/// The requested `dt` is the **whole deficit**, clamped to
/// [`MAX_MACRO_STEP_DT`]: a model that fell behind asks for a bigger macro step
/// and closes the gap over the next few ticks, instead of silently dropping the
/// ticks it missed (the old code always sent `Time<Fixed>::delta` and skipped
/// entirely whenever a step was in flight — so at 30 FPS the model ran at half
/// speed, and at `rate = 10` it ran 10× too slow).
///
/// While a step is in flight we do not dispatch another (one macro step per
/// model at a time — the worker owns one `SimStepper` per entity). The deficit
/// is NOT lost: it keeps growing and the next dispatched step carries it.
pub(crate) fn plan_macro_step(target_time: f64, current_time: f64, in_flight: bool) -> Option<f64> {
    if in_flight {
        return None;
    }
    let deficit = target_time - current_time;
    if deficit < MIN_MACRO_STEP_DT {
        // Already at (or, through micro-step rounding, just past) the
        // communication point. Overshoot corrects itself: the deficit goes
        // slightly negative and the next tick's fixed delta absorbs it.
        return None;
    }
    Some(deficit.min(MAX_MACRO_STEP_DT))
}

/// Sends `Step` commands for each active model — **the co-simulation master's
/// macro-step dispatch**.
///
/// Runs in [`FixedUpdate`]. Each live model's clock is driven toward
/// `target_time`, which advances by exactly one `Time<Fixed>` delta per FIXED
/// TICK. Model time is therefore a pure function of the fixed-step clock: it does
/// not depend on the render frame rate, on GPU load, or on window focus.
///
/// Also measures the model-vs-world lag and publishes it to [`CosimLag`] — the
/// only thing in the system that compares the two clocks at all.
pub fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    time: Res<Time<Fixed>>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
    mut lag: ResMut<CosimLag>,
    // Auto-compile request goes out as a core event; the UI relays it to the
    // `CompileModel` command. Core no longer references the UI command.
    mut compile_requests: MessageWriter<crate::CompileRequested>,
) {
    // The FIXED delta — constant (1/`FIXED_HZ`) by construction. `rate` bursts
    // show up as MORE fixed ticks, never as a longer one, so accumulating it
    // per tick is exactly "one tick of world time".
    let fixed_dt = time.delta_secs_f64();

    let mut worst_secs = 0.0_f64;
    let mut worst_entity = None;
    let mut live_models = 0usize;

    for (entity, mut model) in q_models.iter_mut() {
        if model.paused {
            // A paused model's clock is frozen WITH the world's: the target does
            // not advance, so unpausing does not trigger a catch-up burst for
            // time the model was never supposed to simulate.
            continue;
        }

        // First-step path: model has been unpaused (user pressed Run)
        // but no Compile has succeeded yet — the worker has no stepper
        // and a Step would just bounce back as "Click Compile first".
        // Auto-trigger CompileModel instead. The observer flips
        // `is_compiling`/`is_stepping` and bumps `session_id`, so the guard
        // below stops us re-triggering on subsequent ticks; on a successful
        // result the response handler sets `is_compiled = true` and unpauses.
        if !model.is_compiled {
            let doc = model.document;
            let compile_in_flight = model.is_compiling || model.is_stepping;
            if doc != lunco_doc::DocumentId::default() && !compile_in_flight {
                compile_requests.write(crate::CompileRequested {
                    doc,
                    class: if model.model_name.is_empty() {
                        None
                    } else {
                        Some(model.model_name.clone())
                    },
                    force: false,
                    // Compile-on-first-step: preserve whatever resume
                    // intent the model already carries (this path never
                    // arms a new one).
                    resume_after_compile: false,
                });
            }
            // Don't ship a Step this tick either way — let the
            // compile flow run. The model isn't running yet, so its target
            // clock stays put (no phantom catch-up debt accrues while the
            // compile is in flight).
            continue;
        }

        // ── The world clock advances by exactly one FIXED tick ──────────────
        model.target_time += fixed_dt;

        // ── Lag measurement (A3.2) ─────────────────────────────────────────
        live_models += 1;
        let lag_secs = (model.target_time - model.current_time).abs();
        if lag_secs > worst_secs {
            worst_secs = lag_secs;
            worst_entity = Some(entity);
        }

        // ── Macro step to the communication point (A3.1) ────────────────────
        let Some(dt) = plan_macro_step(model.target_time, model.current_time, model.is_stepping)
        else {
            continue;
        };

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

    lag.worst_secs = worst_secs;
    lag.worst_entity = worst_entity;
    lag.models = live_models;

    // Rate-limited divergence alarm. A sustained lag means the forces avian is
    // integrating come from a model state this far in the past — the coupling is
    // no longer a co-simulation, it's an extrapolation.
    if lag.cooldown > 0 {
        lag.cooldown -= 1;
    } else if worst_secs > LAG_WARN_SECS {
        warn!(
            "[cosim] Modelica model clock is {:.3}s behind the fixed-step world clock \
             (entity {:?}, {} live model(s)). The solver cannot keep up with the sim rate; \
             forces are being computed from a stale model state.",
            worst_secs, worst_entity, live_models,
        );
        lag.cooldown = LAG_WARN_COOLDOWN_TICKS;
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
    mut _workbench_state: ResMut<crate::state::WorkbenchState>,
    // Core compile-state (UI-agnostic). Optional so headless cosim tests run
    // without it.
    compile_states: Option<ResMut<lunco_doc_bevy::DocumentDiagnostics>>,
    // Lifecycle messages leave as core events; the reactive UI console observer
    // projects them. Core no longer references the console panel.
    mut notices: MessageWriter<crate::ModelicaNotice>,
    // Live sim samples leave the core handler through this UI-agnostic queue;
    // the reactive UI viz observer (`ui::core_observers::drain_sim_samples_to_viz`)
    // drains it into `lunco_viz`. Core no longer references any viz/plot types.
    mut sample_stream: ResMut<crate::SimSampleStream>,
    runner_res: Option<Res<crate::ModelicaRunnerResource>>,
    source_roots: Option<ResMut<crate::source_roots::SourceRootRegistry>>,
) {
    let mut compile_states = compile_states;
    let mut source_roots = source_roots;
    while let Ok(result) = channels.rx.try_recv() {
        // Source-root load ack: route to the registry and short-
        // circuit before any of the sim-result handling below
        // (which keys on `result.entity` — LoadSourceRoot uses
        // `Entity::PLACEHOLDER`).
        if let Some(root_id) = result.loaded_source_root_id.as_ref() {
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
            // Status-bar projection of this load result is handled by the
            // reactive UI observer of `SourceRootRegistry` — core only sets the
            // registry state above.
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
                        // The live worker path carries `Interval` only; the
                        // `NumberOfIntervals` count flows through the batch
                        // experiments path (compile.rs ModelDefaults builder).
                        number_of_intervals: None,
                        // Parse the annotation's solver string into the typed
                        // choice once here; an unrecognized name falls to
                        // `None` (= backend default) instead of being carried
                        // as a free string. See `lunco_experiments::SolverChoice`.
                        solver: result
                            .experiment_solver
                            .as_deref()
                            .and_then(|s| s.parse().ok()),
                    },
                );
            }
        }

        if result.entity == Entity::PLACEHOLDER {
            let msg = "Simulation worker crashed and restarted.";
            warn!("{msg}");
            notices.write(crate::ModelicaNotice {
                level: crate::NoticeLevel::Error,
                text: msg.to_string(),
            });
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
                    notices.write(crate::ModelicaNotice {
                        level: crate::NoticeLevel::Info,
                        text: format!("[{}] {msg}", model.model_name),
                    });
                }
            }

            // Transition compile state for this entity's document, but only on
            // compile-shaped lifecycle results (new-model / parameter-update /
            // reset) — the same grouping the `is_compiling` and log blocks above
            // use. Plain Step results arrive continuously and must not clobber
            // Ready/Error classifications. `is_reset` MUST be included: a
            // successful reset means the model re-initialised healthy, so it has
            // to reconcile `state` back to `Ready`. Without it, the success
            // branch below still clears the diagnostics list while `state` stays
            // `Error`, leaving the UI stuck on a red "compilation failed" chip
            // with no underlying message.
            let is_compile_result =
                result.is_new_model || result.is_parameter_update || result.is_reset;
            if is_compile_result && !model.document.is_unassigned() {
                let new_state = if result.error.is_some() {
                    lunco_doc::CompileState::Error
                } else {
                    lunco_doc::CompileState::Ready
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
                            lunco_doc::CompileState::Error => {
                                warn!(
                                    "[Modelica] Compile finished with error for `{}` in {}",
                                    model.model_name, human
                                );
                                notices.write(crate::ModelicaNotice {
                                    level: crate::NoticeLevel::Error,
                                    text: format!(
                                        "⏹ Compile FAILED: '{}' in {}",
                                        model.model_name, human
                                    ),
                                });
                            }
                            lunco_doc::CompileState::Ready => {
                                info!(
                                    "[Modelica] Compile finished for `{}` in {}",
                                    model.model_name, human
                                );
                                notices.write(crate::ModelicaNotice {
                                    level: crate::NoticeLevel::Info,
                                    text: format!(
                                        "✓ Compile finished: '{}' in {}",
                                        model.model_name, human
                                    ),
                                });
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
                    // Carry structured located diagnostics when the worker
                    // shipped them (compile failures) so the panel can
                    // render click-to-source rows; empty for solver/reset
                    // errors falls back to the flat `err` string.
                    let diags = if result.compile_diagnostics.is_empty() {
                        vec![lunco_doc::Diagnostic::message_only(err.clone())]
                    } else {
                        result.compile_diagnostics.clone()
                    };
                    cs.set_error(model.document, diags);
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
                notices.write(crate::ModelicaNotice {
                    level: crate::NoticeLevel::Error,
                    text: format!("[{}] {prefix}: {err}", model.model_name),
                });
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
                model.target_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_parameter_update {
                model.current_time = 0.0;
                model.target_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_reset {
                model.current_time = 0.0;
                // The world clock this model is coupled to restarts WITH it —
                // otherwise the fresh model would immediately owe the catch-up
                // path every second the old one had run (A3).
                model.target_time = 0.0;
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

            // CQ-524: only advance the model clock on a genuine step or
            // compile/reset result. Pure acks (LoadSourceRoot → carries
            // `loaded_source_root_id`; worker-panic/error reports → carry
            // `error`) all set `new_time = 0.0`; assigning that would
            // momentarily zero a running sim's clock. An errored step also
            // didn't progress, so leave the clock where it was.
            if result.error.is_none() && result.loaded_source_root_id.is_none() {
                model.current_time = result.new_time;
                model.last_step_time = result.new_time;
            }
            let time_val = model.current_time;

            // Emit this step's observable samples to the reactive UI layer.
            // The core handler no longer knows about plots / `lunco_viz`: it
            // just appends UI-agnostic samples that `ui::core_observers::
            // drain_sim_samples_to_viz` projects into the SignalRegistry (clear
            // on a fresh compile, push every scalar, attach doc-index meta, and
            // reset the default graph). Bounded at the producer so a headless
            // build (no drainer) can't grow the queue without limit.
            if sample_stream.batches.len() < 16_384 {
                let samples: Vec<(String, f64)> = result
                    .outputs
                    .iter()
                    .chain(result.detected_symbols.iter())
                    .map(|(n, v)| (n.clone(), *v))
                    .collect();
                sample_stream.batches.push(crate::SimSampleBatch {
                    entity: result.entity,
                    document: model.document,
                    time: time_val,
                    samples,
                    is_new_model: result.is_new_model,
                    is_parameter_update: result.is_parameter_update,
                });
            }
        }
    }
}

// ===========================================================================
// The macro-step contract (A3/A4/A5)
// ===========================================================================
#[cfg(test)]
mod macro_step_tests {
    use super::*;

    /// Stand-in for the worker: integrate what the worker WOULD integrate for a
    /// requested `dt` — an integer number of fixed micro-steps — and return the
    /// model's new own-clock value. This is the same arithmetic
    /// [`integrate_macro_step`] performs, without a `SimStepper`.
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
        let fixed_dt = lunco_core::SECS_PER_TICK;
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
            assert!((w - world).abs() < 1e-9, "world clock must not depend on latency");
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
        let fixed_dt = lunco_core::SECS_PER_TICK;
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
    /// every peer, clamped, and never zero for a positive `dt` (A4).
    #[test]
    fn micro_step_ladder_is_deterministic_and_clamped() {
        assert_eq!(micro_steps_for(0.0), 0);
        assert_eq!(micro_steps_for(-1.0), 0);
        assert_eq!(micro_steps_for(LIVE_MICRO_DT), 1);
        assert_eq!(micro_steps_for(lunco_core::SECS_PER_TICK), 3);
        assert_eq!(micro_steps_for(2.0 * lunco_core::SECS_PER_TICK), 6);
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
            model_path: PathBuf::new(),
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
}