//! Render-free Modelica runtime contracts.
//!
//! This package owns the ECS state and serialized worker protocol shared by
//! the compiler host, USD co-simulation, headless status surfaces, and UI
//! adapters. Rumoca compilation, DAE caching, and worker scheduling remain in
//! `lunco-modelica-core`; consumers that only exchange runtime state do not
//! depend on that implementation closure.

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use lunco_modelica_ast::ast_extract::ModelicaVariableMetadata;
use lunco_signal::{SignalExposure, SimStream};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

pub mod generated_source;
pub mod source_asset;

pub use source_asset::{ModelicaSource, ModelicaSourceAssetPlugin, ModelicaSourceLoader};

/// Maximum interval one live worker transaction may integrate.
pub const MAX_MACRO_STEP_DT: f64 = lunco_core_runtime::SECS_PER_TICK / 3.0 * 32.0;

/// Default communication period for a live Modelica participant.
pub const DEFAULT_COMMUNICATION_PERIOD_SECS: f64 = 0.1;

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

/// One exact communication transaction awaiting a worker result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InFlightModelicaStep {
    pub step_id: u64,
    pub start_time: f64,
    pub stop_time: f64,
}

/// Channels for communicating with the background simulation worker.
#[derive(Resource)]
pub struct ModelicaChannels {
    pub tx: Sender<ModelicaCommand>,
    pub rx: Receiver<ModelicaResult>,
    #[cfg(target_arch = "wasm32")]
    pub rx_cmd: Receiver<ModelicaCommand>,
    #[cfg(target_arch = "wasm32")]
    pub tx_res: Sender<ModelicaResult>,
}

/// Commands sent to the background simulation worker.
#[derive(Serialize, Deserialize)]
pub enum ModelicaCommand {
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
        model_name: String,
        source: String,
        realtime_safe: bool,
        doc_uri: String,
        extra_sources: Vec<(String, String)>,
        #[serde(default)]
        parameter_overrides: Vec<(String, f64)>,
        #[serde(skip)]
        stream: Option<SimStream>,
    },
    UpdateParameters {
        entity: Entity,
        session_id: u64,
        model_name: String,
        source: String,
    },
    Reset {
        entity: Entity,
        session_id: u64,
    },
    Despawn {
        entity: Entity,
    },
    LoadSourceRoot {
        id: String,
        payload: LoadSourceRootPayload,
    },
}

/// Source-root payload carried to the compiler worker.
#[derive(Serialize, Deserialize)]
pub enum LoadSourceRootPayload {
    Disk {
        root_dir: PathBuf,
    },
    InMemory {
        label: String,
        files: Vec<(String, String)>,
    },
}

/// Result received from the background simulation worker.
#[derive(Serialize, Deserialize)]
pub struct ModelicaResult {
    pub entity: Entity,
    pub session_id: u64,
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
    pub detected_input_names: Vec<String>,
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

/// Component attached to every live Modelica participant.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ModelicaModel {
    pub model_name: String,
    #[reflect(ignore)]
    pub source_uri: String,
    pub current_time: f64,
    pub target_time: f64,
    pub communication_period_secs: f64,
    #[reflect(ignore)]
    pub next_communication_time: f64,
    pub last_step_time: f64,
    pub session_id: u64,
    pub paused: bool,
    pub parameters: HashMap<String, f64>,
    pub inputs: HashMap<String, f64>,
    #[reflect(ignore)]
    pub compiled_input_names: BTreeSet<String>,
    pub variables: HashMap<String, f64>,
    #[reflect(ignore)]
    pub last_error: Option<String>,
    #[reflect(ignore)]
    pub document: lunco_doc::DocumentId,
    #[reflect(ignore)]
    pub is_stepping: bool,
    #[reflect(ignore)]
    pub in_flight_step: Option<InFlightModelicaStep>,
    #[reflect(ignore)]
    pub next_step_id: u64,
    #[reflect(ignore)]
    pub is_compiling: bool,
    #[reflect(ignore)]
    pub is_compiled: bool,
    #[reflect(ignore)]
    pub compiled_generation: u64,
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

impl ModelicaModel {
    /// Validate the model's authored communication period.
    #[inline]
    pub fn validated_communication_period_secs(&self) -> Result<f64, String> {
        validate_communication_period_secs(self.communication_period_secs)
    }

    /// Recompute the next communication point from the model's current clock.
    #[inline]
    pub fn reset_communication_schedule(&mut self) -> Result<(), String> {
        let period = self.validated_communication_period_secs()?;
        self.next_communication_time = self.current_time + period;
        Ok(())
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

/// Core request to compile a Modelica document.
#[derive(Message, Clone)]
pub struct CompileRequested {
    pub doc: lunco_doc::DocumentId,
    pub class: Option<String>,
    pub force: bool,
    pub resume_after_compile: bool,
}

/// One simulation step's observable samples.
pub struct SimSampleBatch {
    pub entity: Entity,
    pub document: lunco_doc::DocumentId,
    pub time: f64,
    pub samples: Vec<(String, f64)>,
    pub is_new_model: bool,
    pub is_parameter_update: bool,
}

/// UI-agnostic queue of live simulation samples awaiting projection.
#[derive(Resource, Default)]
pub struct SimSampleStream {
    pub batches: Vec<SimSampleBatch>,
}

/// System sets for asynchronous worker lifecycle and fixed-step exchange.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelicaSet {
    HandleResponses,
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
