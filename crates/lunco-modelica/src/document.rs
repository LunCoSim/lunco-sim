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

use std::path::{Path, PathBuf};

use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin};

/// The canonical Document representation of one Modelica source file.
///
/// Owns the source text + a [`DocumentOrigin`] describing where it
/// came from (which drives save behavior, tab title, read-only
/// badge). The parsed AST is **not** cached on the document today —
/// callers continue to parse on demand via
/// `crate::ast_extract::parse_to_ast` until a concrete caller makes
/// caching worth the complexity (invalidation on every op).
#[derive(Debug, Clone)]
pub struct ModelicaDocument {
    id: DocumentId,
    source: String,
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
        Self {
            id,
            source: source.into(),
            generation: 0,
            origin,
            last_saved_generation,
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
