//! `ModelicaDocument` — the Document System representation of one `.mo` file.
//!
//! # Canonicality: source text, AST cached
//!
//! The Document owns the **source text** as its canonical state. Text is what
//! the user types, what lives on disk, and what preserves comments + formatting
//! losslessly — the things both a human code editor and an AI `Edit` tool
//! depend on.
//!
//! Alongside the text, the Document caches a **parsed AST**
//! ([`AstCache`]). The cache is refreshed eagerly after every mutation so
//! panels that need structural access (diagram, parameter inspector,
//! placement extractor) can read `doc.ast()` without reparsing. Parse
//! failures are observable via [`AstCache::result`] — the cache is always
//! present, but it may hold an error.
//!
//! Documents are keyed by [`lunco_doc::DocumentId`] inside
//! [`ui::ModelicaDocumentRegistry`]. Every place that spawns a
//! `ModelicaModel` entity allocates a document in the registry and writes
//! its id into [`crate::ModelicaModel::document`].
//!
//! # Op set
//!
//! Text-level ops (comfortable for human editors and AI text tools):
//!
//! - [`ModelicaOp::ReplaceSource`] — coarse full-buffer swap. Used by
//!   CodeEditor's Compile and by any caller that produces the whole new
//!   source (e.g. template expansion).
//! - [`ModelicaOp::EditText`] — byte-range replacement. Used for granular
//!   text edits that should participate in undo/redo without losing
//!   precision.
//!
//! AST-level ops (planned for Task 4) will splice text via AST-node spans
//! so structural edits from the diagram / parameter panels land as
//! minimal text diffs, preserving surrounding formatting and comments.
//!
//! Inverses: [`ReplaceSource`](ModelicaOp::ReplaceSource)'s inverse carries
//! the previous full source. [`EditText`](ModelicaOp::EditText)'s inverse
//! is another `EditText` against the *new* range with the previous slice
//! as replacement.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin};
use rumoca_phase_parse::parse_to_ast;
use rumoca_session::parsing::ast::StoredDefinition;

use crate::pretty::{self, ComponentDecl, ConnectEquation};

/// Eagerly-refreshed AST cache attached to a [`ModelicaDocument`].
///
/// Every mutation replaces this with a freshly-parsed cache so readers
/// never observe a stale AST. Cheap to clone via `Arc`.
#[derive(Debug, Clone)]
pub struct AstCache {
    /// Document generation at which this cache was produced.
    pub generation: u64,
    /// Parse outcome. `Ok` carries the parsed AST; `Err` carries a
    /// human-readable parser diagnostic (preserved so panels can show
    /// syntax errors without reparsing).
    pub result: Result<Arc<StoredDefinition>, String>,
}

impl AstCache {
    /// Parse `source` into a fresh cache at the given generation.
    pub fn from_source(source: &str, generation: u64) -> Self {
        let result = match parse_to_ast(source, "model.mo") {
            Ok(ast) => Ok(Arc::new(ast)),
            Err(e) => Err(e.to_string()),
        };
        Self { generation, result }
    }

    /// Shortcut: the parsed AST, if parsing succeeded.
    pub fn ast(&self) -> Option<&StoredDefinition> {
        self.result.as_ref().ok().map(|a| a.as_ref())
    }
}

/// The canonical Document representation of one Modelica source file.
///
/// Owns the source text + a [`DocumentOrigin`] describing where it
/// came from (which drives save behavior, tab title, read-only
/// badge) + a parsed-AST cache ([`AstCache`]) refreshed eagerly after
/// every mutation.
#[derive(Debug, Clone)]
pub struct ModelicaDocument {
    id: DocumentId,
    source: String,
    ast: Arc<AstCache>,
    generation: u64,
    origin: DocumentOrigin,
    /// Generation at which the document was last persisted to disk.
    /// `None` means never saved (freshly created in-memory); `Some(g)`
    /// means last saved at generation `g`. See [`is_dirty`](Self::is_dirty).
    last_saved_generation: Option<u64>,
}

impl ModelicaDocument {
    /// Build an in-memory `ModelicaDocument` with the given name as
    /// its Untitled identifier. Starts dirty (never-saved).
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self::with_origin(
            id,
            source,
            DocumentOrigin::untitled(format!("Untitled-{}", id.raw())),
        )
    }

    /// Build a `ModelicaDocument` with an explicit origin.
    ///
    /// For on-disk origins (read-only library entries, writable user
    /// files) the source is assumed to match disk at generation 0, so
    /// the document starts clean. Untitled origins start dirty.
    pub fn with_origin(
        id: DocumentId,
        source: impl Into<String>,
        origin: DocumentOrigin,
    ) -> Self {
        let last_saved_generation = if origin.is_untitled() {
            None
        } else {
            Some(0)
        };
        let source = source.into();
        let ast = Arc::new(AstCache::from_source(&source, 0));
        Self {
            id,
            source,
            ast,
            generation: 0,
            origin,
            last_saved_generation,
        }
    }

    /// The current source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The cached parsed AST. Always present (refreshed after every
    /// mutation), but the inner [`AstCache::result`] may carry a parse
    /// error rather than a successful AST.
    pub fn ast(&self) -> &AstCache {
        &self.ast
    }

    /// Byte length of the source.
    pub fn len(&self) -> usize {
        self.source.len()
    }

    /// True when the source buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// Where this document came from — drives Save behaviour, tab
    /// title, read-only badges. See [`DocumentOrigin`].
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    /// File path the source was loaded from, if any. `None` for
    /// untitled in-memory documents (Save-As required before
    /// write-back). Convenience wrapper over
    /// [`DocumentOrigin::canonical_path`].
    pub fn canonical_path(&self) -> Option<&Path> {
        self.origin.canonical_path()
    }

    /// True when this document is treated as read-only by the UI.
    /// Read-only == library / bundled origin or untitled with no path.
    pub fn is_read_only(&self) -> bool {
        !self.origin.is_writable()
    }

    /// Set or change the origin (e.g. after Save-As binds a path to
    /// an untitled document). Does not touch generation or source.
    pub fn set_origin(&mut self, origin: DocumentOrigin) {
        self.origin = origin;
    }

    /// Back-compat setter: change the canonical path while keeping
    /// the current writability classification. For Untitled docs,
    /// binding a path promotes them to a writable `File` origin.
    pub fn set_canonical_path(&mut self, path: Option<PathBuf>) {
        match path {
            Some(p) => {
                let writable = self.origin.is_writable() || self.origin.is_untitled();
                self.origin = DocumentOrigin::File { path: p, writable };
            }
            None => {
                // Reverting to untitled drops the path; name defaults
                // to the current display name.
                self.origin = DocumentOrigin::untitled(self.origin.display_name());
            }
        }
    }

    /// Whether the document has unsaved changes — current generation
    /// differs from the last-saved one, or it has never been saved.
    pub fn is_dirty(&self) -> bool {
        match self.last_saved_generation {
            Some(g) => g != self.generation,
            None => true,
        }
    }

    /// Record that the document was just persisted at its current
    /// generation. The Save observer calls this after a successful
    /// disk write.
    pub fn mark_saved(&mut self) {
        self.last_saved_generation = Some(self.generation);
    }
}

/// The op type for [`ModelicaDocument`].
///
/// Text-level ops land today. AST-level ops (`SetParameter`,
/// `AddComponent`, `AddConnection`, `SetPlacement`, …) arrive alongside
/// the pretty-printer in a follow-up commit; they will be expressed as
/// span-based [`EditText`](Self::EditText) patches internally so
/// surrounding formatting and comments stay intact.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ModelicaOp {
    /// Replace the entire source buffer. The inverse is another
    /// `ReplaceSource` carrying the previous source text.
    ReplaceSource {
        /// The new source text to install.
        new: String,
    },
    /// Replace a byte range with new text. Used by granular text
    /// editors and by AST-level ops that splice at a span.
    ///
    /// The inverse is another `EditText` whose `range` covers the
    /// newly-inserted text and whose `replacement` is the text that
    /// was removed.
    EditText {
        /// Byte range in the current source buffer to replace.
        /// Must fall on `char` boundaries.
        range: Range<usize>,
        /// Replacement text. May be shorter or longer than `range.len()`.
        replacement: String,
    },
    /// Append a new component declaration to the body of `class`.
    ///
    /// Insertion point is chosen AST-aware: just before the
    /// `equation`/`algorithm` keyword if the class has one, otherwise
    /// just before `end ClassName;`. The decl is rendered via
    /// [`crate::pretty::component_decl`] and spliced as an [`EditText`]
    /// internally, so the inverse is a straightforward deletion
    /// (`EditText` with empty replacement).
    AddComponent {
        /// Target class name (must be a top-level class today — nested
        /// class support is a follow-up). Case-sensitive.
        class: String,
        /// Declaration payload. See [`pretty::ComponentDecl`].
        decl: ComponentDecl,
    },
    /// Append a new `connect(...)` equation inside `class`'s equation
    /// section. Creates an `equation` section if one does not exist.
    ///
    /// Rendered via [`crate::pretty::connect_equation`] and spliced as
    /// an [`EditText`] internally; inverse is the matching deletion.
    AddConnection {
        /// Target class name (top-level today).
        class: String,
        /// Equation payload. See [`pretty::ConnectEquation`].
        eq: ConnectEquation,
    },
    /// Remove a component declaration from `class` by instance name.
    ///
    /// Removes the whole declaration line(s) including the trailing
    /// semicolon and newline. Uses `Component.location` as the span
    /// anchor. Inverse is an [`EditText`] that reinserts the deleted
    /// text verbatim — including any comments / annotations that were
    /// attached to the declaration.
    RemoveComponent {
        /// Target class name (top-level today).
        class: String,
        /// Instance name to remove.
        name: String,
    },
    /// Remove a `connect(from.component.from.port, to.component.to.port)`
    /// equation from `class`. Matches by component+port pair (order
    /// insensitive).
    ///
    /// Spans the full equation including the trailing semicolon and
    /// any trailing `annotation(Line(...))`. Inverse is a byte-exact
    /// [`EditText`] reinsertion.
    RemoveConnection {
        /// Target class name.
        class: String,
        /// One endpoint of the connection.
        from: pretty::PortRef,
        /// Other endpoint.
        to: pretty::PortRef,
    },
    /// Set or replace the `Placement` annotation on a component.
    ///
    /// If the component already has an `annotation(Placement(...))`,
    /// the Placement fragment is replaced in place. Otherwise a fresh
    /// `annotation(Placement(...))` is inserted just before the
    /// declaration's trailing semicolon. Other annotations (Dialog,
    /// Documentation, etc.) are preserved in both cases.
    SetPlacement {
        /// Target class name.
        class: String,
        /// Component instance name.
        name: String,
        /// New placement.
        placement: pretty::Placement,
    },
    /// Set or replace a parameter modification on a component.
    ///
    /// If the component declaration already carries a modifications
    /// list with `param = …`, the right-hand side is replaced. If the
    /// list exists but the param is absent, `param = value` is appended.
    /// If no modifications list exists yet, a `(param = value)` is
    /// inserted after the component name.
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
}

impl DocumentOp for ModelicaOp {}

impl Document for ModelicaDocument {
    type Op = ModelicaOp;

    fn id(&self) -> DocumentId {
        self.id
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError> {
        match op {
            ModelicaOp::AddComponent { class, decl } => {
                let patch = compute_add_component_patch(&self.source, &self.ast, &class, &decl)?;
                self.apply(ModelicaOp::EditText {
                    range: patch.0,
                    replacement: patch.1,
                })
            }
            ModelicaOp::AddConnection { class, eq } => {
                let patch = compute_add_connection_patch(&self.source, &self.ast, &class, &eq)?;
                self.apply(ModelicaOp::EditText {
                    range: patch.0,
                    replacement: patch.1,
                })
            }
            ModelicaOp::RemoveComponent { class, name } => {
                let patch = compute_remove_component_patch(&self.source, &self.ast, &class, &name)?;
                self.apply(ModelicaOp::EditText {
                    range: patch.0,
                    replacement: patch.1,
                })
            }
            ModelicaOp::RemoveConnection { class, from, to } => {
                let patch = compute_remove_connection_patch(&self.source, &self.ast, &class, &from, &to)?;
                self.apply(ModelicaOp::EditText {
                    range: patch.0,
                    replacement: patch.1,
                })
            }
            ModelicaOp::SetPlacement { class, name, placement } => {
                let patch = compute_set_placement_patch(&self.source, &self.ast, &class, &name, &placement)?;
                self.apply(ModelicaOp::EditText {
                    range: patch.0,
                    replacement: patch.1,
                })
            }
            ModelicaOp::SetParameter { class, component, param, value } => {
                let patch = compute_set_parameter_patch(&self.source, &self.ast, &class, &component, &param, &value)?;
                self.apply(ModelicaOp::EditText {
                    range: patch.0,
                    replacement: patch.1,
                })
            }
            ModelicaOp::ReplaceSource { new } => {
                let old = std::mem::replace(&mut self.source, new);
                self.generation = self.generation.saturating_add(1);
                self.ast = Arc::new(AstCache::from_source(&self.source, self.generation));
                Ok(ModelicaOp::ReplaceSource { new: old })
            }
            ModelicaOp::EditText { range, replacement } => {
                if range.start > range.end || range.end > self.source.len() {
                    return Err(DocumentError::ValidationFailed(format!(
                        "EditText range {}..{} out of bounds (len={})",
                        range.start,
                        range.end,
                        self.source.len()
                    )));
                }
                if !self.source.is_char_boundary(range.start)
                    || !self.source.is_char_boundary(range.end)
                {
                    return Err(DocumentError::ValidationFailed(format!(
                        "EditText range {}..{} not on char boundaries",
                        range.start, range.end
                    )));
                }
                let removed: String = self.source[range.clone()].to_string();
                self.source.replace_range(range.clone(), &replacement);
                self.generation = self.generation.saturating_add(1);
                self.ast = Arc::new(AstCache::from_source(&self.source, self.generation));
                let inverse_range = range.start..(range.start + replacement.len());
                Ok(ModelicaOp::EditText {
                    range: inverse_range,
                    replacement: removed,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AST-level op helpers
// ---------------------------------------------------------------------------
//
// These functions turn a high-level AST-level op request into a concrete
// `(range, replacement)` text patch, using the cached AST's token spans
// to locate insertion points. They never mutate the document directly —
// apply() delegates to `EditText`, which gives us uniform undo behavior
// and keeps all source mutation on one code path.
//
// Today we only support top-level classes (looked up by name in
// `StoredDefinition::classes`). Nested classes, qualified paths, and
// more surgical ops (SetParameter, Remove*, SetPlacement, SetLine) are
// deliberate follow-ups.

/// Resolve a top-level class by name, producing a useful error if the
/// AST is in a parse-error state or the class isn't present.
fn resolve_class<'a>(
    ast: &'a AstCache,
    class: &str,
) -> Result<&'a rumoca_session::parsing::ast::ClassDef, DocumentError> {
    let stored = match &ast.result {
        Ok(s) => s.as_ref(),
        Err(msg) => {
            return Err(DocumentError::ValidationFailed(format!(
                "cannot apply AST op while source has a parse error: {}",
                msg
            )));
        }
    };
    stored.classes.get(class).ok_or_else(|| {
        DocumentError::ValidationFailed(format!("class `{}` not found in document", class))
    })
}

/// Return the byte offset of the start of the line containing `byte_pos`.
/// Used to splice whole lines instead of mid-line inserts — keeps the
/// resulting source readable and the patch ranges easy to reason about.
fn line_start_byte(source: &str, byte_pos: usize) -> usize {
    source[..byte_pos.min(source.len())]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Compute the text patch for `AddComponent`.
///
/// Insertion point (first match wins):
///   1. start of the line containing `equation` / `initial equation` /
///      `algorithm` / `initial algorithm` keyword, whichever appears first;
///   2. start of the line containing the `end ClassName;` clause.
///
/// Returns the patch as `(empty_range_at_insertion_point, rendered_decl)`.
fn compute_add_component_patch(
    source: &str,
    ast: &AstCache,
    class: &str,
    decl: &ComponentDecl,
) -> Result<(Range<usize>, String), DocumentError> {
    let class_def = resolve_class(ast, class)?;
    let insertion_byte = class_section_insertion_point(class_def).ok_or_else(|| {
        DocumentError::ValidationFailed(format!(
            "could not locate insertion point in class `{}`",
            class
        ))
    })?;
    let line_start = line_start_byte(source, insertion_byte);
    Ok((line_start..line_start, pretty::component_decl(decl)))
}

/// Compute the text patch for `AddConnection`.
///
/// If the class has an `equation` section, insert the connect equation at
/// the start of the `end` line (appending to the section). If not, insert
/// `equation\n<connect>\n` at the `end` line so a fresh section is created.
fn compute_add_connection_patch(
    source: &str,
    ast: &AstCache,
    class: &str,
    eq: &ConnectEquation,
) -> Result<(Range<usize>, String), DocumentError> {
    let class_def = resolve_class(ast, class)?;

    let end_name_byte = class_def
        .end_name_token
        .as_ref()
        .map(|t| t.location.start as usize)
        .ok_or_else(|| {
            DocumentError::ValidationFailed(format!(
                "class `{}` has no `end` clause location",
                class
            ))
        })?;
    let end_line_start = line_start_byte(source, end_name_byte);

    let connect_line = pretty::connect_equation(eq);
    let replacement = if class_def.equation_keyword.is_some() {
        connect_line
    } else {
        format!("equation\n{}", connect_line)
    };

    Ok((end_line_start..end_line_start, replacement))
}

/// Locate the best byte position to insert a new component declaration
/// into a class — just before the first body-section keyword, or if none
/// exists, just before the class's `end` clause.
fn class_section_insertion_point(
    class_def: &rumoca_session::parsing::ast::ClassDef,
) -> Option<usize> {
    let keyword_positions = [
        class_def.equation_keyword.as_ref(),
        class_def.initial_equation_keyword.as_ref(),
        class_def.algorithm_keyword.as_ref(),
        class_def.initial_algorithm_keyword.as_ref(),
    ];
    let earliest_keyword = keyword_positions
        .into_iter()
        .flatten()
        .map(|t| t.location.start as usize)
        .min();
    if let Some(pos) = earliest_keyword {
        return Some(pos);
    }
    class_def
        .end_name_token
        .as_ref()
        .map(|t| t.location.start as usize)
}

/// Extend a declaration/equation span to swallow leading indentation
/// and a trailing newline, so removal leaves a clean source buffer
/// without a dangling blank line.
fn extend_span_to_whole_lines(source: &str, raw: Range<usize>) -> Range<usize> {
    let line_start = source[..raw.start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    // Extend backward only past whitespace on the same line.
    let preceding = &source[line_start..raw.start];
    let start = if preceding.chars().all(|c| c == ' ' || c == '\t') {
        line_start
    } else {
        raw.start
    };
    // Extend forward to and past the following newline if any.
    let end = source[raw.end..]
        .find('\n')
        .map(|i| raw.end + i + 1)
        .unwrap_or(source.len());
    start..end
}

/// Locate the byte position of the semicolon that ends a declaration /
/// equation whose first token starts at `from_byte`. Respects nested
/// parentheses and braces so a `;` inside `annotation(...)` doesn't
/// fool us.
fn find_statement_terminator(source: &str, from_byte: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth: i32 = 0;
    let mut i = from_byte;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth -= 1,
            b';' if depth <= 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Compute the text patch for `RemoveComponent`.
fn compute_remove_component_patch(
    source: &str,
    ast: &AstCache,
    class: &str,
    name: &str,
) -> Result<(Range<usize>, String), DocumentError> {
    let class_def = resolve_class(ast, class)?;
    let component = class_def.components.get(name).ok_or_else(|| {
        DocumentError::ValidationFailed(format!(
            "component `{}` not found in class `{}`",
            name, class
        ))
    })?;
    let raw_start = component.location.start as usize;
    // Component.location.end sometimes stops before the semicolon
    // depending on rumoca's recording — be conservative and extend
    // via terminator scan.
    let term = find_statement_terminator(source, component.name_token.location.start as usize)
        .ok_or_else(|| {
            DocumentError::ValidationFailed(format!(
                "could not find `;` terminating component `{}`",
                name
            ))
        })?;
    let span = extend_span_to_whole_lines(source, raw_start..(term + 1));
    Ok((span, String::new()))
}

/// Match a `ComponentReference` against a `PortRef` (expected form
/// `component.port`). Returns true when the dotted AST path equals the
/// two-part PortRef pair, in that order.
fn cref_matches_port(
    cref: &rumoca_session::parsing::ast::ComponentReference,
    port: &pretty::PortRef,
) -> bool {
    use rumoca_session::parsing::ast::ComponentRefPart;
    let parts: Vec<&ComponentRefPart> = cref.parts.iter().collect();
    if parts.len() != 2 {
        return false;
    }
    parts[0].ident.text.as_ref() == port.component
        && parts[1].ident.text.as_ref() == port.port
}

/// Compute the text patch for `RemoveConnection`.
fn compute_remove_connection_patch(
    source: &str,
    ast: &AstCache,
    class: &str,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
) -> Result<(Range<usize>, String), DocumentError> {
    use rumoca_session::parsing::ast::Equation;
    let class_def = resolve_class(ast, class)?;
    let eq = class_def
        .equations
        .iter()
        .find(|e| match e {
            Equation::Connect { lhs, rhs } => {
                (cref_matches_port(lhs, from) && cref_matches_port(rhs, to))
                    || (cref_matches_port(lhs, to) && cref_matches_port(rhs, from))
            }
            _ => false,
        })
        .ok_or_else(|| {
            DocumentError::ValidationFailed(format!(
                "connect({}.{}, {}.{}) not found in class `{}`",
                from.component, from.port, to.component, to.port, class
            ))
        })?;
    let start_loc = eq.get_location().ok_or_else(|| {
        DocumentError::Internal("matched connect equation has no location".into())
    })?;
    let raw_start = start_loc.start as usize;
    // Scan backward to the `connect` keyword if it precedes the first
    // component-ref token (it always does for a well-formed connect
    // equation, but ComponentReference.get_location reports the lhs
    // cref's first token).
    let connect_start = source[..raw_start]
        .rfind("connect")
        .filter(|&i| source[i..].starts_with("connect") && i + 7 <= raw_start)
        .unwrap_or(raw_start);
    let term = find_statement_terminator(source, raw_start).ok_or_else(|| {
        DocumentError::ValidationFailed("could not find `;` terminating connect equation".into())
    })?;
    let span = extend_span_to_whole_lines(source, connect_start..(term + 1));
    Ok((span, String::new()))
}

/// Locate a top-level `annotation(` substring inside `[start, end)`,
/// respecting nesting (i.e. must not be inside another parenthesized
/// expression). Returns the byte range covering the whole
/// `annotation(...)` including the outer parens.
fn find_annotation_span(source: &str, span: Range<usize>) -> Option<Range<usize>> {
    let slice = source.get(span.clone())?;
    // Walk the slice tracking paren depth; look for `annotation(` at
    // depth 0.
    let bytes = slice.as_bytes();
    let mut depth: i32 = 0;
    let mut i: usize = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'(' || c == b'{' || c == b'[' {
            depth += 1;
            i += 1;
            continue;
        }
        if c == b')' || c == b'}' || c == b']' {
            depth -= 1;
            i += 1;
            continue;
        }
        if depth == 0 && bytes[i..].starts_with(b"annotation") {
            // Check that the preceding char is not an ident char so we
            // don't match `myannotation(`.
            let prev_ok = i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if prev_ok {
                // Skip the keyword and locate the `(`.
                let mut j = i + "annotation".len();
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'(' {
                    // Find matching `)`.
                    let mut d = 0;
                    let mut k = j;
                    while k < bytes.len() {
                        match bytes[k] {
                            b'(' | b'{' | b'[' => d += 1,
                            b')' | b'}' | b']' => {
                                d -= 1;
                                if d == 0 {
                                    return Some((span.start + i)..(span.start + k + 1));
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    return None;
                }
            }
        }
        i += 1;
    }
    None
}

/// Find the span of the first `Placement(...)` call inside a byte
/// range, matched at top level (paren depth 0 within the range).
fn find_placement_span(source: &str, span: Range<usize>) -> Option<Range<usize>> {
    let slice = source.get(span.clone())?;
    let bytes = slice.as_bytes();
    let mut i: usize = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"Placement") {
            let prev_ok = i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if prev_ok {
                let mut j = i + "Placement".len();
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'(' {
                    let mut d = 0;
                    let mut k = j;
                    while k < bytes.len() {
                        match bytes[k] {
                            b'(' | b'{' | b'[' => d += 1,
                            b')' | b'}' | b']' => {
                                d -= 1;
                                if d == 0 {
                                    return Some((span.start + i)..(span.start + k + 1));
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Compute the text patch for `SetPlacement`.
///
/// Strategy:
///   1. If the component's decl has an `annotation(...)` block and that
///      block contains `Placement(...)`, replace the `Placement` call
///      in place — other annotations (Dialog, Documentation) untouched.
///   2. If the decl has an `annotation(...)` block without `Placement`,
///      prepend `Placement(...), ` inside it.
///   3. If there is no annotation at all, insert
///      ` annotation(Placement(...))` just before the decl's `;`.
fn compute_set_placement_patch(
    source: &str,
    ast: &AstCache,
    class: &str,
    name: &str,
    placement: &pretty::Placement,
) -> Result<(Range<usize>, String), DocumentError> {
    let class_def = resolve_class(ast, class)?;
    let component = class_def.components.get(name).ok_or_else(|| {
        DocumentError::ValidationFailed(format!(
            "component `{}` not found in class `{}`",
            name, class
        ))
    })?;
    let decl_start = component.location.start as usize;
    let term = find_statement_terminator(source, component.name_token.location.start as usize)
        .ok_or_else(|| {
            DocumentError::ValidationFailed("component decl has no terminating `;`".into())
        })?;
    let decl_span = decl_start..term;
    let new_placement = pretty::placement_inner(placement);

    if let Some(ann_span) = find_annotation_span(source, decl_span.clone()) {
        // ann_span covers `annotation(...)` including outer parens.
        // The interior span is (ann_span.start + "annotation(".len() ..
        // ann_span.end - 1).
        let prefix_len = "annotation(".len();
        let inner_start = ann_span.start + prefix_len;
        let inner_end = ann_span.end - 1;
        if let Some(p_span) = find_placement_span(source, inner_start..inner_end) {
            return Ok((p_span, new_placement));
        } else {
            // Insert Placement fragment at the start of the annotation
            // contents, followed by `, ` to keep the remaining entries
            // well-formed.
            let insert_at = inner_start;
            return Ok((
                insert_at..insert_at,
                format!("{}, ", new_placement),
            ));
        }
    }
    // No annotation at all — insert one just before the `;`.
    Ok((
        term..term,
        format!(" annotation({})", new_placement),
    ))
}

/// Compute the text patch for `SetParameter`.
///
/// Locates the component's modifications list (the `(...)` immediately
/// after the instance name). If absent, inserts a fresh
/// `(param=value)`. If present and the param exists, replaces its
/// value. If present and the param is missing, appends `, param=value`.
fn compute_set_parameter_patch(
    source: &str,
    ast: &AstCache,
    class: &str,
    component: &str,
    param: &str,
    value: &str,
) -> Result<(Range<usize>, String), DocumentError> {
    let class_def = resolve_class(ast, class)?;
    let comp = class_def.components.get(component).ok_or_else(|| {
        DocumentError::ValidationFailed(format!(
            "component `{}` not found in class `{}`",
            component, class
        ))
    })?;
    let name_end = comp.name_token.location.end as usize;
    let term = find_statement_terminator(source, name_end).ok_or_else(|| {
        DocumentError::ValidationFailed("component decl has no terminating `;`".into())
    })?;
    // Scan from just after the name token to the terminator looking
    // for `(` before any alphanumeric token (which would indicate an
    // annotation / binding, not modifications).
    let bytes = source.as_bytes();
    let mut i = name_end;
    while i < term {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            b'(' => {
                // Found modifications list. Locate its matching `)`.
                let mut d = 0;
                let mut k = i;
                let close = loop {
                    match bytes[k] {
                        b'(' | b'{' | b'[' => d += 1,
                        b')' | b'}' | b']' => {
                            d -= 1;
                            if d == 0 {
                                break Some(k);
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                    if k >= term {
                        break None;
                    }
                };
                let close = close.ok_or_else(|| {
                    DocumentError::ValidationFailed(
                        "unterminated `(` in component modifications".into(),
                    )
                })?;
                return Ok(modify_mod_list(source, (i + 1)..close, param, value));
            }
            _ => {
                // No modifications list — insert one right after the name.
                let rendered = format!("({}={})", param, value);
                return Ok((name_end..name_end, rendered));
            }
        }
    }
    // Reached terminator without encountering a `(` — insert fresh list.
    let rendered = format!("({}={})", param, value);
    Ok((name_end..name_end, rendered))
}

/// Helper: emit the patch that either updates or appends `param=value`
/// within the top-level modification list occupying `inner_span`
/// (exclusive of the outer parens).
fn modify_mod_list(
    source: &str,
    inner_span: Range<usize>,
    param: &str,
    value: &str,
) -> (Range<usize>, String) {
    let bytes = source.as_bytes();
    // Walk the list at depth 0, splitting entries by `,`. For each
    // entry, check if it starts (after whitespace) with `param` and is
    // followed by `=` or `(` (modification or nested modification).
    let start = inner_span.start;
    let end = inner_span.end;
    let mut entry_start = start;
    let mut d = 0;
    let mut i = start;
    while i < end {
        let c = bytes[i];
        match c {
            b'(' | b'{' | b'[' => d += 1,
            b')' | b'}' | b']' => d -= 1,
            b',' if d == 0 => {
                if let Some(patch) = match_entry(source, entry_start..i, param, value) {
                    return patch;
                }
                entry_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    // Final entry.
    if let Some(patch) = match_entry(source, entry_start..end, param, value) {
        return patch;
    }
    // Not found — append.
    let trimmed_end = {
        let mut e = end;
        while e > start
            && (source.as_bytes()[e - 1] == b' '
                || source.as_bytes()[e - 1] == b'\t'
                || source.as_bytes()[e - 1] == b'\n'
                || source.as_bytes()[e - 1] == b'\r')
        {
            e -= 1;
        }
        e
    };
    let insertion = if trimmed_end == start {
        format!("{}={}", param, value)
    } else {
        format!(", {}={}", param, value)
    };
    (trimmed_end..trimmed_end, insertion)
}

/// If `entry` (a slice of the modifications list) names `param`, return
/// the patch to replace its right-hand value with `value`. Otherwise
/// return `None`.
fn match_entry(
    source: &str,
    entry: Range<usize>,
    param: &str,
    value: &str,
) -> Option<(Range<usize>, String)> {
    let slice = source.get(entry.clone())?;
    // Skip leading whitespace.
    let pre_ws = slice.chars().take_while(|c| c.is_whitespace()).count();
    let name_start = entry.start + pre_ws;
    let remainder = source.get(name_start..entry.end)?;
    if !remainder.starts_with(param) {
        return None;
    }
    // Ensure the next char is an identifier boundary.
    let after_idx = name_start + param.len();
    let after_char = source.as_bytes().get(after_idx).copied();
    if matches!(after_char, Some(b'=') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')) {
        // Find the `=` and replace everything after it (trimmed) up to
        // entry end.
        let eq_pos = source.get(after_idx..entry.end)?.find('=')?;
        let value_start = after_idx + eq_pos + 1;
        // Strip trailing whitespace from entry end for a clean replace.
        let mut value_end = entry.end;
        while value_end > value_start {
            let b = source.as_bytes()[value_end - 1];
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                value_end -= 1;
            } else {
                break;
            }
        }
        let replacement = format!("{}{}", if source.as_bytes()[value_start] == b' ' { "" } else { " " }, value);
        return Some((value_start..value_end, replacement));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::DocumentHost;

    fn doc() -> DocumentHost<ModelicaDocument> {
        DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model Empty end Empty;\n",
        ))
    }

    #[test]
    fn fresh_document_state() {
        let host = doc();
        assert_eq!(host.generation(), 0);
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
        assert_eq!(host.document().id(), DocumentId::new(1));
        assert!(!host.can_undo());
        assert!(!host.can_redo());
        assert!(!host.document().is_empty());
    }

    #[test]
    fn replace_source_mutates_and_bumps_generation() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "model NewModel end NewModel;".into(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "model NewModel end NewModel;");
        assert_eq!(host.generation(), 1);
        assert!(host.can_undo());
    }

    #[test]
    fn undo_restores_previous_source() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "replaced".into(),
        })
        .unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
    }

    #[test]
    fn redo_reapplies_replaced_source() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "replaced".into(),
        })
        .unwrap();
        host.undo().unwrap();
        host.redo().unwrap();
        assert_eq!(host.document().source(), "replaced");
    }

    #[test]
    fn multi_step_undo_redo_round_trip() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource { new: "a".into() }).unwrap();
        host.apply(ModelicaOp::ReplaceSource { new: "b".into() }).unwrap();
        host.apply(ModelicaOp::ReplaceSource { new: "c".into() }).unwrap();
        assert_eq!(host.document().source(), "c");
        assert_eq!(host.generation(), 3);

        host.undo().unwrap();
        host.undo().unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), "model Empty end Empty;\n");

        host.redo().unwrap();
        host.redo().unwrap();
        host.redo().unwrap();
        assert_eq!(host.document().source(), "c");
    }

    #[test]
    fn generation_monotonic_across_undo_redo() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource { new: "a".into() }).unwrap();
        assert_eq!(host.generation(), 1);
        host.undo().unwrap();
        // Undo is itself a mutation — panels that key on generation need a
        // fresh signal either way.
        assert_eq!(host.generation(), 2);
        host.redo().unwrap();
        assert_eq!(host.generation(), 3);
    }

    #[test]
    fn new_apply_clears_redo_branch() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource { new: "first".into() }).unwrap();
        host.undo().unwrap();
        assert!(host.can_redo());

        host.apply(ModelicaOp::ReplaceSource { new: "second".into() }).unwrap();
        assert!(!host.can_redo());
        assert_eq!(host.document().source(), "second");
    }

    #[test]
    fn ast_cache_parses_fresh_document() {
        let host = doc();
        let cache = host.document().ast();
        assert_eq!(cache.generation, 0);
        let ast = cache.ast().expect("fresh doc should parse");
        assert!(ast.classes.contains_key("Empty"));
    }

    #[test]
    fn ast_cache_refreshes_after_mutation() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "model Foo end Foo;".into(),
        })
        .unwrap();
        let cache = host.document().ast();
        assert_eq!(cache.generation, 1);
        let ast = cache.ast().expect("should parse");
        assert!(ast.classes.contains_key("Foo"));
        assert!(!ast.classes.contains_key("Empty"));
    }

    #[test]
    fn ast_cache_holds_error_on_invalid_source() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "model M Real x end M;".into(), // missing semicolon → parse err
        })
        .unwrap();
        assert!(host.document().ast().result.is_err());
    }

    #[test]
    fn edit_text_replaces_range_and_is_invertible() {
        // "model Empty end Empty;\n"
        //  0         1
        //  0123456789012345678901
        let mut host = doc();
        // Replace "Empty" at positions 6..11 with "Thing"
        host.apply(ModelicaOp::EditText {
            range: 6..11,
            replacement: "Thing".into(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "model Thing end Empty;\n");
        assert_eq!(host.generation(), 1);

        host.undo().unwrap();
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
    }

    #[test]
    fn edit_text_supports_insertion_and_deletion() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "abcdef".to_string(),
        ));
        // Insert "XYZ" at position 3 (empty range)
        host.apply(ModelicaOp::EditText {
            range: 3..3,
            replacement: "XYZ".into(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "abcXYZdef");

        // Delete "XYZ" (range 3..6, empty replacement)
        host.apply(ModelicaOp::EditText {
            range: 3..6,
            replacement: String::new(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "abcdef");

        host.undo().unwrap();
        assert_eq!(host.document().source(), "abcXYZdef");
        host.undo().unwrap();
        assert_eq!(host.document().source(), "abcdef");
    }

    #[test]
    fn edit_text_rejects_out_of_bounds_range() {
        let mut host = doc();
        let err = host
            .apply(ModelicaOp::EditText {
                range: 0..999,
                replacement: String::new(),
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
        // Unchanged on error.
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
        assert_eq!(host.generation(), 0);
    }

    // ------------------------------------------------------------------
    // AST-level ops: AddComponent / AddConnection
    // ------------------------------------------------------------------

    #[test]
    fn add_component_appends_before_end_when_no_equation_section() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model M\n  Real a;\nend M;\n".to_string(),
        ));
        host.apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: "b".into(),
                modifications: vec![],
                placement: None,
            },
        })
        .unwrap();
        assert_eq!(
            host.document().source(),
            "model M\n  Real a;\n  Real b;\nend M;\n"
        );
        // AST cache must reflect the new component.
        let ast = host.document().ast().ast().expect("parse ok");
        assert!(ast.classes.get("M").unwrap().components.contains_key("b"));
    }

    #[test]
    fn add_component_inserts_before_equation_section() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model M\n  Real a;\nequation\n  a = 1;\nend M;\n".to_string(),
        ));
        host.apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: "b".into(),
                modifications: vec![],
                placement: None,
            },
        })
        .unwrap();
        assert_eq!(
            host.document().source(),
            "model M\n  Real a;\n  Real b;\nequation\n  a = 1;\nend M;\n"
        );
    }

    #[test]
    fn add_component_is_invertible() {
        let original = "model M\n  Real a;\nend M;\n";
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            original.to_string(),
        ));
        host.apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: "b".into(),
                modifications: vec![],
                placement: None,
            },
        })
        .unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), original);
    }

    #[test]
    fn add_component_errors_on_unknown_class() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model M end M;\n".to_string(),
        ));
        let err = host
            .apply(ModelicaOp::AddComponent {
                class: "Other".into(),
                decl: ComponentDecl {
                    type_name: "Real".into(),
                    name: "x".into(),
                    modifications: vec![],
                    placement: None,
                },
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
        assert_eq!(host.generation(), 0);
    }

    #[test]
    fn add_connection_appends_to_existing_equation_section() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model M\n  Real a;\n  Real b;\nequation\n  a = 1;\nend M;\n".to_string(),
        ));
        host.apply(ModelicaOp::AddConnection {
            class: "M".into(),
            eq: ConnectEquation {
                from: crate::pretty::PortRef::new("a", "p"),
                to: crate::pretty::PortRef::new("b", "n"),
                line: None,
            },
        })
        .unwrap();
        assert_eq!(
            host.document().source(),
            "model M\n  Real a;\n  Real b;\nequation\n  a = 1;\n  connect(a.p, b.n);\nend M;\n"
        );
    }

    #[test]
    fn add_connection_creates_equation_section_when_missing() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model M\n  Real a;\n  Real b;\nend M;\n".to_string(),
        ));
        host.apply(ModelicaOp::AddConnection {
            class: "M".into(),
            eq: ConnectEquation {
                from: crate::pretty::PortRef::new("a", "p"),
                to: crate::pretty::PortRef::new("b", "n"),
                line: None,
            },
        })
        .unwrap();
        assert_eq!(
            host.document().source(),
            "model M\n  Real a;\n  Real b;\nequation\n  connect(a.p, b.n);\nend M;\n"
        );
    }

    #[test]
    fn add_component_rejects_when_source_has_parse_error() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "model M Real x end M;".into(),
        })
        .unwrap();
        let err = host
            .apply(ModelicaOp::AddComponent {
                class: "M".into(),
                decl: ComponentDecl {
                    type_name: "Real".into(),
                    name: "y".into(),
                    modifications: vec![],
                    placement: None,
                },
            })
            .unwrap_err();
        assert!(matches!(err, DocumentError::ValidationFailed(_)));
    }
}
