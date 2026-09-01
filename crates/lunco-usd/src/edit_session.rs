//! USD Assembly Editor review state.
//!
//! A proposal is a pending, typed authoring plan.  It is deliberately not a
//! document, layer, journal, or second operation log: the document registry
//! remains the only owner of authored state and the Twin journal remains the
//! only history stream.  The proposal is validated against a cloned
//! [`UsdDocument`] so humans and agents can inspect a complete plan without
//! changing the document being reviewed.

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_doc::{Document, DocumentError, DocumentId};

use crate::{UsdDocument, UsdOp};

/// The explicit semantic target of an Assembly Editor proposal.
///
/// These scopes are authoring policy, not storage layers.  Source assets and
/// assemblies are separate USD documents; an instance override is a root-layer
/// opinion below a composed reference in the assembly document.  Keeping the
/// scope beside the plan prevents a caller from silently turning an instance
/// edit into a source-asset edit.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize,
)]
pub enum UsdEditScope {
    /// Edit the standalone source asset's own authored opinions.
    SourceAsset,
    /// Edit the authored assembly document and its composition structure.
    Assembly,
    /// Edit a local root-layer opinion below a referenced or payloaded asset.
    InstanceOverride,
}

impl UsdEditScope {
    /// Stable API spelling used by status and agent responses.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceAsset => "source_asset",
            Self::Assembly => "assembly",
            Self::InstanceOverride => "instance_override",
        }
    }
}

/// Review state for a proposal that has not yet entered the document journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize)]
pub enum UsdProposalState {
    /// Ready for human or agent review.
    Pending,
    /// Intentionally hidden from the active review list but still available.
    Muted,
    /// The document or backing file changed since the proposal was prepared.
    Conflict,
}

impl UsdProposalState {
    /// Stable API spelling used by status and agent responses.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Muted => "muted",
            Self::Conflict => "conflict",
        }
    }
}

/// Session-local id for a pending proposal.  It identifies a review plan, not
/// a journal entry; the journal allocates its own change-set id on commit.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct UsdProposalId(pub u64);

/// Result of validating a proposal against one document generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdProposalValidation {
    /// The generation against which the candidate was checked.
    pub generation: u64,
    /// The authored base-layer revision at validation time.
    pub base_revision: u64,
    /// A stable document identity marker used to reject Save-As rebinding.
    pub origin: String,
    /// Affected prim/property paths, deduplicated in input order.
    pub affected_paths: Vec<String>,
    /// Validation diagnostics. An empty vector means valid.
    pub diagnostics: Vec<String>,
}

impl UsdProposalValidation {
    /// Whether validation produced no errors.
    pub fn is_valid(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// One pending Assembly Editor proposal.
#[derive(Debug, Clone)]
pub struct UsdProposal {
    /// Review id allocated by [`UsdEditSessions`].
    pub id: UsdProposalId,
    /// Document the plan applies to.
    pub doc: DocumentId,
    /// Explicit source/assembly/instance authoring scope.
    pub scope: UsdEditScope,
    /// Human-readable intent label, also used as the journal change-set label.
    pub label: String,
    /// Generation the author read before preparing the plan.
    pub parent_generation: u64,
    /// Base-layer revision the author read before preparing the plan.
    pub base_revision: u64,
    /// Document origin identity the author read before preparing the plan.
    pub origin: String,
    /// Typed operations waiting for review. This is a plan, not history.
    pub ops: Vec<UsdOp>,
    /// Current review state.
    pub state: UsdProposalState,
    /// Last validation diagnostics, including conflict diagnostics.
    pub diagnostics: Vec<String>,
    /// Paths touched by the plan.
    pub affected_paths: Vec<String>,
}

/// Read-only proposal data carried by the change-gated USD browser view.
///
/// The browser needs enough information to render and dispatch review actions,
/// but it does not need to clone the complete typed plan every frame. The full
/// [`UsdProposal`] remains owned by [`UsdEditSessions`] and is exposed through
/// the agent query when operation-level inspection is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdProposalSummary {
    /// Review id of the proposal.
    pub id: UsdProposalId,
    /// Explicit authoring scope.
    pub scope: UsdEditScope,
    /// Human-readable intent label.
    pub label: String,
    /// Current review state.
    pub state: UsdProposalState,
    /// Last validation or conflict diagnostics.
    pub diagnostics: Vec<String>,
    /// Prim/property paths touched by the plan.
    pub affected_paths: Vec<String>,
    /// Number of typed operations in the plan.
    pub operation_count: usize,
}

impl UsdProposal {
    /// Build the presentation snapshot used by the change-gated browser view.
    pub fn summary(&self) -> UsdProposalSummary {
        UsdProposalSummary {
            id: self.id,
            scope: self.scope,
            label: self.label.clone(),
            state: self.state,
            diagnostics: self.diagnostics.clone(),
            affected_paths: self.affected_paths.clone(),
            operation_count: self.ops.len(),
        }
    }
}

/// All pending Assembly Editor proposals for the current process.
///
/// This resource owns review metadata only.  It never mirrors authored USD
/// bytes, and it is cleared when a document closes or is explicitly discarded.
#[derive(Resource, Default)]
pub struct UsdEditSessions {
    next_id: u64,
    revision: u64,
    proposals: HashMap<UsdProposalId, UsdProposal>,
}

impl UsdEditSessions {
    /// Revision of the review view-model. UI producers include this in their
    /// change gate because proposal state does not bump document generation.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Return a proposal by id.
    pub fn proposal(&self, id: UsdProposalId) -> Option<&UsdProposal> {
        self.proposals.get(&id)
    }

    /// Iterate proposals belonging to one document.
    pub fn for_document(&self, doc: DocumentId) -> impl Iterator<Item = &UsdProposal> {
        self.proposals
            .values()
            .filter(move |proposal| proposal.doc == doc)
    }

    /// Insert a validated proposal and return its review id.
    pub fn insert(
        &mut self,
        doc: DocumentId,
        scope: UsdEditScope,
        label: String,
        parent_generation: u64,
        validation: UsdProposalValidation,
        ops: Vec<UsdOp>,
    ) -> UsdProposalId {
        self.next_id = self.next_id.saturating_add(1);
        let id = UsdProposalId(self.next_id);
        self.proposals.insert(
            id,
            UsdProposal {
                id,
                doc,
                scope,
                label,
                parent_generation,
                base_revision: validation.base_revision,
                origin: validation.origin,
                ops,
                state: UsdProposalState::Pending,
                diagnostics: validation.diagnostics,
                affected_paths: validation.affected_paths,
            },
        );
        self.revision = self.revision.wrapping_add(1);
        id
    }

    /// Remove one proposal after explicit rejection or successful commit.
    pub fn remove(&mut self, id: UsdProposalId) -> Option<UsdProposal> {
        let removed = self.proposals.remove(&id);
        if removed.is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
        removed
    }

    /// Remove all pending review state for a closed or discarded document.
    pub fn remove_document(&mut self, doc: DocumentId) -> usize {
        let before = self.proposals.len();
        self.proposals.retain(|_, proposal| proposal.doc != doc);
        let removed = before - self.proposals.len();
        if removed != 0 {
            self.revision = self.revision.wrapping_add(1);
        }
        removed
    }

    /// Change one pending proposal's review state without touching authored USD.
    pub fn set_state(&mut self, id: UsdProposalId, state: UsdProposalState) -> Result<(), String> {
        let proposal = self
            .proposals
            .get_mut(&id)
            .ok_or_else(|| format!("unknown USD proposal {}", id.0))?;
        if state == UsdProposalState::Conflict {
            return Err(format!(
                "USD proposal {} enters Conflict only through validation",
                id.0
            ));
        }
        if proposal.state == UsdProposalState::Conflict && state != UsdProposalState::Conflict {
            return Err(format!(
                "USD proposal {} is conflicted; create a new proposal from the current generation",
                id.0
            ));
        }
        if proposal.state == state {
            return Ok(());
        }
        proposal.state = state;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    /// Replace diagnostics and mark a proposal as a conflict without applying
    /// any of its operations.
    pub fn mark_conflict(&mut self, id: UsdProposalId, diagnostic: String) {
        if let Some(proposal) = self.proposals.get_mut(&id) {
            proposal.state = UsdProposalState::Conflict;
            proposal.diagnostics = vec![diagnostic];
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

/// Validate a complete proposal plan against `document` without mutating it.
///
/// The clone is the same typed USD document owner used by
/// `DocumentHost::apply_group_against`; every operation is therefore checked
/// by the real USD authoring validator, including structure, schema values,
/// transforms, relationships, and layer permissions.
pub fn validate_proposal(
    document: &UsdDocument,
    scope: UsdEditScope,
    parent_generation: u64,
    ops: &[UsdOp],
) -> UsdProposalValidation {
    let mut diagnostics = Vec::new();
    let generation = document.generation();
    let base_revision = document.base_revision();
    let origin = document.origin().session_uri();
    let affected_paths = unique_paths(ops);

    if parent_generation != generation {
        diagnostics.push(format!(
            "stale document generation: proposal parent {parent_generation}, current {generation}"
        ));
    }
    if ops.is_empty() {
        diagnostics.push("proposal must contain at least one USD operation".to_owned());
    }
    for op in ops {
        if !op.edit_target().is_root() {
            diagnostics.push(format!(
                "proposal operation at {:?} must target @root@; runtime edits are not persistent Assembly Editor proposals",
                op.affected_paths()
            ));
        }
    }

    match scope {
        UsdEditScope::SourceAsset => {
            for path in &affected_paths {
                match document.path_is_under_composed_arc(path) {
                    Ok(true) => diagnostics.push(format!(
                        "source-asset proposal cannot edit composed instance path `{path}`; use InstanceOverride"
                    )),
                    Ok(false) => {}
                    Err(error) => diagnostics.push(error.to_string()),
                }
            }
        }
        UsdEditScope::Assembly => {}
        UsdEditScope::InstanceOverride => {
            if affected_paths.is_empty() {
                diagnostics
                    .push("instance-override proposal needs an affected composed path".to_owned());
            }
            for path in &affected_paths {
                match document.path_is_under_composed_arc(path) {
                    Ok(true) => {}
                    Ok(false) => diagnostics.push(format!(
                        "instance-override path `{path}` is not below an authored reference or payload"
                    )),
                    Err(error) => diagnostics.push(error.to_string()),
                }
            }
        }
    }

    if diagnostics.is_empty() {
        let mut candidate = document.clone();
        for op in ops {
            if let Err(error) = candidate.apply(op.clone()) {
                diagnostics.push(document_error(error));
                break;
            }
        }
        if diagnostics.is_empty()
            && lunco_usd_compose::layer_dependency_arcs(&candidate.source()).is_none()
        {
            diagnostics.push("candidate authored layer is not valid USDA composition".to_owned());
        }
    }

    UsdProposalValidation {
        generation,
        base_revision,
        origin,
        affected_paths,
        diagnostics,
    }
}

fn document_error(error: DocumentError) -> String {
    error.to_string()
}

fn unique_paths(ops: &[UsdOp]) -> Vec<String> {
    let mut paths = Vec::new();
    for path in ops.iter().flat_map(UsdOp::affected_paths) {
        if !paths.iter().any(|known| known == &path) {
            paths.push(path);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::{Document, DocumentId};

    fn assembly_document() -> UsdDocument {
        UsdDocument::new(DocumentId::new(1), "#usda 1.0\ndef Xform \"Assembly\" {}\n")
    }

    fn add_prim(name: &str) -> UsdOp {
        UsdOp::AddPrim {
            edit_target: crate::LayerId::root(),
            parent_path: "/Assembly".to_owned(),
            name: name.to_owned(),
            type_name: Some("Xform".to_owned()),
            reference: None,
        }
    }

    #[test]
    fn validates_a_complete_assembly_plan_without_mutating_the_document() {
        let document = assembly_document();
        let validation =
            validate_proposal(&document, UsdEditScope::Assembly, 0, &[add_prim("Chassis")]);

        assert!(validation.is_valid(), "{validation:?}");
        assert_eq!(validation.generation, 0);
        assert_eq!(validation.affected_paths, vec!["/Assembly/Chassis"]);
        assert!(!document
            .authored_prim_exists(&crate::LayerId::root(), "/Assembly/Chassis")
            .unwrap());
    }

    #[test]
    fn rejects_empty_stale_and_runtime_plans_at_the_review_boundary() {
        let document = assembly_document();
        let empty = validate_proposal(&document, UsdEditScope::Assembly, 0, &[]);
        assert!(!empty.is_valid());
        assert!(empty.diagnostics.iter().any(|d| d.contains("at least one")));

        let stale = validate_proposal(&document, UsdEditScope::Assembly, 1, &[add_prim("Chassis")]);
        assert!(!stale.is_valid());
        assert!(stale
            .diagnostics
            .iter()
            .any(|d| d.contains("stale document")));

        let runtime = UsdOp::AddPrim {
            edit_target: crate::LayerId::runtime(),
            parent_path: "/Assembly".to_owned(),
            name: "RuntimeOnly".to_owned(),
            type_name: Some("Xform".to_owned()),
            reference: None,
        };
        let runtime_plan = validate_proposal(&document, UsdEditScope::Assembly, 0, &[runtime]);
        assert!(!runtime_plan.is_valid());
        assert!(runtime_plan
            .diagnostics
            .iter()
            .any(|d| d.contains("@root@")));
    }

    #[test]
    fn scope_uses_authored_usd_arcs_for_source_and_instance_override_policy() {
        let document = UsdDocument::new(
            DocumentId::new(2),
            "#usda 1.0\ndef Xform \"Assembly\" (\n    references = @asset.usda@\n) {}\n",
        );
        assert!(document.parse_error().is_none());

        let instance_override = UsdOp::SetAttribute {
            edit_target: crate::LayerId::root(),
            path: "/Assembly/Chassis".to_owned(),
            name: "user:role".to_owned(),
            type_name: "string".to_owned(),
            value: "lander".to_owned(),
        };
        let instance = validate_proposal(
            &document,
            UsdEditScope::InstanceOverride,
            0,
            std::slice::from_ref(&instance_override),
        );
        assert!(instance.is_valid(), "{instance:?}");

        let source = validate_proposal(
            &document,
            UsdEditScope::SourceAsset,
            0,
            &[instance_override],
        );
        assert!(!source.is_valid());
        assert!(source
            .diagnostics
            .iter()
            .any(|d| d.contains("InstanceOverride")));
    }

    #[test]
    fn review_state_and_conflicts_are_document_scoped() {
        let document = assembly_document();
        let validation =
            validate_proposal(&document, UsdEditScope::Assembly, 0, &[add_prim("Chassis")]);
        let mut sessions = UsdEditSessions::default();
        let id = sessions.insert(
            document.id(),
            UsdEditScope::Assembly,
            "Add chassis".to_owned(),
            0,
            validation,
            vec![add_prim("Chassis")],
        );
        assert_eq!(sessions.for_document(document.id()).count(), 1);
        assert_eq!(sessions.for_document(DocumentId::new(99)).count(), 0);

        sessions.set_state(id, UsdProposalState::Muted).unwrap();
        assert_eq!(
            sessions.for_document(document.id()).next().unwrap().state,
            UsdProposalState::Muted
        );
        sessions.mark_conflict(id, "generation changed".to_owned());
        assert_eq!(
            sessions.for_document(document.id()).next().unwrap().state,
            UsdProposalState::Conflict
        );
        assert!(sessions.set_state(id, UsdProposalState::Pending).is_err());
        assert_eq!(sessions.remove_document(document.id()), 1);
        assert!(sessions.proposal(id).is_none());
    }
}
