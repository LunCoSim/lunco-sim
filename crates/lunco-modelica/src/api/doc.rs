//! API handlers for document-level operations.

use super::util::resolve_doc;
use crate::document::ModelicaOp;
use crate::state::ModelicaDocumentRegistry;
use bevy::prelude::*;
use lunco_core::{on_command, Command};
use lunco_doc::DocumentId;

/// Replace an open document's entire source text.
#[Command(default)]
pub struct SetDocumentSource {
    pub doc: DocumentId,
    pub source: String,
}

/// Experiment ids with a live (non-terminal: Pending/Queued/Running) run
/// originating from `doc`. Empty when the doc has no in-flight run.
fn live_runs_for_doc(world: &World, doc: DocumentId) -> Vec<lunco_experiments::ExperimentId> {
    let (Some(sources), Some(registry)) = (
        world.get_resource::<crate::experiments_runner::ExperimentSources>(),
        world.get_resource::<lunco_experiments::ExperimentRegistry>(),
    ) else {
        return Vec::new();
    };
    sources
        .0
        .iter()
        .filter(|(id, src_doc)| {
            **src_doc == doc
                && registry
                    .get(**id)
                    .map(|e| !e.status.is_terminal())
                    .unwrap_or(false)
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Stop every in-flight run for `doc` before its source is swapped. Signals
/// the cooperative cancel flag on each matching [`RunHandle`] (honored at the
/// next solver-step boundary, ≤100 ms) and flips the registry status to
/// `Cancelled` immediately so the run is terminal from the caller's view.
/// Returns the number of runs stopped.
///
/// Why cancel instead of reject: the runner keeps its own compiled DAE on a
/// worker thread, so a mid-run `ReplaceSource` can't corrupt the in-flight
/// solve — but leaving the run alive against a model that no longer exists
/// strands it (the "lost in-progress simulation" the crash report flagged)
/// and invalidates the compile-once cache underneath it. Cleanly retiring the
/// old run, then applying the new source, is the coherent behavior — the
/// caller starts a fresh run against the new model.
fn stop_live_runs_for_doc(world: &mut World, doc: DocumentId) -> usize {
    let ids = live_runs_for_doc(world, doc);
    if ids.is_empty() {
        return 0;
    }
    if let Some(handles) = world.get_resource::<crate::experiments_runner::PendingHandles>() {
        for h in handles.0.iter() {
            if ids.contains(&h.run_id) {
                h.cancel();
            }
        }
    }
    if let Some(mut registry) = world.get_resource_mut::<lunco_experiments::ExperimentRegistry>() {
        for id in &ids {
            registry.set_status(*id, lunco_experiments::RunStatus::Cancelled);
        }
    }
    ids.len()
}

#[on_command(SetDocumentSource)]
pub fn on_set_document_source(trigger: On<SetDocumentSource>, mut commands: Commands) {
    let doc_raw = trigger.event().doc;
    let source = trigger.event().source.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, doc_raw) else {
            bevy::log::warn!("[SetDocumentSource] no doc for id {}", doc_raw);
            return;
        };
        let unchanged = world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc))
            .map(|h| h.document().source() == source)
            .unwrap_or(false);
        if unchanged {
            return;
        }
        // Retire any run still executing against the old source before we
        // swap it out — the runner is decoupled (own DAE on a worker thread),
        // so this is a clean cancel, not a mid-solve yank. The caller starts a
        // fresh run against the new model.
        let stopped = stop_live_runs_for_doc(world, doc);
        if stopped > 0 {
            bevy::log::info!(
                "[SetDocumentSource] doc={} cancelled {stopped} live run(s) before replacing source",
                doc.raw(),
            );
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

    /// Build a world holding one experiment (originating from `run_doc`) at
    /// `status`, then list the live runs the source-swap would cancel for `doc`.
    fn live_runs(
        doc: DocumentId,
        run_doc: DocumentId,
        status: RunStatus,
    ) -> Vec<lunco_experiments::ExperimentId> {
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
        live_runs_for_doc(&world, doc)
    }

    #[test]
    fn selects_live_runs_on_the_target_doc() {
        let doc = DocumentId(2);
        assert_eq!(
            live_runs(doc, doc, RunStatus::Running { t_current: 162.0 }).len(),
            1
        );
        assert_eq!(live_runs(doc, doc, RunStatus::Queued).len(), 1);
        assert_eq!(live_runs(doc, doc, RunStatus::Pending).len(), 1);
    }

    #[test]
    fn ignores_terminal_runs() {
        let doc = DocumentId(2);
        assert!(live_runs(doc, doc, RunStatus::Done { wall_time_ms: 5 }).is_empty());
        assert!(live_runs(doc, doc, RunStatus::Cancelled).is_empty());
        assert!(live_runs(
            doc,
            doc,
            RunStatus::Failed {
                error: "x".into(),
                partial: false
            }
        )
        .is_empty());
    }

    #[test]
    fn ignores_live_run_on_a_different_document() {
        // A live run on doc 3 must not be cancelled when editing doc 2.
        assert!(live_runs(
            DocumentId(2),
            DocumentId(3),
            RunStatus::Running { t_current: 1.0 }
        )
        .is_empty());
    }

    #[test]
    fn empty_when_no_runs_exist() {
        let mut world = World::new();
        world.insert_resource(ExperimentRegistry::new());
        world.insert_resource(ExperimentSources::default());
        assert!(live_runs_for_doc(&world, DocumentId(2)).is_empty());
    }

    /// End-to-end of the cancel path: a live run on the doc is signalled to
    /// cancel and flipped to terminal `Cancelled`; a run on another doc is
    /// left untouched.
    #[test]
    fn stop_live_runs_cancels_only_target_doc() {
        use crate::experiments_runner::PendingHandles;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let doc = DocumentId(2);
        let other = DocumentId(3);
        let mut world = World::new();
        let mut registry = ExperimentRegistry::new();
        let mint = |reg: &mut ExperimentRegistry| {
            reg.insert_new(
                TwinId("t".into()),
                ModelRef("M".into()),
                BTreeMap::new(),
                BTreeMap::new(),
                RunBounds::default(),
            )
        };
        let target_id = mint(&mut registry);
        let other_id = mint(&mut registry);
        registry.set_status(target_id, RunStatus::Running { t_current: 5.0 });
        registry.set_status(other_id, RunStatus::Running { t_current: 5.0 });

        let mut sources = ExperimentSources::default();
        sources.0.insert(target_id, doc);
        sources.0.insert(other_id, other);

        // Handles whose cancel hook bumps a shared counter so we can assert
        // exactly which runs were signalled.
        let hits = Arc::new(AtomicUsize::new(0));
        let mk_handle = |id, hits: Arc<AtomicUsize>| {
            let (_tx, rx) = crossbeam_channel::unbounded();
            lunco_experiments::RunHandle {
                run_id: id,
                progress_rx: rx,
                cancel: Box::new(move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                }),
            }
        };
        let handles = PendingHandles(vec![
            mk_handle(target_id, hits.clone()),
            mk_handle(other_id, hits.clone()),
        ]);

        world.insert_resource(registry);
        world.insert_resource(sources);
        world.insert_resource(handles);

        let stopped = stop_live_runs_for_doc(&mut world, doc);
        assert_eq!(stopped, 1);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "only the target doc's run is cancelled"
        );

        let reg = world.resource::<ExperimentRegistry>();
        assert_eq!(reg.get(target_id).unwrap().status, RunStatus::Cancelled);
        assert!(matches!(
            reg.get(other_id).unwrap().status,
            RunStatus::Running { .. }
        ));
    }
}
