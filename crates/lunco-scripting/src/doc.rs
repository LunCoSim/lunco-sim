use bevy::prelude::*;
use lunco_core::TelemetryValue;
use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

/// Supported scripting languages for Digital Twin integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect, Default)]
pub enum ScriptLanguage {
    /// Pure-Rust embedded engine (rhai). The default browser-capable backend.
    #[default]
    Rhai,
    Python,
}

/// Defines how an independently hosted scenario behaves when the active scene
/// is replaced. The policy is attached to the scenario launch, so scene
/// transitions stay generic and other authored flows can reuse the same
/// lifecycle contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Reflect, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioReloadPolicy {
    /// Keep the scenario's host, state, and compiled program alive.
    #[default]
    Retain,
    /// Re-run the scenario's `on_start` after the replacement scene is ready.
    Restart,
}

/// Typed launch parameters for one scenario instance.
///
/// The command/API boundary serializes this as one natural JSON object. Once
/// accepted, the value is kept as [`TelemetryValue`] and converted directly to
/// the target scripting backend; no JSON string or JSON round-trip is used by
/// the runtime. The empty object is the only semantic default.
#[derive(Debug, Clone, Default, PartialEq, Reflect)]
#[reflect(Serialize, Deserialize)]
pub struct ScenarioParameters(BTreeMap<String, TelemetryValue>);

impl ScenarioParameters {
    /// Borrow the validated parameter map for backend conversion or inspection.
    pub fn as_map(&self) -> &BTreeMap<String, TelemetryValue> {
        &self.0
    }

    /// Convert the parameters to the shared typed value used by scripting
    /// bridges. This clone happens once per program compilation, not per tick.
    pub fn as_telemetry_value(&self) -> TelemetryValue {
        TelemetryValue::Map(self.0.clone())
    }
}

/// Serialize a telemetry value using the natural object/array/scalar shape
/// expected at command and storage boundaries. `TelemetryValue` itself keeps
/// its tagged serde representation for the generic telemetry protocol, so this
/// adapter is deliberately local to scenario launch parameters.
struct NaturalTelemetryValue<'a>(&'a TelemetryValue);

impl Serialize for NaturalTelemetryValue<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self.0 {
            TelemetryValue::F64(value) => serializer.serialize_f64(*value),
            TelemetryValue::I64(value) => serializer.serialize_i64(*value),
            TelemetryValue::Bool(value) => serializer.serialize_bool(*value),
            TelemetryValue::String(value) => serializer.serialize_str(value),
            TelemetryValue::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&NaturalTelemetryValue(value))?;
                }
                seq.end()
            }
            TelemetryValue::Map(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(key, &NaturalTelemetryValue(value))?;
                }
                map.end()
            }
        }
    }
}

struct NaturalTelemetryValueVisitor;

impl<'de> Visitor<'de> for NaturalTelemetryValueVisitor {
    type Value = TelemetryValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON scalar, array, or object (null is not supported)")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(TelemetryValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(TelemetryValue::I64(value))
    }

    fn visit_f32<E>(self, value: f32) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_f64(value as f64)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        i64::try_from(value)
            .map(TelemetryValue::I64)
            .map_err(|_| E::custom("unsigned parameter exceeds i64 range"))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.is_finite() {
            Ok(TelemetryValue::F64(value))
        } else {
            Err(E::custom("scenario parameters must contain finite numbers"))
        }
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(TelemetryValue::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(TelemetryValue::String(value))
    }

    fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = access.next_element::<NaturalTelemetryValueOwned>()? {
            values.push(value.0);
        }
        Ok(TelemetryValue::Array(values))
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some((key, value)) = access.next_entry::<String, NaturalTelemetryValueOwned>()? {
            values.insert(key, value.0);
        }
        Ok(TelemetryValue::Map(values))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::custom("null is not a valid scenario parameter"))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::custom("null is not a valid scenario parameter"))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

struct NaturalTelemetryValueOwned(TelemetryValue);

impl<'de> Deserialize<'de> for NaturalTelemetryValueOwned {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer
            .deserialize_any(NaturalTelemetryValueVisitor)
            .map(Self)
    }
}

impl<'de> Deserialize<'de> for ScenarioParameters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ParametersVisitor;

        impl<'de> Visitor<'de> for ParametersVisitor {
            type Value = ScenarioParameters;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an object of scenario parameters")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) =
                    access.next_entry::<String, NaturalTelemetryValueOwned>()?
                {
                    if values.insert(key.clone(), value.0).is_some() {
                        return Err(de::Error::custom(format!(
                            "duplicate scenario parameter `{key}`"
                        )));
                    }
                }
                Ok(ScenarioParameters(values))
            }
        }

        deserializer.deserialize_map(ParametersVisitor)
    }
}

impl Serialize for ScenarioParameters {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, &NaturalTelemetryValue(value))?;
        }
        map.end()
    }
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
    pub origin: DocumentOrigin,
    /// Metadata about expected input pins (e.g., "battery_voltage"). Drives the
    /// cosim port graph (`lunco-usd-sim`) for Python scripted models.
    pub inputs: Vec<String>,
    /// Metadata about expected output pins (e.g., "motor_current").
    pub outputs: Vec<String>,
    /// Canonical asset id this script was loaded from (`twin://ep1/main.rhai`),
    /// or `None` when the source is not file-backed (inline USD `info:sourceCode`,
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
    pub asset_id: Option<String>,
    /// Generation this document was last written to (or read from) disk at.
    /// `None` = never saved (untitled) ⇒ always dirty. Drives
    /// [`is_dirty`](Self::is_dirty), and therefore whether a re-open is allowed
    /// to refresh this document from disk or must preserve the user's unsaved
    /// work. Mirrors `ModelicaDocument` / `UsdDocument`.
    ///
    pub last_saved_generation: Option<u64>,
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
    /// Replace one UTF-8 byte range in the source buffer.
    EditText {
        /// Inclusive-start, exclusive-end byte range.
        range: Range<usize>,
        /// Replacement UTF-8 text.
        replacement: String,
    },
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

    fn mark_saved(&mut self) {
        ScriptDocument::mark_saved(self);
    }

    fn reload_base(&mut self, source: &str) -> bool {
        if self.source == source {
            // The external source and resident document agree. Re-baseline the
            // watermark even when no content mutation was necessary.
            self.mark_saved();
            return true;
        }

        // This is an authoritative external-source refresh, not a user edit.
        // Replace the base directly so a read-only bundled document can still
        // follow its asset, and so external file changes do not become undoable
        // editor operations. Generation remains the one invalidation signal for
        // every downstream consumer, including the scenario lifecycle driver.
        self.source.clear();
        self.source.push_str(source);
        self.generation += 1;
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
        #[cfg(feature = "python")]
        if self.language == ScriptLanguage::Python
            && crate::python::get_python_status() != crate::python::PythonStatus::Available
        {
            return Err(DocumentError::ValidationFailed(
                "Python is not available on this system. Editing Python scripts is disabled."
                    .to_string(),
            ));
        }

        let inverse = match op {
            ScriptOp::SetSource(new_source) => {
                let old = self.source.clone();
                self.source = new_source;
                ScriptOp::SetSource(old)
            }
            ScriptOp::EditText { range, replacement } => {
                if range.start > range.end || range.end > self.source.len() {
                    return Err(DocumentError::ValidationFailed(format!(
                        "text range {}..{} out of bounds (len={})",
                        range.start,
                        range.end,
                        self.source.len()
                    )));
                }
                if !self.source.is_char_boundary(range.start)
                    || !self.source.is_char_boundary(range.end)
                {
                    return Err(DocumentError::ValidationFailed(format!(
                        "text range {}..{} not on UTF-8 boundaries",
                        range.start, range.end
                    )));
                }
                let old = self.source[range.clone()].to_owned();
                self.source.replace_range(range.clone(), &replacement);
                ScriptOp::EditText {
                    range: range.start..range.start + replacement.len(),
                    replacement: old,
                }
            }
            ScriptOp::AddInput(name) => {
                if self.inputs.contains(&name) {
                    return Err(DocumentError::ValidationFailed(format!(
                        "Input '{}' already exists",
                        name
                    )));
                }
                self.inputs.push(name.clone());
                ScriptOp::RemoveInput(name)
            }
            ScriptOp::RemoveInput(name) => {
                let pos = self.inputs.iter().position(|x| x == &name).ok_or_else(|| {
                    DocumentError::ValidationFailed(format!("Input '{}' not found", name))
                })?;
                self.inputs.remove(pos);
                ScriptOp::AddInput(name)
            }
            ScriptOp::AddOutput(name) => {
                if self.outputs.contains(&name) {
                    return Err(DocumentError::ValidationFailed(format!(
                        "Output '{}' already exists",
                        name
                    )));
                }
                self.outputs.push(name.clone());
                ScriptOp::RemoveOutput(name)
            }
            ScriptOp::RemoveOutput(name) => {
                let pos = self
                    .outputs
                    .iter()
                    .position(|x| x == &name)
                    .ok_or_else(|| {
                        DocumentError::ValidationFailed(format!("Output '{}' not found", name))
                    })?;
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
    /// Scene replacement policy for this scenario's lifecycle host.
    pub reload_policy: ScenarioReloadPolicy,
    pub paused: bool,
    /// Typed launch parameters for this attached program instance. The source
    /// document stays reusable; this is the instance-specific context exposed
    /// to Rhai lifecycle hooks.
    pub parameters: ScenarioParameters,
    /// Changes whenever the attached program receives a new parameter object.
    /// The neutral scenario driver uses this cheap revision to invalidate the
    /// backend instance without cloning the map every tick.
    pub parameters_revision: u64,
    /// Current input values synced from Bevy ECS to Script.
    pub inputs: HashMap<String, f64>,
    /// Current output values synced from Script to Bevy ECS.
    pub outputs: HashMap<String, f64>,
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
    fn text_edit_is_utf8_safe_and_reversible() {
        let mut doc = ScriptDocument::new(3, ScriptLanguage::Rhai, "α = 1;");
        let inverse = doc
            .apply(ScriptOp::EditText {
                range: "α".len().."α = 1".len(),
                replacement: "value".into(),
            })
            .unwrap();
        assert_eq!(doc.source, "αvalue;");
        assert_eq!(doc.generation, 1);
        doc.apply(inverse).unwrap();
        assert_eq!(doc.source, "α = 1;");
        assert_eq!(doc.generation, 2);
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
    fn external_reload_replaces_a_readonly_base_without_becoming_an_edit() {
        let mut doc = ScriptDocument::new(2, ScriptLanguage::Rhai, "v1");
        doc.set_origin(DocumentOrigin::readonly_file("/libs/formation.rhai"));

        assert!(lunco_doc::FileBacked::reload_base(&mut doc, "v2"));
        assert_eq!(doc.source, "v2");
        assert_eq!(doc.generation, 1);
        assert_eq!(doc.last_saved_generation, Some(1));
        assert!(!doc.is_dirty());
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
            .insert_document(
                id,
                ScriptDocument::new(42, ScriptLanguage::Rhai, "print(1);"),
            );

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
