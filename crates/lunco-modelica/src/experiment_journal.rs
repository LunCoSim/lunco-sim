//! Experiment-*definition* journaling — the experiment twin of [`crate::journal`].
//!
//! `ExperimentRegistry` (in the deliberately lean, backend-agnostic
//! `lunco-experiments` crate) is a plain Bevy resource, **not** a
//! `lunco_doc::Document` — so the `JournalOpRecorder` auto-bridge that USD /
//! Modelica / Script docs use doesn't apply. Instead this module owns the
//! experiment op vocabulary here (where both `lunco-experiments` and
//! `lunco-twin-journal` are already deps — the orphan rule forbids impl'ing the
//! foreign `OpPayload` for a foreign `ExperimentOp` in a third crate) and
//! records ops **explicitly** at the definition-mutation chokepoints, applying
//! them through the registry's public mutators.
//!
//! Scope: only the *definition* is journaled (create / rename / bounds / params
//! / delete). Run **status** rides the ephemeral presence plane and run
//! **results** ride the content plane as CID'd artifacts — see
//! `NETWORKING_STATE_SYNC_TAXONOMY_DESIGN.md`.

use std::collections::BTreeMap;
use std::time::Duration;

use lunco_doc::DocumentId;
use lunco_doc_bevy::JournalResource;
use lunco_experiments::{
    Experiment, ExperimentId, ExperimentRegistry, ModelRef, ParamPath, ParamValue, RunBounds,
    RunStatus, TwinId,
};
use lunco_twin_journal::{AuthorTag, DomainKind, OpPayload};
use serde::{Deserialize, Serialize};

/// A journaled edit to an experiment *definition*. Serialized into the canonical
/// twin journal (`DomainKind::Experiment`); replayed on peers via
/// [`replay_experiment_op`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExperimentOp {
    /// Full snapshot of a newly-created experiment (carries the authored id so a
    /// peer reconstructs the same row — later `SetName`/`SetBounds` ops resolve).
    Create {
        id: ExperimentId,
        twin_id: TwinId,
        model_ref: ModelRef,
        name: String,
        overrides: BTreeMap<ParamPath, ParamValue>,
        inputs: BTreeMap<ParamPath, ParamValue>,
        bounds: RunBounds,
        color_hint: u8,
        /// Millis since the Unix epoch, so the peer's row sorts identically.
        created_at_ms: u64,
    },
    SetName {
        id: ExperimentId,
        name: String,
    },
    SetBounds {
        id: ExperimentId,
        bounds: RunBounds,
    },
    SetParams {
        id: ExperimentId,
        overrides: BTreeMap<ParamPath, ParamValue>,
        inputs: BTreeMap<ParamPath, ParamValue>,
    },
    Delete {
        id: ExperimentId,
    },
}

impl OpPayload for ExperimentOp {
    fn domain(&self) -> DomainKind {
        DomainKind::Experiment
    }
    // `referenced_entities` stays default-empty — same stance as the other
    // domains' `OpPayload` impls; conflict enrichment lands with multi-user.
}

impl ExperimentOp {
    fn target(&self) -> ExperimentId {
        match self {
            ExperimentOp::Create { id, .. }
            | ExperimentOp::SetName { id, .. }
            | ExperimentOp::SetBounds { id, .. }
            | ExperimentOp::SetParams { id, .. }
            | ExperimentOp::Delete { id } => *id,
        }
    }
}

/// A stable `DocumentId` for an experiment, folded from its UUID — experiments
/// aren't documents, but `record_op` keys entries by a `DocumentId`, so we derive
/// a deterministic one (same on every peer for the same experiment).
fn doc_id_for(id: ExperimentId) -> DocumentId {
    let b = id.0.as_u128();
    DocumentId::new((b ^ (b >> 64)) as u64)
}

/// Build a `Create` op capturing a fully-resolved experiment (post auto-name /
/// color allocation), so replay reproduces it exactly.
fn create_op(exp: &Experiment) -> ExperimentOp {
    let created_at_ms = exp
        .created_at
        .duration_since(web_time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    ExperimentOp::Create {
        id: exp.id,
        twin_id: exp.twin_id.clone(),
        model_ref: exp.model_ref.clone(),
        name: exp.name.clone(),
        overrides: exp.overrides.clone(),
        inputs: exp.inputs.clone(),
        bounds: exp.bounds.clone(),
        color_hint: exp.color_hint,
        created_at_ms,
    }
}

/// Record `(forward, inverse)` into the journal under the experiment's derived
/// doc id, attributed to the local user. No-op logging on serialize failure.
fn record(journal: &JournalResource, forward: &ExperimentOp, inverse: &ExperimentOp) {
    let doc = doc_id_for(forward.target());
    journal.with_write(|j| {
        if let Err(e) = j.record_op(AuthorTag::local_user(), doc, forward, inverse, None) {
            bevy::log::warn!("[experiment-journal] record_op failed: {e}");
        }
    });
}

/// Record a `Create` for an experiment that was just inserted locally (the
/// registry already holds it — `insert_new` minted id/name/color). The inverse
/// is `Delete`. Call at the create chokepoint (`dispatch_experiment`).
pub fn record_create(journal: &JournalResource, exp: &Experiment) {
    let forward = create_op(exp);
    let inverse = ExperimentOp::Delete { id: exp.id };
    record(journal, &forward, &inverse);
}

/// Record a `Delete` for an experiment that was **already** removed from the
/// registry (so the deletion path keeps its existing counter cleanup). Replay is
/// idempotent, so the self-referential inverse is harmless — undo-of-delete
/// (re-create from a snapshot) isn't a headline feature.
pub fn record_delete(journal: &JournalResource, id: ExperimentId) {
    let op = ExperimentOp::Delete { id };
    record(journal, &op, &op);
}

/// Apply a definition edit locally **and** record it (with a typed inverse read
/// from current state). The one funnel for rename / bounds / params / delete so
/// the edit journals + syncs by design. `journal` is `None` before the journal
/// is wired (pre-network / tests) — then it only applies.
pub fn apply_and_record(
    registry: &mut ExperimentRegistry,
    journal: Option<&JournalResource>,
    op: ExperimentOp,
) {
    let inverse = registry.get(op.target()).map(|e| match &op {
        ExperimentOp::SetName { id, .. } => ExperimentOp::SetName {
            id: *id,
            name: e.name.clone(),
        },
        ExperimentOp::SetBounds { id, .. } => ExperimentOp::SetBounds {
            id: *id,
            bounds: e.bounds.clone(),
        },
        ExperimentOp::SetParams { id, .. } => ExperimentOp::SetParams {
            id: *id,
            overrides: e.overrides.clone(),
            inputs: e.inputs.clone(),
        },
        // Inverse of a delete is re-creating the snapshot; inverse of a create
        // is a delete (create shouldn't reach here — use `record_create`).
        ExperimentOp::Delete { .. } => create_op(e),
        ExperimentOp::Create { .. } => ExperimentOp::Delete { id: op.target() },
    });
    apply_op(registry, &op);
    if let Some(journal) = journal {
        let inverse = inverse.unwrap_or_else(|| ExperimentOp::Delete { id: op.target() });
        record(journal, &op, &inverse);
    }
}

/// Apply an `ExperimentOp` to the registry **without** recording — the replay
/// entry point (op arrived via the journal, already logged). Returns `false`
/// (logged) if the payload isn't an `ExperimentOp`.
pub fn replay_experiment_op(registry: &mut ExperimentRegistry, op_json: &serde_json::Value) -> bool {
    match serde_json::from_value::<ExperimentOp>(op_json.clone()) {
        Ok(op) => {
            apply_op(registry, &op);
            true
        }
        Err(e) => {
            bevy::log::warn!("[experiment-journal] op payload is not an ExperimentOp: {e}");
            false
        }
    }
}

/// Apply an op to the registry through its public mutators. Shared by
/// [`apply_and_record`] (local edits) and [`replay_experiment_op`] (remote).
fn apply_op(registry: &mut ExperimentRegistry, op: &ExperimentOp) {
    match op {
        ExperimentOp::Create {
            id,
            twin_id,
            model_ref,
            name,
            overrides,
            inputs,
            bounds,
            color_hint,
            created_at_ms,
        } => {
            let created_at = web_time::SystemTime::UNIX_EPOCH + Duration::from_millis(*created_at_ms);
            registry.insert_with_id(Experiment {
                id: *id,
                twin_id: twin_id.clone(),
                model_ref: model_ref.clone(),
                name: name.clone(),
                overrides: overrides.clone(),
                inputs: inputs.clone(),
                bounds: bounds.clone(),
                status: RunStatus::Pending,
                result: None,
                created_at,
                color_hint: *color_hint,
            });
        }
        ExperimentOp::SetName { id, name } => {
            registry.set_name(*id, name.clone());
        }
        ExperimentOp::SetBounds { id, bounds } => {
            registry.set_bounds(*id, bounds.clone());
        }
        ExperimentOp::SetParams {
            id,
            overrides,
            inputs,
        } => {
            registry.set_params(*id, overrides.clone(), inputs.clone());
        }
        ExperimentOp::Delete { id } => {
            registry.delete(*id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_bounds() -> RunBounds {
        RunBounds {
            t_start: 0.0,
            t_end: 1.0,
            dt: None,
            n_intervals: None,
            tolerance: None,
            solver: None,
            h0: None,
            runtime: Default::default(),
        }
    }

    #[test]
    fn create_then_replay_reconstructs_with_same_id() {
        // Author a Create op on "peer A", replay its JSON on "peer B".
        let mut a = ExperimentRegistry::new();
        let id = a.insert_new(
            TwinId("t".into()),
            ModelRef("M".into()),
            Default::default(),
            Default::default(),
            empty_bounds(),
        );
        let op = create_op(a.get(id).unwrap());
        let json = serde_json::to_value(&op).unwrap();

        let mut b = ExperimentRegistry::new();
        assert!(replay_experiment_op(&mut b, &json));
        let e = b.get(id).expect("replayed with the SAME id");
        assert_eq!(e.model_ref, ModelRef("M".into()));

        // SetName replays onto the same row.
        let rename = serde_json::to_value(ExperimentOp::SetName {
            id,
            name: "renamed".into(),
        })
        .unwrap();
        assert!(replay_experiment_op(&mut b, &rename));
        assert_eq!(b.get(id).unwrap().name, "renamed");
    }

    #[test]
    fn bad_payload_is_rejected_softly() {
        let mut r = ExperimentRegistry::new();
        assert!(!replay_experiment_op(&mut r, &serde_json::json!({ "nope": 1 })));
    }
}
