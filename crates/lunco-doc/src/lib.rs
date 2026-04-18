//! # lunco-doc
//!
//! Document System foundation for LunCoSim.
//!
//! Defines the core traits and machinery for **canonical, mutable, observable**
//! structured artifacts — everything the user edits (Modelica models, USD
//! scenes, SysML blocks, missions) is a Document.
//!
//! This crate is **UI-free and headless-capable.** It has zero runtime
//! dependencies. Apps that need UI build `DocumentView`s on top (typically in
//! `lunco-ui`); apps that need the ECS integration wire Documents as Bevy
//! components or resources themselves.
//!
//! ## Core concepts
//!
//! - [`Document`] — a typed, mutable artifact with a unique id and generation.
//! - [`DocumentOp`] — a typed, reversible mutation.
//! - [`DocumentHost`] — holds a Document and runs the op/apply/undo/redo loop.
//! - [`DocumentError`] — the fallible-apply error type.
//! - [`DocumentId`] — stable identifier for a Document.
//!
//! ## Example
//!
//! ```
//! use lunco_doc::{Document, DocumentOp, DocumentHost, DocumentError, DocumentId};
//!
//! // Define a minimal document type:
//! struct Counter { id: DocumentId, value: i32, generation: u64 }
//!
//! #[derive(Clone, Debug)]
//! enum CounterOp { Inc(i32) }
//! impl DocumentOp for CounterOp {}
//!
//! impl Document for Counter {
//!     type Op = CounterOp;
//!     fn id(&self) -> DocumentId { self.id }
//!     fn generation(&self) -> u64 { self.generation }
//!     fn apply(&mut self, op: CounterOp) -> Result<CounterOp, DocumentError> {
//!         let CounterOp::Inc(n) = op;
//!         self.value += n;
//!         self.generation += 1;
//!         Ok(CounterOp::Inc(-n))   // the inverse
//!     }
//! }
//!
//! let mut host = DocumentHost::new(Counter { id: DocumentId::new(1), value: 0, generation: 0 });
//! host.apply(CounterOp::Inc(5)).unwrap();
//! assert_eq!(host.document().value, 5);
//! host.undo().unwrap();
//! assert_eq!(host.document().value, 0);
//! host.redo().unwrap();
//! assert_eq!(host.document().value, 5);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt;
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// SymbolPath — opaque cross-document reference
// ─────────────────────────────────────────────────────────────────────────────

/// A domain-agnostic path into a [`Document`].
///
/// Examples — each format interprets the string in its own syntax:
///
/// - Modelica: `"Rocket.engine.thrust"` (dotted qualified name)
/// - USD: `"/World/Rocket.xformOp:translate"` (prim path + attribute)
/// - SysML v2: `"Rocket::engine::thrust"` (double-colon qualified name)
///
/// `lunco-doc` treats the string as opaque. Resolution is the owning
/// Document's job via the [`Resolver`] trait. This type exists so that
/// **binding documents** (cross-format links) can store
/// `(DocumentId, SymbolPath)` pairs without depending on domain crates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolPath(String);

impl SymbolPath {
    /// Wrap a string as a symbol path. No validation — format-specific.
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// The raw path string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when the path is the empty string.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SymbolPath {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SymbolPath {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Resolver — symbol lookup within a document
// ─────────────────────────────────────────────────────────────────────────────

/// Resolves a [`SymbolPath`] to a domain-specific handle inside one document.
///
/// Implemented by each Document type using its own AST / scene graph / model.
/// Binding documents call this to validate that both ends of a cross-document
/// link still exist after edits.
///
/// `Target` is domain-defined (e.g. an AST node handle, a USD prim path,
/// a SysML element id). Callers that only need to know whether the symbol
/// resolves can ignore the value.
pub trait Resolver {
    /// The domain-specific handle returned by a successful resolution.
    type Target;

    /// Look up `path` inside this document. Returns `None` when the symbol
    /// does not exist.
    fn resolve(&self, path: &SymbolPath) -> Option<Self::Target>;
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentId
// ─────────────────────────────────────────────────────────────────────────────

/// Stable identifier for a [`Document`].
///
/// Backed by a `u64`. Applications are free to assign ids however they want —
/// an incrementing counter, a hash of a file path, a Bevy entity bits, etc.
/// `lunco-doc` treats ids as opaque and only requires them to be unique within
/// the app's Document population.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(u64);

impl DocumentId {
    /// Construct a [`DocumentId`] from a raw `u64`.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Extract the raw `u64` value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// True when this id is the default / unassigned sentinel (`0`).
    pub const fn is_unassigned(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocumentId({})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentOrigin — where a document came from + whether it can be saved
// ─────────────────────────────────────────────────────────────────────────────

/// Where a document originated, which drives save behavior +
/// UI affordances (tab title, read-only badge, Save button).
///
/// Deliberately minimal: two variants. Fancier classifications
/// (MSL / bundled / third-party library / user project) are a
/// *Package Browser* concern — at the document level, all that
/// matters is "does it have a path we can write to?".
///
/// Architectural seed: when documents can come from a remote Nucleus
/// server or a URL-addressed library, a `Remote { url }` variant slots
/// in here without touching Document trait surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentOrigin {
    /// Never written to disk; `name` is the in-session display id
    /// (e.g. `"Untitled1"`). Saving requires Save-As to bind a path.
    Untitled {
        /// Human-readable identifier (shown on the tab until saved).
        name: String,
    },
    /// Backed by a filesystem path. `writable` gates the Save button
    /// so library / bundled assets stay read-only even though they
    /// have a concrete path.
    File {
        /// Canonical filesystem path (absolute preferred).
        path: PathBuf,
        /// Whether writes are permitted. `false` for library /
        /// bundled-example files.
        writable: bool,
    },
}

impl DocumentOrigin {
    /// Shorthand: a user-writable filesystem document.
    pub fn writable_file(path: impl Into<PathBuf>) -> Self {
        Self::File {
            path: path.into(),
            writable: true,
        }
    }

    /// Shorthand: a read-only filesystem document (library entry,
    /// bundled example).
    pub fn readonly_file(path: impl Into<PathBuf>) -> Self {
        Self::File {
            path: path.into(),
            writable: false,
        }
    }

    /// Shorthand: an in-memory untitled scratch document.
    pub fn untitled(name: impl Into<String>) -> Self {
        Self::Untitled { name: name.into() }
    }

    /// Filesystem path, if any. `None` for [`Untitled`](Self::Untitled).
    pub fn canonical_path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } => Some(path.as_path()),
            Self::Untitled { .. } => None,
        }
    }

    /// Whether Save may write to this origin. `false` for read-only
    /// library entries and for untitled docs without a bound path
    /// (Save-As is required for the latter).
    pub fn is_writable(&self) -> bool {
        matches!(self, Self::File { writable: true, .. })
    }

    /// Whether this document has never been written to disk in this
    /// session (Save-As is required before Save can work).
    pub fn is_untitled(&self) -> bool {
        matches!(self, Self::Untitled { .. })
    }

    /// Best-effort display name — the tab title before any
    /// domain-specific overrides. File stem for `File`, the stashed
    /// `name` for `Untitled`.
    pub fn display_name(&self) -> String {
        match self {
            Self::Untitled { name } => name.clone(),
            Self::File { path, .. } => path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentError
// ─────────────────────────────────────────────────────────────────────────────

/// Error produced when applying a [`DocumentOp`] fails.
///
/// This enum is `#[non_exhaustive]` — forward-compatible with new variants
/// in future releases.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentError {
    /// The operation is invalid for the current document state.
    ValidationFailed(String),
    /// An internal error occurred during application.
    Internal(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValidationFailed(msg) => write!(f, "Validation failed: {}", msg),
            Self::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for DocumentError {}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentOp
// ─────────────────────────────────────────────────────────────────────────────

/// Trait for a typed, reversible mutation to a [`Document`].
pub trait DocumentOp: Clone + fmt::Debug + Send + Sync + 'static {}

// ─────────────────────────────────────────────────────────────────────────────
// Document
// ─────────────────────────────────────────────────────────────────────────────

/// A structured, mutable, observable piece of user intent.
///
/// Every editable artifact in the Twin (Modelica models, missions, etc)
/// implements this trait.
pub trait Document: Send + Sync + 'static {
    /// The specific operation type that can mutate this document.
    type Op: DocumentOp;

    /// Stable identifier for this document.
    fn id(&self) -> DocumentId;

    /// Current generation of the document. Incremented on every change.
    fn generation(&self) -> u64;

    /// Apply an operation to the document.
    ///
    /// On success, returns the **inverse operation** which, when applied
    /// back to the resulting state, restores the document to its exact
    /// previous state.
    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentHost
// ─────────────────────────────────────────────────────────────────────────────

/// Manager for a [`Document`] that provides undo/redo and change tracking.
///
/// Applications typically do not hold documents directly; they hold
/// a `DocumentHost` inside their own Bevy component or resource.
///
/// ## Undo / redo
///
/// - [`apply`](Self::apply) runs the document's op, pushes the inverse onto
///   the undo stack, and clears the redo stack.
/// - [`undo`](Self::undo) pops the most recent inverse, applies it, and
///   pushes the resulting inverse (the original forward op) onto the redo
///   stack.
/// - [`redo`](Self::redo) pops from the redo stack and applies it, pushing
///   the resulting inverse back onto the undo stack.
///
/// This symmetric design means undo and redo are just "apply an op" — the
/// document itself doesn't know whether it's a forward edit or an undo.
pub struct DocumentHost<D: Document> {
    document: D,
    undo_stack: Vec<D::Op>,
    redo_stack: Vec<D::Op>,
}

impl<D: Document> DocumentHost<D> {
    /// Create a new host wrapping the given document. Undo/redo stacks
    /// start empty.
    pub fn new(document: D) -> Self {
        Self {
            document,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Access the wrapped document immutably. Views read through here.
    pub fn document(&self) -> &D {
        &self.document
    }

    /// Direct mutable access to the wrapped document, **bypassing** the
    /// op / undo machinery.
    ///
    /// Only the minimum is exposed because direct mutations defeat the
    /// point of the Document System — use [`apply`](Self::apply) for
    /// anything that should be undoable. The legitimate callers are
    /// bookkeeping fields that aren't part of the document's
    /// user-visible state: cached layout, "last saved at generation N"
    /// markers, telemetry counters. The caller is responsible for not
    /// touching fields that should flow through ops.
    pub fn document_mut(&mut self) -> &mut D {
        &mut self.document
    }

    /// The document's current generation. See [`Document::generation`].
    pub fn generation(&self) -> u64 {
        self.document.generation()
    }

    /// Apply a forward op to the document.
    ///
    /// On success, the op's inverse is recorded for undo and the redo
    /// stack is cleared (since a new edit invalidates any pending redo
    /// branch — standard linear-history semantics).
    ///
    /// On failure, the document is left unchanged and the error is
    /// propagated. Undo/redo stacks are unaffected.
    pub fn apply(&mut self, op: D::Op) -> Result<(), DocumentError> {
        let inverse = self.document.apply(op)?;
        self.undo_stack.push(inverse);
        self.redo_stack.clear();
        Ok(())
    }

    /// Undo the most recent op. Returns `Ok(false)` if the undo stack is
    /// empty (nothing to undo), `Ok(true)` if an op was undone.
    pub fn undo(&mut self) -> Result<bool, DocumentError> {
        let Some(op) = self.undo_stack.pop() else {
            return Ok(false);
        };
        let inverse = self.document.apply(op)?;
        self.redo_stack.push(inverse);
        Ok(true)
    }

    /// Redo the most recently undone op. Returns `Ok(false)` if the redo
    /// stack is empty, `Ok(true)` if an op was redone.
    pub fn redo(&mut self) -> Result<bool, DocumentError> {
        let Some(op) = self.redo_stack.pop() else {
            return Ok(false);
        };
        let inverse = self.document.apply(op)?;
        self.undo_stack.push(inverse);
        Ok(true)
    }

    /// Whether there is at least one op available to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether there is at least one op available to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Number of ops on the undo stack.
    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    /// Number of ops on the redo stack.
    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }

    /// Consume the host and return the underlying document.
    pub fn into_document(self) -> D {
        self.document
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TextDocument {
        id: DocumentId,
        text: String,
        generation: u64,
    }

    #[derive(Clone, Debug, PartialEq)]
    enum TextOp {
        /// Insert `text` at byte position `pos`.
        Insert { pos: usize, text: String },
        /// Delete `len` bytes starting at byte position `pos`.
        Delete { pos: usize, len: usize },
    }

    impl DocumentOp for TextOp {}

    impl Document for TextDocument {
        type Op = TextOp;

        fn id(&self) -> DocumentId {
            self.id
        }

        fn generation(&self) -> u64 {
            self.generation
        }

        fn apply(&mut self, op: TextOp) -> Result<TextOp, DocumentError> {
            match op {
                TextOp::Insert { pos, text } => {
                    if pos > self.text.len() {
                        return Err(DocumentError::ValidationFailed(format!(
                            "Insert position {} out of bounds (len={})",
                            pos,
                            self.text.len()
                        )));
                    }
                    self.text.insert_str(pos, &text);
                    self.generation += 1;
                    Ok(TextOp::Delete {
                        pos,
                        len: text.len(),
                    })
                }
                TextOp::Delete { pos, len } => {
                    if pos + len > self.text.len() {
                        return Err(DocumentError::ValidationFailed(format!(
                            "Delete range {}..{} out of bounds (len={})",
                            pos,
                            pos + len,
                            self.text.len()
                        )));
                    }
                    let old_text = self.text[pos..pos + len].to_string();
                    self.text.replace_range(pos..pos + len, "");
                    self.generation += 1;
                    Ok(TextOp::Insert { pos, text: old_text })
                }
            }
        }
    }

    #[test]
    fn test_document_host_undo_redo() {
        let mut host = DocumentHost::new(TextDocument {
            id: DocumentId::new(1),
            text: "Hello".to_string(),
            generation: 0,
        });

        host.apply(TextOp::Insert {
            pos: 5,
            text: " World".to_string(),
        })
        .unwrap();
        assert_eq!(host.document().text, "Hello World");
        assert_eq!(host.generation(), 1);

        host.undo().unwrap();
        assert_eq!(host.document().text, "Hello");
        assert_eq!(host.generation(), 2);

        host.redo().unwrap();
        assert_eq!(host.document().text, "Hello World");
        assert_eq!(host.generation(), 3);
    }

    #[test]
    fn test_document_id() {
        let id = DocumentId::new(42);
        assert_eq!(id.raw(), 42);
        assert_eq!(format!("{}", id), "DocumentId(42)");
    }
}
