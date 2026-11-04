//! Render-free Modelica runtime contracts.
//!
//! This package owns the ECS state and serialized worker protocol shared by
//! the compiler host, USD co-simulation, headless status surfaces, and UI
//! adapters. Rumoca compilation, DAE caching, and worker scheduling remain in
//! `lunco-modelica-worker`; consumers that only exchange runtime state do not
//! depend on that implementation closure.

use bevy::ecs::reflect::ReflectEvent;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use lunco_modelica_ast::ast_extract::ModelicaVariableMetadata;
use lunco_signal::SignalExposure;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub mod generated_source;
pub mod source_asset;

pub use source_asset::{ModelicaSource, ModelicaSourceAssetPlugin, ModelicaSourceLoader};

/// Edit one mutable scalar parameter in the selected participant's source and
/// install the checked replacement session. The headless Modelica core owns
/// validation, authored mutation and terminal completion in every host.
#[lunco_core::Command]
pub struct SetModelicaParameter {
    /// Existing participant selected by its stable API identity over transports.
    #[authz_target]
    pub entity: Entity,
    /// Exact scalar parameter declared by the selected source class.
    pub key: String,
    /// Finite value admitted by the parameter's checked bounds.
    pub value: f64,
}

/// Exact document and admitted library revisions supplied to a live compile.
/// The core document owner captures and validates this lease; solver workers
/// retain the corresponding immutable source bytes independently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelicaSourceSnapshot {
    /// Exact runtime lease, text generation and compiler URI for each source.
    pub documents: Vec<(lunco_workspace::PinnedDocumentRuntimeOwner, u64, String)>,
    pub source_roots: Vec<(String, u64)>,
}

/// Facts for one scalar identity in the installed checked FMI component.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelicaScalarMetadata {
    pub unit: Option<String>,
    pub description: Option<String>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
}

/// Maximum interval one live worker transaction may integrate.
pub const MAX_MACRO_STEP_DT: f64 = lunco_core_runtime::SECS_PER_TICK / 3.0 * 32.0;

/// Omitted communication periods exchange on every master physics tick.
pub const DEFAULT_COMMUNICATION_PERIOD_SECS: f64 = lunco_core_runtime::SECS_PER_TICK;

/// Immutable shared-cache entry limits captured before each worker starts.
/// Graph budgets bound retained graph count. Native artifacts are bounded by
/// both count and bytes. Live participant state is separate. Insert before
/// `ModelicaExecutionPlugin` to configure these budgets.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelicaCacheLimits {
    pub compiled_artifact_entries: usize,
    pub prepared_solve_entries: usize,
    pub native_code_entries: usize,
    pub native_code_bytes: usize,
}

/// Positive cache capacities admitted before starting Modelica workers.
#[derive(Clone, Copy, Debug)]
pub struct ModelicaCacheCapacities {
    pub compiled_artifacts: std::num::NonZeroUsize,
    pub prepared_solves: std::num::NonZeroUsize,
    pub native_code_entries: std::num::NonZeroUsize,
    pub native_code_bytes: std::num::NonZeroUsize,
}

impl Default for ModelicaCacheLimits {
    fn default() -> Self {
        Self {
            compiled_artifact_entries: 64,
            prepared_solve_entries: 64,
            native_code_entries: 32_768,
            native_code_bytes: 64 * 1024 * 1024,
        }
    }
}

impl ModelicaCacheLimits {
    /// Validate the runtime DTO once at admission and return positive capacities.
    pub fn validate(&self) -> Result<ModelicaCacheCapacities, String> {
        let compiled =
            std::num::NonZeroUsize::new(self.compiled_artifact_entries).ok_or_else(|| {
                "Modelica compiled-artifact cache capacity must be positive".to_owned()
            })?;
        let prepared = std::num::NonZeroUsize::new(self.prepared_solve_entries)
            .ok_or_else(|| "Modelica prepared-solve cache capacity must be positive".to_owned())?;
        let native_code_entries = std::num::NonZeroUsize::new(self.native_code_entries)
            .ok_or_else(|| "Modelica native-code cache capacity must be positive".to_owned())?;
        let native_code_bytes = std::num::NonZeroUsize::new(self.native_code_bytes)
            .ok_or_else(|| "Modelica native-code cache byte budget must be positive".to_owned())?;
        Ok(ModelicaCacheCapacities {
            compiled_artifacts: compiled,
            prepared_solves: prepared,
            native_code_entries,
            native_code_bytes,
        })
    }
}

const COMMUNICATION_EPS: f64 = 1e-9;

/// Validate a Modelica communication period against the fixed-step master.
pub fn validate_communication_period_secs(value: f64) -> Result<f64, String> {
    if !value.is_finite()
        || !(lunco_core_runtime::SECS_PER_TICK..=MAX_MACRO_STEP_DT).contains(&value)
    {
        return Err(format!(
            "invalid Modelica communication period {value:?}; expected a finite value in [{:.9}, {MAX_MACRO_STEP_DT:.9}]s",
            lunco_core_runtime::SECS_PER_TICK
        ));
    }
    let fixed_ticks = (value / lunco_core_runtime::SECS_PER_TICK).round();
    let represented = fixed_ticks * lunco_core_runtime::SECS_PER_TICK;
    if fixed_ticks < 1.0 || (represented - value).abs() > COMMUNICATION_EPS {
        return Err(format!(
            "invalid Modelica communication period {value:?}; it must be an integer multiple of the master fixed tick {:.9}s",
            lunco_core_runtime::SECS_PER_TICK
        ));
    }
    Ok(value)
}

/// Resolve the authored communication-period opinion shared by USD and the
/// live participant. Omission uses the documented schema default; an explicit
/// malformed value is an authoring error.
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

/// Values exchanged in one accepted Modelica communication transaction.
///
/// Inputs and outputs are retained together so observations can distinguish a
/// consumed input from a newer connected value and identify the exact outputs
/// produced by that input vector.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelicaStepSample {
    pub session_id: u64,
    pub step_id: u64,
    pub input_time_s: f64,
    pub output_time_s: f64,
    pub inputs: Vec<(String, f64)>,
    /// Worker outputs owned by this accepted transaction.
    pub outputs: Vec<(String, f64)>,
}

/// One exact communication transaction awaiting a worker result.
#[derive(Debug, Clone, PartialEq)]
pub struct InFlightModelicaStep {
    pub step_id: u64,
    /// Integer endpoint on this participant's master-tick timeline.
    pub stop_tick: u64,
    pub start_time: f64,
    pub stop_time: f64,
    /// The values sent to the solver, in stable name order.
    pub sampled_inputs: Vec<(String, f64)>,
    /// Monotonic owner timestamp used for diagnostic response-latency samples.
    pub submitted_at: web_time::Instant,
}

/// Channels for communicating with the background simulation worker.
#[derive(Resource)]
pub struct ModelicaChannels {
    pub tx: Sender<ModelicaCommand>,
    pub rx: Receiver<ModelicaResult>,
    pub preparation_rx: Receiver<ModelicaPreparationResult>,
    #[cfg(target_arch = "wasm32")]
    pub rx_cmd: Receiver<ModelicaCommand>,
    #[cfg(target_arch = "wasm32")]
    pub tx_res: ModelicaResponseSender,
}

impl ModelicaChannels {
    /// Wake the preparation owner for either a queued reply or a terminal
    /// transport disconnection, without consuming the request's result.
    pub fn preparation_result_ready(&self) -> bool {
        let mut select = crossbeam_channel::Select::new();
        select.recv(&self.preparation_rx);
        select.try_ready().is_ok()
    }
}

/// Identity of one scene-owned preparation request, independent of ECS actors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModelicaPreparationId {
    pub scene_generation: u64,
    pub sequence: u64,
}

#[derive(Serialize, Deserialize)]
pub struct ModelicaInitializationOutcome {
    /// Settled observations in exactly the requested probe order.
    pub values: Vec<f64>,
    /// Settled observations of the original DAE's named state coordinates.
    /// These are joint-solution values, not start guesses or solver-chart slots.
    pub state_values: Vec<(String, f64)>,
    pub solver: Arc<ModelicaLiveSolverSnapshot>,
    pub construction_profile: ModelicaConstructionProfile,
}

/// A source declaration preparation is bound to the exact requesting actor
/// lease. It requires a checked DAE and creates no solver or participant.
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelicaInterfaceOutcome {
    pub entity: Entity,
    pub session_id: u64,
    pub doc_uri: String,
    pub parameter_overrides: Vec<(String, f64)>,
    pub authored_bindings: ModelicaAuthoredBindings,
    pub interface: lunco_modelica_ast::resolved_interface::ResolvedModelicaInterface,
    pub library_revision: u64,
}

/// Source declarations accepted for this actor's exact source lease.
#[derive(Component, Clone)]
pub struct ModelicaResolvedInterface(pub Arc<ModelicaInterfaceOutcome>);

#[derive(Serialize, Deserialize)]
pub enum ModelicaPreparationOutcome {
    Interface(ModelicaInterfaceOutcome),
    Initialization(ModelicaInitializationOutcome),
}

#[derive(Serialize, Deserialize)]
pub struct ModelicaPreparationResult {
    pub request_id: ModelicaPreparationId,
    pub outcome: Result<ModelicaPreparationOutcome, String>,
}

/// Worker replies carry their owning lifecycle across every transport.
#[derive(Serialize, Deserialize)]
pub enum ModelicaWorkerResponse {
    Participant(ModelicaResult),
    Preparation(ModelicaPreparationResult),
}

impl From<ModelicaResult> for ModelicaWorkerResponse {
    fn from(result: ModelicaResult) -> Self {
        Self::Participant(result)
    }
}

impl From<ModelicaPreparationResult> for ModelicaWorkerResponse {
    fn from(result: ModelicaPreparationResult) -> Self {
        Self::Preparation(result)
    }
}

/// Route a worker response directly to its consumer. Native workers and the
/// browser transport use the same typed boundary without a forwarding thread.
#[derive(Clone)]
pub struct ModelicaResponseSender {
    participant: Sender<ModelicaResult>,
    preparation: Sender<ModelicaPreparationResult>,
}

impl ModelicaResponseSender {
    pub fn new(
        participant: Sender<ModelicaResult>,
        preparation: Sender<ModelicaPreparationResult>,
    ) -> Self {
        Self {
            participant,
            preparation,
        }
    }

    pub fn send(
        &self,
        response: impl Into<ModelicaWorkerResponse>,
    ) -> Result<(), crossbeam_channel::SendError<ModelicaWorkerResponse>> {
        match response.into() {
            ModelicaWorkerResponse::Participant(result) => self
                .participant
                .send(result)
                .map_err(|error| crossbeam_channel::SendError(error.0.into())),
            ModelicaWorkerResponse::Preparation(result) => self
                .preparation
                .send(result)
                .map_err(|error| crossbeam_channel::SendError(error.0.into())),
        }
    }
}

/// The admitted actor's execution domain, retained through cached rebuilds.
/// Native document construction must not occupy a scene solver owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelicaExecutionDomain {
    Scene,
    Document,
}

/// Immutable checked batch artifact from the shared native compiler owner.
#[cfg(not(target_arch = "wasm32"))]
pub struct ModelicaBatchArtifact {
    pub dae: std::sync::Arc<rumoca_ir_dae::Dae>,
    pub source_content: lunco_hash::content::Cid,
}

/// Instance bindings awaiting checked compiler classification.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ModelicaAuthoredBindings {
    pub values: Vec<(String, f64)>,
    /// Names proven to have a distinct input owner on the same entity. The
    /// compiler first admits its own declared inputs and parameters; a name
    /// absent from that boundary may then remain with the external owner.
    pub external_inputs: BTreeSet<String>,
}

/// Commands sent to the background simulation worker.
#[derive(Serialize, Deserialize)]
pub enum ModelicaCommand {
    /// Compile the authored parameter intent and expose its checked scalar interface.
    PrepareInterface {
        request_id: ModelicaPreparationId,
        entity: Entity,
        session_id: u64,
        model_name: String,
        source: String,
        doc_uri: String,
        extra_sources: Vec<(String, String)>,
        parameter_overrides: Vec<(String, f64)>,
        authored_bindings: ModelicaAuthoredBindings,
    },
    /// Solve initial equations and observe selected probes without installing
    /// an actor, a live integrator, or any telemetry publication.
    Initialize {
        request_id: ModelicaPreparationId,
        model_name: String,
        source: String,
        doc_uri: String,
        extra_sources: Vec<(String, String)>,
        /// Compiler source-root revision admitted by interface preparation.
        expected_library_revision: Option<u64>,
        parameter_overrides: Vec<(String, f64)>,
        authored_bindings: ModelicaAuthoredBindings,
        initial_inputs: Vec<(String, f64)>,
        probes: Vec<String>,
    },
    CancelPreparation {
        request_id: ModelicaPreparationId,
    },
    Step {
        entity: Entity,
        session_id: u64,
        step_id: u64,
        start_time: f64,
        stop_time: f64,
        model_name: String,
        inputs: Vec<(String, f64)>,
        dt: f64,
    },
    Compile {
        entity: Entity,
        session_id: u64,
        execution_domain: ModelicaExecutionDomain,
        model_name: String,
        source: String,
        realtime_safe: bool,
        doc_uri: String,
        extra_sources: Vec<(String, String)>,
        /// Compiler source-root revision admitted by interface preparation.
        expected_library_revision: Option<u64>,
        #[serde(default)]
        parameter_overrides: Vec<(String, f64)>,
        /// Authored scalar bindings whose Modelica variability is resolved by
        /// the checked compiler, including inherited declarations.
        authored_bindings: ModelicaAuthoredBindings,
        /// Explicit finite input values admitted before solver initialization.
        initial_inputs: Vec<(String, f64)>,
        /// Named source-state observations from a checked coupled initial
        /// solution. None requests ordinary initialization; Some requires the
        /// owner to validate completeness even when the vector is empty.
        /// Accepted free-state coordinates become exact initialization
        /// constraints, never numerical start guesses.
        initial_state_values: Option<Vec<(String, f64)>>,
    },
    UpdateParameters {
        entity: Entity,
        session_id: u64,
        model_name: String,
        source: String,
        /// Exact changed scalar; the worker retains all other instance overrides.
        parameter_name: String,
        parameter_value: f64,
        /// Current admitted inputs, including edits made while paused.
        initial_inputs: Vec<(String, f64)>,
    },
    Reset {
        entity: Entity,
        session_id: u64,
        /// Current admitted inputs, including writes made while paused.
        initial_inputs: Vec<(String, f64)>,
    },
    Despawn {
        entity: Entity,
    },
    LoadSourceRoot {
        id: String,
        /// Source-root operation identity assigned by the registry. A newer
        /// load or unload for the same id fences late preparation results.
        operation_id: u64,
        payload: LoadSourceRootPayload,
    },
    UnloadSourceRoot {
        id: String,
        /// Monotonic registry operation identity that fences older loads.
        operation_id: u64,
    },
    /// Native batch preparation shares the admitted source roots and cache.
    /// This typed in-process request never crosses the serialized worker wire.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    CompileBatch {
        model_name: String,
        source: String,
        doc_uri: String,
        extra_sources: Vec<(String, String)>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        reply: Sender<Result<ModelicaBatchArtifact, String>>,
    },
}

/// Source-root payload carried to the compiler worker.
#[derive(Serialize, Deserialize)]
pub enum LoadSourceRootPayload {
    /// The complete paired source artifact owned by the installed library.
    SourceLibrary,
    Disk {
        root_dir: PathBuf,
    },
    /// One file owned by the bundled Modelica asset library. Native workers
    /// resolve and read the asset during immutable source-root preparation.
    BundledModel {
        filename: String,
    },
    /// A structured package owned by the bundled Modelica asset library.
    BundledPackage {
        root: String,
    },
    /// One workspace file admitted by the workspace document owner.
    WorkspaceFile {
        path: PathBuf,
    },
    InMemory {
        label: String,
        files: Vec<(String, String)>,
    },
}

/// Application-lifetime worker failure. Scene teardown cannot revive a dead
/// transport; subsequent compile requests reject with this exact diagnostic.
#[derive(Resource, Default)]
pub struct ModelicaWorkerFailure(pub Option<String>);

/// Result received from the background simulation worker.
#[derive(Serialize, Deserialize)]
pub struct ModelicaResult {
    /// Worker-wide transport/startup failure, consumed before entity routing.
    #[serde(default)]
    pub worker_failure: Option<String>,
    pub entity: Entity,
    pub session_id: u64,
    /// Presentation-only preparation transition; never a solver transaction.
    #[serde(default)]
    pub preparation_phase: Option<ModelicaPreparationPhase>,
    #[serde(default)]
    pub step_id: Option<u64>,
    pub new_time: f64,
    pub outputs: Vec<(String, f64)>,
    pub detected_symbols: Vec<(String, f64)>,
    /// Exact scalar declaration facts from the checked FMI component.
    /// Published once when construction installs or replaces a live stepper.
    pub variable_metadata: Option<BTreeMap<String, ModelicaScalarMetadata>>,
    pub error: Option<String>,
    pub log_message: Option<String>,
    pub is_new_model: bool,
    pub is_parameter_update: bool,
    pub is_reset: bool,
    pub detected_input_names: Vec<String>,
    /// Parameter declarations from the checked DAE, independent of whether
    /// their source binding was a literal or an inherited expression.
    pub detected_parameter_names: Vec<String>,
    /// Checked type and source-edit authority for the exact installed instance.
    pub checked_interface:
        Option<Arc<lunco_modelica_ast::resolved_interface::ResolvedModelicaInterface>>,
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
    #[serde(default)]
    pub compiled_model_name: Option<String>,
    #[serde(default)]
    pub loaded_source_root_id: Option<String>,
    /// Authored top-level namespaces installed by this exact source-set operation.
    #[serde(default)]
    pub loaded_source_root_namespaces: Vec<String>,
    #[serde(default)]
    pub loaded_source_root_sources: Vec<lunco_modelica_ast::resource_uri::SourceFileFacts>,
    /// Exact source snapshot from the admitted operation, for authoring queries.
    #[serde(default)]
    pub loaded_source_root_files: Vec<(String, String)>,
    /// Source-set unload acknowledgement from the Modelica worker.
    #[serde(default)]
    pub unloaded_source_root_id: Option<String>,
    /// Registry operation identity associated with a source-root result.
    #[serde(default)]
    pub source_root_operation_id: Option<u64>,
    #[serde(default)]
    pub compile_diagnostics: Vec<lunco_doc::Diagnostic>,
    /// Solver execution duration for a completed live worker step, in ns.
    /// Absent when the result did not execute the solver (for example, a
    /// deferred first-step preparation).
    #[serde(default)]
    pub worker_step_duration_ns: Option<u64>,
    /// Number of queued/in-flight worker tasks when this step starts.
    /// Browser worker transports may not expose their internal queue depth.
    #[serde(default)]
    pub worker_backlog_count: Option<u64>,
    /// Resolved live solver and exact backend-neutral settings used by the
    /// worker for this model session. Present on successful compile, reset,
    /// parameter-update, and step results.
    #[serde(default)]
    pub live_solver_snapshot: Option<Arc<ModelicaLiveSolverSnapshot>>,
}

impl ModelicaResult {
    /// A terminal worker-owner diagnostic has no participant identity.
    pub fn worker_failure(error: impl Into<String>) -> Self {
        Self {
            worker_failure: Some(error.into()),
            ..Default::default()
        }
    }
}

impl Default for ModelicaResult {
    fn default() -> Self {
        Self {
            worker_failure: None,
            entity: Entity::PLACEHOLDER,
            session_id: 0,
            preparation_phase: None,
            step_id: None,
            new_time: 0.0,
            outputs: Vec::new(),
            detected_symbols: Vec::new(),
            variable_metadata: None,
            error: None,
            log_message: None,
            is_new_model: false,
            is_parameter_update: false,
            is_reset: false,
            detected_input_names: Vec::new(),
            detected_parameter_names: Vec::new(),
            checked_interface: None,
            experiment_start_time: None,
            experiment_stop_time: None,
            experiment_tolerance: None,
            experiment_interval: None,
            experiment_solver: None,
            compiled_model_name: None,
            loaded_source_root_id: None,
            loaded_source_root_namespaces: Vec::new(),
            loaded_source_root_sources: Vec::new(),
            loaded_source_root_files: Vec::new(),
            unloaded_source_root_id: None,
            source_root_operation_id: None,
            compile_diagnostics: Vec::new(),
            worker_step_duration_ns: None,
            worker_backlog_count: None,
            live_solver_snapshot: None,
        }
    }
}

/// Owner-reported stages of preparing one live Modelica participant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ModelicaPreparationPhase {
    QueuedForCompilation,
    CheckingSolverCache,
    LoweringEquations,
    CachedSolverLoaded,
    SolverPrepared { elapsed_secs: f64 },
    LiveSolverConstructed(ModelicaConstructionProfile),
}

/// Wall-clock service measurements for one successful live construction.
/// These diagnostics do not participate in solver state or scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelicaConstructionProfile {
    /// Unavailable when the transport does not expose the construction queue.
    pub queue_wait_ns: Option<u64>,
    pub session_ns: u64,
    pub metadata_ns: u64,
    pub initial_observation_ns: u64,
    pub native_code_cache: Option<ModelicaNativeCodeCacheProfile>,
}

/// Native compilation work; rejections are also counted in compilations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelicaNativeCodeCacheProfile {
    pub attempts: u64,
    pub hits: u64,
    pub compilations: u64,
    pub rejected_entries: u64,
    /// Artifacts evicted by insertions from this construction scope.
    pub evictions: u64,
    /// Shared cache residency observed when this construction completes.
    pub retained_entries: u64,
    pub retained_bytes: u64,
}

/// The registered solver capabilities selected for one live Modelica model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelicaSolverCapabilities {
    /// The backend declares that it is safe inside the frame loop.
    pub usable_live: bool,
    /// The backend declares that it follows the caller's fixed step sequence.
    pub fixed_step: bool,
    /// The backend declares reproducible results for identical inputs.
    pub deterministic: bool,
}

/// Runtime context used to resolve a live Modelica solver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelicaRuntimeProfile {
    /// The model is stepped by the live frame-loop co-simulation path.
    pub live: bool,
    /// The model drives a client-predicted body.
    pub predicted: bool,
}

/// Backend-neutral numerical settings passed to the resolved live solver.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelicaSolverParameters {
    /// Absolute error tolerance.
    pub atol: f64,
    /// Relative error tolerance.
    pub rtol: f64,
    /// Initial or maximum internal solver step.
    pub h0: Option<f64>,
    /// Solver time window start.
    pub t_start: f64,
    /// Solver time window end.
    pub t_end: f64,
}

/// Admitted live solver facts retained by a `ModelicaModel`.
///
/// Parameter overrides are sorted by name and retain their native `f64`
/// values. The snapshot is emitted by the same worker plan used to construct
/// the live stepper, so capture cannot recompute a different solver choice.
/// The worker cache, receipts and admitted model share these immutable facts;
/// stepping does not copy parameter names or initial samples.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelicaLiveSolverSnapshot {
    /// Compiler-owned source-root revision captured for this construction.
    /// Absent only when the transport cannot provide that owner identity.
    pub admitted_library_revision: Option<u64>,
    /// Canonical id of the registered backend selected for this model.
    pub solver_id: String,
    /// Capabilities declared by the selected backend.
    pub capabilities: ModelicaSolverCapabilities,
    /// Live and prediction requirements used for resolution.
    pub profile: ModelicaRuntimeProfile,
    /// Numerical settings passed to the backend option translator.
    pub parameters: ModelicaSolverParameters,
    /// Sorted instance overrides passed into DAE lowering.
    pub parameter_overrides: Vec<(String, f64)>,
    /// Sorted finite input samples admitted before initialization.
    pub initial_inputs: Vec<(String, f64)>,
    /// Exact free-state constraints accepted by checked initialization
    /// admission. This contains the applied constraints, not all candidate
    /// source-state observations supplied by a coupled solution.
    pub initial_state_constraints: Vec<(String, f64)>,
}

/// Bounded aggregate diagnostics for the live Modelica step handoff.
///
/// This records one compact sample per completed worker step. It does not
/// retain traces or variable payloads, so `CosimStatus` can expose the worker
/// service cost and response delay without adding per-frame sampling work.
#[derive(Resource, Default)]
pub struct ModelicaStepDiagnostics {
    participants: HashMap<Entity, ModelicaParticipantStepDiagnostics>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelicaParticipantStepDiagnostics {
    pub session_id: u64,
    pub samples: u64,
    pub last_worker_step_duration_ns: u64,
    pub total_worker_step_duration_ns: u64,
    pub max_worker_step_duration_ns: u64,
    pub last_response_latency_ns: u64,
    pub max_response_latency_ns: u64,
    pub last_worker_backlog_count: Option<u64>,
    pub max_worker_backlog_count: Option<u64>,
}

impl ModelicaStepDiagnostics {
    pub fn participant(&self, entity: Entity) -> Option<&ModelicaParticipantStepDiagnostics> {
        self.participants.get(&entity)
    }

    pub fn participants(
        &self,
    ) -> impl Iterator<Item = (Entity, &ModelicaParticipantStepDiagnostics)> {
        self.participants
            .iter()
            .map(|(entity, diagnostics)| (*entity, diagnostics))
    }

    pub fn begin_session(&mut self, entity: Entity, session_id: u64) {
        self.participants.insert(
            entity,
            ModelicaParticipantStepDiagnostics {
                session_id,
                ..Default::default()
            },
        );
    }

    pub fn record_step(
        &mut self,
        entity: Entity,
        session_id: u64,
        worker_step_duration_ns: u64,
        response_latency_ns: u64,
        worker_backlog_count: Option<u64>,
    ) {
        let diagnostics = self.participants.entry(entity).or_default();
        if diagnostics.session_id != session_id {
            *diagnostics = ModelicaParticipantStepDiagnostics {
                session_id,
                ..Default::default()
            };
        }
        diagnostics.samples = diagnostics.samples.saturating_add(1);
        diagnostics.last_worker_step_duration_ns = worker_step_duration_ns;
        diagnostics.total_worker_step_duration_ns = diagnostics
            .total_worker_step_duration_ns
            .saturating_add(worker_step_duration_ns);
        diagnostics.max_worker_step_duration_ns = diagnostics
            .max_worker_step_duration_ns
            .max(worker_step_duration_ns);
        diagnostics.last_response_latency_ns = response_latency_ns;
        diagnostics.max_response_latency_ns =
            diagnostics.max_response_latency_ns.max(response_latency_ns);
        diagnostics.last_worker_backlog_count = worker_backlog_count;
        if let Some(count) = worker_backlog_count {
            diagnostics.max_worker_backlog_count = Some(
                diagnostics
                    .max_worker_backlog_count
                    .map_or(count, |current| current.max(count)),
            );
        }
    }

    pub fn remove(&mut self, entity: Entity) {
        self.participants.remove(&entity);
    }
}

/// Landed observable variables of one Modelica participant.
///
/// Native component change detection tracks the output snapshot independently
/// of input and communication-clock updates. The worker commits this map and
/// the model clock together at its serial result-admission boundary. Native
/// IndexMap storage retains reflected string-keyed reads and lets admission
/// align stable observation rows without hashing each observable name.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ModelicaVariables {
    pub values: indexmap::IndexMap<String, f64, bevy::platform::hash::FixedHasher>,
}

/// Component attached to every live Modelica participant.
#[derive(Component, Reflect)]
#[reflect(Component)]
#[require(ModelicaVariables)]
pub struct ModelicaModel {
    pub model_name: String,
    #[reflect(ignore)]
    pub source_uri: String,
    pub current_time: f64,
    /// Seconds projected from `master_clock`, never an accumulated duration.
    pub target_time: f64,
    pub communication_period_secs: f64,
    /// Resettable child of the authoritative physics tick.
    #[reflect(ignore)]
    pub master_clock: lunco_core_runtime::TickClock,
    /// Last accepted communication endpoint in master intervals.
    #[reflect(ignore)]
    pub communication_tick: u64,
    #[reflect(ignore)]
    pub next_communication_time: f64,
    pub last_step_time: f64,
    pub session_id: u64,
    pub paused: bool,
    pub parameters: bevy::platform::collections::HashMap<String, f64>,
    pub inputs: bevy::platform::collections::HashMap<String, f64>,
    /// Inputs latched at the start of the current communication interval.
    /// Slower participants retain this sample across intervening master ticks.
    #[reflect(ignore)]
    pub pending_step_inputs: Option<Vec<(String, f64)>>,
    #[reflect(ignore)]
    pub compiled_input_names: BTreeSet<String>,
    #[reflect(ignore)]
    pub compiled_parameter_names: BTreeSet<String>,
    #[reflect(ignore)]
    pub checked_interface:
        Option<Arc<lunco_modelica_ast::resolved_interface::ResolvedModelicaInterface>>,
    #[reflect(ignore)]
    pub last_error: Option<String>,
    #[reflect(ignore)]
    pub document: lunco_doc::DocumentId,
    #[reflect(ignore)]
    pub is_stepping: bool,
    #[reflect(ignore)]
    pub in_flight_step: Option<InFlightModelicaStep>,
    /// Exact input sample that produced the currently published outputs.
    #[reflect(ignore)]
    pub last_accepted_step: Option<ModelicaStepSample>,
    #[reflect(ignore)]
    pub next_step_id: u64,
    #[reflect(ignore)]
    pub is_compiling: bool,
    #[reflect(ignore)]
    pub is_compiled: bool,
    #[reflect(ignore)]
    pub compiled_generation: u64,
    /// Source closure admitted for the pending or installed document session.
    #[reflect(ignore)]
    pub source_snapshot: Option<ModelicaSourceSnapshot>,
    /// Exact live solver plan that produced the installed stepper.
    #[reflect(ignore)]
    pub live_solver_snapshot: Option<Arc<ModelicaLiveSolverSnapshot>>,
    /// Immutable compiled facts admitted with this solver session.
    #[reflect(ignore)]
    pub variable_metadata: Option<Arc<BTreeMap<String, ModelicaScalarMetadata>>>,
    /// Last successful construction for this participant's admitted session.
    #[reflect(ignore)]
    pub construction_profile: Option<ModelicaConstructionProfile>,
    #[reflect(ignore)]
    pub pending_generation: u64,
    #[reflect(ignore)]
    pub resume_after_compile: bool,
}

impl Default for ModelicaModel {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            source_uri: String::new(),
            current_time: 0.0,
            target_time: 0.0,
            communication_period_secs: DEFAULT_COMMUNICATION_PERIOD_SECS,
            master_clock: Default::default(),
            communication_tick: 0,
            next_communication_time: DEFAULT_COMMUNICATION_PERIOD_SECS,
            last_step_time: 0.0,
            session_id: 0,
            paused: false,
            parameters: Default::default(),
            inputs: Default::default(),
            pending_step_inputs: None,
            compiled_input_names: BTreeSet::new(),
            compiled_parameter_names: BTreeSet::new(),
            checked_interface: None,
            last_error: None,
            document: lunco_doc::DocumentId::default(),
            is_stepping: false,
            in_flight_step: None,
            last_accepted_step: None,
            next_step_id: 1,
            is_compiling: false,
            is_compiled: false,
            compiled_generation: 0,
            source_snapshot: None,
            live_solver_snapshot: None,
            variable_metadata: None,
            construction_profile: None,
            pending_generation: 0,
            resume_after_compile: false,
        }
    }
}

impl ModelicaModel {
    /// Validate the model's authored communication period.
    #[inline]
    pub fn validated_communication_period_secs(&self) -> Result<f64, String> {
        validate_communication_period_secs(self.communication_period_secs)
    }

    /// Recompute the next communication point on the integer master grid.
    #[inline]
    pub fn reset_communication_schedule(&mut self) -> Result<(), String> {
        self.next_communication_time =
            lunco_core_runtime::SimTick(self.next_communication_tick()?).seconds();
        Ok(())
    }

    /// Next endpoint, validated before dispatch or publication.
    pub fn next_communication_tick(&self) -> Result<u64, String> {
        let period = self.validated_communication_period_secs()?;
        let ticks = (period / lunco_core_runtime::SECS_PER_TICK).round() as u64;
        self.communication_tick
            .checked_add(ticks)
            .ok_or_else(|| "Modelica communication tick range exhausted".to_owned())
    }

    /// Admit a physics boundary, or freeze this child while inactive.
    pub fn observe_master_tick(
        &mut self,
        master: lunco_core_runtime::SimTick,
        active: bool,
    ) -> Result<(), String> {
        self.master_clock
            .observe(master, active)
            .map_err(str::to_owned)?;
        self.target_time = self.master_clock.elapsed_secs();
        Ok(())
    }

    /// Retire the local timeline together with its solver session.
    pub fn clear_clock(&mut self) {
        self.master_clock = Default::default();
        self.communication_tick = 0;
        self.current_time = 0.0;
        self.target_time = 0.0;
        self.next_communication_time = 0.0;
        self.last_step_time = 0.0;
    }
}

/// UI-agnostic notice emitted by the Modelica runtime.
#[derive(Message, Clone)]
pub struct ModelicaNotice {
    pub level: NoticeLevel,
    pub text: String,
}

/// Severity of a [`ModelicaNotice`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

/// Exact participant installation acknowledged by a document compile request.
#[derive(Clone, Debug)]
pub struct ModelicaCompileReceipt {
    pub entity: Entity,
    pub session_id: u64,
    pub generation: u64,
}

/// One terminal reply shared by cloned or coalesced compile intents.
#[derive(Clone)]
pub struct ModelicaCompileCompletion(
    Arc<std::sync::Mutex<Option<Sender<Result<ModelicaCompileReceipt, String>>>>>,
);

pub struct ModelicaCompileReply(Receiver<Result<ModelicaCompileReceipt, String>>);

impl ModelicaCompileCompletion {
    pub fn channel() -> (Self, ModelicaCompileReply) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        (
            Self(Arc::new(std::sync::Mutex::new(Some(tx)))),
            ModelicaCompileReply(rx),
        )
    }

    pub fn complete(&self, result: Result<ModelicaCompileReceipt, String>) {
        if let Some(sender) = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(result);
        }
    }
}

impl ModelicaCompileReply {
    /// Retirement of every request owner is a terminal cancellation.
    pub fn try_take(&self) -> Option<Result<ModelicaCompileReceipt, String>> {
        match self.0.try_recv() {
            Ok(result) => Some(result),
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(crossbeam_channel::TryRecvError::Disconnected) => Some(Err(
                "Modelica compile request retired before completion".into(),
            )),
        }
    }
}

/// Core request to compile a Modelica document.
#[derive(Message, Clone)]
pub struct CompileRequested {
    pub completions: Vec<ModelicaCompileCompletion>,
    pub source: lunco_workspace::PinnedDocumentRuntimeOwner,
    /// Existing live participant when compilation is part of scene startup;
    /// `None` asks the document owner to reuse or create the document's model.
    pub entity: Option<Entity>,
    pub class: Option<String>,
    pub force: bool,
    pub resume_after_compile: bool,
}

/// A checked live solver session has been installed. Presentation may retire
/// bindings for that exact session without owning sample history.
#[derive(Message, Clone, Copy)]
pub struct ModelicaSessionStarted {
    pub entity: Entity,
    pub session_id: u64,
}

/// System sets for asynchronous worker lifecycle and fixed-step exchange.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelicaSet {
    /// Admit worker completions in `First`, before the virtual clock samples
    /// the causal barrier for this application update.
    HandleResponses,
    /// Admit compile intent independently of the fixed simulation clock.
    AdmitCompileRequests,
    SpawnRequests,
}

/// Render-free identity of a generated Modelica value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelicaSignalProvenance {
    pub source_asset: Option<String>,
    pub model_class: Option<String>,
    pub model_variable: Option<String>,
    pub canonical_name: Option<String>,
}

/// Authored-structure address map for a generated Modelica participant.
#[derive(Component, Clone, Debug, Default)]
pub struct ModelicaSignalLayout {
    pub exact_paths: BTreeMap<String, String>,
    pub prefixes: Vec<(String, String)>,
    pub exact_provenance: BTreeMap<String, ModelicaSignalProvenance>,
    pub provenance_prefixes: Vec<(String, ModelicaSignalProvenance)>,
    pub public_exact_paths: HashSet<String>,
    pub metadata: BTreeMap<String, ModelicaVariableMetadata>,
    pub root_path: String,
}

impl ModelicaSignalLayout {
    /// Project source-language metadata once at port admission. Live port
    /// consumers use the backend-neutral contract rather than this provenance map.
    pub fn port_contracts(&self) -> Result<lunco_port_core::ScalarPortContracts, String> {
        lunco_port_core::ScalarPortContracts::new(
            self.metadata
                .iter()
                .map(|(name, metadata)| {
                    let unit = metadata
                        .unit
                        .as_deref()
                        .map(lunco_engineering_values::UnitReference::identified)
                        .transpose()
                        .map_err(|error| format!("invalid unit for '{name}': {error}"))?;
                    Ok((
                        name.clone(),
                        lunco_port_core::ScalarPortContract {
                            unit,
                            ..Default::default()
                        },
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?,
        )
    }

    pub fn group_path(&self, variable: &str) -> Option<&str> {
        if let Some(path) = self.exact_paths.get(variable) {
            return Some(path);
        }
        self.prefixes
            .iter()
            .filter(|(prefix, _)| variable.starts_with(prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, path)| path.as_str())
            .or_else(|| (!self.root_path.is_empty()).then_some(self.root_path.as_str()))
    }

    pub fn exposure(&self, variable: &str) -> SignalExposure {
        if self.public_exact_paths.contains(variable) {
            SignalExposure::Public
        } else {
            SignalExposure::Internal
        }
    }

    pub fn provenance(&self, variable: &str) -> Option<ModelicaSignalProvenance> {
        if let Some(identity) = self.exact_provenance.get(variable) {
            return Some(identity.clone());
        }
        self.provenance_prefixes
            .iter()
            .filter(|(prefix, _)| variable.starts_with(prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(prefix, identity)| {
                let mut resolved = identity.clone();
                let suffix = variable.strip_prefix(prefix).unwrap_or_default();
                if !suffix.is_empty() {
                    resolved.model_variable = Some(suffix.to_string());
                }
                resolved
            })
    }
}

/// Marks the transient actor created by editor document compilation.
/// Supplied USD actors retain their geometry when their Modelica participation
/// is retired; only document-created actors are despawned in full.
#[derive(Component)]
pub struct EditorModelicaActor;

/// Last admitted or retired Modelica session for a reusable ECS actor.
/// Component removal preserves this epoch; entity despawn ends its lifetime.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ModelicaSessionEpoch(pub u64);
