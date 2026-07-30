//! Co-simulation model component.
//!
//! Represents any non-Avian simulation model (Modelica, FMU, GMAT, etc.)
//! attached to an entity. Engine plugins create these when models compile.

use bevy::prelude::*;
use std::collections::HashMap;

/// Authored documentation for the observable outputs of a co-simulation model.
///
/// This is a projection cache, not a second authoring surface: the solver
/// adapter builds it from its source document (for example Modelica declaration
/// descriptions and `unit` modifiers). The common telemetry publisher consumes
/// it without knowing which solver produced a value.
#[derive(Component, Debug, Clone, Default)]
pub struct CosimOutputMetadata {
    /// Metadata keyed by the solver's output name.
    pub outputs: HashMap<String, CosimOutputDescriptor>,
}

/// Human-facing metadata for one observable co-simulation output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosimOutputDescriptor {
    /// Authored explanation of the value. `None` means the model did not state
    /// one; consumers must not synthesize an explanation from its identifier.
    pub description: Option<String>,
    /// Authored engineering unit, if the model declares one.
    pub unit: Option<String>,
    /// Origin of this authored metadata, such as `"modelica"`.
    pub provenance: String,
    /// Canonical user-facing signal path. `None` keeps the generic `sim.*`
    /// namespace for non-generated co-simulation models; generated USD
    /// networks fill this from their source-to-wrapper map.
    pub canonical_name: Option<String>,
    /// Authored component path that owns this value, when the solver wrapper
    /// publishes several model domains through one runtime entity.
    pub group_path: Option<String>,
}

/// A co-simulation model on an entity.
///
/// Created by engine plugins (e.g., `lunco-modelica`) when a model is loaded/compiled.
/// The co-simulation bridge reads from `inputs`, writes to `outputs`, and never
/// cares which engine produces the values.
///
/// ## Input/Output Flow
///
/// ```text
/// Other models ──wire──→ inputs  ──engine──→ outputs ──wire──→ Other models
/// ```
///
/// ## Example
///
/// A balloon Modelica model:
/// ```text
/// SimComponent {
///     model_name: "Balloon",
///     inputs:  { height: 1200.0, velocity: 3.2, g: 9.81 },
///     outputs: { netForce: 49.0, volume: 85.0 },
///     parameters: { maxVolume: 100.0, mass: 5.0 },
/// }
/// ```
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SimComponent {
    /// Human-readable model identifier (for logs, UI).
    pub model_name: String,
    /// Input connectors — values received from wires or other models.
    ///
    /// These are read by the engine during `step()` to compute new outputs.
    pub inputs: HashMap<String, f64>,
    /// Output connectors — values produced by the model.
    ///
    /// Other models and Avian read these through [`crate::SimConnection`] connections.
    pub outputs: HashMap<String, f64>,
    /// Compile-time parameters — set before simulation starts.
    ///
    /// Unlike inputs, these typically don't change during simulation
    /// (though engines may support runtime parameter updates).
    pub parameters: HashMap<String, f64>,
    /// Current simulation status.
    pub status: SimStatus,
    /// Prevents duplicate step commands while waiting for results.
    ///
    /// "A step is in flight" is a FLAG, not a status: it is orthogonal to whether
    /// the model is running, paused or errored, and the two spellings drifted —
    /// a `SimStatus::Stepping` variant existed alongside it that no engine ever
    /// produced, so `can_step()` guarded a state nothing could be in.
    pub is_stepping: bool,
}

impl Default for SimComponent {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            inputs: HashMap::default(),
            outputs: HashMap::default(),
            parameters: HashMap::default(),
            status: SimStatus::Idle,
            is_stepping: false,
        }
    }
}

/// Current status of a [`crate::SimComponent`].
#[derive(Debug, Clone, PartialEq, Default, Reflect)]
pub enum SimStatus {
    /// Model is loaded but not yet run.
    #[default]
    Idle,
    /// Model is being compiled (Modelica) or loaded (FMU).
    Compiling,
    /// Model is running normally.
    Running,
    /// Model is paused — outputs hold last values.
    Paused,
    /// Model encountered an error.
    Error(String),
}

impl SimStatus {
    /// Returns true if the model can accept step commands.
    pub fn can_step(&self) -> bool {
        matches!(self, SimStatus::Running | SimStatus::Idle)
    }
}
