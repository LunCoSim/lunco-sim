//! Obstacle-field *config* journaling — the singleton twin of the per-document
//! op journaling (USD / Modelica / Script / Shader).
//!
//! The `ObstacleFieldSpec` is a single global resource, not a
//! [`lunco_doc::Document`], so the `JournalOpRecorder` auto-bridge doesn't apply.
//! Instead this module owns a one-variant op vocabulary here (where both the spec
//! type and `lunco-twin-journal` are in scope — the orphan rule forbids impl'ing
//! the foreign `OpPayload` for a foreign type elsewhere) and records a `SetSpec`
//! **explicitly** at the [`UpdateObstacleFieldSpec`](crate::plugin::UpdateObstacleFieldSpec)
//! chokepoint.
//!
//! This replaces the former bespoke host→client broadcast
//! (`sync_obstacle_field_spec`): the spec now rides the journal plane, so a tweak
//! syncs **both** directions (any peer can tune), persists across a restart, and
//! joins the canonical twin history — one plane, per
//! `NETWORKING_STATE_SYNC_TAXONOMY_DESIGN.md`.

use lunco_doc::DocumentId;
use lunco_doc_bevy::JournalResource;
use lunco_twin_journal::{AuthorTag, DomainKind, OpPayload};
use serde::{Deserialize, Serialize};

use crate::spec::ObstacleFieldSpec;

/// A journaled edit to the obstacle-field config. The spec is a singleton, so a
/// full-snapshot `SetSpec` (carrying the whole spec) is the entire vocabulary —
/// replay just installs it. Serialized into the canonical twin journal
/// (`DomainKind::ObstacleField`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObstacleFieldOp {
    SetSpec { spec: ObstacleFieldSpec },
}

impl OpPayload for ObstacleFieldOp {
    fn domain(&self) -> DomainKind {
        DomainKind::ObstacleField
    }
}

/// Stable singleton `DocumentId` for the one obstacle-field spec — identical on
/// every peer, so `record_op` keys all its entries under the same document.
fn doc_id() -> DocumentId {
    // Fixed sentinel (a fold of "lunco:obstacle-field"); the spec is a singleton
    // so there is never more than one such document.
    DocumentId::new(0x0B57_AC1E_F1E1_D000)
}

/// Record a spec change `(prev → next)` into the journal under the singleton doc
/// id, attributed to the local user. The inverse is the previous spec, so a
/// per-author undo restores it. No-op logging on serialize failure.
pub fn record_set_spec(
    journal: &JournalResource,
    prev: &ObstacleFieldSpec,
    next: &ObstacleFieldSpec,
) {
    let forward = ObstacleFieldOp::SetSpec { spec: next.clone() };
    let inverse = ObstacleFieldOp::SetSpec { spec: prev.clone() };
    journal.with_write(|j| {
        if let Err(e) = j.record_op(AuthorTag::local_user(), doc_id(), &forward, &inverse, None) {
            bevy::log::warn!("[obstacle-field-journal] record_op failed: {e}");
        }
    });
}

/// Decode a replayed op (arrived via the journal) into the spec to install.
/// `None` (logged) if the payload isn't an `ObstacleFieldOp`. The replay entry
/// point — it does **not** record (the op is already in the journal).
pub fn replay_spec(op_json: &serde_json::Value) -> Option<ObstacleFieldSpec> {
    match serde_json::from_value::<ObstacleFieldOp>(op_json.clone()) {
        Ok(ObstacleFieldOp::SetSpec { spec }) => Some(spec),
        Err(e) => {
            bevy::log::warn!("[obstacle-field-journal] op payload is not an ObstacleFieldOp: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_spec_round_trips_through_json() {
        let mut spec = ObstacleFieldSpec::default();
        spec.seed = 0xABCD;
        spec.region_half_extent = 123.0;
        let op = ObstacleFieldOp::SetSpec { spec: spec };
        assert_eq!(op.domain(), DomainKind::ObstacleField);
        let json = serde_json::to_value(&op).unwrap();
        let back = replay_spec(&json).expect("decodes");
        assert_eq!(back.seed, 0xABCD);
        assert_eq!(back.region_half_extent, 123.0);
    }

    #[test]
    fn bad_payload_is_rejected_softly() {
        assert!(replay_spec(&serde_json::json!({ "nope": 1 })).is_none());
    }
}
