//! Off-thread Modelica simulation worker + Bevy bridge.
//!
//! `modelica_worker` runs on its own OS thread (it owns a
//! `!Send` `SimulationSession`, so it can't live on the Bevy main loop). The
//! Bevy systems `spawn_modelica_requests` and
//! `handle_modelica_responses` exchange `ModelicaCommand` /
//! `ModelicaResult` messages with it via crossbeam channels.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};

use lunco_assets::modelica_dir;

use crate::ast_extract::{strip_input_defaults_with_report, InputDefaultIssue};
use crate::simulation_session::LiveStepper;
use crate::ModelicaCompiler;
use lunco_experiments::solver;
use lunco_signal::{SimSnapshot, SimStream};

const PREPARED_SOLVE_CACHE_VERSION: u32 = 3;

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
    crate::solver_backends::ensure_builtin_solvers();

    let spec = solver::resolve(&solver::SolverRequest {
        profile,
        // The live path takes no authored override: an `experiment(...)`
        // annotation is an offline knob, and the same reasoning that keeps its
        // tolerance out of this loop keeps its solver out.
        authored: None,
    })?;

    let options = crate::solver_backends::rumoca_options(
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

/// A prepared solve model is reusable only for the exact compiled DAE and the
/// exact parameter override vector that produced it. The DAE identity is the
/// `Arc` identity held by `DaeCompilationResult`; the shared compilation cache
/// deliberately preserves that identity across USD instances.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct PreparedSolveKey {
    dae_identity: usize,
    solver_id: String,
    parameter_overrides: Vec<(String, u64)>,
}

#[derive(Deserialize, Serialize)]
struct PreparedSolveDiskRecord {
    version: u32,
    source_key: u64,
    library_fingerprint: u64,
    parameter_overrides: Vec<(String, u64)>,
    model: rumoca_ir_solve::SolveModel,
}

#[derive(Default)]
struct PreparedSolveCache {
    models: HashMap<PreparedSolveKey, rumoca_ir_solve::SolveModel>,
    #[cfg(not(target_arch = "wasm32"))]
    library_fingerprint: Option<u64>,
    #[cfg(not(target_arch = "wasm32"))]
    persistent_enabled: bool,
}

impl PreparedSolveCache {
    #[cfg(not(target_arch = "wasm32"))]
    fn new() -> Self {
        Self {
            models: HashMap::default(),
            library_fingerprint: None,
            persistent_enabled: true,
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn new() -> Self {
        Self::default()
    }

    fn key(
        comp_res: &rumoca_compile::compile::DaeCompilationResult,
        spec: &solver::SolverSpec,
        parameter_overrides: &[(String, f64)],
    ) -> PreparedSolveKey {
        PreparedSolveKey {
            dae_identity: Arc::as_ptr(&comp_res.dae) as usize,
            solver_id: spec.id.to_string(),
            parameter_overrides: parameter_overrides
                .iter()
                .map(|(name, value)| (name.clone(), value.to_bits()))
                .collect(),
        }
    }

    fn clear(&mut self) {
        self.models.clear();
    }

    fn remove_compiled(&mut self, comp_res: &rumoca_compile::compile::DaeCompilationResult) {
        let dae_identity = Arc::as_ptr(&comp_res.dae) as usize;
        self.models
            .retain(|key, _| key.dae_identity != dae_identity);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disk_path(
        source_key: u64,
        library_fingerprint: u64,
        parameter_overrides: &[(String, u64)],
    ) -> std::path::PathBuf {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        PREPARED_SOLVE_CACHE_VERSION.hash(&mut hasher);
        source_key.hash(&mut hasher);
        library_fingerprint.hash(&mut hasher);
        parameter_overrides.hash(&mut hasher);
        let key = hasher.finish();
        modelica_dir()
            .join("prepared-solve-v3")
            .join(format!("{source_key:016x}-{key:016x}.bin.zst"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn hash_tree(root: &std::path::Path, hasher: &mut impl std::hash::Hasher) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            use std::hash::Hash;
            relative.to_string_lossy().hash(hasher);
            if path.is_dir() {
                Self::hash_tree(&path, hasher);
            } else if let Ok(bytes) = std::fs::read(&path) {
                bytes.hash(hasher);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn compute_library_fingerprint() -> u64 {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        PREPARED_SOLVE_CACHE_VERSION.hash(&mut hasher);
        let model_root = std::env::current_dir()
            .ok()
            .map(|root| root.join("assets/models"));
        if let Some(root) = model_root {
            root.to_string_lossy().hash(&mut hasher);
            Self::hash_tree(&root, &mut hasher);
        }
        if let Some(root) = lunco_assets::msl_source_root_path() {
            root.to_string_lossy().hash(&mut hasher);
            Self::hash_tree(&root, &mut hasher);
        }
        hasher.finish()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn library_fingerprint(&mut self) -> Option<u64> {
        if !self.persistent_enabled {
            return None;
        }
        Some(
            *self
                .library_fingerprint
                .get_or_insert_with(Self::compute_library_fingerprint),
        )
    }

    #[cfg(target_arch = "wasm32")]
    fn library_fingerprint(&mut self) -> Option<u64> {
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disable_persistent(&mut self) {
        self.persistent_enabled = false;
        self.clear();
    }

    #[cfg(target_arch = "wasm32")]
    fn disable_persistent(&mut self) {
        self.clear();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn load_disk(
        &self,
        source_key: u64,
        library_fingerprint: u64,
        parameter_overrides: &[(String, u64)],
    ) -> Option<rumoca_ir_solve::SolveModel> {
        let path = Self::disk_path(source_key, library_fingerprint, parameter_overrides);
        let compressed = std::fs::read(&path).ok()?;
        let bytes = zstd::stream::decode_all(compressed.as_slice()).ok()?;
        let (record, _): (PreparedSolveDiskRecord, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).ok()?;
        if record.version != PREPARED_SOLVE_CACHE_VERSION
            || record.source_key != source_key
            || record.library_fingerprint != library_fingerprint
            || record.parameter_overrides != parameter_overrides
        {
            return None;
        }
        Some(record.model)
    }

    #[cfg(target_arch = "wasm32")]
    fn load_disk(
        &self,
        _source_key: u64,
        _library_fingerprint: u64,
        _parameter_overrides: &[(String, u64)],
    ) -> Option<rumoca_ir_solve::SolveModel> {
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn save_disk(
        &self,
        source_key: u64,
        library_fingerprint: u64,
        parameter_overrides: &[(String, u64)],
        model: &rumoca_ir_solve::SolveModel,
    ) {
        let path = Self::disk_path(source_key, library_fingerprint, parameter_overrides);
        let Some(parent) = path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let record = PreparedSolveDiskRecord {
            version: PREPARED_SOLVE_CACHE_VERSION,
            source_key,
            library_fingerprint,
            parameter_overrides: parameter_overrides.to_vec(),
            model: model.clone(),
        };
        let Ok(bytes) = bincode::serde::encode_to_vec(record, bincode::config::standard()) else {
            return;
        };
        let Ok(compressed) = zstd::stream::encode_all(bytes.as_slice(), 3) else {
            return;
        };
        // Write beside the final path and rename so an interrupted recording
        // can leave at most an ignored .tmp file, never a partial cache hit.
        let tmp = path.with_extension("bin.zst.tmp");
        if std::fs::write(&tmp, compressed).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn save_disk(
        &self,
        _source_key: u64,
        _library_fingerprint: u64,
        _parameter_overrides: &[(String, u64)],
        _model: &rumoca_ir_solve::SolveModel,
    ) {
    }
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

fn build_stepper(
    comp_res: &rumoca_compile::compile::DaeCompilationResult,
    profile: solver::RuntimeProfile,
    parameter_overrides: &[(String, f64)],
    source_key: u64,
    prepared: &mut PreparedSolveCache,
) -> Result<LiveStepper, rumoca_sim::SimulationDiagnosticError> {
    // USD property traversal is not an ordering contract. Canonicalize before
    // lowering and before forming cache keys so two fresh recorder processes
    // see the same instance parameter set even when the upstream map was
    // enumerated in a different order.
    let parameter_overrides = canonical_parameter_overrides(parameter_overrides);
    let (spec, mut opts) = live_stepper_options(profile).map_err(|e| {
        rumoca_sim::SimulationDiagnosticError::Solver(format!("solver selection failed: {e}"))
    })?;
    // Parameter overrides must enter Rumoca's lowering boundary. That is where
    // parameter dependents and initial-equation states are recomputed. Mutating
    // `Dae.variables.parameters[*].start` after compilation leaves the already
    // compiled initialization vector stale, which turns authored release values
    // into a physically plausible but wrong startup impulse.
    opts.param_overrides = parameter_overrides.clone();
    let key = PreparedSolveCache::key(comp_res, &spec, &parameter_overrides);
    if !prepared.models.contains_key(&key) {
        let override_key: Vec<(String, u64)> = parameter_overrides
            .iter()
            .map(|(name, value)| (name.clone(), value.to_bits()))
            .collect();
        let model = if let Some(library_fingerprint) = prepared.library_fingerprint() {
            if let Some(model) = prepared.load_disk(source_key, library_fingerprint, &override_key)
            {
                bevy::log::info!(
                    "[modelica-runtime] loaded prepared solver IR for `{}`: cache=disk-hit",
                    spec.id,
                );
                Some(model)
            } else {
                None
            }
        } else {
            None
        };
        let model = if let Some(model) = model {
            model
        } else {
            let lower_started = web_time::Instant::now();
            let model = crate::simulation_session::lower_for_live(&comp_res.dae, &opts)?;
            let lower_elapsed = lower_started.elapsed();
            bevy::log::info!(
                "[modelica-runtime] prepared solver IR for `{}`: lower={lower_elapsed:?} cache=miss",
                spec.id,
            );
            if let Some(library_fingerprint) = prepared.library_fingerprint() {
                prepared.save_disk(source_key, library_fingerprint, &override_key, &model);
            }
            model
        };
        prepared.models.insert(key.clone(), model);
    } else {
        bevy::log::info!(
            "[modelica-runtime] reused prepared solver IR for `{}`: cache=hit",
            spec.id,
        );
    }
    let model = prepared
        .models
        .get(&key)
        .expect("prepared solver model inserted or found above");
    crate::simulation_session::live_from_solve_model(model, &spec, opts)
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
/// fencing stale results. The worker owns all `SimulationSession` instances, keyed by entity.
///
/// Derives `Serialize`/`Deserialize` so the wasm Web Worker transport can ship
/// commands over `postMessage`. The `Compile.stream` field carries an
/// `Arc<ArcSwap<…>>` that can't cross a worker boundary; it's `#[serde(skip)]`
/// — wasm builds always use the `outputs`-via-result path instead of the
/// shared snapshot fast-path. Native still uses the shared snapshot
/// in-process and never touches serde here.
#[derive(Serialize, Deserialize)]
pub enum ModelicaCommand {
    /// Advance simulation by one master-selected communication interval. Sent
    /// from `spawn_modelica_requests` only at a fixed simulation tick when the
    /// participant reaches its declared communication point.
    Step {
        entity: Entity,
        session_id: u64,
        /// Monotonic communication-point sequence within this model session.
        /// The main thread uses it to reject duplicate, reordered, or late
        /// results instead of treating every same-session message as the next
        /// step.
        step_id: u64,
        /// FMI-CS-style communication interval selected by the master.
        /// `dt` is retained as the solver interval; these endpoints are the
        /// authoritative transaction identity and are validated by the
        /// worker before integration.
        start_time: f64,
        stop_time: f64,
        model_name: String,
        inputs: Vec<(String, f64)>,
        dt: f64,
    },
    /// Compile Modelica source code into a DAE and create a new SimulationSession.
    ///
    /// The compiled artifact (`DaeCompilationResult`) is cached per entity so
    /// Reset and Step auto-init rebuild a fresh stepper from it WITHOUT
    /// recompiling — see [`CachedModel`].
    Compile {
        entity: Entity,
        session_id: u64,
        model_name: String,
        source: String,
        /// Does this model drive a client-predicted body — i.e. did its program
        /// prim declare `lunco:program:realtimeSafe`?
        ///
        /// Carried on the command because the worker thread has no ECS: it is
        /// the prediction fact solver selection needs. DECLARED upstream, never
        /// inferred from the compiled DAE: solvability and backend lowering are
        /// owned by the selected solver.
        realtime_safe: bool,
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
        /// USD-authored values for Modelica parameters. Parameters are
        /// compile-time values, so the worker passes these to Rumoca's
        /// simulation lowering before building the live stepper.
        #[serde(default)]
        parameter_overrides: Vec<(String, f64)>,
        /// Lock-free snapshot handle the worker publishes into after
        /// every successful Step when the command stays in this address space.
        /// `None` = result-stream path; main thread still receives per-sample
        /// data via `ModelicaResult.outputs` and pushes it into
        /// `SignalRegistry`. When `Some`, the worker updates the
        /// stream directly and the main-thread handler can skip the
        /// per-sample push loop.
        ///
        /// Skipped by serde: the `Arc<ArcSwap<_>>` only makes sense
        /// inside one address space. On wasm (Web Worker transport)
        /// this is always serialized as `None`, using the
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
    /// Reset the stepper to initial conditions. Rebuilds a fresh stepper from
    /// the cached compiled artifact — instant, no recompilation — unless a
    /// `LoadSourceRoot` has landed since the artifact was built, in which case
    /// the cached source is recompiled first (see [`rebuild_from_cache`]).
    Reset { entity: Entity, session_id: u64 },
    /// Remove the stepper, the cached compiled model, and (native only) the
    /// entity's on-disk compile temp dirs (entity despawned).
    Despawn { entity: Entity },
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
    /// warm-bundle). Other COMPILE-lane commands queue behind it; Steps of
    /// already-live models jump ahead via the step lane (see
    /// [`modelica_worker`]'s scheduling contract).
    ///
    /// Bumps the worker's library generation: a newly-loaded root can change
    /// what cached sources resolve to, so every cached compiled artifact is
    /// invalidated and the next Reset / Step auto-init recompiles instead of
    /// reusing it.
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
    /// Present on every response to a `Step`, including a step error. Lifecycle
    /// and source-root responses leave it absent.
    #[serde(default)]
    pub step_id: Option<u64>,
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
            step_id: None,
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
    fn with_experiment(mut self, comp_res: &rumoca_compile::compile::DaeCompilationResult) -> Self {
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
/// M3: this holds the ACTUAL compiled artifact, not just the source. rumoca's
/// `DaeCompilationResult` is `Clone` and carries the DAE behind an `Arc`; a
/// fresh `SimulationSession` is built from `&dae` alone
/// ([`crate::simulation_session::live`]), so Reset and Step auto-init rebuild
/// steppers from `compiled` WITHOUT touching the compiler — instant, where the
/// old source-only cache recompiled for seconds on MSL-heavy models.
///
/// The artifact is valid only for what it was built from: `unit_hash` keys the
/// assembled [`CompileUnit`] (stripped primary + extras + model name + session
/// URI), and `library_gen` records the worker's library generation (bumped on
/// every `LoadSourceRoot`). [`rebuild_from_cache`] recompiles — and refreshes
/// this entry — when either no longer matches.
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

/// Stable cross-process identity for the solve-IR cache. It includes the
/// complete authored source set and the worker library generation; unlike a
/// Bevy entity or a Rumoca source id it is identical in a fresh recorder
/// process.
fn prepared_unit_hash(
    model_name: &str,
    doc_uri: &str,
    unit: &CompileUnit,
    library_gen: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    compile_unit_hash(model_name, doc_uri, unit).hash(&mut h);
    library_gen.hash(&mut h);
    h.finish()
}

/// Stable key for a compiled DAE that can be shared by multiple scene
/// participants.  The document URI is intentionally absent: it identifies
/// the authoring document, not the Modelica equations.  Two USD instances
/// with the same model name, assembled source, and library generation have
/// the same compile artifact; their parameter bindings and live steppers are
/// still created independently below.
fn shared_compile_hash(model_name: &str, unit: &CompileUnit, library_gen: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    model_name.hash(&mut h);
    unit.source.hash(&mut h);
    for (uri, text) in &unit.extras {
        uri.hash(&mut h);
        text.hash(&mut h);
    }
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
    let key = shared_compile_hash(model_name, unit, library_gen);
    if let Some(compiled) = artifacts.get(&key) {
        log::debug!("[worker] shared Modelica artifact hit for `{model_name}` (key={key})");
        return Ok(compiled.clone());
    }
    let outcome = if unit.extras.is_empty() {
        compiler.compile_str(model_name, &unit.source, doc_uri)
    } else {
        compiler.compile_str_multi(model_name, &unit.source, doc_uri, &unit.extras)
    };
    if let Ok(compiled) = &outcome {
        artifacts.insert(key, compiled.clone());
    }
    outcome
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
struct CacheRebuild {
    model_name: String,
    doc_uri: String,
    /// The instance values that must be supplied to Rumoca when the cached DAE
    /// is lowered into a fresh live stepper.
    parameter_overrides: Vec<(String, f64)>,
    /// Stable source-set identity used by the cross-process solve-IR cache.
    unit_key: u64,
    /// Assembled from the cached source set — carries the `input_defaults`
    /// to re-seed and the stripped primary for error diagnostics.
    unit: CompileUnit,
    /// `Ok` = artifact to build the stepper from (reused or freshly
    /// recompiled); `Err` = rumoca's formatted compile summary.
    outcome: Result<Box<rumoca_compile::compile::DaeCompilationResult>, String>,
    /// True when the cached artifact was reused as-is (no compiler touched).
    reused: bool,
}

/// **The M3 chokepoint**: produce the compiled artifact for an entity's cached
/// source set, reusing [`CachedModel::compiled`] when nothing it was built
/// from has changed ([`artifact_still_valid`]) and recompiling + refreshing
/// the cache entry otherwise. All four rebuild sites — Reset and Step
/// auto-init, native and wasm — route through here, so none can drift back to
/// per-Reset recompiles (or drop the cached extras).
///
/// Returns `None` when the entity has no cached model at all.
fn rebuild_from_cache(
    cached_models: &mut HashMap<Entity, CachedModel>,
    artifacts: &mut HashMap<u64, Box<rumoca_compile::compile::DaeCompilationResult>>,
    compiler: &mut Option<ModelicaCompiler>,
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
    if let Some(c) = compiler.as_ref() {
        unit.merge_library_defaults(c.library_input_defaults());
    }
    if artifact_still_valid(cached_hash, cached_gen, hash, library_gen) {
        let compiled = cached_models
            .get(&entity)
            .expect("checked above")
            .compiled
            .clone();
        return Some(CacheRebuild {
            model_name,
            doc_uri,
            parameter_overrides,
            unit_key,
            unit,
            outcome: Ok(compiled),
            reused: true,
        });
    }
    let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
    let outcome = compile_shared(
        artifacts,
        compiler,
        &model_name,
        &unit,
        &doc_uri,
        library_gen,
    );
    if let Ok(comp_res) = &outcome {
        if let Some(c) = cached_models.get_mut(&entity) {
            c.compiled = comp_res.clone();
            c.unit_hash = hash;
            c.library_gen = library_gen;
        }
    }
    Some(CacheRebuild {
        model_name,
        doc_uri,
        parameter_overrides,
        unit_key,
        unit,
        outcome,
        reused: false,
    })
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
/// Derived from [`lunco_core::SECS_PER_TICK`], so the model's stop-time lattice
/// is a pure function of the FIXED-STEP clock — never of the render frame, GPU
/// load, or window focus (A3).
const LIVE_MICRO_DT: f64 = lunco_core::SECS_PER_TICK / 3.0;

/// Hard cap on micro-steps integrated inside ONE `Step` command.
///
/// A large deficit can still occur after an intentional pause or a rate change,
/// so the requested macro step is capped at this many micro-steps (~0.178 s).
/// A worker stall is handled differently: the fixed-step coupling barrier holds
/// physics and does not advance `target_time` while a step is in flight. The
/// clamp therefore bounds an explicit authored time jump, not accumulated worker
/// debt.
const MAX_MICRO_STEPS_PER_MACRO: u32 = 32;

/// Largest `dt` one `Step` command may carry (= the clamp above, in seconds).
/// `spawn_modelica_requests` clamps the requested catch-up to this; the worker
/// clamps again ([`micro_steps_for`]) so a hand-built `Step` can't blow past it.
pub const MAX_MACRO_STEP_DT: f64 = LIVE_MICRO_DT * MAX_MICRO_STEPS_PER_MACRO as f64;

/// Below this deficit we don't dispatch a `Step` at all — the model is already
/// at the communication point (within half a micro-step) and a sub-micro-step
/// `dt` would just round to a full micro-step and overshoot.
const MIN_MACRO_STEP_DT: f64 = LIVE_MICRO_DT * 0.5;

/// Default communication period for a live Modelica participant.
///
/// Modelica is a continuous-time participant, but it is not a 60 Hz render
/// callback.  The master algorithm samples its inputs and publishes its
/// outputs at declared communication points; between points the last output is
/// held.  Ten Hz is the documented semantic default for a participant that
/// does not author a different period.  A participant that must exchange state
/// on every physics tick authors one fixed-tick period explicitly.
pub const DEFAULT_COMMUNICATION_PERIOD_SECS: f64 = 0.1;

const COMMUNICATION_EPS: f64 = 1e-9;
const COMMUNICATION_TIME_EPS: f64 = 1e-8;

/// Validate a live Modelica communication period against the master clock.
/// This is shared by USD projection and the runtime participant so a value
/// cannot be accepted at load time and rejected under a different rule on the
/// first tick.
///
/// The master admits at most one asynchronous transaction for a participant in
/// one `FixedUpdate`. Communication points therefore must lie on the master
/// fixed-tick lattice. A sub-tick period would be legal for a standalone FMI
/// master that runs an inner loop, but this master does not have that second
/// clock; accepting it would silently make the participant run slow.
pub fn validate_communication_period_secs(value: f64) -> Result<f64, String> {
    if !value.is_finite() || !(lunco_core::SECS_PER_TICK..=MAX_MACRO_STEP_DT).contains(&value) {
        return Err(format!(
            "invalid Modelica communication period {value:?}; expected a finite value in [{:.9}, {MAX_MACRO_STEP_DT:.9}]s",
            lunco_core::SECS_PER_TICK
        ));
    }
    let fixed_ticks = (value / lunco_core::SECS_PER_TICK).round();
    let represented = fixed_ticks * lunco_core::SECS_PER_TICK;
    if fixed_ticks < 1.0 || (represented - value).abs() > COMMUNICATION_EPS {
        return Err(format!(
            "invalid Modelica communication period {value:?}; it must be an integer multiple of the master fixed tick {:.9}s",
            lunco_core::SECS_PER_TICK
        ));
    }
    Ok(value)
}

/// Resolve the authored communication-period opinion shared by every USD
/// Modelica projection. An omitted opinion is the documented schema default;
/// an explicit missing or invalid value is an authoring error.
pub fn resolve_communication_period_secs(
    authored: bool,
    value: Option<f64>,
) -> Result<f64, String> {
    if !authored {
        return Ok(DEFAULT_COMMUNICATION_PERIOD_SECS);
    }
    value
        .ok_or_else(|| "not a valid authored real value".to_string())
        .and_then(validate_communication_period_secs)
}

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
/// The ONE integration loop for the live path — native worker and wasm inline
/// worker both call it, so the two `#[cfg]` twins cannot drift on step policy.
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

/// Build the terminal response for a command that panicked inside the solver
/// worker. The response must retain the command's lifecycle shape: a Compile
/// panic closes compilation, a Step panic closes its exact transaction, and a
/// source-root panic resolves the root load. A placeholder-only response
/// cannot clear any of those state machines.
pub fn panic_result_for_command(cmd: &ModelicaCommand, message: &str) -> ModelicaResult {
    let mut result = ModelicaResult {
        error: Some(format!("Modelica worker panic: {message}")),
        log_message: Some("Modelica worker recovered after a command panic".to_string()),
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
            let (line, col) = crate::document::core::byte_offset_to_line_col(source, *byte_offset);
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

/// M8 — the worker's two-lane scheduler, on ONE thread.
///
/// The `SimulationSession`s are `!Send` and the rumoca `Session` is owned by
/// the same thread, so a genuine compile-thread/step-thread split would have
/// to move one of them across threads — not available. The alternative the
/// architecture does support is PRIORITY scheduling: commands are queued into
/// two lanes and every runnable Step is processed before the next queued
/// compile-shaped command, so a slow compile (or a 10-60 s `LoadSourceRoot`)
/// delays other live models' Steps by at most the command currently executing,
/// never by the whole queue.
///
/// Lanes:
/// * **step lane** — `Step` for an entity with NO pending compile-lane
///   command. Drained completely at the top of every scheduling round.
/// * **compile lane** — everything else (`Compile`, `UpdateParameters`,
///   `Reset`, `Despawn`, `LoadSourceRoot`), strictly FIFO, ONE per round.
///
/// **The per-entity ordering guarantee is preserved**: a `Step` whose entity
/// has any command pending in the compile lane is appended to the compile lane
/// instead (at its arrival position), so "Compile then Step sees the compiled
/// model" (`source_roots.rs` relies on the compile lane's FIFO for
/// LoadSourceRoot → Compile the same way) still holds command-by-command for
/// each entity. Only OTHER entities' steps jump the queue.
#[cfg(not(target_arch = "wasm32"))]
fn enqueue_command(
    cmd: ModelicaCommand,
    compile_lane: &mut VecDeque<ModelicaCommand>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &Sender<ModelicaResult>,
) {
    let is_step = matches!(cmd, ModelicaCommand::Step { .. });
    if is_step {
        let entity = cmd_entity(&cmd);
        let blocked = compile_lane.iter().any(|c| cmd_entity(c) == entity);
        if !blocked {
            step_lane.push_back(cmd);
            return;
        }
        // Fall through: this entity has pending compile-lane work, so its Step
        // takes the FIFO slot behind it. (`is_squashable` is false for Step,
        // so the squash below never collapses it.)
    }
    // The setpoint squash, unchanged in meaning: consecutive
    // Compile/UpdateParameters for the same entity+session collapse to the
    // latest, acking the dropped one (see `is_squashable`).
    if let Some(last) = compile_lane.back_mut() {
        if is_squashable(last, &cmd) && cmd_session(last) == cmd_session(&cmd) {
            let _ = tx.send(result_ok(cmd_entity(last), cmd_session(last)));
            *last = cmd;
            return;
        }
    }
    compile_lane.push_back(cmd);
}

/// After the compile lane's front command has been taken for execution, hoist
/// every deferred `Step` that is no longer behind compile-lane work for its
/// entity back into the step lane (in order), so it runs next round instead of
/// trickling out one-per-round behind unrelated compiles.
#[cfg(not(target_arch = "wasm32"))]
fn promote_unblocked_steps(
    compile_lane: &mut VecDeque<ModelicaCommand>,
    step_lane: &mut VecDeque<ModelicaCommand>,
) {
    let mut i = 0;
    while i < compile_lane.len() {
        if matches!(compile_lane[i], ModelicaCommand::Step { .. }) {
            let entity = cmd_entity(&compile_lane[i]);
            let blocked = compile_lane.iter().take(i).any(|c| cmd_entity(c) == entity);
            if !blocked {
                let cmd = compile_lane.remove(i).expect("index checked");
                step_lane.push_back(cmd);
                continue;
            }
        }
        i += 1;
    }
}

/// M11 — prune this entity index's on-disk compile temp dirs
/// (`modelica_dir()/<index>_<generation>/model.mo`).
///
/// Bevy reuses entity indices across generations, so `<index>_<old-gen>` can
/// never be stepped again once a newer generation writes its dir — and a
/// despawned entity's dir is dead outright. Called with `keep = Some(dir)`
/// when a Compile writes generation `dir`, and `keep = None` on Despawn.
/// Only names of the exact `<index>_<digits>` shape are touched.
#[cfg(not(target_arch = "wasm32"))]
fn prune_entity_temp_dirs(
    base: &std::path::Path,
    entity_index: impl std::fmt::Display,
    keep: Option<&str>,
) {
    let prefix = format!("{entity_index}_");
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if keep == Some(name) {
            continue;
        }
        if entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The background worker that owns the !Send SimulationSessions and the
/// per-entity compiled-artifact cache, scheduling commands over the two-lane
/// policy documented on [`enqueue_command`].
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
    // Cross-entity cache: identical Modelica source gets one rumoca DAE even
    // when USD instantiates it more than once. Parameters and steppers remain
    // per entity, so this changes startup cost without coupling simulations.
    let mut compiled_artifacts: HashMap<u64, Box<rumoca_compile::compile::DaeCompilationResult>> =
        HashMap::default();
    // DAE compilation and solve-IR preparation are separate caches. The latter
    // is keyed by the shared DAE Arc plus authored overrides so two USD
    // instances do not lower identical networks twice during scene startup.
    let mut prepared_solve_cache = PreparedSolveCache::new();
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

    // M3: cached compiled artifacts are valid only for the library set they
    // were compiled against — every LoadSourceRoot bumps this and thereby
    // invalidates all of them (see `CachedModel::library_gen`).
    let mut library_gen: u64 = 0;
    // M8: the two scheduling lanes — see `enqueue_command` for the contract.
    let mut compile_lane: VecDeque<ModelicaCommand> = VecDeque::new();
    let mut step_lane: VecDeque<ModelicaCommand> = VecDeque::new();

    loop {
        // Block only when idle; otherwise just soak up whatever has arrived
        // since the last command, so Steps that landed during a long compile
        // are scheduled ahead of older queued compiles.
        if compile_lane.is_empty() && step_lane.is_empty() {
            match rx.recv() {
                Ok(cmd) => enqueue_command(cmd, &mut compile_lane, &mut step_lane, &tx),
                Err(_) => return,
            }
        }
        while let Ok(cmd) = rx.try_recv() {
            enqueue_command(cmd, &mut compile_lane, &mut step_lane, &tx);
        }

        // One scheduling round: every runnable Step, then ONE compile-lane
        // command. Steps deferred behind their own entity's compile are
        // hoisted back out once that compile has been taken for execution.
        let mut to_process: Vec<ModelicaCommand> = step_lane.drain(..).collect();
        if let Some(cmd) = compile_lane.pop_front() {
            to_process.push(cmd);
            promote_unblocked_steps(&mut compile_lane, &mut step_lane);
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

                        // M3: rebuild the stepper from the cached compiled
                        // artifact — instant unless a LoadSourceRoot has
                        // invalidated it, in which case this recompiles the
                        // cached source set (and refreshes the cache).
                        if let Some(rb) = rebuild_from_cache(
                            &mut cached_models,
                            &mut compiled_artifacts,
                            &mut compiler,
                            entity,
                            library_gen,
                        ) {
                            match rb.outcome {
                                Ok(comp_res) => {
                                    match build_stepper(
                                        &comp_res,
                                        profile_for(entity, &realtime_models),
                                        &rb.parameter_overrides,
                                        rb.unit_key,
                                        &mut prepared_solve_cache,
                                    ) {
                                        Ok(mut stepper) => {
                                            apply_input_defaults_validated(
                                                &mut stepper,
                                                &rb.unit.input_defaults,
                                                "Init",
                                            );
                                            let input_names: Vec<String> =
                                                stepper.input_names().to_vec();
                                            let symbols = collect_stepper_observables(&stepper);
                                            steppers.insert(
                                                entity,
                                                (session_id, rb.model_name.clone(), stepper),
                                            );
                                            let _ = tx_inner.send(reset_ok(
                                                entity,
                                                session_id,
                                                symbols,
                                                input_names,
                                                if rb.reused {
                                                    "Reset complete."
                                                } else {
                                                    "Reset complete (recompiled: library set changed)."
                                                },
                                            ));
                                        }
                                        Err(e) => {
                                            let mut r = result_ok(entity, session_id);
                                            r.error = Some(format!("Stepper Init Error: {e}"));
                                            // rumoca-sim structured error → located
                                            // diagnostics (click-to-source for solver
                                            // lowering failures).
                                            r.compile_diagnostics =
                                                crate::diagnostics_from_sim_error(
                                                    &e,
                                                    &rb.unit.source,
                                                );
                                            r.is_reset = true;
                                            let _ = tx_inner.send(r);
                                        }
                                    }
                                }
                                Err(e) => {
                                    let mut r = result_ok(entity, session_id);
                                    // `e` is rumoca's formatted compile summary string.
                                    r.error = Some(format!("Reset compile error: {e}"));
                                    r.compile_diagnostics = compiler
                                        .get_or_insert_with(ModelicaCompiler::new)
                                        .compile_diagnostics(&rb.model_name, &rb.doc_uri);
                                    r.is_reset = true;
                                    let _ = tx_inner.send(r);
                                }
                            }
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
                        let mut unit = assemble_compile_unit(&source, Vec::new());

                        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
                        unit.merge_library_defaults(compiler.library_input_defaults());
                        match compile_shared(
                            &mut compiled_artifacts,
                            compiler,
                            &model_name,
                            &unit,
                            &doc_uri,
                            library_gen,
                        ) {
                            Ok(comp_res) => match build_stepper(
                                &comp_res,
                                profile_for(entity, &realtime_models),
                                &[],
                                prepared_unit_hash(&model_name, &doc_uri, &unit, library_gen),
                                &mut prepared_solve_cache,
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
                                    cached_models.insert(
                                        entity,
                                        CachedModel {
                                            model_name: model_name.clone(),
                                            source: Arc::from(source),
                                            // UpdateParameters compiles the primary alone
                                            // (parameter substitution rewrites one doc),
                                            // matching the compile above.
                                            extra_sources: Vec::new(),
                                            parameter_overrides: Vec::new(),
                                            doc_uri: doc_uri.clone(),
                                            compiled: comp_res.clone(),
                                            unit_hash,
                                            library_gen,
                                        },
                                    );
                                    steppers
                                        .insert(entity, (session_id, model_name.clone(), stepper));
                                    let _ = tx_inner.send(ModelicaResult {
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
                                    let mut r = result_ok(entity, session_id);
                                    r.error = Some(format!("Stepper Init Error: {e}"));
                                    r.compile_diagnostics =
                                        crate::diagnostics_from_sim_error(&e, &unit.source);
                                    r.is_parameter_update = true;
                                    let _ = tx_inner.send(r);
                                }
                            },
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

                        // Keep the raw sibling docs for the cache: an
                        // invalidated-artifact recompile (Reset / auto-init
                        // after a LoadSourceRoot) must replay the SAME source
                        // set this compile used.
                        let raw_extras = extra_sources.clone();
                        // Strip input defaults (primary AND extras) so they
                        // become real runtime slots
                        let mut unit = assemble_compile_unit(&source, extra_sources);

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
                        unit.merge_library_defaults(compiler.library_input_defaults());
                        bevy::log::debug!(
                            "[worker] calling compile_str for `{}` ({} bytes)",
                            model_name,
                            unit.source.len(),
                        );
                        let t_compile = web_time::Instant::now();
                        let _compile_outcome = compile_shared(
                            &mut compiled_artifacts,
                            compiler,
                            &model_name,
                            &unit,
                            &doc_uri,
                            library_gen,
                        );
                        bevy::log::debug!(
                            "[worker] compile_str returned for `{}` in {:.2}s ({})",
                            model_name,
                            t_compile.elapsed().as_secs_f64(),
                            if _compile_outcome.is_ok() {
                                "OK"
                            } else {
                                "ERR"
                            },
                        );
                        match _compile_outcome {
                            Ok(comp_res) => {
                                let stepper_result = build_stepper(
                                    &comp_res,
                                    profile_for(entity, &realtime_models),
                                    &parameter_overrides,
                                    prepared_unit_hash(&model_name, &doc_uri, &unit, library_gen),
                                    &mut prepared_solve_cache,
                                );
                                match stepper_result {
                                    Ok(mut stepper) => {
                                        // Set input defaults via set_input so they're runtime-changeable
                                        apply_input_defaults_validated(
                                            &mut stepper,
                                            &unit.input_defaults,
                                            "Compile",
                                        );
                                        let input_names: Vec<String> =
                                            stepper.input_names().to_vec();
                                        let symbols = collect_stepper_observables(&stepper);
                                        let dir_name =
                                            format!("{}_{}", entity.index(), entity.generation());
                                        // M11: a reused entity index leaves
                                        // `<index>_<older-gen>` dirs behind —
                                        // prune them before writing this one.
                                        prune_entity_temp_dirs(
                                            &modelica_dir(),
                                            entity.index(),
                                            Some(&dir_name),
                                        );
                                        let temp_dir = modelica_dir().join(&dir_name);
                                        let _ = std::fs::create_dir_all(&temp_dir);
                                        let temp_path = temp_dir.join("model.mo");
                                        let _ = std::fs::write(&temp_path, &source);

                                        let unit_hash =
                                            compile_unit_hash(&model_name, &doc_uri, &unit);
                                        cached_models.insert(
                                            entity,
                                            CachedModel {
                                                model_name: model_name.clone(),
                                                source: Arc::from(source),
                                                extra_sources: raw_extras,
                                                parameter_overrides,
                                                doc_uri: doc_uri.clone(),
                                                compiled: comp_res.clone(),
                                                unit_hash,
                                                library_gen,
                                            },
                                        );
                                        steppers.insert(
                                            entity,
                                            (session_id, model_name.clone(), stepper),
                                        );
                                        let _ = tx_inner.send(
                                            ModelicaResult {
                                                entity,
                                                session_id,
                                                new_time: 0.0,
                                                outputs: Vec::new(),
                                                detected_symbols: symbols,
                                                error: None,
                                                log_message: Some(format!(
                                                    "Model '{}' compiled.",
                                                    model_name
                                                )),
                                                is_new_model: true,
                                                is_parameter_update: false,
                                                is_reset: false,
                                                detected_input_names: input_names,
                                                compiled_model_name: Some(model_name.clone()),
                                                loaded_source_root_id: None,
                                                // Unresolvable input defaults (non-literal
                                                // bindings) surface even on a green compile —
                                                // that is exactly when they'd otherwise run
                                                // at 0.0 in silence.
                                                compile_diagnostics: unit.default_diagnostics,
                                                ..Default::default()
                                            }
                                            .with_experiment(&comp_res),
                                        );
                                    }
                                    Err(e) => {
                                        let mut r = result_ok(entity, session_id);
                                        r.error = Some(format!("Stepper Error: {e}"));
                                        // rumoca-sim structured error → located
                                        // diagnostics so a solver-lowering failure
                                        // (e.g. an un-lowerable equation) is
                                        // click-to-source like a compile error.
                                        r.compile_diagnostics =
                                            crate::diagnostics_from_sim_error(&e, &unit.source);
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

                        let needs_init = match steppers.get(&entity) {
                            Some((s_id, s_name, _)) => *s_id < session_id || s_name != &model_name,
                            None => true,
                        };

                        if needs_init {
                            // Try the cached compiled artifact first (M3) — a fresh
                            // stepper is built straight from it; a recompile happens
                            // only if a LoadSourceRoot invalidated it. Every failure
                            // here is sent as a result naming its actual cause: the
                            // cached source compiled once already, so a failure now is
                            // a real error, not something to fall through.
                            let cached_name_matches = cached_models
                                .get(&entity)
                                .is_some_and(|c| c.model_name == model_name);
                            if cached_name_matches {
                                if let Some(rb) = rebuild_from_cache(
                                    &mut cached_models,
                                    &mut compiled_artifacts,
                                    &mut compiler,
                                    entity,
                                    library_gen,
                                ) {
                                    match rb.outcome {
                                        Ok(comp_res) => match build_stepper(
                                            &comp_res,
                                            profile_for(entity, &realtime_models),
                                            &rb.parameter_overrides,
                                            rb.unit_key,
                                            &mut prepared_solve_cache,
                                        ) {
                                            Ok(mut s) => {
                                                apply_input_defaults_validated(
                                                    &mut s,
                                                    &rb.unit.input_defaults,
                                                    "Init",
                                                );
                                                // Then apply any user-provided input overrides
                                                for (name, val) in &inputs {
                                                    set_input_or_warn(
                                                        &mut s,
                                                        &mut rejected_inputs,
                                                        entity,
                                                        name,
                                                        *val,
                                                    );
                                                }
                                                steppers
                                                    .insert(entity, (session_id, model_name, s));
                                            }
                                            Err(e) => {
                                                let mut r =
                                                    step_result_ok(entity, session_id, step_id);
                                                r.error = Some(format!(
                                                    "Initialization Failed: stepper init from \
                                                     cached model of `{model_name}`: {e}"
                                                ));
                                                r.compile_diagnostics =
                                                    crate::diagnostics_from_sim_error(
                                                        &e,
                                                        &rb.unit.source,
                                                    );
                                                let _ = tx_inner.send(r);
                                                return;
                                            }
                                        },
                                        Err(e) => {
                                            let mut r = step_result_ok(entity, session_id, step_id);
                                            r.error = Some(format!(
                                                "Initialization Failed: recompile of cached \
                                                 source of `{model_name}`: {e}"
                                            ));
                                            r.compile_diagnostics = compiler
                                                .get_or_insert_with(ModelicaCompiler::new)
                                                .compile_diagnostics(&rb.model_name, &rb.doc_uri);
                                            let _ = tx_inner.send(r);
                                            return;
                                        }
                                    }
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
                        if let Some(cached) = cached_models.remove(&entity) {
                            prepared_solve_cache.remove_compiled(&cached.compiled);
                        }
                        sim_streams.remove(&entity);
                        // M11: this entity's compile temp dirs are dead —
                        // delete every generation of its index.
                        prune_entity_temp_dirs(&modelica_dir(), entity.index(), None);
                    }
                    ModelicaCommand::LoadSourceRoot { id, payload } => {
                        // M3: a new root can change what every cached source
                        // resolves to — invalidate all cached compiled
                        // artifacts (next Reset / auto-init recompiles).
                        library_gen += 1;
                        compiled_artifacts.clear();
                        prepared_solve_cache.disable_persistent();
                        let compiler = compiler.get_or_insert_with(ModelicaCompiler::new);
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
            } else {
                log::debug!("[worker] end: {} took {:?}", cmd_label, elapsed);
            }

            if result.is_err() {
                if let Some(entity) = panic_entity {
                    steppers.remove(&entity);
                    if let Some(cached) = cached_models.remove(&entity) {
                        prepared_solve_cache.remove_compiled(&cached.compiled);
                    }
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
/// SimulationSession internally uses Rc<RefCell<>> which is !Send, but since no threads
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
        self.inner.compiled_artifacts.clear();
        self.inner.prepared_solve_cache.clear();
        // The fresh compiler will see the just-landed MSL bundle — anything
        // compiled against the old (possibly MSL-less) session is stale.
        self.inner.library_gen += 1;
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
    let Ok(cmd) = channels.rx_cmd.try_recv() else {
        return;
    };
    let tx = channels.tx_res.clone();
    let panic_result = panic_result_for_command(
        &cmd,
        "the affected Modelica command was aborted; see the worker log",
    );
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        process_inline_command(&mut worker.inner, cmd, |r| {
            let _ = tx.send(r);
        });
    }));
    if outcome.is_err() {
        worker.inner = InlineWorkerInner::default();
        let _ = tx.send(panic_result);
    }
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
/// `state` carries the per-entity `SimulationSession` map, DAE cache, and the lazy
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
                    if let Some(rb) = rebuild_from_cache(
                        &mut w.cached_models,
                        &mut w.compiled_artifacts,
                        &mut w.compiler,
                        entity,
                        w.library_gen,
                    ) {
                        if let Ok(comp_res) = rb.outcome {
                            if let Ok(mut s) = build_stepper(
                                &comp_res,
                                profile_for(entity, &w.realtime_models),
                                &rb.parameter_overrides,
                                rb.unit_key,
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
                            send(
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
                                }
                                .with_experiment(&comp_res),
                            );
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
                                compile_diagnostics: crate::diagnostics_from_sim_error(
                                    &e,
                                    &unit.source,
                                ),
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
            if let Some(rb) = rebuild_from_cache(
                &mut w.cached_models,
                &mut w.compiled_artifacts,
                &mut w.compiler,
                entity,
                w.library_gen,
            ) {
                match rb.outcome {
                    Ok(comp_res) => {
                        if let Ok(mut stepper) = build_stepper(
                            &comp_res,
                            profile_for(entity, &w.realtime_models),
                            &rb.parameter_overrides,
                            rb.unit_key,
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
                            compile_diagnostics: w
                                .compiler
                                .get_or_insert_with(ModelicaCompiler::new)
                                .compile_diagnostics(&rb.model_name, &rb.doc_uri),
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
                                compile_diagnostics: crate::diagnostics_from_sim_error(
                                    &e,
                                    &unit.source,
                                ),
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
            if let Some(cached) = w.cached_models.remove(&entity) {
                w.prepared_solve_cache.remove_compiled(&cached.compiled);
            }
            w.sim_streams.remove(&entity);
        }
        ModelicaCommand::LoadSourceRoot { id, payload } => {
            // Wasm path: matches the native handler. Worker thread
            // (whether off-main Web Worker or inline) merges the
            // library into its session. Idempotent.
            // M3: invalidate cached compiled artifacts (see native arm).
            w.library_gen += 1;
            w.compiled_artifacts.clear();
            w.prepared_solve_cache.disable_persistent();
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

/// One master-issued Modelica communication transaction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InFlightModelicaStep {
    pub step_id: u64,
    pub start_time: f64,
    pub stop_time: f64,
}

/// Component that attaches a Modelica model to an entity.
///
/// Holds the model name, session ID, parameters, inputs, and observable variables.
/// The `is_stepping` flag prevents duplicate Step commands while waiting for results.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ModelicaModel {
    pub model_name: String,
    /// Canonical source asset/document URI used for user-facing readiness
    /// notifications. Empty for generated or manually constructed models.
    #[reflect(ignore)]
    pub source_uri: String,
    /// The model's OWN clock — `stepper.time()` as of the last result that
    /// landed. Lags [`Self::target_time`] by at least the in-flight macro step.
    pub current_time: f64,
    /// The **world clock this model is coupled to**, in model-local seconds
    /// (0 at compile/reset). Advanced by exactly one `Time<Fixed>` delta per
    /// unpaused FIXED TICK — never per render frame (A3). It is compared with
    /// `next_communication_time`; the worker is asked to integrate only at the
    /// declared communication points, not at every fixed tick.
    pub target_time: f64,
    /// Simulation seconds between Modelica communication points. Inputs are
    /// sampled and outputs are published at these points; the last published
    /// output is held between them. This is the explicit co-simulation
    /// communication policy, not a wall-clock sleep or a render cadence.
    pub communication_period_secs: f64,
    /// Next communication point on the model's local clock.
    #[reflect(ignore)]
    pub next_communication_time: f64,
    pub last_step_time: f64,
    pub session_id: u64,
    pub paused: bool,
    /// Tunable constants (parameter Real ...)
    pub parameters: HashMap<String, f64>,
    /// Control inputs (input Real ...)
    pub inputs: HashMap<String, f64>,
    /// Input names reported by the successfully compiled DAE.
    ///
    /// This is deliberately separate from [`Self::inputs`]. Callers may seed
    /// that map from an authored interface while a source asset is loading, so
    /// it is a write buffer, not evidence that the compiler accepted a port.
    /// The worker owns this set because only its compile result is authoritative
    /// about the live solver interface.
    #[reflect(ignore)]
    pub compiled_input_names: BTreeSet<String>,
    /// All other observable variables (Real soc, etc)
    pub variables: HashMap<String, f64>,
    /// Last compile or solver failure for this model.
    ///
    /// This persists after the one-shot notice is consumed so status
    /// projections remain truthful. Any successful worker response clears it.
    #[reflect(ignore)]
    pub last_error: Option<String>,
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
    /// Exact communication transaction awaiting a worker result. A boolean
    /// alone cannot fence a reordered or duplicate same-session response.
    #[reflect(ignore)]
    pub in_flight_step: Option<InFlightModelicaStep>,
    /// Next communication-point sequence in the current model session.
    #[reflect(ignore)]
    pub next_step_id: u64,
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

impl ModelicaModel {
    /// Validate the authored communication period before the master uses it.
    ///
    /// The schema default is installed by [`Default`] for programmatically
    /// constructed models and by the USD projection for authored programs.
    /// Once a value exists, an invalid value is a configuration error—not a
    /// request to substitute another schedule. This keeps a malformed model
    /// from producing plausible but incorrectly timed results.
    #[inline]
    pub fn validated_communication_period_secs(&self) -> Result<f64, String> {
        validate_communication_period_secs(self.communication_period_secs)
    }

    /// Recompute the next communication point from the model's current clock.
    ///
    /// No defaulting occurs here: callers must surface the returned error and
    /// keep the participant out of the live simulation until its authored
    /// schedule is repaired.
    #[inline]
    pub fn reset_communication_schedule(&mut self) -> Result<(), String> {
        let period = self.validated_communication_period_secs()?;
        self.next_communication_time = self.current_time + period;
        Ok(())
    }
}

impl Default for ModelicaModel {
    fn default() -> Self {
        Self::default_fields()
    }
}

impl ModelicaModel {
    fn default_fields() -> Self {
        Self {
            model_name: String::new(),
            source_uri: String::new(),
            current_time: 0.0,
            target_time: 0.0,
            communication_period_secs: DEFAULT_COMMUNICATION_PERIOD_SECS,
            next_communication_time: DEFAULT_COMMUNICATION_PERIOD_SECS,
            last_step_time: 0.0,
            session_id: 0,
            paused: false,
            parameters: HashMap::new(),
            inputs: HashMap::new(),
            compiled_input_names: BTreeSet::new(),
            variables: HashMap::new(),
            last_error: None,
            document: lunco_doc::DocumentId::default(),
            is_stepping: false,
            in_flight_step: None,
            next_step_id: 1,
            is_compiling: false,
            is_compiled: false,
            compiled_generation: 0,
            pending_generation: 0,
            resume_after_compile: false,
        }
    }
}

/// Tears the model down on the worker when its `ModelicaModel` component goes away, so a
/// despawned entity does not leave a `SimulationSession` alive in the worker thread.
/// Registered in `lib.rs` (`.add_observer(worker::on_remove_modelica)`).
pub fn on_remove_modelica(
    trigger: On<Remove, ModelicaModel>,
    channels: Res<ModelicaChannels>,
    mut sim_registry: ResMut<lunco_signal::SimRegistry>,
) {
    let entity = trigger.entity;
    sim_registry.remove_entity(entity);
    let _ = channels.tx.send(ModelicaCommand::Despawn { entity });
    info!(
        "[modelica] observer: sent Despawn to Modelica for entity {:?}",
        entity
    );
}

/// Decide this tick's macro step for one model.
///
/// **The macro-step contract** (A3), factored out as a pure function so it is
/// testable without a worker thread or an `App`:
///
/// * `target_time` — the world clock (model-local), advanced one fixed delta per
///   fixed tick when no step is in flight. NEVER a render-frame quantity.
/// * `current_time` — the model's own clock, from the last worker result.
/// * `in_flight` — a `Step` is already out at the worker for this model.
///
/// Returns the `dt` to request, or `None` for "nothing to do this tick".
///
/// The requested `dt` is the **whole deficit**, clamped to
/// [`MAX_MACRO_STEP_DT`]. A step in flight returns `None`. A coupled caller
/// holds the shared target clock until the result lands; an independent caller
/// may continue advancing its target clock while the worker runs, preserving
/// the same communication-point semantics without stalling unrelated physics.
///
/// While a step is in flight we do not dispatch another (one macro step per
/// model at a time — the worker owns one `SimulationSession` per entity).
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
    mut fixed_time: ResMut<Time<Fixed>>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
    mut lag: ResMut<CosimLag>,
    participants: Option<Res<lunco_core::SimulationBarrierParticipants>>,
    coupling: Option<ResMut<lunco_core::SimulationBarrier>>,
    faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    // Auto-compile request goes out as a core event; the UI relays it to the
    // `CompileModel` command. Core no longer references the UI command.
    mut compile_requests: MessageWriter<crate::CompileRequested>,
) {
    // The FIXED delta — constant (1/`FIXED_HZ`) by construction. `rate` bursts
    // show up as MORE fixed ticks, never as a longer one, so accumulating it
    // per tick is exactly "one tick of world time".
    let fixed_dt = fixed_time.delta_secs_f64();

    let mut worst_secs = 0.0_f64;
    let mut worst_entity = None;
    let mut live_models = 0usize;
    let mut shared_clock_models = 0usize;
    let mut coupling_held = false;
    let mut faults = faults;

    for (entity, mut model) in q_models.iter_mut() {
        let shared_clock_participant = participants
            .as_deref()
            .is_none_or(|participants| participants.requires_barrier(entity));

        if let Err(error) = model.validated_communication_period_secs() {
            let first_report = model.last_error.as_deref() != Some(error.as_str());
            model.paused = true;
            model.is_compiled = false;
            model.is_stepping = false;
            model.last_error = Some(error.clone());
            if first_report {
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "invalid-modelica-communication-period",
                        Some(entity),
                        model.model_name.clone(),
                        error.clone(),
                    );
                }
                error!("[modelica] {error} for `{}`", model.model_name);
            }
            continue;
        }

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

        // A live Modelica step is a barrier only when the resolved topology says
        // this participant can affect shared state. A telemetry/electrical
        // participant still owns an explicit communication schedule and may be
        // in flight, but its zero-order-held outputs must not stop Avian or the
        // controllers. The unresolved topology state is fail-closed above.
        if model.is_stepping {
            if !shared_clock_participant {
                // The world keeps advancing while this independent participant
                // solves. Account for that world time instead of freezing its
                // target clock at the dispatch instant; the next communication
                // point then represents the actual shared-world timestamp.
                model.target_time += fixed_dt;
            } else {
                coupling_held = true;
                shared_clock_models += 1;
            }
            live_models += 1;
            let lag_secs = (model.target_time - model.current_time).abs();
            if lag_secs > worst_secs {
                worst_secs = lag_secs;
                worst_entity = Some(entity);
            }
            continue;
        }

        // ── The world clock advances by exactly one FIXED tick ──────────────
        model.target_time += fixed_dt;

        // ── Lag measurement (A3.2) ─────────────────────────────────────────
        live_models += 1;
        if shared_clock_participant {
            shared_clock_models += 1;
        }
        let lag_secs = (model.target_time - model.current_time).abs();
        if lag_secs > worst_secs {
            worst_secs = lag_secs;
            worst_entity = Some(entity);
        }

        // ── Macro step to the next declared communication point ─────────────
        // The validation at the top of this iteration established the field's
        // invariant. Read the authored value directly so a malformed value can
        // never be converted into a different schedule here.
        let period = model.communication_period_secs;
        if model.target_time + COMMUNICATION_EPS < model.next_communication_time {
            continue;
        }
        let Some(dt) = plan_macro_step(model.next_communication_time, model.current_time, false)
        else {
            // The solver can land a tiny amount past a point because its
            // micro-step ladder is discrete. Move the schedule to the next
            // point instead of repeatedly dispatching a sub-micro-step.
            model.next_communication_time = model.current_time + period;
            continue;
        };

        let inputs: Vec<(String, f64)> = model
            .inputs
            .iter()
            .map(|(name, val)| (name.clone(), *val))
            .collect();

        let Some(next_step_id) = model.next_step_id.checked_add(1) else {
            let error = format!(
                "Modelica communication-point sequence exhausted for `{}`",
                model.model_name
            );
            model.paused = true;
            model.is_compiled = false;
            model.last_error = Some(error.clone());
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "modelica-step-sequence-exhausted",
                    Some(entity),
                    model.model_name.clone(),
                    error,
                );
            }
            continue;
        };
        let step_id = model.next_step_id;
        let start_time = model.current_time;
        let stop_time = model.next_communication_time;

        let sent = channels.tx.send(ModelicaCommand::Step {
            entity,
            session_id: model.session_id,
            step_id,
            start_time,
            stop_time,
            model_name: model.model_name.clone(),
            inputs,
            dt,
        });
        if sent.is_ok() {
            model.next_step_id = next_step_id;
            model.in_flight_step = Some(InFlightModelicaStep {
                step_id,
                start_time,
                stop_time,
            });
            model.is_stepping = true;
            coupling_held |= shared_clock_participant;
        } else {
            model.paused = true;
            model.is_compiled = false;
            model.last_error = Some("Modelica worker channel closed".to_string());
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "modelica-worker-unavailable",
                    Some(entity),
                    model.model_name.clone(),
                    "the Modelica worker channel closed while dispatching a fixed-step request",
                );
            }
            error!(
                "[modelica] worker channel closed while dispatching a step for `{}`",
                model.model_name
            );
        }
    }

    lag.worst_secs = worst_secs;
    lag.worst_entity = worst_entity;
    lag.models = live_models;

    if let Some(mut coupling) = coupling {
        if faults
            .as_deref()
            .is_some_and(lunco_core::RuntimeFaults::active)
        {
            coupling_held = true;
        }
        coupling.held = coupling_held;
        coupling.active_participants = live_models;
        coupling.shared_clock_participants = shared_clock_models;
        coupling.worst_lag_secs = worst_secs;
        coupling.worst_entity = worst_entity;
    }

    if coupling_held {
        // Bevy's fixed runner may have accumulated several fixed periods for
        // this render frame before this first solver request was dispatched.
        // The current fixed iteration is the only valid one; discard the
        // remaining overstep so the runner cannot execute another tick after
        // the barrier has been raised. `advance_world_clock` pauses the virtual
        // clock before the next frame, which then keeps every FixedUpdate
        // consumer (SimTick, Rhai, controllers, Modelica, and Avian) stopped
        // until the result is released in Update.
        lunco_time::discard_fixed_overstep(&mut fixed_time);
    }

    // Rate-limited divergence alarm. A coupled participant keeps the shared
    // simulation held while it waits; an independent participant is allowed
    // to finish asynchronously while its last validated output remains
    // zero-order-held. The worker is asynchronous in wall-clock execution,
    // never a second simulation-time authority.
    if lag.cooldown > 0 {
        lag.cooldown -= 1;
    } else if worst_secs > LAG_WARN_SECS {
        warn!(
            "[cosim] Modelica participant is {:.3}s behind its communication point \
             (entity {:?}, {} live model(s)); causal membership determines \
             whether the shared simulation waits for the result.",
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
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
    // `workbench_state` was the home of `compilation_error` (B.3
    // phase 4, retired). Param kept in the signature in case other
    // worker paths need it; prefix `_` silences the unused warning.
    mut _workbench_state: ResMut<crate::state::WorkbenchState>,
    // Core compile-state (UI-agnostic). Optional so headless cosim tests run
    // without it.
    compile_states: Option<ResMut<lunco_doc_bevy::DocumentDiagnostics>>,
    // Generated USD networks establish their document before dispatch. Keep
    // the registry available as the authoritative generation source for a
    // late-linked model too; a successful compile must never be marked stale
    // merely because its document generation was assigned after dispatch.
    documents: Option<Res<crate::state::ModelicaDocumentRegistry>>,
    // Lifecycle messages leave as core events; the reactive UI console observer
    // projects them. Core no longer references the console panel.
    mut notices: MessageWriter<crate::ModelicaNotice>,
    // Live sim samples leave the core handler through this UI-agnostic queue;
    // the reactive UI viz observer (`ui::core_observers::drain_sim_samples_to_viz`)
    // drains it into `lunco_viz`. Core no longer references any viz/plot types.
    mut sample_stream: ResMut<crate::SimSampleStream>,
    runner_res: Option<Res<crate::ModelicaRunnerResource>>,
    source_roots: Option<ResMut<crate::source_roots::SourceRootRegistry>>,
    participants: Option<Res<lunco_core::SimulationBarrierParticipants>>,
    coupling: Option<ResMut<lunco_core::SimulationBarrier>>,
    faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    let mut compile_states = compile_states;
    let mut source_roots = source_roots;
    let mut faults = faults;
    while let Ok(result) = channels.rx.try_recv() {
        // Source-root load ack: route to the registry and short-
        // circuit before any of the sim-result handling below
        // (which keys on `result.entity` — LoadSourceRoot uses
        // `Entity::PLACEHOLDER`).
        if let Some(root_id) = result.loaded_source_root_id.as_ref() {
            if let Some(roots) = source_roots.as_deref_mut() {
                if let Some(entry) = roots.roots.get_mut(root_id) {
                    if let Some(err) = result.error.as_ref() {
                        bevy::log::warn!("[source-roots] `{}` load failed: {}", root_id, err,);
                        entry.state = crate::source_roots::LoadState::Failed(err.clone());
                    } else {
                        bevy::log::info!("[source-roots] `{}` is now Ready", root_id,);
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
                        // Resolve the annotation's solver name against the
                        // REGISTRY once here. A name nobody registered falls to
                        // `None` (= let the resolver pick from what the model
                        // needs) rather than being carried as a free string that
                        // some later layer parses differently.
                        solver: result.experiment_solver.as_deref().and_then(|s| {
                            crate::solver_backends::ensure_builtin_solvers();
                            let id = lunco_experiments::SolverId::from(s);
                            lunco_experiments::solver::get(&id).map(|spec| spec.id)
                        }),
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

        let lifecycle_result = result.is_new_model || result.is_parameter_update || result.is_reset;
        if let Ok((_, mut model)) = q_models.get_mut(result.entity) {
            // ALWAYS check session ID before resetting is_stepping
            // Stale results must NOT reset the flag.
            if result.session_id < model.session_id {
                if lifecycle_result {
                    warn!(
                        "[Modelica] ignoring stale lifecycle result for `{}`: result session {} < model session {}",
                        model.model_name, result.session_id, model.session_id
                    );
                }
                continue;
            }
            if result.session_id > model.session_id {
                let detail = format!(
                    "Modelica worker returned future session {} for `{}` (current session {})",
                    result.session_id, model.model_name, model.session_id
                );
                warn!("[Modelica] protocol violation: {detail}");
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "modelica-session-protocol-violation",
                        Some(result.entity),
                        model.model_name.clone(),
                        detail.clone(),
                    );
                }
                model.in_flight_step = None;
                model.is_stepping = false;
                model.paused = true;
                model.is_compiled = false;
                model.last_error = Some(detail);
                continue;
            }

            // A plain result is a response to exactly one master-issued Step.
            // Validate its sequence and communication point before clearing the
            // in-flight flag or touching the model clock. This is the local
            // equivalent of an FMI master's `doStep` transaction fence.
            if !lifecycle_result {
                let Some(in_flight) = model.in_flight_step else {
                    let detail = format!(
                        "Modelica worker returned step {} for `{}` without an in-flight request",
                        result
                            .step_id
                            .map_or_else(|| "<missing>".to_string(), |id| id.to_string()),
                        model.model_name
                    );
                    warn!("[Modelica] protocol violation: {detail}");
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "modelica-step-protocol-violation",
                            Some(result.entity),
                            model.model_name.clone(),
                            detail.clone(),
                        );
                    }
                    model.is_stepping = false;
                    model.paused = true;
                    model.is_compiled = false;
                    model.last_error = Some(detail);
                    continue;
                };
                let identity_matches = result.step_id == Some(in_flight.step_id);
                let endpoint_matches = result.error.is_some()
                    || (result.new_time.is_finite()
                        && communication_times_close(result.new_time, in_flight.stop_time));
                if !identity_matches || !endpoint_matches {
                    let detail = format!(
                        "Modelica worker returned invalid step transaction for `{}`: step_id={:?}, expected {}, new_time={:.12}, expected_stop={:.12}",
                        model.model_name,
                        result.step_id,
                        in_flight.step_id,
                        result.new_time,
                        in_flight.stop_time,
                    );
                    warn!("[Modelica] protocol violation: {detail}");
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "modelica-step-protocol-violation",
                            Some(result.entity),
                            model.model_name.clone(),
                            detail.clone(),
                        );
                    }
                    model.in_flight_step = None;
                    model.is_stepping = false;
                    model.paused = true;
                    model.is_compiled = false;
                    model.last_error = Some(detail);
                    continue;
                }
                model.in_flight_step = None;
            } else {
                // A lifecycle transition supersedes any older transaction only
                // after its session has advanced. It starts a fresh sequence.
                model.in_flight_step = None;
                model.next_step_id = 1;
            }

            if lifecycle_result {
                info!(
                    "[Modelica] applying lifecycle result for `{}`: result session {} model session {} new={} update={} reset={} error={}",
                    model.model_name,
                    result.session_id,
                    model.session_id,
                    result.is_new_model,
                    result.is_parameter_update,
                    result.is_reset,
                    result.error.is_some()
                );
            }

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
                debug!("[Modelica] {msg}");
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
                                debug!(
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
                // A failed in-flight solver step has no valid replacement
                // state. It is a terminal shared-simulation fault, not a
                // reason to release the barrier and let Avian/Rhai continue
                // against stale Modelica outputs. Compile/reset/parameter
                // diagnostics remain scoped to their document lifecycle; only
                // an error on an already-running step reaches this terminal
                // runtime boundary.
                if !lifecycle_result {
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "modelica-step-failed",
                            Some(result.entity),
                            model.model_name.clone(),
                            err.clone(),
                        );
                    }
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
                model.last_error = Some(err.clone());
            } else {
                model.last_error = None;
                if let Some(cs) = compile_states.as_mut() {
                    cs.clear_error(model.document);
                }
            }

            if result.is_new_model {
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
                    model.compiled_generation = if model.pending_generation != 0 {
                        model.pending_generation
                    } else if !model.document.is_unassigned() {
                        documents
                            .as_ref()
                            .and_then(|registry| registry.host(model.document))
                            .map(|host| host.document().generation_owned())
                            .unwrap_or(0)
                    } else {
                        0
                    };
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
                model.compiled_input_names = result.detected_input_names.iter().cloned().collect();
                for name in &result.detected_input_names {
                    model
                        .inputs
                        .entry(name.clone())
                        .or_insert_with(|| *ui_inputs.get(name).unwrap_or(&0.0));
                }
                for (name, val) in ui_inputs {
                    model.inputs.entry(name).or_insert(val);
                }

                model.current_time = 0.0;
                model.target_time = 0.0;
                model.next_communication_time = 0.0;
                model.last_step_time = 0.0;

                info!(
                    "[Modelica] lifecycle state for `{}`: compiled={} compiling={} stepping={} paused={}",
                    model.model_name,
                    model.is_compiled,
                    model.is_compiling,
                    model.is_stepping,
                    model.paused
                );
            } else if result.is_parameter_update {
                model.current_time = 0.0;
                model.target_time = 0.0;
                model.next_communication_time = 0.0;
                model.last_step_time = 0.0;
            } else if result.is_reset {
                model.current_time = 0.0;
                // The world clock this model is coupled to restarts WITH it —
                // otherwise the fresh model would immediately owe the catch-up
                // path every second the old one had run (A3).
                model.target_time = 0.0;
                model.next_communication_time = 0.0;
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
                if let Err(error) = model.reset_communication_schedule() {
                    model.paused = true;
                    model.is_compiled = false;
                    model.last_error = Some(error.clone());
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "invalid-modelica-communication-period",
                            Some(result.entity),
                            model.model_name.clone(),
                            error,
                        );
                    }
                }
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
        } else if lifecycle_result {
            warn!(
                "[Modelica] dropped lifecycle result for missing entity {:?}: session {} new={} update={} reset={}",
                result.entity,
                result.session_id,
                result.is_new_model,
                result.is_parameter_update,
                result.is_reset
            );
        }
    }

    // A result landing is the only release edge for the coupling barrier. The
    // next FixedUpdate may dispatch the following step, but PreUpdate has
    // already observed this release, so the current physics step consumes only
    // the fresh output that just arrived.
    if let Some(mut coupling) = coupling {
        coupling.held = q_models.iter().any(|(entity, model)| {
            let shared_clock_participant = participants
                .as_deref()
                .is_none_or(|participants| participants.requires_barrier(entity));
            shared_clock_participant && !model.paused && model.is_compiled && model.is_stepping
        });
        if faults
            .as_deref()
            .is_some_and(lunco_core::RuntimeFaults::active)
        {
            coupling.held = true;
        }
    }
}

// ===========================================================================
// The macro-step contract
// ===========================================================================
#[cfg(test)]
mod macro_step_tests {
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
    /// every peer, clamped, and never zero for a positive `dt`.
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
        let mut model = ModelicaModel::default();
        model.current_time = 3.25;

        assert_eq!(
            model.validated_communication_period_secs().unwrap(),
            DEFAULT_COMMUNICATION_PERIOD_SECS
        );
        model.reset_communication_schedule().unwrap();
        assert_eq!(
            model.next_communication_time,
            3.25 + DEFAULT_COMMUNICATION_PERIOD_SECS
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
                DEFAULT_COMMUNICATION_PERIOD_SECS
            );
        }
    }

    #[test]
    fn communication_schedule_must_be_representable_and_bounded() {
        assert!(validate_communication_period_secs(DEFAULT_COMMUNICATION_PERIOD_SECS).is_ok());
        assert!(validate_communication_period_secs(lunco_core::SECS_PER_TICK).is_ok());
        assert!(validate_communication_period_secs(3.0 * lunco_core::SECS_PER_TICK).is_ok());
        assert!(validate_communication_period_secs(0.13).is_err());
        assert!(validate_communication_period_secs(MAX_MACRO_STEP_DT + LIVE_MICRO_DT).is_err());
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
            resolve_communication_period_secs(false, None).unwrap(),
            DEFAULT_COMMUNICATION_PERIOD_SECS
        );
        assert!(resolve_communication_period_secs(true, None).is_err());
        assert!(resolve_communication_period_secs(true, Some(0.13)).is_err());
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
// M8 — the two-lane scheduler (see `enqueue_command`)
// ===========================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod lane_tests {
    use super::*;

    fn ent(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("valid test entity index")
    }

    fn step(e: Entity) -> ModelicaCommand {
        ModelicaCommand::Step {
            entity: e,
            session_id: 1,
            step_id: 1,
            start_time: 0.0,
            stop_time: 0.016,
            model_name: "M".into(),
            inputs: Vec::new(),
            dt: 0.016,
        }
    }

    fn compile(e: Entity, session_id: u64) -> ModelicaCommand {
        ModelicaCommand::Compile {
            entity: e,
            session_id,
            model_name: "M".into(),
            source: String::new(),
            realtime_safe: false,
            doc_uri: "doc.mo".into(),
            extra_sources: Vec::new(),
            parameter_overrides: Vec::new(),
            stream: None,
        }
    }

    fn load_root() -> ModelicaCommand {
        ModelicaCommand::LoadSourceRoot {
            id: "Modelica".into(),
            payload: LoadSourceRootPayload::InMemory {
                label: "t".into(),
                files: Vec::new(),
            },
        }
    }

    struct Lanes {
        compile: VecDeque<ModelicaCommand>,
        step: VecDeque<ModelicaCommand>,
        tx: Sender<ModelicaResult>,
        rx: Receiver<ModelicaResult>,
    }

    impl Lanes {
        fn new() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                compile: VecDeque::new(),
                step: VecDeque::new(),
                tx,
                rx,
            }
        }
        fn push(&mut self, cmd: ModelicaCommand) {
            enqueue_command(cmd, &mut self.compile, &mut self.step, &self.tx);
        }
    }

    /// The M8 point: another entity's Step jumps a queued slow compile.
    #[test]
    fn unrelated_step_jumps_queued_compile() {
        let mut l = Lanes::new();
        l.push(load_root());
        l.push(compile(ent(1), 2));
        l.push(step(ent(2)));
        assert_eq!(l.step.len(), 1, "entity 2's Step must take the step lane");
        assert_eq!(l.compile.len(), 2);
    }

    /// The preserved contract: "Compile then Step sees the compiled model" —
    /// a Step whose entity has a PENDING compile-lane command stays behind it.
    #[test]
    fn step_behind_own_entitys_compile_does_not_jump() {
        let mut l = Lanes::new();
        l.push(compile(ent(1), 2));
        l.push(step(ent(1)));
        assert!(
            l.step.is_empty(),
            "entity 1's Step must wait for its compile"
        );
        assert_eq!(l.compile.len(), 2);
        assert!(matches!(l.compile[0], ModelicaCommand::Compile { .. }));
        assert!(matches!(l.compile[1], ModelicaCommand::Step { .. }));
    }

    /// A LoadSourceRoot alone blocks nobody's Step — only an entity's own
    /// queued compile does (its Compile queued BEHIND the root load blocks
    /// its steps transitively, via the previous test's rule).
    #[test]
    fn load_source_root_does_not_block_live_steps() {
        let mut l = Lanes::new();
        l.push(load_root());
        l.push(step(ent(3)));
        assert_eq!(l.step.len(), 1);
    }

    /// Once the blocking compile is taken for execution, the deferred Step is
    /// hoisted back to the step lane (in order) instead of trickling out
    /// one-per-round behind unrelated compile-lane work.
    #[test]
    fn deferred_step_promoted_after_its_compile_is_taken() {
        let mut l = Lanes::new();
        l.push(compile(ent(1), 2));
        l.push(compile(ent(9), 2));
        l.push(step(ent(1)));
        assert!(l.step.is_empty());

        // The scheduling round: pop the front compile (entity 1's), promote.
        let front = l.compile.pop_front().expect("front compile");
        assert_eq!(cmd_entity(&front), ent(1));
        promote_unblocked_steps(&mut l.compile, &mut l.step);
        assert_eq!(
            l.step.len(),
            1,
            "entity 1's Step is runnable once its compile has been taken"
        );
        assert_eq!(l.compile.len(), 1, "entity 9's compile still queued");
    }

    /// The setpoint squash survives the lane split: adjacent same-entity,
    /// same-session Compiles collapse to the latest and ack the dropped one.
    #[test]
    fn compile_lane_still_squashes_setpoints() {
        let mut l = Lanes::new();
        l.push(compile(ent(1), 2));
        l.push(compile(ent(1), 2));
        assert_eq!(l.compile.len(), 1, "second Compile replaced the first");
        let ack = l.rx.try_recv().expect("dropped command must be acked");
        assert_eq!(ack.session_id, 2);
        // Steps are integrations, never squashed — even in the compile lane.
        l.push(step(ent(1)));
        l.push(step(ent(1)));
        assert_eq!(l.compile.len(), 3);
        assert!(l.rx.try_recv().is_err(), "no synthetic ack for a Step");
    }
}

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

    fn shared_hash_of(model: &str, source: &str, extras: Vec<(String, String)>) -> u64 {
        let unit = assemble_compile_unit(source, extras);
        shared_compile_hash(model, &unit, 4)
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
            shared_hash_of("M", "model M end M;", Vec::new()),
            shared_hash_of("M", "model M end M;", Vec::new())
        );
        assert_ne!(
            shared_hash_of("M", "model M end M;", Vec::new()),
            shared_hash_of("M2", "model M end M;", Vec::new())
        );
        assert_ne!(
            shared_hash_of("M", "model M end M;", Vec::new()),
            shared_hash_of("M", "model M Real x; end M;", Vec::new())
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
// M11 — compile temp dir pruning
// ===========================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod temp_dir_tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunco_modelica_temp_dir_test_{tag}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn mk(base: &std::path::Path, name: &str) {
        std::fs::create_dir_all(base.join(name).join("inner")).expect("mkdir");
    }

    /// Writing generation N prunes the same index's older generations and
    /// nothing else — other indices, and names that merely share a prefix,
    /// are untouched.
    #[test]
    fn newer_generation_prunes_older_same_index_only() {
        let base = scratch("gen");
        mk(&base, "3_1");
        mk(&base, "3_2");
        mk(&base, "31_1"); // different index sharing a string prefix
        mk(&base, "4_1"); // different entity
        mk(&base, "3_notagen"); // not this scheme's shape

        prune_entity_temp_dirs(&base, 3, Some("3_2"));

        assert!(!base.join("3_1").exists(), "older generation pruned");
        assert!(base.join("3_2").exists(), "current generation kept");
        assert!(base.join("31_1").exists(), "index 31 is not index 3");
        assert!(base.join("4_1").exists(), "other entities untouched");
        assert!(
            base.join("3_notagen").exists(),
            "non-scheme names untouched"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Despawn (`keep = None`) removes every generation of the index.
    #[test]
    fn despawn_prunes_all_generations_of_the_index() {
        let base = scratch("despawn");
        mk(&base, "5_1");
        mk(&base, "5_2");
        mk(&base, "6_1");

        prune_entity_temp_dirs(&base, 5, None);

        assert!(!base.join("5_1").exists());
        assert!(!base.join("5_2").exists());
        assert!(base.join("6_1").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A missing base dir is a no-op, not a panic (fresh install, nothing
    /// compiled yet).
    #[test]
    fn missing_base_dir_is_a_noop() {
        let base = std::env::temp_dir().join("lunco_modelica_temp_dir_test_nonexistent");
        let _ = std::fs::remove_dir_all(&base);
        prune_entity_temp_dirs(&base, 1, None);
    }
}
