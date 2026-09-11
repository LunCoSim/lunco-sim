//! Operation and change types for Modelica documents.

use crate::pretty::{self, ComponentDecl, ConnectEquation, Placement, PortRef};
use std::ops::Range;

/// How many structured changes the document retains for consumer
/// polling.
pub const CHANGE_HISTORY_CAPACITY: usize = 256;

/// A structural change to a [`crate::document::ModelicaDocument`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ModelicaChange {
    /// Text-level change or undo/redo.
    TextReplaced,
    /// A component was added to `class`.
    ComponentAdded {
        /// Qualified class name (supports dotted nested paths).
        class: String,
        /// Instance name of the new component.
        name: String,
    },
    /// A component was removed from `class`.
    ComponentRemoved {
        /// Qualified class name.
        class: String,
        /// Instance name that was removed.
        name: String,
    },
    /// A connect equation was added to `class`'s equation section.
    ConnectionAdded {
        /// Qualified class name.
        class: String,
        /// Source port.
        from: PortRef,
        /// Target port.
        to: PortRef,
    },
    /// A connect equation was removed from `class`'s equation section.
    ConnectionRemoved {
        /// Qualified class name.
        class: String,
        /// Source port.
        from: PortRef,
        /// Target port.
        to: PortRef,
    },
    /// The `annotation(Line(points={...}))` of a `connect(...)` was set
    /// or cleared.
    ConnectionLineChanged {
        /// Qualified class name.
        class: String,
        /// Source port.
        from: PortRef,
        /// Target port.
        to: PortRef,
    },
    /// Style fields of `Line(...)` (color, thickness, smooth) were
    /// edited on a `connect(...)` equation.
    ConnectionLineStyleChanged {
        class: String,
        from: PortRef,
        to: PortRef,
    },
    /// Source/target swapped on a `connect(...)` equation.
    ConnectionReversed {
        class: String,
        from: PortRef,
        to: PortRef,
    },
    /// A component's `Placement` annotation was set or replaced.
    PlacementChanged {
        /// Qualified class name.
        class: String,
        /// Component instance name.
        component: String,
        /// The placement now in effect.
        placement: Placement,
    },
    /// A component's parameter modification was set or replaced.
    ParameterChanged {
        /// Qualified class name.
        class: String,
        /// Component instance name.
        component: String,
        /// Parameter name.
        param: String,
        /// Replacement value expression (emitted verbatim).
        value: String,
    },
    /// A class was added.
    ClassAdded {
        /// Fully-qualified class name.
        qualified: String,
        /// Class kind keyword (`model`, `block`, `connector`, ...).
        kind: pretty::ClassKindSpec,
    },
    /// A class was removed.
    ClassRemoved {
        /// Fully-qualified class name that no longer exists.
        qualified: String,
    },
    /// A class was renamed in place (e.g. user retyped the class
    /// header in the text editor). Identity-preserving — emitted
    /// instead of a (`ClassRemoved`, `ClassAdded`) pair when the
    /// rebuild can confidently match one removed class to one
    /// added class on the same generation.
    ///
    /// Consumers keyed by class name (open tabs, experiment
    /// records, parameter drafts, MSL caches…) re-key from `old`
    /// to `new` instead of dropping state.
    ClassRenamed {
        /// Prior fully-qualified class name.
        old: String,
        /// New fully-qualified class name.
        new: String,
    },
}

/// The op type for [`crate::document::ModelicaDocument`].
///
/// Derives `Serialize`/`Deserialize` so the canonical Twin journal records the
/// **real op** (lossless, replayable) via `record_op` — see [`crate::journal`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ModelicaOp {
    /// Replace the entire source buffer.
    ReplaceSource {
        /// The new source text to install.
        new: String,
    },
    /// Replace a byte range with new text.
    EditText {
        /// Byte range in the current source buffer to replace.
        range: Range<usize>,
        /// Replacement text.
        replacement: String,
    },
    /// Append a new component declaration to the body of `class`.
    AddComponent {
        /// Target class name.
        class: String,
        /// Declaration payload.
        decl: ComponentDecl,
    },
    /// Append a new `connect(...)` equation inside `class`'s equation
    /// section.
    AddConnection {
        /// Target class name.
        class: String,
        /// Equation payload.
        eq: ConnectEquation,
    },
    /// Remove a component declaration from `class` by instance name.
    RemoveComponent {
        /// Target class name.
        class: String,
        /// Instance name to remove.
        name: String,
    },
    /// Remove a `connect(...)` equation from `class`.
    RemoveConnection {
        /// Target class name.
        class: String,
        /// One endpoint of the connection.
        from: pretty::PortRef,
        /// Other endpoint.
        to: pretty::PortRef,
    },
    /// Set or clear the `annotation(Line(points={...}))` of an existing
    /// `connect(...)` equation.
    SetConnectionLine {
        /// Target class name.
        class: String,
        /// One endpoint of the connection.
        from: pretty::PortRef,
        /// Other endpoint.
        to: pretty::PortRef,
        /// Polyline points in Modelica diagram coordinates.
        points: Vec<(f32, f32)>,
    },
    /// Edit one or more style fields of an existing `connect(...)`
    /// `Line(...)` annotation.
    SetConnectionLineStyle {
        class: String,
        from: pretty::PortRef,
        to: pretty::PortRef,
        color: Option<[u8; 3]>,
        thickness: Option<f64>,
        smooth_bezier: Option<bool>,
    },
    /// Swap `from`/`to` on an existing `connect(...)` equation.
    ReverseConnection {
        class: String,
        from: pretty::PortRef,
        to: pretty::PortRef,
    },
    /// Set or replace the `Placement` annotation on a component.
    SetPlacement {
        /// Target class name.
        class: String,
        /// Component instance name.
        name: String,
        /// New placement.
        placement: pretty::Placement,
    },
    /// Set or replace a parameter modification on a component.
    SetParameter {
        /// Target class name.
        class: String,
        /// Component instance name.
        component: String,
        /// Parameter / modifier name.
        param: String,
        /// Replacement value expression (emitted verbatim).
        value: String,
    },
    /// Append a `LunCoAnnotations.PlotNode(...)` record to the
    /// class's `annotation(__LunCo(plotNodes={...}))` array.
    AddPlotNode {
        /// Target class name.
        class: String,
        /// Plot annotation to add or update.
        plot: pretty::LunCoPlotNodeSpec,
    },
    /// Remove the first `LunCoAnnotations.PlotNode(...)` whose
    /// `signal=` matches `signal_path` from the class's
    /// `annotation(__LunCo(plotNodes=...))` array.
    RemovePlotNode {
        /// Target class name.
        class: String,
        /// Signal path identifying which plot entry to remove.
        signal_path: String,
    },
    /// Update the `extent={{...}}` argument of the first
    /// `LunCoAnnotations.PlotNode(...)` whose `signal=` matches
    /// `signal_path`.
    SetPlotNodeExtent {
        /// Target class name.
        class: String,
        /// Signal path identifying which plot entry to update.
        signal_path: String,
        /// New rectangle in diagram coordinates.
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    /// Update the `title=` argument (or insert one) on the first
    /// `LunCoAnnotations.PlotNode(...)` whose `signal=` matches
    /// `signal_path`.
    SetPlotNodeTitle {
        /// Target class name.
        class: String,
        /// Signal path identifying which plot entry to update.
        signal_path: String,
        /// New title (empty → remove the `title=` field).
        title: String,
    },
    /// Replace the `extent={{…}}` argument of the i-th `Text(...)`
    /// entry inside the class's `Diagram(graphics)` array.
    SetDiagramTextExtent {
        /// Target class name.
        class: String,
        /// Index of the Text item within `Diagram(graphics={...})`.
        index: usize,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    /// Replace the `textString=` argument of the i-th `Text(...)`
    /// entry inside the class's `Diagram(graphics)` array.
    SetDiagramTextString {
        /// Target class name.
        class: String,
        /// Index of the Text item within `Diagram(graphics={...})`.
        index: usize,
        /// New `textString=` value (the writer adds the quotes).
        text: String,
    },
    /// Remove the i-th `Text(...)` entry from the class's
    /// `Diagram(graphics)` array.
    RemoveDiagramText {
        /// Target class name.
        class: String,
        /// Index of the Text item within `Diagram(graphics={...})`.
        index: usize,
    },

    // ── Layer 2: full class authoring ───────────────────────────────────────
    /// Add a new empty class (model/connector/package/...) inside `parent`.
    AddClass {
        parent: String,
        name: String,
        kind: pretty::ClassKindSpec,
        description: String,
        partial: bool,
    },
    /// Remove a class by qualified name.
    RemoveClass { qualified: String },
    /// Add a short-class definition.
    AddShortClass {
        parent: String,
        name: String,
        kind: pretty::ClassKindSpec,
        base: String,
        prefixes: Vec<String>,
        modifications: Vec<(String, String)>,
    },
    /// Add a variable declaration to a class body.
    AddVariable {
        class: String,
        decl: pretty::VariableDecl,
    },
    /// Remove a variable declaration by name.
    RemoveVariable { class: String, name: String },
    /// Append an equation to a class equation section.
    AddEquation {
        class: String,
        eq: pretty::EquationDecl,
    },
    /// Append a graphic to the class's `annotation(Icon(graphics={...}))`.
    AddIconGraphic {
        class: String,
        graphic: pretty::GraphicSpec,
    },
    /// Append a graphic to the class's `annotation(Diagram(graphics={...}))`.
    AddDiagramGraphic {
        class: String,
        graphic: pretty::GraphicSpec,
    },
    /// Set or replace the `experiment(...)` argument inside the class
    /// annotation.
    SetExperimentAnnotation {
        class: String,
        start_time: f64,
        stop_time: f64,
        tolerance: f64,
        interval: f64,
    },
}

impl lunco_doc::DocumentOp for ModelicaOp {}

impl ModelicaOp {
    /// Classify this op as either a **structured edit** or a **raw text edit**.
    pub fn classify(&self) -> OpKind {
        match self {
            Self::ReplaceSource { .. } | Self::EditText { .. } => OpKind::Text,
            _ => OpKind::Structured,
        }
    }
}

/// Op classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// Raw text edit.
    Text,
    /// Structured (AST-canonical) edit.
    Structured,
}

/// The "fresh AST" channel between `op_to_patch` and
/// `ModelicaDocument::apply_patch`.
#[derive(Debug)]
pub enum FreshAst {
    /// Structured op handed back this freshly-mutated AST.
    Mutated(std::sync::Arc<rumoca_compile::parsing::ast::StoredDefinition>),
    /// Raw text edit — no fresh AST.
    TextEdit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pretty::{ComponentDecl, Placement};

    /// A2: structured ops serialize **losslessly** — `serde_json` round-trips
    /// an `AddComponent` (nested `ComponentDecl` + `Placement`) back to an
    /// equal op, so the canonical Twin journal records replayable entries
    /// instead of the old lossy summaries.
    #[test]
    fn modelica_op_serde_round_trips_losslessly() {
        let op = ModelicaOp::AddComponent {
            class: "Circuit".to_string(),
            decl: ComponentDecl {
                type_name: "Resistor".to_string(),
                name: "R1".to_string(),
                modifications: vec![("R".to_string(), "100".to_string())],
                placement: Some(Placement::at(10.0, -20.0)),
            },
        };
        let json = serde_json::to_value(&op).expect("serialize");
        let back: ModelicaOp = serde_json::from_value(json).expect("deserialize");
        assert_eq!(op, back);
    }
}
