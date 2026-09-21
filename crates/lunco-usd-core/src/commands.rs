//! Typed USD document mutation commands.
//!
//! These are the shared command contracts for applying authored USD operations.
//! The runtime observer lives in `lunco-usd-commands`; keeping the data types here lets
//! scene and authoring packages submit USD edits without depending on the
//! aggregate runtime package.

use crate::edit_session::{UsdEditScope, UsdProposalId};
use bevy::ecs::reflect::ReflectEvent;
use bevy::prelude::{Reflect, Resource};
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;
use lunco_doc::{DocumentId, OpenOutcome};
use lunco_usd_document::document::UsdOp;

/// Apply one [`UsdOp`] to a document through the typed command bus.
///
/// The `lunco-usd-commands` runtime observes this command and routes it through the
/// document registry so undo/redo, change notification, and read-only
/// enforcement remain centralized there.
#[Command(default)]
pub struct ApplyUsdOp {
    /// Target document.
    pub doc_id: DocumentId,
    /// Generation the caller edited from. When present, the operation is
    /// rejected if the document advanced before it arrived.
    pub parent_gen: Option<u64>,
    /// Operation to apply.
    pub op: UsdOp,
}

/// Apply one authored intent consisting of several USD operations.
///
/// The `lunco-usd-commands` runtime journals this list as one undo unit and observes it
/// only after the document reaches its complete shape.
#[Command(default)]
pub struct ApplyUsdOps {
    /// Target document.
    pub doc_id: DocumentId,
    /// Generation the caller edited from. When present, the complete compound
    /// edit is rejected if the document advanced before it arrived.
    pub parent_gen: Option<u64>,
    /// Human-readable undo/journal label.
    pub label: String,
    /// Ordered primitive USD operations comprising the one intent.
    pub ops: Vec<UsdOp>,
}

/// Apply a compound USD edit to the disposable view layer rather than to user-
/// authored history. The document and typed operation log still advance, so
/// the live canonical stage receives the same ordered delta; the view layer is
/// excluded from the runtime sidecar, source save, undo/redo, and journal.
#[Command(default)]
pub struct ApplyUsdTransientOps {
    /// Target document.
    pub doc_id: DocumentId,
    /// Generation the view was derived from.
    pub parent_gen: Option<u64>,
    /// Human-readable diagnostic label.
    pub label: String,
    /// Ordered view operations.
    pub ops: Vec<UsdOp>,
}

/// Stable id for the USD document kind in the shared document registry.
pub const USD_DOCUMENT_KIND: &str = "usd";

/// A reason the mounted USD scene is empty, recorded by the runtime and read
/// by UI or headless hosts. The field is public because scene admission owns
/// the diagnostic text while presentation only displays it.
#[derive(Resource, Default)]
pub struct EmptyViewportReason(pub Option<String>);

/// Emitted after a USD file has been admitted to the canonical document
/// registry. Presentation adapters can claim a preview and surface a
/// non-fatal reload outcome; headless consumers can ignore it.
#[derive(bevy::prelude::Event, Clone, Copy, Debug)]
pub struct UsdDocumentReady {
    /// The admitted document.
    pub doc: DocumentId,
    /// Whether the registry allocated, refreshed, or retained the document.
    pub outcome: OpenOutcome,
}

/// Return whether `path` names a supported USD file extension.
pub fn is_usd_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        std::path::Path::new(&lower)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("usda") | Some("usdc") | Some("usd")
    )
}

/// Prepare a typed USD edit plan for review without mutating the document.
#[Command]
pub struct CreateUsdProposal {
    /// Document that owns the authored target.
    pub doc_id: DocumentId,
    /// Explicit source-asset, assembly, or instance-override scope.
    pub scope: UsdEditScope,
    /// Human-readable intent and eventual journal change-set label.
    pub label: String,
    /// Generation read by the proposal author.
    pub parent_gen: u64,
    /// Complete typed plan, kept out of the document until commit.
    pub ops: Vec<UsdOp>,
}

/// Review-only actions for a pending proposal.
#[derive(Debug, Clone, Copy, Reflect, serde::Serialize, serde::Deserialize)]
pub enum UsdProposalReviewAction {
    /// Keep the plan but remove it from the active review queue.
    Mute,
    /// Return a muted plan to active review.
    Unmute,
    /// Discard the plan without touching authored USD.
    Reject,
}

/// Change review state without applying any USD operation.
#[Command]
pub struct ReviewUsdProposal {
    /// Proposal allocated by [`CreateUsdProposal`].
    pub proposal: UsdProposalId,
    /// Review decision.
    pub action: UsdProposalReviewAction,
}

/// Merge one accepted proposal through the ordinary grouped USD edit path.
#[Command]
pub struct CommitUsdProposal {
    /// Proposal to accept and merge into its explicit document target.
    pub proposal: UsdProposalId,
}

/// Attach one source-backed simulation program to an existing USD prim.
#[Command(default)]
pub struct AttachProgram {
    /// Target USD document.
    pub doc_id: DocumentId,
    /// Complete program attachment intent.
    pub spec: crate::program::ProgramAttachSpec,
}

/// Attach one component asset to a host body as one journalled USD change set.
#[Command(default)]
pub struct AttachComponent {
    /// Target document.
    pub doc_id: DocumentId,
    /// The attachment to perform.
    pub spec: crate::attach::AttachSpec,
}

/// Remove one attached component as one atomic authored intent.
#[Command(default)]
pub struct DetachComponent {
    /// Target document.
    pub doc_id: DocumentId,
    /// Exact component attachment to remove.
    pub spec: crate::attach::DetachSpec,
}
