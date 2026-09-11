//! Errors from structural AST mutation.

/// Errors from structural AST mutation. Stays small on purpose —
/// callers (e.g. `op_to_patch`) translate these into `DocumentError`.
#[derive(Debug, Clone, PartialEq)]
pub enum AstMutError {
    /// Target class is not in the parsed `StoredDefinition`. Names use
    /// dotted form (`"Foo.Bar.Baz"`) and resolve top-down through nested
    /// `classes` maps.
    ClassNotFound(String),
    /// Component name not present in `class.components`. Often a stale
    /// op against an out-of-date AST snapshot.
    ComponentNotFound {
        /// Class the component was looked up in.
        class: String,
        /// Component name that was missing.
        component: String,
    },
    /// Failed to parse a value fragment into an [`Expression`]. Carries
    /// the offending source text to make UI surfacing easy.
    ValueParseFailed {
        /// The offending value text.
        value: String,
    },
    /// `add_component` was called with a component name that already
    /// exists. Adding a duplicate would silently shadow the existing
    /// declaration in `components: IndexMap`, which is rarely the
    /// caller's intent — surface explicitly so they can decide
    /// (remove-then-add for type changes, `set_parameter` for
    /// modification updates).
    DuplicateComponent {
        /// Class the component was being inserted into.
        class: String,
        /// Component name that already exists.
        component: String,
    },
    /// No `LunCoAnnotations.PlotNode(signal=…)` matched in the class's
    /// `annotation(__LunCo(plotNodes=...))` array.
    PlotNodeNotFound {
        /// Class whose Diagram annotation was searched.
        class: String,
        /// Signal path that was not found.
        signal: String,
    },
    /// `set_diagram_text_*` / `remove_diagram_text` was given an index
    /// past the end of the Text-only sequence in `Diagram(graphics)`.
    DiagramTextIndexOutOfRange {
        /// Class whose Diagram annotation was searched.
        class: String,
        /// Index requested by the caller.
        index: usize,
    },
    /// `add_class` was called with a class name that already exists in
    /// the target parent (or top level). Same rationale as
    /// [`Self::DuplicateComponent`].
    DuplicateClass {
        /// Parent class qualified name, or `"(top-level)"` for the
        /// `StoredDefinition.classes` root.
        parent: String,
        /// Class name that already exists.
        name: String,
    },
    /// `remove_connection` did not find a matching `connect(from, to)`
    /// equation. Direction-sensitive: the canvas emits canonical
    /// direction, so this isn't expected to false-positive in
    /// practice; if it does we'll widen to direction-insensitive match.
    ConnectionNotFound {
        /// Class whose equations were searched.
        class: String,
        /// `component.port` form of the missing source endpoint.
        from: String,
        /// `component.port` form of the missing target endpoint.
        to: String,
    },
    /// A mutation recorded two [`super::edit::Splice`]s claiming the same
    /// bytes. The merge would have to drop one, so we refuse instead: a
    /// silently-dropped splice is exactly the class of bug the splice engine
    /// exists to prevent.
    OverlappingSplice {
        /// Debug form of the earlier range.
        first: String,
        /// Debug form of the range that overlaps it.
        second: String,
    },
    /// A splice pointed past the end of the source — an AST span that no longer
    /// matches the text it was parsed from (a stale AST snapshot).
    SpliceOutOfBounds {
        /// End offset the splice asked for.
        end: usize,
        /// Actual source length.
        len: usize,
    },
    /// A structural anchor could not be located in the source — e.g. a
    /// declaration with no terminating `;`, or a class with no `end` keyword.
    /// Indicates the AST and the source have drifted apart.
    AnchorNotFound {
        /// What we were looking for.
        what: String,
    },
}

impl std::fmt::Display for AstMutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AstMutError::ClassNotFound(name) => write!(f, "class not found: {name}"),
            AstMutError::ComponentNotFound { class, component } => {
                write!(f, "component `{component}` not found in class `{class}`")
            }
            AstMutError::ValueParseFailed { value } => {
                write!(
                    f,
                    "could not parse value `{value}` as a Modelica expression"
                )
            }
            AstMutError::DuplicateComponent { class, component } => write!(
                f,
                "component `{component}` already exists in class `{class}`"
            ),
            AstMutError::DuplicateClass { parent, name } => {
                write!(f, "class `{name}` already exists under `{parent}`")
            }
            AstMutError::PlotNodeNotFound { class, signal } => write!(
                f,
                "no LunCoAnnotations.PlotNode with signal `{signal}` in class `{class}`"
            ),
            AstMutError::DiagramTextIndexOutOfRange { class, index } => write!(
                f,
                "Diagram text index {index} out of range in class `{class}`"
            ),
            AstMutError::ConnectionNotFound { class, from, to } => write!(
                f,
                "connection `connect({from}, {to})` not found in class `{class}`"
            ),
            AstMutError::OverlappingSplice { first, second } => {
                write!(f, "internal: overlapping text splices {first} and {second}")
            }
            AstMutError::SpliceOutOfBounds { end, len } => write!(
                f,
                "internal: text splice ends at {end}, past the {len}-byte source (stale AST)"
            ),
            AstMutError::AnchorNotFound { what } => {
                write!(f, "could not locate {what} in the source (stale AST)")
            }
        }
    }
}

impl std::error::Error for AstMutError {}
