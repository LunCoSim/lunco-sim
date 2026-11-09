//! API handlers for document-level operations.

use super::util::resolve_doc;
use crate::ModelicaDocuments;
use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command};
use lunco_doc::DocumentId;
use lunco_experiments::ExperimentOrigins;
use lunco_experiments::ExperimentRegistry;
use lunco_modelica_document::ModelicaOp;
use lunco_modelica_runner::PendingHandles;
use std::collections::HashSet;

#[cfg(not(target_arch = "wasm32"))]
fn twin_local_modelica_destination(
    twin_root: &std::path::Path,
    requested: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    use std::path::{Component, PathBuf};

    if requested.as_os_str().is_empty()
        || requested.extension().and_then(|ext| ext.to_str()) != Some("mo")
        || requested
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("SaveScratchModelicaDocument requires a safe Twin-local .mo path".into());
    }
    let root = std::fs::canonicalize(twin_root)
        .map_err(|error| format!("resolve active Twin root: {error}"))?;
    let destination = if requested.is_absolute() {
        PathBuf::from(requested)
    } else {
        root.join(requested)
    };
    let parent = destination
        .parent()
        .ok_or_else(|| "SaveScratchModelicaDocument destination has no parent".to_owned())?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|error| format!("resolve Modelica destination directory: {error}"))?;
    if !parent.starts_with(&root) {
        return Err(
            "SaveScratchModelicaDocument destination must stay inside the active Twin".into(),
        );
    }
    let file_name = destination
        .file_name()
        .ok_or_else(|| "SaveScratchModelicaDocument destination has no filename".to_owned())?;
    let destination = parent.join(file_name);
    if destination.exists() {
        let existing = std::fs::canonicalize(&destination)
            .map_err(|error| format!("resolve existing Modelica file: {error}"))?;
        if !existing.starts_with(&root) {
            return Err(
                "SaveScratchModelicaDocument cannot replace a file outside the active Twin".into(),
            );
        }
    }
    Ok(destination)
}

/// Identities of untitled documents allocated specifically for generated
/// API/tool work. Kept separate from generic untitled Editor documents so the
/// scratch-only close command cannot discard a user's unsaved model.
#[derive(Resource, Default)]
pub struct ToolScratchDocuments(HashSet<DocumentId>);

/// Create an untitled Modelica document without requiring the workbench UI.
/// UI hosts may additionally open a tab; headless/script hosts use the same
/// document registry and lifecycle path for generated or scratch models.
#[Command(default)]
pub struct CreateScratchModelicaDocument {
    pub source: String,
    pub name: String,
}

/// Persist an API-owned scratch document as a Modelica file inside the active
/// Twin. This keeps document persistence available to native HTML/Rhai tools
/// in headless hosts that do not install the Modelica editor UI plugin.
#[Command(default)]
pub struct SaveScratchModelicaDocument {
    pub doc_id: DocumentId,
    pub path: String,
}

#[on_command(SaveScratchModelicaDocument)]
pub fn on_save_scratch_modelica_document(
    trigger: On<SaveScratchModelicaDocument>,
    mut registry: ResMut<ModelicaDocuments>,
    mut scratch_documents: ResMut<ToolScratchDocuments>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let request = trigger.event();
    let doc_id = request.doc_id;
    if !scratch_documents.0.contains(&doc_id) {
        return Err("SaveScratchModelicaDocument only saves API-owned scratch documents".into());
    }
    let host = registry
        .host(doc_id)
        .ok_or_else(|| format!("SaveScratchModelicaDocument: unknown document {doc_id}"))?;
    if !host.document().origin().is_untitled() {
        return Err("SaveScratchModelicaDocument requires an untitled scratch document".into());
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = request;
        return Err("local Modelica file saving is unavailable on wasm".into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let raw_path = std::path::Path::new(&request.path);
        let twin_id = workspace
            .0
            .active_twin
            .ok_or_else(|| "SaveScratchModelicaDocument requires an active Twin".to_owned())?;
        let twin = workspace
            .0
            .twin(twin_id)
            .ok_or_else(|| "SaveScratchModelicaDocument active Twin is unavailable".to_owned())?;
        let destination = twin_local_modelica_destination(&twin.root, raw_path)?;
        let source = registry
            .host(doc_id)
            .ok_or_else(|| format!("SaveScratchModelicaDocument: unknown document {doc_id}"))?
            .document()
            .source()
            .to_string();
        let storage = lunco_storage::FileStorage::new();
        lunco_storage::Storage::write_sync(
            &storage,
            &lunco_storage::StorageHandle::File(destination.clone()),
            source.as_bytes(),
        )
        .map_err(|error| format!("save Modelica Twin file {}: {error}", destination.display()))?;
        let host = registry
            .host_mut(doc_id)
            .ok_or_else(|| format!("SaveScratchModelicaDocument: unknown document {doc_id}"))?;
        host.document_mut()
            .set_origin(lunco_doc::DocumentOrigin::File {
                path: destination.clone(),
                writable: true,
            });
        registry.mark_document_saved(doc_id);
        scratch_documents.0.remove(&doc_id);
        commands.trigger(lunco_doc_bevy::DocumentSaved::local(doc_id));
        Ok(Ack::with_data(
            OpId::new(),
            lunco_api_core::api_value!({
                "doc_id": doc_id.raw(),
                "path": destination.display().to_string()
            }),
        ))
    }
}

#[on_command(CreateScratchModelicaDocument)]
pub fn on_create_scratch_modelica_document(
    trigger: On<CreateScratchModelicaDocument>,
    mut registry: ResMut<ModelicaDocuments>,
    mut scratch_documents: ResMut<ToolScratchDocuments>,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) -> Result<Ack, String> {
    let request = trigger.event();
    if request.source.trim().is_empty() {
        return Err("CreateScratchModelicaDocument requires non-empty Modelica source".into());
    }
    let name = request.name.trim();
    if name.is_empty() {
        return Err("CreateScratchModelicaDocument requires a display name".into());
    }
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let doc_id = lunco_modelica_core::doc_ops::allocate_scratch_document(
        &mut registry,
        workspace.as_deref_mut().map(|workspace| &mut workspace.0),
        replication.as_ref(),
        request.source.clone(),
        name.to_owned(),
    );
    scratch_documents.0.insert(doc_id);
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "doc_id": doc_id.raw() }),
    ))
}

/// Cancel owned experiments and close an untitled API scratch document through
/// the shared document lifecycle, including its linked execution entities.
/// Completed results remain under the experiment history owner. Saved documents
/// continue through the ordinary document close flow.
#[Command(default)]
pub struct CloseScratchModelicaDocument {
    pub doc_id: DocumentId,
}

#[on_command(CloseScratchModelicaDocument)]
pub fn on_close_scratch_modelica_document(
    trigger: On<CloseScratchModelicaDocument>,
    registry: Res<ModelicaDocuments>,
    scratch_documents: Res<ToolScratchDocuments>,
    mut experiments: Option<ResMut<ExperimentRegistry>>,
    sources: Option<Res<ExperimentOrigins>>,
    mut pending: Option<ResMut<PendingHandles>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let doc = trigger.event().doc_id;
    if !scratch_documents.0.contains(&doc) {
        return Err(
            "CloseScratchModelicaDocument only closes API-owned generated scratch documents".into(),
        );
    }
    let host = registry
        .host(doc)
        .ok_or_else(|| format!("CloseScratchModelicaDocument: unknown document {doc}"))?;
    if !host.document().origin().is_untitled() {
        return Err("CloseScratchModelicaDocument only closes untitled scratch documents".into());
    }

    let run_ids: Vec<_> = sources
        .as_ref()
        .map(|sources| {
            sources
                .iter()
                .filter_map(|(id, origin)| {
                    origin
                        .local_document()
                        .filter(|source| source.document == doc)
                        .map(|_| *id)
                })
                .collect()
        })
        .unwrap_or_default();
    let live_run_ids: Vec<_> = experiments
        .as_ref()
        .map(|experiments| {
            run_ids
                .iter()
                .copied()
                .filter(|id| {
                    experiments
                        .get(*id)
                        .is_some_and(|experiment| !experiment.status.is_terminal())
                })
                .collect()
        })
        .unwrap_or_default();
    if !live_run_ids.is_empty() && pending.is_none() {
        return Err(
            "CloseScratchModelicaDocument cannot stop live runs without the Modelica runner".into(),
        );
    }
    if let Some(pending) = pending.as_mut() {
        for handle in &pending.0 {
            if live_run_ids.contains(&handle.run_id) {
                handle.cancel();
            }
        }
    }
    if let Some(experiments) = experiments.as_mut() {
        for id in &live_run_ids {
            experiments.set_status(*id, lunco_experiments::RunStatus::Cancelled);
        }
    }

    commands.trigger(lunco_doc_bevy::CloseDocument { doc_id: doc });
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "doc_id": doc.raw() }),
    ))
}

pub fn forget_tool_scratch_document(
    trigger: On<lunco_doc_bevy::DocumentClosed>,
    mut scratch_documents: ResMut<ToolScratchDocuments>,
) {
    scratch_documents.0.remove(&trigger.event().doc);
}

/// Replace an open document's entire source text.
#[Command(default)]
pub struct SetDocumentSource {
    pub doc_id: DocumentId,
    pub source: String,
}

/// Experiment ids with a live (non-terminal: Pending/Queued/Running) run
/// originating from `doc`. Empty when the doc has no in-flight run.
fn live_runs_for_doc(world: &World, doc: DocumentId) -> Vec<lunco_experiments::ExperimentId> {
    let (Some(sources), Some(registry)) = (
        world.get_resource::<lunco_experiments::ExperimentOrigins>(),
        world.get_resource::<lunco_experiments::ExperimentRegistry>(),
    ) else {
        return Vec::new();
    };
    sources
        .iter()
        .filter(|(id, origin)| {
            origin
                .local_document()
                .is_some_and(|source| source.document == doc)
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
    if let Some(handles) = world.get_resource::<lunco_modelica_runner::PendingHandles>() {
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
    let doc_raw = trigger.event().doc_id;
    let source = trigger.event().source.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, doc_raw) else {
            bevy::log::warn!("[SetDocumentSource] no doc for id {}", doc_raw);
            return;
        };
        let unchanged = world
            .get_resource::<ModelicaDocuments>()
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
        match lunco_modelica_core::doc_ops::apply_one_op_as(
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
    use lunco_experiments::ExperimentOrigins;
    use lunco_experiments::{ExperimentRegistry, ModelRef, RunBounds, RunStatus, TwinId};
    use std::collections::BTreeMap;

    #[cfg(not(target_arch = "wasm32"))]
    fn scratch_test_root() -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "luncosim-modelica-save-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("models/recipes")).expect("create test Twin");
        root
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn scratch_save_destination_accepts_a_twin_local_modelica_file() {
        let root = scratch_test_root();
        let requested = std::path::Path::new("models/recipes/Blend.mo");
        let destination = twin_local_modelica_destination(&root, requested).expect("destination");
        let parent = root
            .join("models/recipes")
            .canonicalize()
            .expect("canonical parent");
        assert_eq!(destination, parent.join("Blend.mo"));
        std::fs::remove_dir_all(root).expect("remove test Twin");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn scratch_save_destination_rejects_traversal_and_non_modelica_files() {
        let root = scratch_test_root();
        assert!(
            twin_local_modelica_destination(&root, std::path::Path::new("../outside.mo")).is_err()
        );
        assert!(
            twin_local_modelica_destination(&root, std::path::Path::new("models/recipe.txt"))
                .is_err()
        );
        std::fs::remove_dir_all(root).expect("remove test Twin");
    }

    #[cfg(all(not(target_arch = "wasm32"), unix))]
    #[test]
    fn scratch_save_destination_rejects_symlink_escape() {
        let root = scratch_test_root();
        let outside = root
            .parent()
            .expect("temp parent")
            .join(format!("luncosim-modelica-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).expect("create outside directory");
        std::os::unix::fs::symlink(&outside, root.join("models/recipes/escape"))
            .expect("create Twin symlink");
        assert!(
            twin_local_modelica_destination(
                &root,
                std::path::Path::new("models/recipes/escape/Blend.mo")
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).expect("remove test Twin");
        std::fs::remove_dir_all(outside).expect("remove outside directory");
    }

    /// Build a world holding one experiment (originating from `run_doc`) at
    /// `status`, then list the live runs the source-swap would cancel for `doc`.
    fn live_runs(
        doc: DocumentId,
        run_doc: DocumentId,
        status: RunStatus,
    ) -> Vec<lunco_experiments::ExperimentId> {
        let mut world = World::new();
        let mut registry = ExperimentRegistry::new();
        let mut sources = ExperimentOrigins::default();
        let id = sources.insert_new(
            &mut registry,
            lunco_experiments::ExperimentOrigin::LocalDocument(
                lunco_workspace::PinnedDocumentRuntimeOwner {
                    document: run_doc,
                    runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                },
            ),
            TwinId("t".into()),
            ModelRef("M".into()),
            BTreeMap::new(),
            BTreeMap::new(),
            RunBounds::default(),
        );
        registry.set_status(id, status);
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
        assert!(live_runs(doc, doc, RunStatus::Failed { error: "x".into() }).is_empty());
    }

    #[test]
    fn ignores_live_run_on_a_different_document() {
        // A live run on doc 3 must not be cancelled when editing doc 2.
        assert!(
            live_runs(
                DocumentId(2),
                DocumentId(3),
                RunStatus::Running { t_current: 1.0 }
            )
            .is_empty()
        );
    }

    #[test]
    fn empty_when_no_runs_exist() {
        let mut world = World::new();
        world.insert_resource(ExperimentRegistry::new());
        world.insert_resource(ExperimentOrigins::default());
        assert!(live_runs_for_doc(&world, DocumentId(2)).is_empty());
    }

    /// End-to-end of the cancel path: a live run on the doc is signalled to
    /// cancel and flipped to terminal `Cancelled`; a run on another doc is
    /// left untouched.
    #[test]
    fn stop_live_runs_cancels_only_target_doc() {
        use lunco_modelica_runner::PendingHandles;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let doc = DocumentId(2);
        let other = DocumentId(3);
        let mut world = World::new();
        let mut registry = ExperimentRegistry::new();
        let mut sources = ExperimentOrigins::default();
        let mut mint = |reg: &mut ExperimentRegistry, document| {
            sources.insert_new(
                reg,
                lunco_experiments::ExperimentOrigin::LocalDocument(
                    lunco_workspace::PinnedDocumentRuntimeOwner {
                        document,
                        runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                    },
                ),
                TwinId("t".into()),
                ModelRef("M".into()),
                BTreeMap::new(),
                BTreeMap::new(),
                RunBounds::default(),
            )
        };
        let target_id = mint(&mut registry, doc);
        let other_id = mint(&mut registry, other);
        registry.set_status(target_id, RunStatus::Running { t_current: 5.0 });
        registry.set_status(other_id, RunStatus::Running { t_current: 5.0 });

        // Handles whose cancel hook bumps a shared counter so we can assert
        // exactly which runs were signalled.
        let hits = Arc::new(AtomicUsize::new(0));
        let mk_handle = |id, hits: Arc<AtomicUsize>| {
            let (_tx, rx) = crossbeam_channel::unbounded();
            lunco_modelica_runner::PendingRun {
                artifact_admission: None,
                result_limits: Default::default(),
                origin: sources.get(&id).expect("source").clone(),
                handle: lunco_experiments::RunHandle {
                    run_id: id,
                    progress_rx: rx,
                    cancel: Box::new(move || {
                        hits.fetch_add(1, Ordering::SeqCst);
                    }),
                },
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
