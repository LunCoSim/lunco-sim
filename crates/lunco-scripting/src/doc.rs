use bevy::prelude::*;
use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Supported scripting languages for Digital Twin integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum ScriptLanguage {
    Python,
    Lua,
    /// Pure-Rust embedded engine (rhai). The default browser-capable backend.
    Rhai,
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
    /// Where this script came from + whether it can be saved in place.
    /// Drives persistence (Twin save/load) and the read-only guard in
    /// [`apply`](Self::apply) — mirrors `ModelicaDocument`. Per-entity
    /// scenarios attached via `RunScenario` start `Untitled`; tool-library
    /// files loaded from the Twin carry a writable `File` origin.
    #[serde(default = "default_origin")]
    pub origin: DocumentOrigin,
    /// Scenario parameters as a JSON object string (e.g. `{"speed":1.5}`), empty
    /// for none. Injected into the runtime so the script reads them as a `params`
    /// constant (`params.speed`) — lets one scenario be reused across entities /
    /// missions without baking values into the source. Stored as text so this
    /// (always-compiled) module needs no `serde_json` dep.
    #[serde(default)]
    pub params: String,
}

/// Serde fallback for documents persisted before `origin` existed.
fn default_origin() -> DocumentOrigin {
    DocumentOrigin::untitled("Untitled")
}

impl ScriptDocument {
    /// A new untitled script (the in-session scratch origin — editable, but
    /// needs a Save-As / Twin binding before it can be written to disk).
    pub fn new(id: u64, language: ScriptLanguage, source: impl Into<String>) -> Self {
        Self {
            id,
            generation: 0,
            language,
            source: source.into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            origin: DocumentOrigin::untitled(format!("Untitled-{id}")),
            params: String::new(),
        }
    }

    /// Where this document came from.
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    /// Rebind the origin (e.g. after a Save-As binds a file path, or a Twin
    /// load attaches the source file).
    pub fn set_origin(&mut self, origin: DocumentOrigin) {
        self.origin = origin;
    }

    /// Human-readable label for tabs/logs — the file stem, bundled filename,
    /// or the untitled name.
    pub fn display_name(&self) -> String {
        match &self.origin {
            DocumentOrigin::File { path, .. } => path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("script")
                .to_string(),
            DocumentOrigin::Bundled { filename } => filename.clone(),
            DocumentOrigin::Untitled { name } => name.clone(),
        }
    }
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
        // Origin-level read-only guard (bundled examples, read-only library
        // tool files). `accepts_mutations()` — NOT `is_writable()` — so untitled
        // scratch scripts stay editable. Mirrors `ModelicaDocument::apply`.
        if !self.origin.accepts_mutations() {
            return Err(DocumentError::ValidationFailed(format!(
                "Script '{}' is read-only",
                self.display_name()
            )));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untitled_script_is_editable() {
        let mut doc = ScriptDocument::new(1, ScriptLanguage::Rhai, "print(1);");
        assert!(matches!(doc.origin(), DocumentOrigin::Untitled { .. }));
        // Untitled scratch scripts accept mutations and bump the generation.
        let inv = doc.apply(ScriptOp::SetSource("print(2);".into())).unwrap();
        assert_eq!(doc.source, "print(2);");
        assert_eq!(doc.generation, 1);
        assert!(matches!(inv, ScriptOp::SetSource(s) if s == "print(1);"));
    }

    #[test]
    fn readonly_file_origin_rejects_edits() {
        let mut doc = ScriptDocument::new(2, ScriptLanguage::Rhai, "x");
        doc.set_origin(DocumentOrigin::readonly_file("/libs/formation.rhai"));
        assert_eq!(doc.display_name(), "formation");
        let err = doc.apply(ScriptOp::SetSource("y".into()));
        assert!(matches!(err, Err(DocumentError::ValidationFailed(_))));
        // Source unchanged, generation not bumped.
        assert_eq!(doc.source, "x");
        assert_eq!(doc.generation, 0);
    }

    #[test]
    fn writable_file_origin_allows_edits() {
        let mut doc = ScriptDocument::new(3, ScriptLanguage::Rhai, "a");
        doc.set_origin(DocumentOrigin::writable_file("/twin/tools/nav.rhai"));
        assert_eq!(doc.display_name(), "nav");
        doc.apply(ScriptOp::SetSource("b".into())).unwrap();
        assert_eq!(doc.source, "b");
    }
}
