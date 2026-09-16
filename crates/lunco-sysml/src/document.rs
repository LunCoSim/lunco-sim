//! The canonical Document representation of one SysML source buffer.

use std::ops::Range;
use std::sync::Arc;

use lunco_doc::{
    Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin, FileBacked, ForkableDocument,
};
use lunco_sysml_ast::SysmlAnalysis;
use lunco_twin_journal::{DomainKind, OpPayload};

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

/// A source-backed SysML document with an eagerly refreshed semantic snapshot.
#[derive(Clone)]
pub struct SysmlDocument {
    id: DocumentId,
    source: String,
    origin: DocumentOrigin,
    generation: u64,
    last_saved_generation: Option<u64>,
    analysis: Arc<SysmlAnalysis>,
}

impl SysmlDocument {
    /// Create an untitled document with a fresh semantic snapshot.
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self::with_origin(
            id,
            source.into(),
            DocumentOrigin::untitled(format!("Untitled-{}", id.raw())),
        )
    }

    /// Build a document from source and an explicit origin.
    pub fn with_origin(id: DocumentId, source: impl Into<String>, origin: DocumentOrigin) -> Self {
        let source = source.into();
        let analysis = Arc::new(build_analysis(&origin, &source, 0));
        let saved = (!origin.is_untitled()).then_some(0);
        Self {
            id,
            source,
            origin,
            generation: 0,
            last_saved_generation: saved,
            analysis,
        }
    }

    /// Current source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Shared semantic snapshot for read-side consumers.
    pub fn analysis(&self) -> &SysmlAnalysis {
        &self.analysis
    }

    /// Shared semantic snapshot for worker/UI handoff.
    pub fn analysis_arc(&self) -> Arc<SysmlAnalysis> {
        Arc::clone(&self.analysis)
    }

    /// Current document origin.
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    /// Rebind this document to a new origin after a successful Save-As.
    pub fn set_origin(&mut self, origin: DocumentOrigin) {
        self.origin = origin;
        self.refresh_analysis();
    }

    /// Mark the current generation as persisted.
    pub fn mark_saved(&mut self) {
        self.last_saved_generation = Some(self.generation);
    }

    /// Whether this source has semantic/parser diagnostics.
    pub fn has_diagnostics(&self) -> bool {
        self.analysis.has_errors()
    }

    fn refresh_analysis(&mut self) {
        let name = source_name(&self.origin);
        self.analysis = Arc::new(SysmlAnalysis::build(
            [(name, self.source.clone())],
            true,
            self.generation,
        ));
    }

    fn replace_source(&mut self, source: String) -> Result<SysmlOp, DocumentError> {
        let old = std::mem::replace(&mut self.source, source);
        self.generation = self.generation.saturating_add(1);
        self.refresh_analysis();
        Ok(SysmlOp::ReplaceSource { new: old })
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
        let old = self.source[range.clone()].to_owned();
        self.source.replace_range(range.clone(), &replacement);
        self.generation = self.generation.saturating_add(1);
        self.refresh_analysis();
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

    fn reload_base(&mut self, source: &str) -> bool {
        if self.source != source {
            self.source.clear();
            self.source.push_str(source);
            self.generation = self.generation.saturating_add(1);
            self.refresh_analysis();
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
        Ok(Self::with_origin(
            id,
            self.source.clone(),
            DocumentOrigin::untitled(name),
        ))
    }
}

fn source_name(origin: &DocumentOrigin) -> String {
    let name = origin.session_uri();
    if name.ends_with(".sysml") || name.ends_with(".kerml") {
        name
    } else {
        format!("{name}.sysml")
    }
}

fn build_analysis(origin: &DocumentOrigin, source: &str, generation: u64) -> SysmlAnalysis {
    SysmlAnalysis::build([(source_name(origin), source.to_owned())], true, generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_refresh_semantics_and_inverse() {
        let mut document = SysmlDocument::new(DocumentId::new(7), "part def Rover {}");
        let inverse = document
            .apply(SysmlOp::EditText {
                range: 9..14,
                replacement: "Example".into(),
            })
            .unwrap();
        assert_eq!(document.source(), "part def Example {}");
        assert_eq!(document.generation(), 1);
        assert!(document
            .analysis()
            .elements()
            .iter()
            .any(|element| element.qualified_name == "Example"));
        document.apply(inverse).unwrap();
        assert_eq!(document.source(), "part def Rover {}");
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
