use bevy::prelude::*;
use lunco_doc::{Document, DocumentId, DocumentOp, DocumentError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Supported scripting languages for Digital Twin integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum ScriptLanguage {
    Python,
    Lua,
}

/// A canonical document representing a script in the Digital Twin.
///
/// Mirroring Modelica models, ScriptDocuments are mutable, reversible,
/// and define the logical "Plant" for a subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptDocument {
    pub id: u64,
    pub generation: u64,
    pub language: ScriptLanguage,
    pub source: String,
    /// Metadata about expected input pins (e.g., "battery_voltage").
    pub inputs: Vec<String>,
    /// Metadata about expected output pins (e.g., "motor_current").
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScriptOp {
    SetSource(String),
    AddInput(String),
    RemoveInput(String),
    AddOutput(String),
    RemoveOutput(String),
}

impl DocumentOp for ScriptOp {}

impl Document for ScriptDocument {
    type Op = ScriptOp;

    fn id(&self) -> DocumentId {
        DocumentId::new(self.id)
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn apply(&mut self, op: ScriptOp) -> Result<ScriptOp, DocumentError> {
        if self.language == ScriptLanguage::Python && crate::python::get_python_status() != crate::python::PythonStatus::Available {
            return Err(DocumentError::ValidationFailed("Python is not available on this system. Editing Python scripts is disabled.".to_string()));
        }

        let inverse = match op {
            ScriptOp::SetSource(new_source) => {
                let old = self.source.clone();
                self.source = new_source;
                ScriptOp::SetSource(old)
            }
            ScriptOp::AddInput(name) => {
                if self.inputs.contains(&name) {
                    return Err(DocumentError::ValidationFailed(format!("Input '{}' already exists", name)));
                }
                self.inputs.push(name.clone());
                ScriptOp::RemoveInput(name)
            }
            ScriptOp::RemoveInput(name) => {
                let pos = self.inputs.iter().position(|x| x == &name)
                    .ok_or_else(|| DocumentError::ValidationFailed(format!("Input '{}' not found", name)))?;
                self.inputs.remove(pos);
                ScriptOp::AddInput(name)
            }
            ScriptOp::AddOutput(name) => {
                if self.outputs.contains(&name) {
                    return Err(DocumentError::ValidationFailed(format!("Output '{}' already exists", name)));
                }
                self.outputs.push(name.clone());
                ScriptOp::RemoveOutput(name)
            }
            ScriptOp::RemoveOutput(name) => {
                let pos = self.outputs.iter().position(|x| x == &name)
                    .ok_or_else(|| DocumentError::ValidationFailed(format!("Output '{}' not found", name)))?;
                self.outputs.remove(pos);
                ScriptOp::AddOutput(name)
            }
        };
        self.generation += 1;
        Ok(inverse)
    }
}

/// Runtime component that attaches a scripted subsystem to an entity.
///
/// Mirroring `ModelicaModel`, this component holds the runtime state
/// and a reference to the canonical `ScriptDocument`.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct ScriptedModel {
    pub document_id: Option<u64>,
    pub language: Option<ScriptLanguage>,
    pub paused: bool,
    /// Current input values synced from Bevy ECS to Script.
    pub inputs: HashMap<String, f64>,
    /// Current output values synced from Script to Bevy ECS.
    pub outputs: HashMap<String, f64>,
}

impl Default for ScriptLanguage {
    fn default() -> Self {
        Self::Python
    }
}
