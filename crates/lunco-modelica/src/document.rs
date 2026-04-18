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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}
