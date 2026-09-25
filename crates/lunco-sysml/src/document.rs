//! The canonical Document representation of one SysML source buffer.

use lunco_doc::{
    Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin, FileBacked, ForkableDocument,
};
use lunco_twin_journal::{DomainKind, OpPayload};
use std::ops::Range;
use std::sync::Arc;

/// Reversible source-level edits for a SysML document.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SysmlOp {
    /// Replace the complete UTF-8 source buffer.
    ReplaceSource {
        /// New source text.
        new: String,
    },
    /// Replace a byte range in the current source buffer.
    EditText {
        /// Inclusive-start, exclusive-end byte range.
        range: Range<usize>,
        /// Replacement text.
        replacement: String,
    },
}

impl DocumentOp for SysmlOp {}

impl OpPayload for SysmlOp {
    fn domain(&self) -> DomainKind {
        DomainKind::Sysml
    }
}

/// A source-backed SysML document with revisioned, asynchronous analysis.
#[derive(Clone)]
pub struct SysmlDocument {
    id: DocumentId,
    source: Arc<str>,
    origin: DocumentOrigin,
    generation: u64,
    last_saved_generation: Option<u64>,
}

impl SysmlDocument {
    /// Create an untitled source document.
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self::with_origin(
            id,
            source.into(),
            DocumentOrigin::untitled(format!("Untitled-{}", id.raw())),
        )
    }

    /// Build a document from source and an explicit origin without parsing it.
    pub fn with_origin(id: DocumentId, source: impl Into<String>, origin: DocumentOrigin) -> Self {
        Self::with_source_snapshot(id, Arc::from(source.into()), origin)
    }

    fn with_source_snapshot(id: DocumentId, source: Arc<str>, origin: DocumentOrigin) -> Self {
        let saved = (!origin.is_untitled()).then_some(0);
        Self {
            id,
            source,
            origin,
            generation: 0,
            last_saved_generation: saved,
        }
    }

    /// Current source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn source_snapshot(&self) -> Arc<str> {
        Arc::clone(&self.source)
    }

    /// Current document origin.
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    /// Rebind this document to a new origin after a successful Save-As.
    pub fn set_origin(&mut self, origin: DocumentOrigin) {
        self.origin = origin;
    }

    /// Mark the current generation as persisted.
    pub fn mark_saved(&mut self) {
        self.last_saved_generation = Some(self.generation);
    }
    fn replace_source(&mut self, source: String) -> Result<SysmlOp, DocumentError> {
        let old = std::mem::replace(&mut self.source, Arc::from(source));
        self.generation = self.generation.saturating_add(1);
        Ok(SysmlOp::ReplaceSource {
            new: old.to_string(),
        })
    }

    fn edit_text(
        &mut self,
        range: Range<usize>,
        replacement: String,
    ) -> Result<SysmlOp, DocumentError> {
        if range.start > range.end || range.end > self.source.len() {
            return Err(DocumentError::ValidationFailed(format!(
                "text range {}..{} out of bounds (len={})",
                range.start,
                range.end,
                self.source.len()
            )));
        }
        if !self.source.is_char_boundary(range.start) || !self.source.is_char_boundary(range.end) {
            return Err(DocumentError::ValidationFailed(format!(
                "text range {}..{} not on UTF-8 boundaries",
                range.start, range.end
            )));
        }
        let mut source = self.source.to_string();
        let old = source[range.clone()].to_owned();
        source.replace_range(range.clone(), &replacement);
        self.source = Arc::from(source);
        self.generation = self.generation.saturating_add(1);
        Ok(SysmlOp::EditText {
            range: range.start..range.start + replacement.len(),
            replacement: old,
        })
    }
}

impl Document for SysmlDocument {
    type Op = SysmlOp;

    fn id(&self) -> DocumentId {
        self.id
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError> {
        if !self.origin.accepts_mutations() {
            return Err(DocumentError::ReadOnly);
        }
        match op {
            SysmlOp::ReplaceSource { new } => self.replace_source(new),
            SysmlOp::EditText { range, replacement } => self.edit_text(range, replacement),
        }
    }
}

impl FileBacked for SysmlDocument {
    fn with_origin(id: DocumentId, source: String, origin: DocumentOrigin) -> Self {
        Self::with_origin(id, source, origin)
    }

    fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    fn is_dirty(&self) -> bool {
        match self.last_saved_generation {
            Some(saved) => saved != self.generation,
            None => true,
        }
    }

    fn mark_saved(&mut self) {
        SysmlDocument::mark_saved(self);
    }

    fn reload_base(&mut self, source: &str) -> bool {
        if self.source.as_ref() != source {
            self.source = Arc::from(source);
            self.generation = self.generation.saturating_add(1);
        }
        self.last_saved_generation = Some(self.generation);
        true
    }

    fn reset_to_source(&mut self, source: &str) -> bool {
        self.reload_base(source)
    }
}

impl ForkableDocument for SysmlDocument {
    fn fork(&self, id: DocumentId, name: String) -> Result<Self, DocumentError> {
        Ok(Self::with_source_snapshot(
            id,
            Arc::clone(&self.source),
            DocumentOrigin::untitled(name),
        ))
    }
}

pub(crate) fn source_name(origin: &DocumentOrigin) -> String {
    let name = origin.session_uri();
    if name.ends_with(".sysml") || name.ends_with(".kerml") {
        name
    } else {
        format!("{name}.sysml")
    }
}

pub(crate) fn build_analysis(
    origin: &DocumentOrigin,
    source: &str,
    generation: u64,
) -> lunco_sysml_ast::SysmlAnalysis {
    lunco_sysml_ast::SysmlAnalysis::build(
        [(source_name(origin), source.to_owned())],
        true,
        generation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_edits_advance_generation_and_preserve_inverse() {
        let mut document = SysmlDocument::new(DocumentId::new(7), "part def Rover {}");
        let inverse = document
            .apply(SysmlOp::EditText {
                range: 9..14,
                replacement: "Example".into(),
            })
            .unwrap();
        assert_eq!(document.source(), "part def Example {}");
        assert_eq!(document.generation(), 1);
        document.apply(inverse).unwrap();
        assert_eq!(document.source(), "part def Rover {}");
    }

    #[test]
    fn source_snapshot_remains_immutable_after_an_edit() {
        let mut document = SysmlDocument::new(DocumentId::new(8), "part def Rover {}");
        let snapshot = document.source_snapshot();
        document
            .apply(SysmlOp::ReplaceSource {
                new: "part def Lander {}".to_owned(),
            })
            .unwrap();

        assert_eq!(snapshot.as_ref(), "part def Rover {}");
        assert_eq!(document.source(), "part def Lander {}");
    }

    #[test]
    fn read_only_origins_reject_mutation() {
        let mut document = SysmlDocument::with_origin(
            DocumentId::new(1),
            "part def Rover {}",
            DocumentOrigin::bundled("Rover.sysml"),
        );
        assert_eq!(
            document.apply(SysmlOp::ReplaceSource { new: String::new() }),
            Err(DocumentError::ReadOnly)
        );
    }

    #[test]
    fn invalid_ranges_do_not_mutate() {
        let mut document = SysmlDocument::new(DocumentId::new(1), "part def Rover {}");
        let result = document.apply(SysmlOp::EditText {
            range: 0..999,
            replacement: String::new(),
        });
        assert!(matches!(result, Err(DocumentError::ValidationFailed(_))));
        assert_eq!(document.generation(), 0);
    }
}
