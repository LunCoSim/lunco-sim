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

// ─────────────────────────────────────────────────────────────────────────────
// DocumentId
// ─────────────────────────────────────────────────────────────────────────────

/// Stable identifier for a [`Document`].
///
/// Backed by a `u64`. Applications are free to assign ids however they want —
/// an incrementing counter, a hash of a file path, a Bevy entity bits, etc.
/// `lunco-doc` treats ids as opaque and only requires them to be unique within
/// the app's Document population.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocumentId({})", self.0)
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
    /// The op was structurally valid but violated a validation rule (e.g.,
    /// deleting a range past the end of the document).
    ValidationFailed(String),

    /// The op targeted something that doesn't exist (e.g., removing a
    /// component by a name that isn't in the document).
    NotFound(String),

    /// Applying the op would break an invariant the document maintains.
    InvariantViolation(String),

    /// The op is well-formed but not supported in this context (e.g., not
    /// yet implemented, or deprecated).
    Unsupported(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValidationFailed(msg) => write!(f, "validation failed: {msg}"),
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::InvariantViolation(msg) => write!(f, "invariant violation: {msg}"),
            Self::Unsupported(msg) => write!(f, "unsupported: {msg}"),
        }
    }
}

impl std::error::Error for DocumentError {}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentOp
// ─────────────────────────────────────────────────────────────────────────────

/// A typed, cloneable mutation applicable to a [`Document`].
///
/// Each Document type defines its own Op enum (e.g., `ModelicaOp`, `UsdOp`).
/// Ops MUST produce an inverse when applied — see [`Document::apply`].
///
/// This trait is intentionally minimal. Future versions may add bounds for
/// serialization (for persistence and collaborative sync), but for now Ops
/// are in-memory only.
pub trait DocumentOp: Clone + Send + Sync + 'static {}

// Blanket impl so that any type satisfying the supertrait bounds is a DocumentOp.
// Uncomment when we want that convenience; for now, explicit impls are clearer.
// impl<T: Clone + Send + Sync + 'static> DocumentOp for T {}

// ─────────────────────────────────────────────────────────────────────────────
// Document
// ─────────────────────────────────────────────────────────────────────────────

/// A canonical, mutable, observable structured artifact.
///
/// Each Document has:
///
/// - An **identifier** ([`DocumentId`]) stable for the document's lifetime.
/// - A **generation counter** that increments on every successful mutation,
///   so views can cheaply detect whether they need to re-render.
/// - An **op type** describing the set of valid mutations.
/// - An **apply** method that validates an op, mutates the document, and
///   returns the op's inverse for use in an undo stack.
///
/// Documents are designed to be hosted inside a [`DocumentHost`], which
/// handles the undo/redo machinery uniformly across document types.
pub trait Document: Send + Sync + 'static {
    /// The type of mutations this document accepts.
    type Op: DocumentOp;

    /// Stable identifier for this document.
    fn id(&self) -> DocumentId;

    /// Monotonically-increasing generation counter. Incremented on every
    /// successful [`apply`](Self::apply).
    ///
    /// Views compare their last-seen generation against this to decide
    /// whether to re-render. Also useful for cache invalidation.
    fn generation(&self) -> u64;

    /// Validate and apply an op. On success, return the op's inverse.
    ///
    /// Implementations MUST:
    /// - Validate the op against the document's current state. If invalid,
    ///   return a [`DocumentError`] without mutating the document.
    /// - On success, mutate the document AND bump the generation counter.
    /// - Return an op whose application would exactly reverse this op's
    ///   effect (the inverse). Used by [`DocumentHost`] for undo.
    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentHost
// ─────────────────────────────────────────────────────────────────────────────

/// Holds a [`Document`] plus the undo/redo history.
///
/// `DocumentHost` is a plain struct — not a Bevy `Component` — so it works
/// in any context: inside an ECS app, in a CLI tool, in tests, in a future
/// collaboration server. Applications that need ECS integration can store
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests — including a toy TextDocument to validate the API shape end-to-end
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal text document — a single `String` with insert/delete ops.
    ///
    /// Not production-grade (no support for UTF-8 grapheme boundaries, etc.).
    /// Purpose: exercise the `Document` + `DocumentHost` API.
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
                            "insert position {pos} past end of text (len {})",
                            self.text.len()
                        )));
                    }
                    let len = text.len();
                    self.text.insert_str(pos, &text);
                    self.generation += 1;
                    Ok(TextOp::Delete { pos, len })
                }
                TextOp::Delete { pos, len } => {
                    if pos + len > self.text.len() {
                        return Err(DocumentError::ValidationFailed(format!(
                            "delete range [{pos}..{}) past end of text (len {})",
                            pos + len,
                            self.text.len()
                        )));
                    }
                    let removed: String = self.text.drain(pos..pos + len).collect();
                    self.generation += 1;
                    Ok(TextOp::Insert { pos, text: removed })
                }
            }
        }
    }

    fn new_host() -> DocumentHost<TextDocument> {
        DocumentHost::new(TextDocument {
            id: DocumentId::new(1),
            text: String::new(),
            generation: 0,
        })
    }

    #[test]
    fn document_id_round_trip() {
        let id = DocumentId::new(42);
        assert_eq!(id.raw(), 42);
    }

    #[test]
    fn fresh_host_has_empty_stacks() {
        let host = new_host();
        assert!(!host.can_undo());
        assert!(!host.can_redo());
        assert_eq!(host.undo_depth(), 0);
        assert_eq!(host.redo_depth(), 0);
        assert_eq!(host.generation(), 0);
    }

    #[test]
    fn apply_mutates_and_records_inverse() {
        let mut host = new_host();
        host.apply(TextOp::Insert { pos: 0, text: "Hello".into() })
            .unwrap();
        assert_eq!(host.document().text, "Hello");
        assert_eq!(host.generation(), 1);
        assert!(host.can_undo());
        assert!(!host.can_redo());
    }

    #[test]
    fn undo_reverses_apply() {
        let mut host = new_host();
        host.apply(TextOp::Insert { pos: 0, text: "Hello".into() }).unwrap();
        let undone = host.undo().unwrap();
        assert!(undone);
        assert_eq!(host.document().text, "");
        assert!(!host.can_undo());
        assert!(host.can_redo());
    }

    #[test]
    fn redo_reapplies_undone() {
        let mut host = new_host();
        host.apply(TextOp::Insert { pos: 0, text: "Hello".into() }).unwrap();
        host.undo().unwrap();
        let redone = host.redo().unwrap();
        assert!(redone);
        assert_eq!(host.document().text, "Hello");
        assert!(host.can_undo());
        assert!(!host.can_redo());
    }

    #[test]
    fn undo_on_empty_stack_is_noop() {
        let mut host = new_host();
        let result = host.undo().unwrap();
        assert!(!result);
    }

    #[test]
    fn redo_on_empty_stack_is_noop() {
        let mut host = new_host();
        let result = host.redo().unwrap();
        assert!(!result);
    }

    #[test]
    fn new_apply_clears_redo_stack() {
        let mut host = new_host();
        host.apply(TextOp::Insert { pos: 0, text: "Hello".into() }).unwrap();
        host.undo().unwrap();
        assert!(host.can_redo());

        host.apply(TextOp::Insert { pos: 0, text: "World".into() }).unwrap();
        assert!(!host.can_redo(), "new apply must clear the redo branch");
        assert_eq!(host.document().text, "World");
    }

    #[test]
    fn generation_increments_on_every_mutation_including_undo() {
        let mut host = new_host();
        assert_eq!(host.generation(), 0);
        host.apply(TextOp::Insert { pos: 0, text: "A".into() }).unwrap();
        assert_eq!(host.generation(), 1);
        host.undo().unwrap();
        // Undo is itself a mutation — generation keeps increasing. Views
        // that key on generation get a fresh signal either way.
        assert_eq!(host.generation(), 2);
        host.redo().unwrap();
        assert_eq!(host.generation(), 3);
    }

    #[test]
    fn invalid_op_leaves_document_unchanged() {
        let mut host = new_host();
        host.apply(TextOp::Insert { pos: 0, text: "Hi".into() }).unwrap();
        let generation_before = host.generation();
        let undo_depth_before = host.undo_depth();

        // Deleting past end of "Hi" must fail.
        let result = host.apply(TextOp::Delete { pos: 0, len: 100 });
        assert!(matches!(result, Err(DocumentError::ValidationFailed(_))));

        // Document and undo stack are unchanged.
        assert_eq!(host.document().text, "Hi");
        assert_eq!(host.generation(), generation_before);
        assert_eq!(host.undo_depth(), undo_depth_before);
    }

    #[test]
    fn multi_step_undo_redo_round_trip() {
        let mut host = new_host();
        host.apply(TextOp::Insert { pos: 0, text: "A".into() }).unwrap();
        host.apply(TextOp::Insert { pos: 1, text: "B".into() }).unwrap();
        host.apply(TextOp::Insert { pos: 2, text: "C".into() }).unwrap();
        assert_eq!(host.document().text, "ABC");

        host.undo().unwrap();
        host.undo().unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().text, "");

        host.redo().unwrap();
        host.redo().unwrap();
        host.redo().unwrap();
        assert_eq!(host.document().text, "ABC");
    }

    #[test]
    fn document_id_is_stable() {
        let host = new_host();
        assert_eq!(host.document().id(), DocumentId::new(1));
    }

    #[test]
    fn document_error_displays_meaningfully() {
        let err = DocumentError::ValidationFailed("bad pos".into());
        assert_eq!(err.to_string(), "validation failed: bad pos");
    }

    #[test]
    fn into_document_releases_inner() {
        let host = new_host();
        let doc = host.into_document();
        assert_eq!(doc.id(), DocumentId::new(1));
    }
}
