//! Twin-scoped runtime-layer persistence (C5-A).
//!
//! A [`UsdDocument`](lunco_usd_document::document::UsdDocument) has a source
//! `base`, a user-authored `runtime` overlay, and a disposable `view` layer.
//! This module persists only the runtime layer — spawns, moved transforms, and
//! route edits — separately from the source `.usda`. The view layer is never
//! part of this sidecar. The edit journal records runtime ops but does not
//! replay them on open, so this snapshot restores runtime-layer state.
//!
//! This persists the runtime overlay asynchronously to its **own** file,
//! `<twin-root>/.lunco/runtime/<scene-path-relative-to-twin>`, parallel to the
//! journal (`journal_persistence.rs` in `lunco-workspace`), and can reload it
//! when the document opens — so runtime state survives across sessions without
//! ever touching the authored scene file. Persistence is one Twin-scoped opt-in
//! for both directions and is disabled unless the active Twin says otherwise.
//!
//! - **Load** on [`DocumentOpened`]: only when the active Twin's
//!   [`RUNTIME_PERSISTENCE_SETTING`] is `true`, read the overlay and
//!   [`restore_runtime`](lunco_usd_document::document::UsdDocument::restore_runtime) it into
//!   the freshly-built document.
//! - **Save** on [`DocumentChanged`]: the runtime layer is snapshotted on the
//!   document owner and serialized/written on the I/O pool. One writer per
//!   open document coalesces newer revisions behind an active write. Saving is
//!   controlled by the same Twin setting. A stale or corrupt `.lunco` file
//!   cannot affect the normal authored-scene load path.
//!
//! UI-free + headless; I/O goes through [`lunco_storage`]. No-ops for untitled /
//! non-twin docs (nowhere stable to persist) and when no `WorkspaceResource`
//! is present.

use lunco_usd_document::document::UsdDocument;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{IoTaskPool, Task, futures_lite::future};
use lunco_doc::DocumentId;
use lunco_doc_bevy::{DocumentChanged, DocumentClosed, DocumentOpened};
use lunco_storage::{Storage, StorageError, StorageHandle};
use lunco_usd_core::runtime::runtime_persistence_for_twin;
use lunco_workspace::WorkspaceResource;
use openusd::sdf::SpecType;

use lunco_doc_bevy::DocumentRegistry;

/// Twin-relative subfolder for durable runtime-layer snapshots. The typed
/// journal remains the edit history; this hidden sidecar restores user-authored
/// runtime-layer state when the Twin opens again.
///
/// The constant lives in `lunco-twin` alongside
/// [`is_runtime_state`](lunco_twin::is_runtime_state), the predicate that keeps
/// this directory out of scenario sync and out of release bundles. Writer and
/// excluders must not be able to drift apart.
use lunco_twin::RUNTIME_SUBDIR;

/// Resolve the Twin that owns a document path.
///
/// The most-specific root wins when a workspace contains nested roots. Path
/// ownership and setting lookup must use the same resolver or a child document
/// could write into one Twin while reading policy from another.
fn twin_for_path<'a>(
    workspace: &'a WorkspaceResource,
    path: &Path,
) -> Option<&'a lunco_twin::Twin> {
    workspace
        .twins()
        .filter(|(_, twin)| path.strip_prefix(&twin.root).is_ok())
        .max_by_key(|(_, twin)| twin.root.components().count())
        .map(|(_, twin)| twin)
}

/// Resolve the Twin's runtime-persistence policy for a document.
///
/// Omitted means disabled. A malformed value is an authoring error and is
/// returned to the caller so the owner can report it rather than silently
/// interpreting a typo as permission to write project state.
fn runtime_persistence_enabled(
    workspace: &WorkspaceResource,
    doc_path: &Path,
) -> Result<bool, String> {
    let Some(twin) = twin_for_path(workspace, doc_path) else {
        return Ok(false);
    };
    runtime_persistence_for_twin(twin)
}

/// `<twin-root>/.lunco/runtime/<scene-rel>` for a document whose file lives
/// inside an open twin; `None` for untitled docs or files outside every open
/// twin (nowhere stable to persist).
fn runtime_path(workspace: &WorkspaceResource, doc_path: &Path) -> Option<PathBuf> {
    let twin = twin_for_path(workspace, doc_path)?;
    let rel = doc_path.strip_prefix(&twin.root).ok()?;
    Some(twin.root.join(RUNTIME_SUBDIR).join(rel))
}

/// Resolve a document's runtime-overlay path from the workspace + the doc's
/// origin. `None` unless the doc is a USD doc with a twin-rooted file path.
fn doc_runtime_path(
    workspace: &WorkspaceResource,
    registry: &DocumentRegistry<UsdDocument>,
    doc: DocumentId,
) -> Option<PathBuf> {
    let path = registry.host(doc)?.document().origin().canonical_path()?;
    runtime_path(workspace, path)
}

/// Tolerant read: a missing / unreadable overlay means "start fresh", never an
/// error surfaced to the user.
fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    let handle = StorageHandle::File(path.to_path_buf());
    #[cfg(not(target_arch = "wasm32"))]
    let result = lunco_storage::FileStorage::new().read_sync(&handle);
    #[cfg(target_arch = "wasm32")]
    let result = lunco_storage::WebStorage::new().read_sync(&handle);
    result.ok()
}

/// Write `bytes` through the shared storage boundary. The backend owns parent
/// creation and atomic replacement on native and persistence on wasm.
#[cfg(test)]
fn write_bytes(path: &Path, bytes: &[u8]) -> lunco_storage::StorageResult<()> {
    lunco_storage::write_file_sync(path, bytes)
}

/// True when a runtime layer carries real content (any prim opinion), as
/// opposed to a bare/empty stage — used to skip persisting empty overlays.
fn runtime_has_content(runtime: &openusd::sdf::Data) -> bool {
    runtime.iter().any(|(_, spec)| spec.ty == SpecType::Prim)
}

async fn persist_runtime_overlay(path: PathBuf, runtime: openusd::sdf::Data) -> Result<(), String> {
    let handle = StorageHandle::File(path.clone());
    if !runtime_has_content(&runtime) {
        #[cfg(not(target_arch = "wasm32"))]
        let result = lunco_storage::FileStorage::new().delete(&handle).await;
        #[cfg(target_arch = "wasm32")]
        let result = lunco_storage::WebStorage::new().delete(&handle).await;
        return match result {
            Ok(()) | Err(StorageError::NotFound) => Ok(()),
            Err(error) => Err(format!("delete {} failed: {error}", path.display())),
        };
    }
    let text = lunco_usd_authoring::author::data_to_usda(&runtime)
        .map_err(|error| format!("serialize runtime layer failed: {error}"))?;
    #[cfg(not(target_arch = "wasm32"))]
    let result = lunco_storage::FileStorage::new()
        .write(&handle, text.as_bytes())
        .await;
    #[cfg(target_arch = "wasm32")]
    let result = lunco_storage::WebStorage::new()
        .write(&handle, text.as_bytes())
        .await;
    result.map_err(|error| format!("save to {} failed: {error}", path.display()))
}

/// Restore a document's persisted runtime edits from `.lunco/runtime/…`, if
/// one exists and the runtime layer is still empty. No-op for untitled /
/// non-twin docs or when no overlay exists.
///
/// Two callers share this: the Twin projection drain, which restores BEFORE the
/// scene's first mount so the single stage build composes `base ⊕ runtime`, and
/// the [`DocumentOpened`] observer (every other doc-open path — the observer
/// fires on a later command flush, too late for the twin mount). The
/// empty-runtime guard makes whichever runs second a no-op instead of a second
/// generation bump — whose synthetic `ReplaceSource` marker would force a
/// whole-scene rebuild (every prim despawned + respawned).
pub fn restore_doc_runtime(
    workspace: &WorkspaceResource,
    registry: &mut DocumentRegistry<UsdDocument>,
    doc: DocumentId,
) {
    let Some(doc_path) = registry
        .host(doc)
        .and_then(|host| host.document().origin().canonical_path())
    else {
        return;
    };
    match runtime_persistence_enabled(workspace, doc_path) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            warn!("[usd-runtime] {error}");
            return;
        }
    }
    let Some(path) = doc_runtime_path(workspace, registry, doc) else {
        return;
    };
    let restored = registry
        .host(doc)
        .map(|h| runtime_has_content(h.document().runtime_data()))
        .unwrap_or(true);
    if restored {
        return;
    }
    let Some(bytes) = read_bytes(&path) else {
        return;
    };
    let data = match String::from_utf8(bytes)
        .ok()
        .and_then(|text| lunco_usd_authoring::author::usda_to_data(&text).ok())
    {
        Some(data) => data,
        None => {
            warn!(
                "[usd-runtime] could not parse {} — ignoring",
                path.display()
            );
            return;
        }
    };
    if let Some(host) = registry.host_mut(doc) {
        host.document_mut().restore_runtime(data);
        info!(
            "[usd-runtime] restored runtime overlay from {}",
            path.display()
        );
    }
}

/// Restore the newest in-memory write snapshot for this sidecar when one is
/// still running; otherwise restore its last completed on-disk snapshot.
/// Called before initial stage projection and from every document-open path.
pub fn restore_doc_runtime_with_pending(
    workspace: &WorkspaceResource,
    registry: &mut DocumentRegistry<UsdDocument>,
    saves: &RuntimeSaveJobs,
    doc: DocumentId,
) {
    if !restore_pending_runtime(workspace, registry, saves, doc) {
        restore_doc_runtime(workspace, registry, doc);
    }
}

/// Restore the newest queued snapshot when a scene is reopened before its
/// previous document's asynchronous sidecar write has completed. The queued
/// layer is authoritative for that path until its writer drains, including an
/// empty layer whose pending delete must suppress a stale sidecar on disk.
fn restore_pending_runtime(
    workspace: &WorkspaceResource,
    registry: &mut DocumentRegistry<UsdDocument>,
    saves: &RuntimeSaveJobs,
    doc: DocumentId,
) -> bool {
    let Some(host) = registry.host(doc) else {
        return false;
    };
    let Some(doc_path) = host.document().origin().canonical_path() else {
        return false;
    };
    if !matches!(runtime_persistence_enabled(workspace, doc_path), Ok(true)) {
        return false;
    }
    let Some(path) = doc_runtime_path(workspace, registry, doc) else {
        return false;
    };
    let Some(job) = saves.0.get(&path) else {
        return false;
    };
    if let Some(host) = registry.host_mut(doc) {
        host.document_mut()
            .restore_runtime(job.latest.runtime.clone());
    }
    true
}

/// Load a freshly-opened USD document's persisted runtime overlay on
/// [`DocumentOpened`], so session state survives reload — see
/// [`restore_doc_runtime`] (a no-op when the twin drain already restored it).
fn on_doc_opened_load_runtime(
    trigger: On<DocumentOpened>,
    workspace: Option<Res<WorkspaceResource>>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut saved_revisions: ResMut<SavedRuntimeRevisions>,
    saves: Res<RuntimeSaveJobs>,
) {
    let doc = trigger.event().doc;
    if let Some(workspace) = workspace {
        restore_doc_runtime_with_pending(&workspace, &mut registry, &saves, doc);
    }
    if let Some(host) = registry.host(doc) {
        saved_revisions
            .0
            .insert(doc, host.document().runtime_revision());
    }
}

// TODO(#7 journal replay-on-open): today a reopened document's *current state* is
// reconstructed from the saved `.usda` base + this `.lunco/runtime` overlay, and
// the persisted twin journal (`<twin>/history/journal.json`) is a passive log —
// nothing local replays it. To make the journal an active reconstruct/undo
// source: on open, replay `merged_order(journal)` for this document via
// `DocumentRegistry::<UsdDocument>::replay_op` to rebuild runtime state (and the undo stack
// for cross-session undo), then demote `.lunco/runtime/*.usda` from a parallel
// truth to a snapshot cache-of-replay. Blocker: journal entries don't currently
// carry the owning `DocumentId` (EntityRef enrichment is deferred), so there's no
// entry→doc mapping to select which entries replay onto which document; and the
// primary-source switch risks replay-vs-saved divergence. Left as follow-up — the
// author-once op-replay projection (twin_projection) is the write-side prerequisite.

/// Snapshot a changed runtime layer and serialize/write it on the I/O pool.
/// One write runs per sidecar path; newer revisions replace its pending snapshot
/// so older writes cannot finish last and overwrite newer authored state.
fn on_doc_changed_schedule_runtime_save(
    trigger: On<DocumentChanged>,
    workspace: Option<Res<WorkspaceResource>>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    saved_revisions: ResMut<SavedRuntimeRevisions>,
    mut saves: ResMut<RuntimeSaveJobs>,
) {
    let doc = trigger.event().doc;
    let Some(workspace) = workspace else { return };
    let Some(host) = registry.host(doc) else {
        return;
    };
    let runtime_revision = host.document().runtime_revision();
    if saved_revisions.0.get(&doc) == Some(&runtime_revision) {
        return;
    }
    let Some(path) = doc_runtime_path(&workspace, &registry, doc) else {
        return;
    };
    if saves
        .0
        .get(&path)
        .is_some_and(|job| job.latest.doc == doc && job.latest.revision == runtime_revision)
    {
        return;
    }
    let Some(doc_path) = host.document().origin().canonical_path() else {
        return;
    };
    match runtime_persistence_enabled(&workspace, doc_path) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            warn!("[usd-runtime] {error}");
            return;
        }
    }
    let request = RuntimeSaveRequest {
        doc,
        revision: runtime_revision,
        path: path.clone(),
        runtime: host.document().runtime_data().clone(),
    };
    if let Some(job) = saves.0.get_mut(&path) {
        job.latest = request.clone();
        job.pending = Some(request);
    } else {
        saves.0.insert(path, RuntimeSaveJob::start(request));
    }
}

#[derive(Clone)]
struct RuntimeSaveRequest {
    doc: DocumentId,
    revision: u64,
    path: PathBuf,
    runtime: openusd::sdf::Data,
}

struct RuntimeSaveOutcome {
    doc: DocumentId,
    revision: u64,
    result: Result<(), String>,
}

struct RuntimeSaveJob {
    task: Option<Task<RuntimeSaveOutcome>>,
    pending: Option<RuntimeSaveRequest>,
    latest: RuntimeSaveRequest,
}

impl RuntimeSaveJob {
    fn start(request: RuntimeSaveRequest) -> Self {
        let latest = request.clone();
        let task = IoTaskPool::get().spawn(async move {
            let result = persist_runtime_overlay(request.path, request.runtime).await;
            RuntimeSaveOutcome {
                doc: request.doc,
                revision: request.revision,
                result,
            }
        });
        Self {
            task: Some(task),
            pending: None,
            latest,
        }
    }
}

#[derive(Resource, Default)]
struct SavedRuntimeRevisions(HashMap<DocumentId, u64>);

#[derive(Resource, Default)]
#[doc(hidden)]
pub struct RuntimeSaveJobs(HashMap<PathBuf, RuntimeSaveJob>);

#[derive(Resource, Default)]
struct DeferredRuntimeExit(Vec<bevy::app::AppExit>);

fn poll_runtime_save_jobs(
    mut saves: ResMut<RuntimeSaveJobs>,
    mut saved_revisions: ResMut<SavedRuntimeRevisions>,
    registry: Res<DocumentRegistry<UsdDocument>>,
) {
    let mut completed = Vec::new();
    for (path, job) in &mut saves.0 {
        let Some(task) = job.task.as_mut() else {
            continue;
        };
        if let Some(outcome) = future::block_on(future::poll_once(task)) {
            completed.push((path.clone(), outcome));
        }
    }

    let mut finished = Vec::new();
    for (path, outcome) in completed {
        let Some(job) = saves.0.get_mut(&path) else {
            continue;
        };
        job.task = None;
        match outcome.result {
            Ok(()) => {
                if registry.host(outcome.doc).is_some() {
                    saved_revisions.0.insert(outcome.doc, outcome.revision);
                }
            }
            Err(error) => warn!(
                "[usd-runtime] save of document {} revision {} failed: {error}",
                outcome.doc, outcome.revision
            ),
        }
        if let Some(request) = job.pending.take() {
            *job = RuntimeSaveJob::start(request);
        } else {
            finished.push(path);
        }
    }
    for path in finished {
        saves.0.remove(&path);
    }
}

fn forget_closed_runtime_revision(
    trigger: On<DocumentClosed>,
    mut saved_revisions: ResMut<SavedRuntimeRevisions>,
) {
    let doc = trigger.event().doc;
    saved_revisions.0.remove(&doc);
    // Queued snapshots own immutable layer data and a stable sidecar path, so
    // closing the document must not discard its newest unsaved revision.
}

/// Keep process shutdown behind the runtime-layer writer barrier. An Exit
/// request is replayed after all active/coalesced snapshots finish, so closing
/// immediately after a route edit cannot terminate its I/O task mid-write.
fn defer_exit_until_runtime_saves_finish(
    mut exits: ResMut<bevy::ecs::message::Messages<bevy::app::AppExit>>,
    saves: Res<RuntimeSaveJobs>,
    mut deferred: ResMut<DeferredRuntimeExit>,
) {
    if !saves.0.is_empty() {
        deferred.0.extend(exits.drain());
    } else if !deferred.0.is_empty() {
        exits.write_batch(std::mem::take(&mut deferred.0));
    }
}

/// Install Twin-scoped runtime overlay load/save observers.
pub struct UsdRuntimePersistencePlugin;

impl Plugin for UsdRuntimePersistencePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SavedRuntimeRevisions>();
        app.init_resource::<RuntimeSaveJobs>();
        app.init_resource::<DeferredRuntimeExit>();
        app.add_observer(on_doc_opened_load_runtime);
        app.add_observer(on_doc_changed_schedule_runtime_save);
        app.add_observer(forget_closed_runtime_revision);
        app.add_systems(Update, poll_runtime_save_jobs);
        app.add_systems(Last, defer_exit_until_runtime_saves_finish);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::{Document, DocumentOrigin};
    use lunco_storage::StorageEntryKind;
    use lunco_usd_authoring::author::usda_to_data;
    use lunco_usd_core::runtime::RUNTIME_PERSISTENCE_SETTING;
    use lunco_usd_document::document::{LayerId, UsdDocument, UsdOp};
    use openusd::sdf::Path as SdfPath;

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    /// Open a folder as a twin (mirrors `journal_persistence` tests).
    fn open_twin(p: &Path) -> lunco_twin::Twin {
        match lunco_twin::TwinMode::open(p).unwrap() {
            lunco_twin::TwinMode::Twin(t) | lunco_twin::TwinMode::Folder(t) => t,
            lunco_twin::TwinMode::Orphan(_) => panic!("expected a folder twin"),
        }
    }

    #[test]
    fn runtime_persistence_is_off_without_twin_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin(dir.path()));
        let scene = dir.path().join("scene.usda");
        assert_eq!(runtime_persistence_enabled(&ws, &scene), Ok(false));
    }

    fn open_twin_with_setting(p: &Path, value: lunco_twin::TwinSettingValue) -> lunco_twin::Twin {
        let mut manifest = lunco_twin::TwinManifest::new("runtime persistence test");
        manifest
            .set_setting(RUNTIME_PERSISTENCE_SETTING, value)
            .unwrap();
        manifest
            .write(&p.join(lunco_twin::MANIFEST_FILENAME))
            .unwrap();
        match lunco_twin::TwinMode::open(p).unwrap() {
            lunco_twin::TwinMode::Twin(twin) => twin,
            other => panic!("expected manifest-backed Twin, got {other:?}"),
        }
    }

    #[test]
    fn runtime_persistence_requires_a_boolean_twin_setting() {
        let dir = tempfile::tempdir().unwrap();
        let twin =
            open_twin_with_setting(dir.path(), lunco_twin::TwinSettingValue::Text("yes".into()));
        let mut ws = WorkspaceResource::new();
        ws.add_twin(twin);
        let error = runtime_persistence_enabled(&ws, &dir.path().join("scene.usda"))
            .expect_err("malformed setting must be visible");
        assert!(error.contains("must be a boolean"));
    }

    #[test]
    fn runtime_persistence_is_enabled_only_by_the_twin_setting() {
        let dir = tempfile::tempdir().unwrap();
        let twin = open_twin_with_setting(dir.path(), lunco_twin::TwinSettingValue::Bool(true));
        let mut ws = WorkspaceResource::new();
        ws.add_twin(twin);
        assert_eq!(
            runtime_persistence_enabled(&ws, &dir.path().join("scene.usda")),
            Ok(true)
        );
    }

    #[test]
    fn corrupt_runtime_overlay_is_ignored_when_loading_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin_with_setting(
            dir.path(),
            lunco_twin::TwinSettingValue::Bool(true),
        ));
        let scene_abs = dir.path().join("scene.usda");
        write_bytes(&scene_abs, TINY.as_bytes()).unwrap();
        let path = dir.path().join(".lunco/runtime/scene.usda");
        write_bytes(&path, b"not valid USDA").unwrap();

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file(scene_abs, TINY.to_string());
        restore_doc_runtime(&ws, &mut registry, doc);
        assert!(!runtime_has_content(
            registry.host(doc).unwrap().document().runtime_data()
        ));
    }

    #[test]
    fn runtime_overlay_is_ignored_without_twin_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin(dir.path()));
        let scene_abs = dir.path().join("scene.usda");
        write_bytes(&scene_abs, TINY.as_bytes()).unwrap();

        let mut source = UsdDocument::with_origin(
            DocumentId::new(11),
            TINY,
            DocumentOrigin::writable_file(scene_abs.clone()),
        );
        source
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: None,
                reference: Some("vessels/rovers/skid_rover.usda".into()),
                reference_prim_path: None,
            })
            .unwrap();
        let text = lunco_usd_authoring::author::data_to_usda(source.runtime_data()).unwrap();
        write_bytes(
            &dir.path().join(".lunco/runtime/scene.usda"),
            text.as_bytes(),
        )
        .unwrap();

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file(scene_abs, TINY.to_string());
        restore_doc_runtime(&ws, &mut registry, doc);
        assert!(!runtime_has_content(
            registry.host(doc).unwrap().document().runtime_data()
        ));
    }

    #[test]
    fn runtime_path_maps_scene_under_twin_dotlunco() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin(dir.path()));

        let scene = dir.path().join("scenes/luncosim/scene.usda");
        let rt = runtime_path(&ws, &scene).expect("scene inside twin resolves");
        assert_eq!(
            rt,
            dir.path().join(".lunco/runtime/scenes/luncosim/scene.usda")
        );

        // A path outside every twin has nowhere stable to persist.
        assert!(runtime_path(&ws, Path::new("/elsewhere/x.usda")).is_none());
    }

    #[test]
    fn runtime_overlay_round_trips_and_restores_into_a_fresh_doc() {
        let dir = tempfile::tempdir().unwrap();
        let rt_file = dir.path().join(".lunco/runtime/scene.usda");

        // 1. A document with a C4b spawn authored into its runtime layer.
        let mut src = UsdDocument::with_origin(
            DocumentId::new(1),
            TINY,
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        src.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "rover_1".into(),
            type_name: None,
            reference: Some("vessels/rovers/skid_rover.usda".into()),
            reference_prim_path: None,
        })
        .unwrap();
        assert!(runtime_has_content(src.runtime_data()));

        // 2. Persist the runtime layer through the production async storage
        // path used by document-change snapshots.
        future::block_on(persist_runtime_overlay(
            rt_file.clone(),
            src.runtime_data().clone(),
        ))
        .expect("runtime overlay persists");
        assert!(matches!(
            lunco_storage::entry_kind_file_sync(&rt_file),
            Ok(StorageEntryKind::File)
        ));

        // 3. A FRESH document (base only, empty runtime) — then restore.
        let mut reopened = UsdDocument::with_origin(
            DocumentId::new(2),
            TINY,
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        assert!(
            !runtime_has_content(reopened.runtime_data()),
            "fresh doc has empty runtime"
        );

        let bytes = read_bytes(&rt_file).expect("overlay present");
        let data =
            lunco_usd_authoring::author::usda_to_data(&String::from_utf8(bytes).unwrap()).unwrap();
        reopened.restore_runtime(data);

        // The spawn is back in the runtime layer + composed view, base still clean.
        let prim = SdfPath::new("/World/rover_1").unwrap();
        assert!(
            reopened.runtime_data().spec(&prim).is_some(),
            "runtime spawn restored"
        );
        assert!(
            reopened.data().spec(&prim).is_none(),
            "base untouched by restore"
        );
        assert!(
            !reopened.is_dirty(),
            "restoring a runtime overlay must not dirty the authored base"
        );
        assert!(
            reopened
                .composed_source()
                .contains("@vessels/rovers/skid_rover.usda@"),
            "restored spawn rides the composed view"
        );

        // Deleting the last runtime-authored prim removes the sidecar. An
        // empty but stale overlay must never resurrect a deleted waypoint or
        // other runtime edit on the next document open.
        let empty = UsdDocument::with_origin(
            DocumentId::new(3),
            TINY,
            DocumentOrigin::writable_file("/tmp/empty.usda"),
        );
        future::block_on(persist_runtime_overlay(
            rt_file.clone(),
            empty.runtime_data().clone(),
        ))
        .expect("empty runtime overlay removes the sidecar");
        assert!(read_bytes(&rt_file).is_none(), "empty sidecar is deleted");
    }

    #[test]
    fn latest_pending_snapshot_wins_over_stale_sidecars_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin_with_setting(
            dir.path(),
            lunco_twin::TwinSettingValue::Bool(true),
        ));
        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let mut saves = RuntimeSaveJobs::default();

        let latest_scene = dir.path().join("latest.usda");
        let latest_path = runtime_path(&ws, &latest_scene).unwrap();
        let mut stale = UsdDocument::with_origin(
            DocumentId::new(40),
            TINY,
            DocumentOrigin::writable_file(latest_scene.clone()),
        );
        stale
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "stale".into(),
                type_name: None,
                reference: None,
                reference_prim_path: None,
            })
            .unwrap();
        let stale_text = lunco_usd_authoring::author::data_to_usda(stale.runtime_data()).unwrap();
        write_bytes(&latest_path, stale_text.as_bytes()).unwrap();

        let mut latest = UsdDocument::with_origin(
            DocumentId::new(41),
            TINY,
            DocumentOrigin::writable_file(latest_scene.clone()),
        );
        latest
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "latest".into(),
                type_name: None,
                reference: None,
                reference_prim_path: None,
            })
            .unwrap();
        let latest_data = latest.runtime_data().clone();
        saves.0.insert(
            latest_path.clone(),
            RuntimeSaveJob {
                task: None,
                pending: None,
                latest: RuntimeSaveRequest {
                    doc: DocumentId::new(41),
                    revision: latest.runtime_revision(),
                    path: latest_path.clone(),
                    runtime: latest_data,
                },
            },
        );

        let (latest_doc, _) = registry.open_file(latest_scene, TINY.to_owned());
        restore_doc_runtime_with_pending(&ws, &mut registry, &saves, latest_doc);
        let latest_document = registry.host(latest_doc).unwrap().document();
        assert!(
            latest_document
                .runtime_data()
                .spec(&SdfPath::new("/World/latest").unwrap())
                .is_some()
        );
        assert!(
            latest_document
                .runtime_data()
                .spec(&SdfPath::new("/World/stale").unwrap())
                .is_none()
        );

        let empty_scene = dir.path().join("empty.usda");
        let empty_path = runtime_path(&ws, &empty_scene).unwrap();
        write_bytes(&empty_path, stale_text.as_bytes()).unwrap();
        let empty = UsdDocument::with_origin(
            DocumentId::new(42),
            TINY,
            DocumentOrigin::writable_file(empty_scene.clone()),
        );
        saves.0.insert(
            empty_path.clone(),
            RuntimeSaveJob {
                task: None,
                pending: None,
                latest: RuntimeSaveRequest {
                    doc: DocumentId::new(42),
                    revision: empty.runtime_revision(),
                    path: empty_path,
                    runtime: empty.runtime_data().clone(),
                },
            },
        );
        let (empty_doc, _) = registry.open_file(empty_scene, TINY.to_owned());
        restore_doc_runtime_with_pending(&ws, &mut registry, &saves, empty_doc);
        assert!(!runtime_has_content(
            registry.host(empty_doc).unwrap().document().runtime_data()
        ));
    }

    #[test]
    fn app_exit_waits_for_runtime_sidecar_jobs() {
        let mut app = App::new();
        app.add_message::<AppExit>();
        app.init_resource::<RuntimeSaveJobs>();
        app.init_resource::<DeferredRuntimeExit>();
        app.add_systems(Last, defer_exit_until_runtime_saves_finish);

        let path = PathBuf::from("/twin/.lunco/runtime/scene.usda");
        app.world_mut().resource_mut::<RuntimeSaveJobs>().0.insert(
            path.clone(),
            RuntimeSaveJob {
                task: None,
                pending: None,
                latest: RuntimeSaveRequest {
                    doc: DocumentId::new(43),
                    revision: 1,
                    path,
                    runtime: usda_to_data(TINY).unwrap(),
                },
            },
        );
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<AppExit>>()
            .write(AppExit::Success);

        app.update();
        assert!(
            app.world()
                .resource::<bevy::ecs::message::Messages<AppExit>>()
                .is_empty()
        );
        assert_eq!(
            app.world().resource::<DeferredRuntimeExit>().0,
            [AppExit::Success]
        );

        app.world_mut().resource_mut::<RuntimeSaveJobs>().0.clear();
        app.update();
        assert_eq!(
            app.world()
                .resource::<bevy::ecs::message::Messages<AppExit>>()
                .len(),
            1
        );
        assert!(app.world().resource::<DeferredRuntimeExit>().0.is_empty());
    }

    #[test]
    fn restore_doc_runtime_is_idempotent_across_drain_and_observer() {
        // The twin drain restores BEFORE the scene mounts; the `DocumentOpened`
        // observer fires a flush later and must NOT restore again — a second
        // restore bumps the generation with a coarse `ReplaceSource` marker,
        // which forces a whole-scene rebuild (the old "everything spawns twice
        // on twin open").
        let dir = tempfile::tempdir().unwrap();
        let mut ws = WorkspaceResource::new();
        ws.add_twin(open_twin_with_setting(
            dir.path(),
            lunco_twin::TwinSettingValue::Bool(true),
        ));

        let scene_abs = dir.path().join("scene.usda");
        write_bytes(&scene_abs, TINY.as_bytes()).unwrap();

        // Persist a runtime overlay with one spawn (same shape the app writes).
        let mut src = UsdDocument::with_origin(
            DocumentId::new(10),
            TINY,
            DocumentOrigin::writable_file(scene_abs.clone()),
        );
        src.apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "rover_1".into(),
            type_name: None,
            reference: Some("vessels/rovers/skid_rover.usda".into()),
            reference_prim_path: None,
        })
        .unwrap();
        let text = lunco_usd_authoring::author::data_to_usda(src.runtime_data()).unwrap();
        write_bytes(
            &dir.path().join(".lunco/runtime/scene.usda"),
            text.as_bytes(),
        )
        .unwrap();

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file(scene_abs, TINY.to_string());

        restore_doc_runtime(&ws, &mut registry, doc);
        let host = registry.host(doc).unwrap();
        assert!(
            runtime_has_content(host.document().runtime_data()),
            "first call restores the persisted spawn"
        );
        let gen_after_first = host.document().generation();

        restore_doc_runtime(&ws, &mut registry, doc);
        assert_eq!(
            registry.host(doc).unwrap().document().generation(),
            gen_after_first,
            "second call is a no-op — no generation bump, no forced rebuild"
        );
    }

    #[test]
    fn missing_overlay_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_bytes(&dir.path().join("nope.usda")).is_none());
    }

    #[test]
    fn empty_runtime_layer_is_not_persisted() {
        // A doc with no spawns/moves has nothing to persist.
        let doc = UsdDocument::with_origin(
            DocumentId::new(3),
            TINY,
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        assert!(!runtime_has_content(doc.runtime_data()));
    }
}
