//! `ModelicaDocument` — the Document System representation of one `.mo` file.
//!
//! # Status: dormant first migration step
//!
//! This module exists so the rest of the crate can start depending on a real
//! [`Document`] type, but no panel uses it yet. The current `EditorBufferState`
//! / `OpenModel` / `ModelicaModel.original_source` trio is still the live
//! editing pipeline — see `ui/panels/code_editor.rs`.
//!
//! The migration is split for safety:
//!
//! 1. **This commit**: introduce `ModelicaDocument` + `ModelicaOp` with the
//!    minimum viable op (`ReplaceSource`), plus tests. Nothing calls it.
//! 2. **Next commit**: migrate the CodeEditor panel to drive a
//!    [`DocumentHost<ModelicaDocument>`] for its canonical source text,
//!    replacing `EditorBufferState.text` as the source of truth.
//! 3. **Later**: grow the op set (granular text ops, then AST-level ops like
//!    `AddComponent`, `SetParameter`, `AddConnection`) as more panels migrate.
//!
//! # Op set today
//!
//! Exactly one op: [`ModelicaOp::ReplaceSource`]. Coarse on purpose — it's
//! enough to make CodeEditor participate in Document-level undo/redo and in
//! the cross-panel change notification story (generation counter). Finer
//! ops come when we need them.
//!
//! The inverse of `ReplaceSource { new }` is `ReplaceSource { new: old }`,
//! where `old` is the source text as it was before the op applied.

use lunco_doc::{Document, DocumentError, DocumentId, DocumentOp};

/// The canonical Document representation of one Modelica source file.
///
/// Owns the source text. The parsed AST is **not** cached on the document
/// today — callers continue to parse on demand via
/// `crate::ast_extract::parse_to_ast` until a concrete caller makes caching
/// worth the complexity (invalidation on every op).
#[derive(Debug, Clone)]
pub struct ModelicaDocument {
    id: DocumentId,
    source: String,
    generation: u64,
}

impl ModelicaDocument {
    /// Build a new `ModelicaDocument` from source text.
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self {
            id,
            source: source.into(),
            generation: 0,
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
