//! Domain engine abstraction.
//!
//! A [`DomainEngine`] is the in-process owner of a domain's parser session
//! (e.g. `rumoca_session::Session` for Modelica, `pxr::UsdStage` for USD)
//! and projects each open document into a UI-friendly [`DomainEngine::Index`].
//!
//! UI code reads the Index, never the engine's internal AST. Edits are
//! applied as typed [`DocumentOp`]s; the engine returns
//! the inverse op for undo.
//!
//! One engine instance per process per domain — the engine owns cross-file
//! state (symbol tables, fingerprint caches) so per-document ops stay cheap.
//!
//! ## Why a trait?
//!
//! Two implementers today (Modelica, USD) plus future SysML. The trait keeps
//! workbench callers domain-agnostic: panels iterate engines, render their
//! Indexes, dispatch ops uniformly. Domain-specific behavior stays inside
//! each engine impl.

use crate::{DocumentId, DocumentOp, SymbolPath};

// ─────────────────────────────────────────────────────────────────────────────
// Stable per-AST-node identity
// ─────────────────────────────────────────────────────────────────────────────

/// Stable per-AST-node identifier within one document.
///
/// Engines define their own scheme — Modelica uses a string like
/// `"Rocket.engine|component|thrust"`; USD uses prim+attr paths.
/// Stability across re-parses is what lets the [`DomainEngine::Index`]
/// reconcile incrementally instead of rebuilding from scratch.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct NodeId(pub String);

impl NodeId {
    /// Construct a `NodeId` from any string-like value.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// Borrow the underlying identifier as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-document references
// ─────────────────────────────────────────────────────────────────────────────

/// A reference from a node in this document to a symbol that may live in
/// another document.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SymbolRef {
    /// Fully-qualified target symbol.
    pub path: SymbolPath,
    /// Node in *this* document that holds the reference.
    pub from_node: NodeId,
}

/// A resolved cross-document reference — the document and node a
/// [`SymbolPath`] resolves to.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedRef {
    /// Document the symbol resolved into.
    pub doc: DocumentId,
    /// Node within that document carrying the symbol definition.
    pub node: NodeId,
}

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostics
// ─────────────────────────────────────────────────────────────────────────────

/// Half-open byte range in document source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextRange {
    /// Inclusive byte offset where the range begins.
    pub start: usize,
    /// Exclusive byte offset where the range ends.
    pub end: usize,
}

impl TextRange {
    /// Construct a half-open `[start, end)` byte range.
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Severity level reported by a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DiagnosticSeverity {
    /// Compilation/parse error — blocks downstream work.
    Error,
    /// Non-blocking issue worth surfacing to the user.
    Warning,
    /// Informational note (e.g. style suggestion).
    Info,
    /// Lighter than `Info` — IDE-style improvement hint.
    Hint,
}

/// One diagnostic produced by a domain (Modelica, scripting, …) for a document.
///
/// Location is 1-based `line`/`col` — the form every producer (rumoca, rhai)
/// and every consumer (egui panel click-to-source, API/MCP status) actually
/// uses, so no source-dependent byte conversions are needed at the boundaries.
/// (Absolute byte ranges remain available via [`offset_to_line_col`] /
/// [`line_col_to_offset`] + [`TextRange`], used independently by index spans.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    /// How the diagnostic should be classified by the UI.
    pub severity: DiagnosticSeverity,
    /// Human-readable message body.
    pub message: String,
    /// 1-based source line, if located.
    pub line: Option<u32>,
    /// 1-based source column, if located.
    pub col: Option<u32>,
}

impl Diagnostic {
    /// An error diagnostic at an optional 1-based `(line, col)`.
    pub fn error(message: impl Into<String>, line: Option<u32>, col: Option<u32>) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            message: message.into(),
            line,
            col,
        }
    }

    /// A warning diagnostic at an optional 1-based `(line, col)`.
    pub fn warning(message: impl Into<String>, line: Option<u32>, col: Option<u32>) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
            line,
            col,
        }
    }

    /// An error diagnostic with no source location (renders but isn't clickable).
    pub fn message_only(message: impl Into<String>) -> Self {
        Self::error(message, None, None)
    }
}

impl From<String> for Diagnostic {
    fn from(message: String) -> Self {
        Self::message_only(message)
    }
}

impl From<&str> for Diagnostic {
    fn from(message: &str) -> Self {
        Self::message_only(message)
    }
}

/// Lifecycle of a document's compilation — the unified state every domain
/// (Modelica, scripting, …) reports through, so callers poll one shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CompileState {
    /// Not yet compiled (no result either way).
    #[default]
    Idle,
    /// Compilation in progress (async domains; instant ones skip this).
    Compiling,
    /// Compiled successfully — no error diagnostics.
    Ready,
    /// Compilation failed — see the document's error diagnostics.
    Error,
}

impl CompileState {
    /// Lowercase wire tag (`"idle"`/`"compiling"`/`"ready"`/`"error"`).
    pub fn as_str(self) -> &'static str {
        match self {
            CompileState::Idle => "idle",
            CompileState::Compiling => "compiling",
            CompileState::Ready => "ready",
            CompileState::Error => "error",
        }
    }
}

/// Convert a byte `offset` into `source` to a 1-based `(line, column)`.
/// Column counts Unicode scalar values (chars), not bytes, so it matches
/// what an editor shows. Past-the-end offsets clamp to the final position.
pub fn offset_to_line_col(source: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(source.len());
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Convert a 1-based `(line, column)` in `source` to a byte offset (the inverse
/// of [`offset_to_line_col`]). Out-of-range line/col clamp to the source end.
pub fn line_col_to_offset(source: &str, line: u32, col: u32) -> usize {
    let mut cur_line = 1u32;
    let mut cur_col = 1u32;
    for (i, ch) in source.char_indices() {
        if cur_line == line && cur_col == col {
            return i;
        }
        if ch == '\n' {
            cur_line += 1;
            cur_col = 1;
        } else {
            cur_col += 1;
        }
    }
    source.len()
}

// ─────────────────────────────────────────────────────────────────────────────
// DomainEngine
// ─────────────────────────────────────────────────────────────────────────────

/// A domain-specific editing engine.
///
/// Owns the parser/session for one modeling domain (Modelica, USD, SysML)
/// across all open documents in that domain. Projects each document into a
/// domain-specific [`Self::Index`] type that UI consumes.
///
/// Edit pipeline:
///
/// 1. UI gesture → typed [`Self::Op`].
/// 2. Engine `apply`s op: optimistically patches the Index, returns inverse.
/// 3. Op-driven path: `apply` mutates engine state directly. **No reparse
///    on the hot path** — see `FreshAst::Mutated` in lunco-modelica.
/// 4. Free-form text-edit path (code editor only): a separate driver
///    parses source off-thread, then hands the resulting
///    [`Self::ParsedInput`] back to the engine via [`Self::open`] (or
///    a domain-specific upsert). The engine itself **never accepts raw
///    source**.
///
/// # AST-canonical input
///
/// The trait deliberately has no source-taking method. To install a
/// document, callers must produce the engine's parsed representation
/// themselves. This forces the parse cost to be visible at the call
/// site and makes "skip parse when the AST is already fresh" / "move
/// parse to a worker" / "reuse a cached AST" decidable per call rather
/// than baked into the engine.
///
/// The [`Self::ParsedInput`] type is what each domain hands the
/// engine: `Arc<StoredDefinition>` for Modelica, `UsdStage` for USD,
/// `SyntaxTree` for SysML, etc. Source → ParsedInput parsing is the
/// caller's responsibility; the engine consumes only the parsed form.
///
/// On the producer side, `lunco-modelica`'s `FreshAst::Mutated` /
/// `FreshAst::TextEdit` split mirrors this contract: structured ops
/// produce a fresh AST inline; free-form text edits mark the AST stale
/// and let an async parse driver land a new tree later. The engine
/// surface defined here is the consumer half of the same invariant —
/// neither end accepts raw source on a hot path.
pub trait DomainEngine: Send + Sync + 'static {
    /// The op type this engine accepts.
    type Op: DocumentOp;

    /// The Index type projected per open document. UI reads this.
    type Index;

    /// Domain-specific parsed-input type — what the engine accepts as
    /// authoritative document content. Modelica:
    /// `Arc<rumoca_session::parsing::ast::StoredDefinition>`. USD:
    /// `UsdStage`. SysML: `SyntaxTree`. Etc.
    ///
    /// The parsing step that produces a value of this type is **not**
    /// part of the engine's public surface — callers parse explicitly
    /// (see crate-level docs). This keeps parse cost visible at the
    /// call site and lets producers route around the parse entirely
    /// when they already have a fresh AST (the structured-op fast path).
    type ParsedInput;

    /// Open a document with its parsed initial content. After success,
    /// [`Self::index`] returns Some for this id.
    ///
    /// To replace the document's content (e.g. after an off-thread
    /// reparse from a code-editor edit lands), call `open` again with
    /// the same id — engines treat re-`open` as a content upsert.
    fn open(&mut self, id: DocumentId, parsed: Self::ParsedInput) -> Result<(), DomainEngineError>;

    /// Close a document. Releases per-doc resources.
    fn close(&mut self, id: DocumentId);

    /// Apply an op, returning the inverse for undo.
    ///
    /// Engines apply optimistically — Index is updated synchronously for
    /// instant UI feedback; authoritative reparse is scheduled async.
    fn apply(&mut self, id: DocumentId, op: Self::Op) -> Result<Self::Op, DomainEngineError>;

    /// Read-only access to the Index. Hot path; must be cheap.
    fn index(&self, id: DocumentId) -> Option<&Self::Index>;

    /// Render the document to source text (used for Save).
    fn print(&self, id: DocumentId) -> Option<String>;

    /// Diagnostics for this document.
    fn diagnostics(&self, id: DocumentId) -> &[Diagnostic];

    /// Symbols this document defines, fully-qualified.
    /// Used by [`crate::RefIndex`] to maintain the cross-doc reference table.
    fn defines(&self, id: DocumentId) -> &[SymbolPath];

    /// References emanating from this document.
    /// Used by [`crate::RefIndex`] to track cross-doc dependents.
    fn refs_out(&self, id: DocumentId) -> &[SymbolRef];
}

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

/// Failure modes returned by [`DomainEngine`] operations.
#[derive(Debug)]
pub enum DomainEngineError {
    /// `apply`/`source`/`print` was called for a document the engine
    /// hasn't opened (no prior `open` or `close` was already invoked).
    NotOpen(DocumentId),
    /// The op rejected by the domain (e.g. references a missing class,
    /// violates a domain invariant). Carries a human-readable reason.
    InvalidOp(String),
    /// `apply` failed while mutating the engine's internal state.
    Apply(String),
    /// `open` failed because the source could not be parsed.
    Parse(String),
}

impl std::fmt::Display for DomainEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DomainEngineError::NotOpen(id) => write!(f, "document {} not open", id),
            DomainEngineError::InvalidOp(m) => write!(f, "invalid op: {}", m),
            DomainEngineError::Apply(m) => write!(f, "apply failed: {}", m),
            DomainEngineError::Parse(m) => write!(f, "parse failed: {}", m),
        }
    }
}

impl std::error::Error for DomainEngineError {}

#[cfg(test)]
mod tests {
    use super::{line_col_to_offset, offset_to_line_col};

    #[test]
    fn line_col_offset_roundtrips() {
        let src = "fn a() {\n  let x = ;\n}\n";
        // Start of line 2, col 1 = byte just after the first '\n' (offset 9).
        let off = line_col_to_offset(src, 2, 1);
        assert_eq!(&src[off..off + 2], "  ");
        assert_eq!(offset_to_line_col(src, off), (2, 1));
        // The ';'-ish error position on line 2.
        let (l, c) = offset_to_line_col(src, off + 10);
        assert_eq!(l, 2);
        assert_eq!(line_col_to_offset(src, l, c), off + 10);
    }

    #[test]
    fn offsets_clamp_out_of_range() {
        let src = "abc";
        assert_eq!(offset_to_line_col(src, 999), (1, 4)); // past end → after last char
        assert_eq!(line_col_to_offset(src, 99, 99), src.len());
    }
}
