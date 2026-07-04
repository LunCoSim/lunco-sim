//! API handlers for document-level operations.

use bevy::prelude::*;
use lunco_core::{Command, on_command};
use lunco_doc::DocumentId;
use crate::document::ModelicaOp;
use crate::state::ModelicaDocumentRegistry;
use super::util::resolve_doc;

/// Replace an open document's entire source text.
#[Command(default)]
pub struct SetDocumentSource {
    pub doc: DocumentId,
    pub source: String,
}

/// If a non-terminal experiment (Pending/Queued/Running) originates from
/// `doc`, return a human-readable reason describing why the source must not
/// be replaced right now. `None` when the doc has no live run.
///
/// The runner keeps its own compiled DAE on a worker thread, so a mid-run
/// `ReplaceSource` doesn't corrupt the in-flight solve — but it invalidates
/// the compile-once cache and swaps the model out from under the running
/// experiment, which silently strands the run (the exact "lost in-progress
/// simulation" the API crash report flagged). Refusing keeps the run
/// observable and the caller informed instead of clobbering it.
fn live_run_blocking_source_edit(world: &World, doc: DocumentId) -> Option<String> {
    let sources = world.get_resource::<crate::experiments_runner::ExperimentSources>()?;
    let registry = world.get_resource::<lunco_experiments::ExperimentRegistry>()?;
    for (exp_id, src_doc) in sources.0.iter() {
        if *src_doc != doc {
            continue;
        }
        if let Some(exp) = registry.get(*exp_id) {
            if !exp.status.is_terminal() {
                return Some(format!(
                    "experiment {exp_id:?} is still {:?} on this document — stop the run before replacing its source",
                    exp.status,
                ));
            }
        }
    }
    None
}

#[on_command(SetDocumentSource)]
pub fn on_set_document_source(
    trigger: On<SetDocumentSource>,
    active_id: Res<lunco_core::ActiveCommandId>,
    mut commands: Commands,
) {
    let doc_raw = trigger.event().doc;
    let source = trigger.event().source.clone();
    // Captured now (synchronously, while the dispatcher's ActiveCommandId is
    // still set) so the deferred closure can record a pollable outcome under
    // the caller's request id even though it runs after the trigger returns.
    let cmd_id = active_id.get();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, doc_raw) else {
            bevy::log::warn!("[SetDocumentSource] no doc for id {}", doc_raw);
            return;
        };
        // Simulation guard: never swap a document's source while one of its
        // experiments is live. Record a `Rejected` outcome (pollable via
        // QueryCommandResult) so the caller learns why instead of losing the
        // run to a silent clobber.
        if let Some(reason) = live_run_blocking_source_edit(world, doc) {
            bevy::log::warn!("[SetDocumentSource] doc={} rejected: {reason}", doc.raw());
            if let Some(id) = cmd_id {
                world.resource_mut::<lunco_core::CommandResults>().insert(
                    id,
                    lunco_core::CommandOutcome::Rejected(lunco_core::Reject::InvalidOp(reason)),
                );
            }
            return;
        }
        let unchanged = world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc))
            .map(|h| h.document().source() == source)
            .unwrap_or(false);
        if unchanged {
            return;
        }
        match crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::ReplaceSource { new: source },
            lunco_twin_journal::AuthorTag::for_tool("api"),
        ) {
            Ok(_) => bevy::log::info!("[SetDocumentSource] doc={} replaced", doc.raw()),
            Err(e) => bevy::log::warn!(
                "[SetDocumentSource] doc={} failed: {:?}",
                doc.raw(),
                e
            ),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiments_runner::ExperimentSources;
    use lunco_experiments::{ExperimentRegistry, ModelRef, RunBounds, RunStatus, TwinId};
    use std::collections::BTreeMap;

    /// Build a world holding one experiment (originating from `doc`) at
    /// `status`, then return whether the source-edit guard blocks it.
    fn guard_verdict(doc: DocumentId, run_doc: DocumentId, status: RunStatus) -> Option<String> {
        let mut world = World::new();
        let mut registry = ExperimentRegistry::new();
        let id = registry.insert_new(
            TwinId("t".into()),
            ModelRef("M".into()),
            BTreeMap::new(),
            BTreeMap::new(),
            RunBounds::default(),
        );
        registry.set_status(id, status);
        let mut sources = ExperimentSources::default();
        sources.0.insert(id, run_doc);
        world.insert_resource(registry);
        world.insert_resource(sources);
        live_run_blocking_source_edit(&world, doc)
    }

    #[test]
    fn blocks_source_edit_while_run_is_live() {
        let doc = DocumentId(2);
        assert!(guard_verdict(doc, doc, RunStatus::Running { t_current: 162.0 }).is_some());
        assert!(guard_verdict(doc, doc, RunStatus::Queued).is_some());
        assert!(guard_verdict(doc, doc, RunStatus::Pending).is_some());
    }

    #[test]
    fn allows_source_edit_when_run_is_terminal() {
        let doc = DocumentId(2);
        assert!(guard_verdict(doc, doc, RunStatus::Done { wall_time_ms: 5 }).is_none());
        assert!(guard_verdict(doc, doc, RunStatus::Cancelled).is_none());
        assert!(
            guard_verdict(
                doc,
                doc,
                RunStatus::Failed { error: "x".into(), partial: false }
            )
            .is_none()
        );
    }

    #[test]
    fn ignores_live_run_on_a_different_document() {
        // A live run on doc 3 must not block editing doc 2.
        assert!(
            guard_verdict(
                DocumentId(2),
                DocumentId(3),
                RunStatus::Running { t_current: 1.0 }
            )
            .is_none()
        );
    }

    #[test]
    fn allows_source_edit_when_no_runs_exist() {
        let mut world = World::new();
        world.insert_resource(ExperimentRegistry::new());
        world.insert_resource(ExperimentSources::default());
        assert!(live_run_blocking_source_edit(&world, DocumentId(2)).is_none());
    }
}
