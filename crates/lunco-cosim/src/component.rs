//! Co-simulation model component.
//!
//! Represents any non-Avian simulation model (Modelica, FMU, GMAT, etc.)
//! attached to an entity. Engine plugins create these when models compile.

use bevy::prelude::*;
use std::collections::HashMap;

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
    /// Other models and Avian read these through [`SimWire`] connections.
    pub outputs: HashMap<String, f64>,
    /// Compile-time parameters — set before simulation starts.
    ///
    /// Unlike inputs, these typically don't change during simulation
    /// (though engines may support runtime parameter updates).
    pub parameters: HashMap<String, f64>,
    /// Current simulation status.
    pub status: SimStatus,
    /// Prevents duplicate step commands while waiting for results.
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

/// Current status of a [`SimComponent`].
#[derive(Debug, Clone, PartialEq, Default, Reflect)]
pub enum SimStatus {
    /// Model is loaded but not yet run.
    #[default]
    Idle,
    /// Model is being compiled (Modelica) or loaded (FMU).
    Compiling,
    /// Model is running normally.
    Running,
    /// Waiting for async step result (e.g., GMAT external process).
    Stepping,
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
