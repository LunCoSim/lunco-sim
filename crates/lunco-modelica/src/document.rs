//! `ModelicaDocument` — the Document System representation of one `.mo` file.
//!
//! # Status: live source-of-truth for Modelica text
//!
//! Documents are keyed by [`lunco_doc::DocumentId`] inside
//! [`ui::ModelicaDocumentRegistry`]. Every place that spawns a
//! `ModelicaModel` entity (CodeEditor's Compile, the Diagram panel's
//! auto-compile, `balloon_setup`, the workbench binaries) allocates a
//! document in the registry and writes its id into
//! [`crate::ModelicaModel::document`]. Later Compile / UpdateParameters
//! calls checkpoint new source onto that id. The registry's
//! `DocumentHost<ModelicaDocument>` is the single authority for model
//! source — entities just hold references.
//!
//! Still outside the Document System:
//!
//! - **`EditorBufferState.text`** — the egui TextEdit working buffer. Keeps
//!   per-keystroke edits responsive; committed into the Document on Compile.
//! - **`WorkbenchState.open_model.source`** — the "current file" slot used
//!   by the library browser to feed the Code view before any compile has
//!   produced an entity. Separate concern; a future migration will unify it.
//!
//! # Op set today
//!
//! Exactly one op: [`ModelicaOp::ReplaceSource`]. Coarse on purpose — it's
//! enough to make CodeEditor participate in Document-level undo/redo and in
//! the cross-panel change notification story (generation counter). Finer
//! ops (granular text Insert/Delete, AST-level `AddComponent`,
//! `SetParameter`, `AddConnection`) come when we need them.
//!
//! The inverse of `ReplaceSource { new }` is `ReplaceSource { new: old }`,
//! where `old` is the source text as it was before the op applied.

use std::path::PathBuf;

use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp};

use crate::ui::state::ModelLibrary;

/// The canonical Document representation of one Modelica source file.
///
/// Owns the source text. The parsed AST is **not** cached on the document
/// today — callers continue to parse on demand via
/// `crate::ast_extract::parse_to_ast` until a concrete caller makes caching
/// worth the complexity (invalidation on every op).
///
/// The document knows *where it came from*: `canonical_path` is the file
/// this source was loaded from (and where `SaveDocument` will write back),
/// and `library` tells UI code which library rules apply (MSL is read-only,
/// user models are writable, etc.). A `None` path means an untitled,
/// in-memory document — `SaveDocument` must resolve a path before writing.
#[derive(Debug, Clone)]
pub struct ModelicaDocument {
    id: DocumentId,
    source: String,
    generation: u64,
    canonical_path: Option<PathBuf>,
    library: ModelLibrary,
}

impl ModelicaDocument {
    /// Build an in-memory `ModelicaDocument` with no canonical path.
    /// `library` defaults to [`ModelLibrary::InMemory`].
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self {
            id,
            source: source.into(),
            generation: 0,
            canonical_path: None,
            library: ModelLibrary::InMemory,
        }
    }

    /// Build a `ModelicaDocument` backed by a file on disk (or a bundled
    /// asset path), carrying its library classification for read-only
    /// rules and UI hints.
    pub fn with_origin(
        id: DocumentId,
        source: impl Into<String>,
        canonical_path: Option<PathBuf>,
        library: ModelLibrary,
    ) -> Self {
        Self {
            id,
            source: source.into(),
            generation: 0,
            canonical_path,
            library,
        }
    }

    /// The current source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Byte length of the source.
    pub fn len(&self) -> usize {
        self.source.len()
    }

    /// True when the source buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// File path the source was loaded from, if any. `None` for untitled
    /// in-memory documents (save-as required before write-back).
    pub fn canonical_path(&self) -> Option<&std::path::Path> {
        self.canonical_path.as_deref()
    }

    /// Which library classification this document belongs to. Determines
    /// writability and UI badges.
    pub fn library(&self) -> &ModelLibrary {
        &self.library
    }

    /// True when this document is treated as read-only by the UI.
    /// MSL and Bundled libraries are read-only today.
    pub fn is_read_only(&self) -> bool {
        matches!(self.library, ModelLibrary::MSL | ModelLibrary::Bundled)
    }

    /// Set or change the canonical path (e.g. after Save-As on an
    /// untitled document). Does not re-classify `library`.
    pub fn set_canonical_path(&mut self, path: Option<PathBuf>) {
        self.canonical_path = path;
    }
}

/// The op type for [`ModelicaDocument`].
///
/// Today contains exactly one variant. Additional ops (granular
/// Insert/Delete text ops, then AST-level ops) are planned but deferred
/// until a live panel needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelicaOp {
    /// Replace the entire source buffer. The inverse is another
    /// `ReplaceSource` carrying the previous source text.
    ReplaceSource {
        /// The new source text to install.
        new: String,
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
            ModelicaOp::ReplaceSource { new } => {
                // Move the old source out as the inverse — no cloning.
                let old = std::mem::replace(&mut self.source, new);
                self.generation = self.generation.saturating_add(1);
                Ok(ModelicaOp::ReplaceSource { new: old })
            }
        }
    }
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
}
