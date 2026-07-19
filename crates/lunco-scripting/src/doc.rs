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
    /// Where this script came from + whether it can be saved in place.
    /// Drives persistence (Twin save/load) and the read-only guard in
    /// [`apply`](Self::apply) — mirrors `ModelicaDocument`. Per-entity
    /// scenarios attached via `RunScenario` start `Untitled`; tool-library
    /// files loaded from the Twin carry a writable `File` origin.
    #[serde(default = "default_origin")]
    pub origin: DocumentOrigin,
    /// Metadata about expected input pins (e.g., "battery_voltage"). Drives the
    /// cosim port graph (`lunco-usd-sim`) for legacy Python scripted models.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Metadata about expected output pins (e.g., "motor_current").
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Scenario parameters as a JSON object string (e.g. `{"speed":1.5}`), empty
    /// for none. Injected into the runtime so the script reads them as a `params`
    /// constant (`params.speed`) — lets one scenario be reused across entities /
    /// missions without baking values into the source. Stored as text so this
    /// (always-compiled) module needs no `serde_json` dep.
    #[serde(default)]
    pub params: String,
    /// Canonical asset id this script was loaded from (`twin://ep1/main.rhai`),
    /// or `None` when the source is not file-backed (inline USD `lunco:script`,
    /// a `RunScenario` string, a generated timeline executor).
    ///
    /// This is the script's LOCATION, which is a different thing from
    /// [`origin`](Self::origin): `origin` answers "can this be saved, and where",
    /// while this answers "what does a relative reference INSIDE it mean". The
    /// scenario runtime stamps it onto the compiled `AST` (`AST::set_source`),
    /// which is what rhai passes to `ModuleResolver::resolve` as the importing
    /// script's id — so `import "shot_camera"` next to `main.rhai` resolves to
    /// `twin://ep1/shot_camera.rhai` instead of failing.
    ///
    /// `None` is load-bearing, not a missing value: a script with no location
    /// must NOT have relative imports silently anchored to some default root.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Generation this document was last written to (or read from) disk at.
    /// `None` = never saved (untitled) ⇒ always dirty. Drives
    /// [`is_dirty`](Self::is_dirty), and therefore whether a re-open is allowed
    /// to refresh this document from disk or must preserve the user's unsaved
    /// work. Mirrors `ModelicaDocument` / `UsdDocument`.
    ///
    /// `serde(default)` ⇒ documents persisted before this field existed load as
    /// `None` (dirty), which is the safe direction: a re-open will preserve them
    /// rather than silently overwrite from disk.
    #[serde(default)]
    pub last_saved_generation: Option<u64>,
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
            origin: DocumentOrigin::untitled(format!("Untitled-{id}")),
            inputs: Vec::new(),
            outputs: Vec::new(),
            params: String::new(),
            // Not file-backed until something says otherwise.
            asset_id: None,
            // Untitled = never on disk ⇒ genuinely unsaved.
            last_saved_generation: None,
        }
    }

    /// Whether the document has unsaved changes — i.e. whether memory has
    /// **deliberately** diverged from disk. A clean document is a cache of the
    /// file and must never be trusted over it; a dirty one IS the truth.
    pub fn is_dirty(&self) -> bool {
        match self.last_saved_generation {
            Some(g) => g != self.generation,
            None => true,
        }
    }

    /// Mark the current generation as matching disk (after a save, or after
    /// re-reading the file).
    pub fn mark_saved(&mut self) {
        self.last_saved_generation = Some(self.generation);
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

/// `ScriptOp` participates in the canonical Twin journal. `ScriptOp` derives
/// `Serialize`/`Deserialize`, so a [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder)
/// attached to a `ScriptDocument` host records the **real op** (lossless,
/// replayable) — a live source edit (rover behaviour change) or an input/output
/// pin change enters the journal exactly like a Modelica or USD edit. This is
/// the "scripts sync by default" bridge; the domain tag routes replay.
impl lunco_twin_journal::OpPayload for ScriptOp {
    fn domain(&self) -> lunco_twin_journal::DomainKind {
        lunco_twin_journal::DomainKind::Script
    }
    // `referenced_entities` stays the default empty set — an `EntityRef` also
    // needs the owning `DocumentId`, which the op alone doesn't carry. Same
    // stance as the USD / Modelica `OpPayload` impls; conflict-detection
    // enrichment lands on the multi-user replication path.
}

/// The identity contract — the same one USD and Modelica implement, so
/// [`DocumentRegistry`](lunco_doc_bevy::DocumentRegistry) enforces
/// one-document-per-file for scripts without knowing anything about rhai.
///
/// ⚠ **THIS TYPE IS NOT READY FOR MULTI-USER**, and the registry cannot fix it.
/// Identity/refresh/dirty are solved here; *merging* is not, and it's decided by
/// op ADDRESSING, not by this trait:
///
/// * [`ScriptOp::SetSource`] carries the **whole file**. Two people editing one
///   script replay through `DocumentRegistry::replay_op` as last-writer-wins over
///   the entire text — the loser's work vanishes silently. Omniverse's `.live`
///   layer gets away with LWW because its deltas are per-PROPERTY.
/// * The pin ops (`AddInput`/`RemoveOutput`/…) are name-addressed and DO merge.
///
/// Fixing it means addressing edits at something smaller than the file (e.g.
/// `SetFunction { name, body }`), so two people touching different functions
/// never collide. Until then a script is single-writer; the journal will happily
/// record and replay ops that destroy each other.
impl lunco_doc::FileBacked for ScriptDocument {
    fn with_origin(id: DocumentId, source: String, origin: DocumentOrigin) -> Self {
        // Language is inferred from the origin's extension where there is one —
        // a `.py` file opened from the Twin must not come back as rhai.
        let language = match &origin {
            DocumentOrigin::File { path, .. }
                if path.extension().and_then(|e| e.to_str()) == Some("py") =>
            {
                ScriptLanguage::Python
            }
            _ => ScriptLanguage::Rhai,
        };
        let mut doc = ScriptDocument::new(id.raw(), language, source);
        // Clean ONLY if the source came from somewhere durable. An Untitled doc
        // has never been on disk, so it is genuinely unsaved — marking it clean
        // would let a re-open silently discard it and would hide it from a
        // save-on-quit prompt. Mirrors `UsdDocument::with_origin`.
        doc.last_saved_generation = match &origin {
            DocumentOrigin::File { .. } | DocumentOrigin::Bundled { .. } => Some(doc.generation),
            DocumentOrigin::Untitled { .. } => None,
        };
        doc.origin = origin;
        doc
    }

    fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    fn is_dirty(&self) -> bool {
        ScriptDocument::is_dirty(self)
    }

    fn reload_base(&mut self, source: &str) -> bool {
        if self.source == source {
            return true;
        }
        // Through the op, so generation/undo/journal stay coherent.
        let _ = Document::apply(self, ScriptOp::SetSource(source.to_string()));
        self.mark_saved();
        // Always `true`: a script with a syntax error is still a script you must
        // be able to open and fix. Compilation reports the error later.
        true
    }
}

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
    fn script_op_declares_script_domain() {
        use lunco_twin_journal::{DomainKind, OpPayload};
        assert_eq!(ScriptOp::SetSource("x".into()).domain(), DomainKind::Script);
        assert_eq!(ScriptOp::AddInput("p".into()).domain(), DomainKind::Script);
    }

    #[test]
    fn despawn_scripted_model_keeps_document_registered() {
        let mut app = App::new();
        app.init_resource::<crate::ScriptRegistry>();
        app.add_observer(crate::on_close_script_document);

        let id = DocumentId::new(42);
        app.world_mut()
            .resource_mut::<crate::ScriptRegistry>()
            .insert_document(id, ScriptDocument::new(42, ScriptLanguage::Rhai, "print(1);"));

        let entity = app
            .world_mut()
            .spawn(ScriptedModel {
                document_id: Some(42),
                language: Some(ScriptLanguage::Rhai),
                ..Default::default()
            })
            .id();
        app.update();

        app.world_mut().entity_mut(entity).despawn();
        app.update();

        assert!(
            app.world()
                .resource::<crate::ScriptRegistry>()
                .documents
                .contains_key(&id),
            "despawning a ScriptedModel entity must not close its script document"
        );
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
