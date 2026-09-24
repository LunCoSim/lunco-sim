//! Cross-document reference index.
//!
//! Maintains a workspace-wide map from fully-qualified [`SymbolPath`] to
//! the document and node that define it, plus the inverse "who depends on
//! whom" relation.
//!
//! Each [`DomainEngine`](crate::DomainEngine) reports its document's
//! `defines` and `refs_out`; the workbench feeds those into a single
//! [`RefIndex`] so cross-file rename, dangling-ref validation, and
//! downstream-Index invalidation work uniformly across domains.
//!
//! ## Maintenance contract
//!
//! Whenever a document is opened, mutated, or closed, the workbench must:
//!
//! 1. Pull `engine.defines(id)`, call [`RefIndex::update_doc_definitions`].
//! 2. Pull `engine.refs_out(id)`, call [`RefIndex::update_doc_references`].
//! 3. On close, call [`RefIndex::close_document`].
//!
//! The index is incremental — these calls only reshuffle entries for the
//! one doc, not the whole workspace.

use crate::{DocumentId, NodeId, ResolvedRef, SymbolPath, domain_engine::SymbolRef};
use std::collections::{HashMap, HashSet};

/// Workspace-wide cross-document reference table.
///
/// Single instance per workbench (or per server, in multi-user). Cheap reads,
/// O(refs) writes per doc update.
#[derive(Default)]
pub struct RefIndex {
    /// Fully-qualified symbol → where it's defined.
    defs: HashMap<SymbolPath, ResolvedRef>,

    /// Symbols a given document defines (used to revoke them on close/edit).
    doc_defines: HashMap<DocumentId, HashSet<SymbolPath>>,

    /// Inverse: documents that depend on a given target document.
    /// `dependents[A]` = "documents that have refs into A".
    dependents: HashMap<DocumentId, HashSet<DocumentId>>,

    /// What each document references (used to revoke its dependent edges).
    doc_refs: HashMap<DocumentId, Vec<SymbolPath>>,
}

impl RefIndex {
    /// Construct an empty reference index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the definitions attributed to `doc` with `defines`.
    /// Removes any stale entries this doc previously owned.
    pub fn update_doc_definitions(&mut self, doc: DocumentId, defines: &[SymbolPath]) {
        if let Some(old) = self.doc_defines.remove(&doc) {
            for path in old {
                if let Some(existing) = self.defs.get(&path) {
                    if existing.doc == doc {
                        self.defs.remove(&path);
                    }
                }
            }
        }
        let mut set = HashSet::with_capacity(defines.len());
        for path in defines {
            // NodeId here is the symbol path itself — engines that want
            // finer-grained node identity can override by populating
            // ResolvedRef differently after the fact.
            self.defs.insert(
                path.clone(),
                ResolvedRef {
                    doc,
                    node: NodeId::new(path.as_str()),
                },
            );
            set.insert(path.clone());
        }
        self.doc_defines.insert(doc, set);
    }

    /// Replace the outbound references attributed to `doc`.
    /// Reconciles the dependents inverse-index.
    pub fn update_doc_references(&mut self, doc: DocumentId, refs: &[SymbolRef]) {
        if let Some(old) = self.doc_refs.remove(&doc) {
            for path in &old {
                if let Some(target) = self.defs.get(path) {
                    if let Some(deps) = self.dependents.get_mut(&target.doc) {
                        deps.remove(&doc);
                    }
                }
            }
        }
        let mut paths = Vec::with_capacity(refs.len());
        for r in refs {
            if let Some(target) = self.defs.get(&r.path) {
                self.dependents.entry(target.doc).or_default().insert(doc);
            }
            paths.push(r.path.clone());
        }
        self.doc_refs.insert(doc, paths);
    }

    /// Look up where a symbol is defined.
    pub fn resolve(&self, path: &SymbolPath) -> Option<&ResolvedRef> {
        self.defs.get(path)
    }

    /// Iterate documents that reference `doc`.
    pub fn dependents_of(&self, doc: DocumentId) -> impl Iterator<Item = DocumentId> + '_ {
        self.dependents
            .get(&doc)
            .into_iter()
            .flat_map(|s| s.iter().copied())
    }

    /// Drop all entries owned by or pointing at `doc`.
    pub fn close_document(&mut self, doc: DocumentId) {
        if let Some(defines) = self.doc_defines.remove(&doc) {
            for path in defines {
                if let Some(existing) = self.defs.get(&path) {
                    if existing.doc == doc {
                        self.defs.remove(&path);
                    }
                }
            }
        }
        if let Some(refs) = self.doc_refs.remove(&doc) {
            for path in refs {
                if let Some(target) = self.defs.get(&path) {
                    if let Some(deps) = self.dependents.get_mut(&target.doc) {
                        deps.remove(&doc);
                    }
                }
            }
        }
        self.dependents.remove(&doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain_engine::{NodeId, SymbolRef};

    fn p(s: &str) -> SymbolPath {
        SymbolPath::new(s)
    }

    #[test]
    fn defines_then_resolves() {
        let mut idx = RefIndex::new();
        let doc_a = DocumentId::new(1);
        idx.update_doc_definitions(doc_a, &[p("Rocket.Engine")]);
        let r = idx.resolve(&p("Rocket.Engine")).unwrap();
        assert_eq!(r.doc, doc_a);
    }

    #[test]
    fn dependents_track_outbound_refs() {
        let mut idx = RefIndex::new();
        let doc_a = DocumentId::new(1);
        let doc_b = DocumentId::new(2);
        idx.update_doc_definitions(doc_a, &[p("Rocket.Engine")]);
        idx.update_doc_references(
            doc_b,
            &[SymbolRef {
                path: p("Rocket.Engine"),
                from_node: NodeId::new("Vehicle.engine"),
            }],
        );
        let deps: Vec<_> = idx.dependents_of(doc_a).collect();
        assert_eq!(deps, vec![doc_b]);
    }

    #[test]
    fn close_revokes_defs_and_deps() {
        let mut idx = RefIndex::new();
        let doc_a = DocumentId::new(1);
        let doc_b = DocumentId::new(2);
        idx.update_doc_definitions(doc_a, &[p("Rocket.Engine")]);
        idx.update_doc_references(
            doc_b,
            &[SymbolRef {
                path: p("Rocket.Engine"),
                from_node: NodeId::new("Vehicle.engine"),
            }],
        );
        idx.close_document(doc_a);
        assert!(idx.resolve(&p("Rocket.Engine")).is_none());
        assert_eq!(idx.dependents_of(doc_a).count(), 0);
    }
}
