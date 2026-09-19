//! `UsdCommandsPlugin` — typed-command surface for USD documents and authoring.
//!
//! Plumbs USD into the shared workbench command bus described in
//! `AGENTS.md` §4.2:
//!
//! - **Open**: observes [`OpenFile`]
//!   and handles paths with a USD extension. Modelica observes the same
//!   command for `.mo`; future SysML / mission crates will join the
//!   chorus. Each observer is responsible for its own extension gate so
//!   an `OpenFile { path: "/foo.mo" }` doesn't end up parsed as USD.
//! - **New**: observes [`NewDocument`]
//!   gated on `kind == "usd"`. Lets File→New surface "USD Stage" once
//!   the kind is registered.
//! - **Save**: observes
//!   [`SaveDocument`] gated on
//!   [`DocumentRegistry::<UsdDocument>::contains`].
//! - **Notifications**: each frame drains the registry's pending rings
//!   into [`DocumentOpened`],
//!   [`lunco_doc_bevy::DocumentChanged`], and
//!   [`DocumentClosed`] so views
//!   subscribe through the canonical channels rather than polling the
//!   registry directly.
//!
//! Registers the `usd` document kind in
//! [`DocumentKindRegistry`] on build
//! so File menus, picker dialogs, and `twin.toml` parsers see USD
//! without any central edit.

use lunco_usd_document::document::UsdDocument;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use lunco_api::executor::{finish_command_result, DeferredCommandAppExt, PendingApiRequest};
use lunco_api_core::ApiErrorCode;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{on_command, register_commands, ActiveCommandId, Command, CommandResults};
use lunco_doc::OpenOutcome;
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_doc_bevy::DocumentRegistry;
use lunco_doc_bevy::{
    CloseDocument, DiscardDocument, DocumentChanged, DocumentClosed, DocumentOpened, ForkDocument,
    NewDocument, OpenFile, RedoDocument, SaveAsDocument, SaveDocument, UndoDocument,
};
use lunco_storage::Storage; // brings `write_sync` / `read_sync` into scope
use lunco_twin::{DocumentKindId, DocumentKindMeta, DocumentKindRegistry};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_bevy_stage::{UsdRead, UsdStageAsset};
use lunco_usd_core::commands::{
    is_usd_path, ApplyUsdOp, ApplyUsdOps, ApplyUsdTransientOps, AttachComponent, AttachProgram,
    CommitUsdProposal, CreateUsdProposal, DetachComponent, ReviewUsdProposal, UsdDocumentReady,
    UsdProposalReviewAction, USD_DOCUMENT_KIND,
};
use lunco_usd_core::edit_session::{
    validate_proposal, UsdEditSessions, UsdProposalId, UsdProposalState,
};
use lunco_usd_data::usd_data::UsdDataExt;
use lunco_usd_document::document::{LayerId, UsdOp};
use lunco_workspace::{TwinClosed, WorkspaceResource};
use openusd::schemas::lux::tokens as ltok;

/// Plugin that registers the USD document kind, the typed-command
/// observers, and the pending-event drain system.
///
/// **Layer 2 (domain).** No UI, scene admission, or Bevy renderer touches —
/// added by the application-level USD runtime bundle so any binary that pulls
/// in USD gets the document surface, even headless bins.
pub struct UsdCommandsPlugin;

/// Promote an authored document when the live twin projection is installed.
///
/// The grouped document-editing path must also work for callers that only
/// install `DocumentRegistry`; a missing live Twin projection is not an error
/// for a headless document edit.
fn claim_user_document_if_projected(world: &mut World, doc: DocumentId) {
    let claimed = world
        .get_resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
        .is_some_and(|mut backed| backed.claim_user(doc));
    if claimed {
        world.trigger(lunco_usd_bevy_twin::UsdDocumentUserOwned { doc });
    }
}

/// Session restore is the one document-open path that does not carry an
/// explicit user-open command. A restored document is therefore promoted to a
/// user lease here, while an automatically opened Twin scene is already linked
/// to a scene lease and remains internal until the user acts on it.
fn claim_user_document_on_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    if !registry.contains(doc) || backed.coords_of(doc).is_some() {
        return;
    }
    if backed.claim_user(doc) {
        commands.trigger(lunco_usd_bevy_twin::UsdDocumentUserOwned { doc });
    }
}

/// Keep the shared Workspace document list in step with the USD registry.
///
/// This belongs to the headless-safe command/lifecycle plugin rather than the
/// editor UI: startup Twin scenes are opened by this plugin even when the
/// simulator runs without `UsdUiPlugin`, and API/Rhai document discovery must
/// see the same USD documents as the desktop shell.
fn sync_workspace_on_doc_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    if workspace.document(doc).is_some() {
        workspace.active_document = Some(doc);
        return;
    }
    let origin = host.document().origin().clone();
    let context_twin = origin
        .is_untitled()
        .then_some(workspace.active_twin)
        .flatten();
    workspace.add_document(lunco_workspace::DocumentEntry {
        id: doc,
        kind: DocumentKindId::new(USD_DOCUMENT_KIND),
        title: origin.display_name(),
        origin,
        context_twin,
        dirty: host.document().is_dirty(),
    });
    workspace.active_document = Some(doc);
}

/// Reflect USD edits into the shared Workspace dirty mirror.
fn sync_workspace_on_doc_changed(
    trigger: On<DocumentChanged>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    if let Some(entry) = workspace.document_mut(doc) {
        entry.dirty = host.document().is_dirty();
    }
}

/// Reflect USD Save and Save-As origin changes into the shared Workspace.
fn sync_workspace_on_doc_saved(
    trigger: On<lunco_doc_bevy::DocumentSaved>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    let origin = host.document().origin().clone();
    if let Some(path) = origin.canonical_path() {
        workspace.recents.push_loose(path.to_path_buf());
    }
    if let Some(entry) = workspace.document_mut(doc) {
        entry.title = origin.display_name();
        entry.origin = origin;
        entry.dirty = host.document().is_dirty();
    }
}

/// Remove the shared Workspace entry when a USD registry document closes.
fn sync_workspace_on_doc_closed(
    trigger: On<DocumentClosed>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    workspace.close_document(trigger.event().doc);
}

/// Proposal plans are session state, so their lifetime ends with the document
/// that owns their explicit target.  The authored document and its canonical
/// journal remain responsible for all persisted history.
fn clear_usd_edit_session_on_document_closed(
    trigger: On<DocumentClosed>,
    mut sessions: ResMut<UsdEditSessions>,
) {
    let removed = sessions.remove_document(trigger.event().doc);
    if removed != 0 {
        bevy::log::debug!(
            "[usd-editor] removed {removed} review proposal(s) for closed document {}",
            trigger.event().doc
        );
    }
}

/// Registry closure is the final lifetime edge for projection bookkeeping.
/// Remove both scene coordinates and user claims so a closed document cannot
/// be rediscovered by a later stage-path lookup.
fn forget_backed_document_on_closed(
    trigger: On<DocumentClosed>,
    mut backed: ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    twins: Res<lunco_assets_core::twin_source::TwinRoots>,
) {
    if let Some((name, _rel)) = backed.forget_document(trigger.event().doc) {
        if let Err(error) = twins.unregister_name(&name) {
            warn!("[usd] could not unregister closed preview Twin `{name}`: {error}");
        }
    }
}

impl Plugin for UsdCommandsPlugin {
    fn build(&self, app: &mut App) {
        // Twin authority registration belongs to the asset boundary and is
        // shared by lunica and luncosim. Install it here only for minimal USD
        // hosts/tests that do not compose the normal asset-source root.
        if !app.is_plugin_added::<lunco_assets_runtime::TwinRootsPlugin>() {
            app.add_plugins(lunco_assets_runtime::TwinRootsPlugin);
        }
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdEditSessions>();
        // Self-register with the workbench's plugin-driven document
        // kind registry. `init_resource` defends against the case where
        // the workbench plugin hasn't been added yet — we still own
        // our entry, the workbench picks it up when it boots.
        app.init_resource::<DocumentKindRegistry>();
        app.world_mut()
            .resource_mut::<DocumentKindRegistry>()
            .register(
                DocumentKindId::new(USD_DOCUMENT_KIND),
                DocumentKindMeta {
                    display_name: "USD Stage".into(),
                    extensions: vec!["usda", "usdc", "usd"],
                    can_create_new: true,
                    default_filename: Some("NewStage.usda"),
                    uri_scheme: Some("usd"),
                    manifest_section: Some("usd"),
                },
            );

        // Document *open/load* pipeline (domain-layer, so it works in
        // headless / sandbox bins that don't add `UsdUiPlugin`). Reads
        // run on the `AsyncComputeTaskPool` through `lunco-storage` and
        // land in the registry via `drain_pending_usd_file_loads`. The
        // UI's `browser_dispatch` only translates browser-panel clicks
        // into calls on this pipeline.
        app.init_resource::<PendingUsdLoads>();
        app.init_resource::<PendingUsdDiscards>();
        app.add_observer(cancel_pending_usd_loads_on_twin_closed);
        app.add_systems(
            Update,
            (drain_pending_usd_file_loads, drain_pending_usd_discards),
        );
        app.register_deferred_command::<ApplyUsdOp>()
            .register_deferred_command::<ApplyUsdOps>()
            .register_deferred_command::<ApplyUsdTransientOps>()
            .register_deferred_command::<CreateUsdProposal>()
            .register_deferred_command::<ReviewUsdProposal>()
            .register_deferred_command::<CommitUsdProposal>()
            .register_deferred_command::<DiscardDocument>();

        app.add_systems(Update, drain_usd_pending_events);
        app.add_observer(sync_workspace_on_doc_opened);
        app.add_observer(sync_workspace_on_doc_changed);
        app.add_observer(sync_workspace_on_doc_saved);
        app.add_observer(sync_workspace_on_doc_closed);
        app.add_observer(clear_usd_edit_session_on_document_closed);
        // A3 auto-bridge: when the journal appears, hand it to the registry
        // once (reactive — `resource_added`, not per-frame). Headless builds
        // without a journal never run it.
        app.add_systems(
            Update,
            wire_usd_journal_handle.run_if(resource_added::<lunco_doc_bevy::JournalResource>),
        );
        // Document projection claims are part of the document lifecycle. The
        // scene runtime consumes the same resource when it mounts a Twin.
        app.init_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>();
        app.add_observer(claim_user_document_on_opened);
        app.add_observer(forget_backed_document_on_closed);
        register_all_commands(app);
    }
}

register_commands!(
    on_apply_usd_op,
    on_apply_usd_ops,
    on_apply_usd_transient_ops,
    on_create_usd_proposal,
    on_review_usd_proposal,
    on_commit_usd_proposal,
    // The USD half of the generic `UndoDocument`/`RedoDocument` verbs. Registering the
    // observers here (not in the editor) is what lets a headless binary undo.
    on_undo_usd_document,
    on_redo_usd_document,
    on_fork_usd_document,
    on_close_usd_document,
    on_discard_usd_document,
    on_attach_component,
    on_detach_component,
    on_attach_program,
    on_set_dome_light,
    on_new_document,
    on_open_file_for_usd,
    on_save_document,
    on_save_as_document,
);

// ─────────────────────────────────────────────────────────────────────
// USD document open/load pipeline (domain layer)
//
// Moved here from `ui/browser_dispatch.rs`: file I/O and the `OpenFile`
// document observer are document-lifecycle concerns, not UI. Living in
// `UsdCommandsPlugin` means HTTP API / MCP / `Open`-URI dispatch register USD
// documents even in headless bins that never add `UsdUiPlugin`. The UI's
// `browser_dispatch` keeps only the browser-panel `BrowserAction` → `OpenFile`
// translation.
// ─────────────────────────────────────────────────────────────────────

/// Pending file-read kicked off by [`spawn_usd_load`]. Polled by
/// [`drain_pending_usd_file_loads`] each frame until it completes; the
/// resulting source is allocated as a USD document.
struct PendingUsdLoad {
    path: PathBuf,
    /// Root of the Twin that emitted a browser request, if any. A closed Twin
    /// cancels its pending reads before they can create a stale document or
    /// focus a preview for a replaced workspace.
    twin_root: Option<PathBuf>,
    task: Task<Result<String, String>>,
}

#[derive(Resource, Default)]
pub(crate) struct PendingUsdLoads {
    tasks: Vec<PendingUsdLoad>,
}

/// A confirmed file reset whose source is being read off the ECS thread.
struct PendingUsdDiscard {
    doc: DocumentId,
    path: PathBuf,
    task: Task<Result<String, String>>,
    command_id: Option<u64>,
    correlation_id: Option<u64>,
}

#[derive(Resource, Default)]
struct PendingUsdDiscards {
    tasks: Vec<PendingUsdDiscard>,
}

/// Observer for the workbench's typed [`OpenFile`] command. Picks up
/// `.usda`, `.usd`, and `.usdc` paths so HTTP API / MCP / `Open` URI dispatch all route into
/// the same async-load pipeline the Twin browser uses. Modelica's
/// `on_open_file` ignores non-`.mo` paths, so the observers coexist.
#[on_command(OpenFile)]
fn on_open_file_for_usd(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    commands.queue(move |world: &mut World| {
        // `file://` is a filesystem spelling, not a registered asset source;
        // strip it before deciding whether this is an already-addressable
        // scene URI. Other schemes do not have a filesystem document to read;
        // `on_open_file` sends them through the typed scene transition.
        let stripped = path.strip_prefix("file://").unwrap_or(&path);
        if lunco_assets_core::has_scheme(stripped) {
            return;
        }
        if !is_usd_path(stripped) {
            return;
        }
        let path = PathBuf::from(stripped);
        let twin_root = world
            .get_resource::<WorkspaceResource>()
            .and_then(|workspace| {
                workspace
                    .twins()
                    .map(|(_, twin)| twin.root.clone())
                    .filter(|root| path.strip_prefix(root).is_ok())
                    .max_by_key(|root| root.components().count())
            });
        spawn_usd_load(world, path, twin_root);
    });
}

/// Spawn the async file-read for `abs_path` and queue the result in
/// [`PendingUsdLoads`]. Callers should have already established that the
/// path looks like a USD file. Shared by the [`OpenFile`] observer and
/// the UI's `browser_dispatch::drain_browser_actions_for_usd`.
pub fn spawn_usd_load(world: &mut World, abs_path: PathBuf, twin_root: Option<PathBuf>) {
    if let Some(existing) = world
        .resource_mut::<PendingUsdLoads>()
        .tasks
        .iter_mut()
        .find(|load| load.path == abs_path)
    {
        existing.twin_root = twin_root;
        return;
    }
    let pool = AsyncComputeTaskPool::get();
    let path_for_task = abs_path.clone();
    let task = pool.spawn(async move {
        // Read through the storage abstraction — `std::fs` is clippy-banned
        // in domain crates and absent on wasm; `lunco-storage` owns it.
        // `FileStorage`'s read future wraps synchronous fs, so awaiting on
        // the task thread parks no reactor.
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path_for_task.clone());
        match storage.read(&handle).await {
            Ok(bytes) => String::from_utf8(bytes)
                .map_err(|e| format!("invalid UTF-8 in {}: {e}", path_for_task.display())),
            Err(e) => Err(format!("failed to read {}: {e:?}", path_for_task.display())),
        }
    });
    world
        .resource_mut::<PendingUsdLoads>()
        .tasks
        .push(PendingUsdLoad {
            path: abs_path,
            twin_root,
            task,
        });
}

/// Cancel browser reads owned by a Twin that has just left the workspace.
/// Dropping the task is the cancellation boundary; no later completion can
/// allocate a document or focus the editor for the retired Twin.
fn cancel_pending_usd_loads_on_twin_closed(
    trigger: On<TwinClosed>,
    mut pending: ResMut<PendingUsdLoads>,
) {
    let closed_root = &trigger.event().root;
    pending.tasks.retain(|load| {
        !pending_load_belongs_to_closed_twin(load.twin_root.as_deref(), &load.path, closed_root)
    });
}

/// Whether a pending browser read belongs to the Twin being retired. The
/// explicit owner is authoritative; the path check also covers requests
/// emitted by a scene-closure section that already resolved an absolute path.
fn pending_load_belongs_to_closed_twin(
    owner_root: Option<&Path>,
    path: &Path,
    closed_root: &Path,
) -> bool {
    owner_root.is_some_and(|root| root == closed_root) || path.strip_prefix(closed_root).is_ok()
}

/// Poll outstanding [`PendingUsdLoads`] and finish the open once each
/// file's bytes are in hand. Skips and warns on read errors — continuing
/// leaves no half-loaded document behind.
pub(crate) fn drain_pending_usd_file_loads(world: &mut World) {
    if world.resource::<PendingUsdLoads>().tasks.is_empty() {
        return;
    }

    let taken = std::mem::take(&mut world.resource_mut::<PendingUsdLoads>().tasks);
    let mut still_pending: Vec<PendingUsdLoad> = Vec::new();

    for mut load in taken {
        match block_on(future::poll_once(&mut load.task)) {
            None => still_pending.push(load),
            Some(Err(err)) => {
                bevy::log::warn!("[UsdOpenFile] {}", err);
            }
            Some(Ok(source)) => {
                // Idempotent re-open: the registry owns one document per file and
                // decides whether the freshly read source can replace its base.
                let (doc, outcome) = world
                    .resource_mut::<DocumentRegistry<UsdDocument>>()
                    .open_file(load.path.clone(), source);
                claim_user_document_if_projected(world, doc);
                // A re-open that couldn't take the disk bytes is not an error,
                // but it is a surprise the user should see. Keep the warning
                // in the domain log and publish the typed outcome for a UI
                // adapter to present through its own status surface.
                match outcome {
                    OpenOutcome::KeptDirty => {
                        bevy::log::warn!(
                            "[UsdOpenFile] {} has unsaved edits — keeping them; disk NOT reloaded ({doc})",
                            load.path.display()
                        );
                    }
                    OpenOutcome::KeptUnparsable => {
                        bevy::log::warn!(
                            "[UsdOpenFile] {} does not parse as USDA — keeping the open document ({doc})",
                            load.path.display()
                        );
                    }
                    OpenOutcome::Refreshed => {
                        bevy::log::info!(
                            "[UsdOpenFile] {} already open — refreshed from disk ({doc})",
                            load.path.display()
                        );
                    }
                    OpenOutcome::Allocated => {}
                }
                world.trigger(UsdDocumentReady { doc, outcome });
            }
        }
    }

    world.resource_mut::<PendingUsdLoads>().tasks = still_pending;
}

// ─────────────────────────────────────────────────────────────────────
// Fork / close / discard — document lifecycle
// ─────────────────────────────────────────────────────────────────────

/// Fork a USD document through the registry's typed document snapshot seam.
///
/// The returned document is untitled and has an independent id, cache, undo
/// host, and recorder. Lifecycle observers publish the opened/changed events
/// after the command returns, so the normal workspace and journal consumers
/// see the fork without a second editor registry.
#[on_command(ForkDocument)]
fn on_fork_usd_document(
    trigger: On<ForkDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
) -> Result<Ack, String> {
    let command = trigger.event();
    let doc = registry
        .fork(command.source_doc_id, command.name.clone())
        .map_err(|reject| reject.to_string())?;
    let generation = registry
        .host(doc)
        .map(|host| host.generation())
        .ok_or_else(|| format!("forked document {doc} was not installed"))?;
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "source_doc_id": command.source_doc_id.raw(),
            "doc_id": doc.raw(),
            "name": command.name.clone(),
            "generation": generation,
            "target_layer": LayerId::root().as_str(),
            "diagnostics": [],
        }),
    ))
}

/// Close a USD document owned by this registry. Foreign document ids are a
/// no-op by the shared generic-command ownership contract.
#[on_command(CloseDocument)]
fn on_close_usd_document(
    trigger: On<CloseDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
) {
    let doc = trigger.event().doc_id;
    if registry.remove(doc).is_some() {
        bevy::log::info!("[CloseUsd] closed {doc}");
    }
}

/// Begin an explicit discard/revert operation. File bytes are read through
/// the storage abstraction on the async task pool; the registry reset and its
/// history invalidation happen only after the read succeeds on the ECS owner.
#[on_command(DiscardDocument)]
fn on_discard_usd_document(
    trigger: On<DiscardDocument>,
    mut commands: Commands,
    active_id: Res<ActiveCommandId>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let doc = trigger.event().doc_id;
    let command_id = active_id.get();
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let outcome = {
            let origin = world
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(doc)
                .map(|host| host.document().origin().clone());
            let Some(origin) = origin else {
                finish_command_result(
                    world,
                    command_id,
                    correlation_id,
                    Err(format!("unknown USD document {doc}")),
                    ApiErrorCode::CommandRejected,
                );
                return;
            };
            match origin {
                DocumentOrigin::Untitled { .. } => {
                    let generation = world
                        .resource::<DocumentRegistry<UsdDocument>>()
                        .host(doc)
                        .map(|host| host.generation())
                        .unwrap_or_default();
                    world.resource_mut::<UsdEditSessions>().remove_document(doc);
                    world
                        .resource_mut::<DocumentRegistry<UsdDocument>>()
                        .remove(doc);
                    Ok(Ack::with_data(
                        OpId::new(),
                        lunco_api_core::api_value!({
                            "doc_id": doc.raw(),
                            "action": "closed",
                            "generation": generation,
                            "diagnostics": [],
                        }),
                    ))
                }
                DocumentOrigin::Bundled { .. } => Err(format!(
                    "USD document {doc} is bundled and cannot be discarded"
                )),
                DocumentOrigin::File {
                    writable: false, ..
                } => Err(format!(
                    "USD document {doc} is read-only and cannot be discarded"
                )),
                DocumentOrigin::File {
                    path,
                    writable: true,
                } => {
                    if world
                        .resource::<PendingUsdDiscards>()
                        .tasks
                        .iter()
                        .any(|pending| pending.doc == doc)
                    {
                        Err(format!(
                            "USD document {doc} already has a discard in progress"
                        ))
                    } else {
                        // Discard is an explicit authored-state reset. Invalidate
                        // review plans at admission so none can commit while the
                        // asynchronous source read is in flight.
                        world.resource_mut::<UsdEditSessions>().remove_document(doc);
                        let task_path = path.clone();
                        let task = AsyncComputeTaskPool::get().spawn(async move {
                            let storage = lunco_storage::FileStorage::new();
                            let handle = lunco_storage::StorageHandle::File(task_path.clone());
                            match storage.read(&handle).await {
                                Ok(bytes) => String::from_utf8(bytes).map_err(|error| {
                                    format!("invalid UTF-8 in {}: {error}", task_path.display())
                                }),
                                Err(error) => Err(format!(
                                    "failed to read {}: {error:?}",
                                    task_path.display()
                                )),
                            }
                        });
                        world
                            .resource_mut::<PendingUsdDiscards>()
                            .tasks
                            .push(PendingUsdDiscard {
                                doc,
                                path,
                                task,
                                command_id,
                                correlation_id,
                            });
                        return;
                    }
                }
            }
        };
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

/// Poll explicit discard reads and commit only against the same resident file
/// document that requested them. A closed or Save-As-rebound document is
/// rejected rather than applying bytes to a different document identity.
fn drain_pending_usd_discards(world: &mut World) {
    if world.resource::<PendingUsdDiscards>().tasks.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut world.resource_mut::<PendingUsdDiscards>().tasks);
    let mut still_pending = Vec::new();
    for mut discard in pending {
        let result = match block_on(future::poll_once(&mut discard.task)) {
            None => {
                still_pending.push(discard);
                continue;
            }
            Some(result) => result,
        };
        let outcome = match result {
            Err(error) => Err(error),
            Ok(source) => {
                let current_path = world
                    .resource::<DocumentRegistry<UsdDocument>>()
                    .host(discard.doc)
                    .and_then(|host| host.document().origin().canonical_path())
                    .map(Path::to_path_buf);
                if current_path.as_deref() != Some(discard.path.as_path()) {
                    Err(format!(
                        "USD document {} changed identity while discard was reading",
                        discard.doc
                    ))
                } else {
                    let (doc, outcome) = world
                        .resource_mut::<DocumentRegistry<UsdDocument>>()
                        .reset_file(discard.path.clone(), source);
                    if doc != discard.doc {
                        Err(format!(
                            "discard source resolved to document {doc}, expected {}",
                            discard.doc
                        ))
                    } else {
                        match outcome {
                            lunco_doc::OpenOutcome::Refreshed => {
                                world.resource_mut::<UsdEditSessions>().remove_document(doc);
                                claim_user_document_if_projected(world, doc);
                                let generation = world
                                    .resource::<DocumentRegistry<UsdDocument>>()
                                    .host(doc)
                                    .map(|host| host.generation())
                                    .unwrap_or_default();
                                Ok(Ack::with_data(
                                    OpId::new(),
                                    lunco_api_core::api_value!({
                                        "doc_id": doc.raw(),
                                        "action": "discarded",
                                        "generation": generation,
                                        "target_layer": LayerId::root().as_str(),
                                        "diagnostics": [],
                                    }),
                                ))
                            }
                            lunco_doc::OpenOutcome::KeptUnparsable => Err(format!(
                                "discard source for {} is not valid USDA",
                                discard.doc
                            )),
                            other => Err(format!(
                                "discard of {} did not reset the resident file ({other:?})",
                                discard.doc
                            )),
                        }
                    }
                }
            }
        };
        finish_command_result(
            world,
            discard.command_id,
            discard.correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    }
    world.resource_mut::<PendingUsdDiscards>().tasks = still_pending;
}

// ─────────────────────────────────────────────────────────────────────
// NewDocument — File→New "USD Stage"
// ─────────────────────────────────────────────────────────────────────

#[on_command(NewDocument)]
fn on_new_document(trigger: On<NewDocument>, mut commands: Commands) {
    if trigger.event().kind != USD_DOCUMENT_KIND {
        return;
    }
    commands.queue(|world: &mut World| {
        let doc_id = {
            let mut registry = world.resource_mut::<DocumentRegistry<UsdDocument>>();
            let next = registry.ids().count() + 1;
            registry.allocate(
                DEFAULT_USDA_SCAFFOLD.to_string(),
                lunco_doc::PathlessOrigin::untitled(format!("UntitledStage-{}.usda", next)),
            )
        };
        claim_user_document_if_projected(world, doc_id);
        bevy::log::info!("[NewUsd] created untitled USD stage as {}", doc_id);
    });
}

/// Minimal valid `.usda` source for File→New. One empty `World` Xform
/// — enough that the parser is happy and the user has somewhere to
/// add prims.
const DEFAULT_USDA_SCAFFOLD: &str =
    "#usda 1.0\n(\n    defaultPrim = \"World\"\n    upAxis = \"Y\"\n    metersPerUnit = 1.0\n)\n\ndef Xform \"World\"\n{\n}\n";

// ─────────────────────────────────────────────────────────────────────
// SaveDocument — gated on registry membership
// ─────────────────────────────────────────────────────────────────────

#[on_command(SaveDocument)]
fn on_save_document(trigger: On<SaveDocument>, mut commands: Commands) {
    let doc_id = trigger.event().doc_id;
    commands.queue(move |world: &mut World| {
        let registry = world.resource::<DocumentRegistry<UsdDocument>>();
        let Some(host) = registry.host(doc_id) else {
            return;
        };
        let doc = host.document();
        let path = match doc.origin() {
            DocumentOrigin::File {
                path,
                writable: true,
            } => path.clone(),
            DocumentOrigin::File {
                writable: false, ..
            } => {
                bevy::log::warn!("[SaveUsd] {} is read-only", doc_id);
                return;
            }
            DocumentOrigin::Untitled { .. } => {
                bevy::log::warn!("[SaveUsd] {} is Untitled — Save-As required", doc_id);
                return;
            }
            DocumentOrigin::Bundled { .. } => {
                bevy::log::warn!("[SaveUsd] {} is a bundled example — read-only", doc_id);
                return;
            }
        };
        let source = doc.source();
        // Route through the storage abstraction instead of a direct
        // `std::fs::write` (clippy-banned in domain crates, wasm-broken).
        // `write_sync` blocks on `FileStorage`'s write future, which wraps
        // synchronous fs and is already `Ready` — no reactor, no hang.
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path.clone());
        if let Err(e) = storage.write_sync(&handle, source.as_bytes()) {
            bevy::log::error!(
                "[SaveUsd] {} write to {} failed: {:?}",
                doc_id,
                path.display(),
                e
            );
            return;
        }
        // Borrow mut to mark saved. `host_mut` doesn't bump the
        // change ring because saving doesn't change the document — it
        // only resets the dirty marker.
        {
            let mut reg = world.resource_mut::<DocumentRegistry<UsdDocument>>();
            if let Some(host) = reg.host_mut(doc_id) {
                host.document_mut().mark_saved();
            }
            // Re-baseline the disk watermark: the bytes on disk are now ours, so
            // the staleness check must not flag this write as an external edit.
            reg.note_saved(doc_id);
        }
        bevy::log::info!("[SaveUsd] {} saved to {}", doc_id, path.display());
    });
}

/// Persist a USD document to a new path and rebind its canonical origin.
///
/// Untitled stages are real documents, so Save-As is the promotion edge that
/// makes their edits visible to the ordinary file/Twin workflow. The domain
/// owns the bytes and origin update; a UI adapter supplies a path when a
/// dialog is needed.
#[on_command(SaveAsDocument)]
fn on_save_as_document(
    trigger: On<SaveAsDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut commands: Commands,
) {
    let doc_id = trigger.event().doc_id;
    let target_path = trigger.event().path.clone();
    let Some(host) = registry.host(doc_id) else {
        bevy::log::warn!("[SaveAsUsd] unknown document {doc_id}");
        return;
    };
    let document = host.document();
    let source = document.source();
    if target_path.is_empty() {
        bevy::log::warn!("[SaveAsUsd] {doc_id} has no target path; the caller must provide one");
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
        let _ = (source, commands);
        bevy::log::warn!("[SaveAsUsd] {doc_id} cannot save a local file on wasm");
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = std::path::PathBuf::from(target_path);
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path.clone());
        if let Err(error) = storage.write_sync(&handle, source.as_bytes()) {
            bevy::log::error!(
                "[SaveAsUsd] {doc_id} write to {} failed: {error}",
                path.display()
            );
            return;
        }
        if let Some(host) = registry.host_mut(doc_id) {
            host.document_mut().set_origin(DocumentOrigin::File {
                path: path.clone(),
                writable: true,
            });
            host.document_mut().mark_saved();
        }
        bevy::log::info!("[SaveAsUsd] {doc_id} saved to {}", path.display());
        commands.trigger(lunco_doc_bevy::DocumentSaved::local(doc_id));
    }
}

fn proposal_ack(
    proposal: UsdProposalId,
    doc: DocumentId,
    action: &str,
    generation: u64,
    state: UsdProposalState,
) -> Ack {
    Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "proposal": proposal.0,
            "doc_id": doc.raw(),
            "action": action,
            "generation": generation,
            "state": state.as_str(),
            "diagnostics": [],
        }),
    )
}

fn proposal_diagnostics(diagnostics: &[String]) -> String {
    diagnostics.join("; ")
}

/// Supply the document owner with the existing resolver closure, never a
/// flattened scene or a second filesystem resolver. The document replaces the
/// recipe root with its current opinions for each synchronous operation.
fn refresh_authoring_recipe(world: &mut World, doc: DocumentId) {
    let recipe = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc).map(|stage| {
        lunco_usd_compose::recipe::StageRecipe::new(
            stage.scene_layer.clone(),
            stage.layer_bytes_snapshot(),
        )
    });
    if let Some(host) = world
        .resource_mut::<DocumentRegistry<UsdDocument>>()
        .host_mut(doc)
    {
        host.document_mut().set_authoring_recipe(recipe);
    }
}

#[on_command(CreateUsdProposal)]
fn on_create_usd_proposal(
    trigger: On<CreateUsdProposal>,
    mut commands: Commands,
    active_id: Option<Res<ActiveCommandId>>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let command = trigger.event().clone();
    let command_id = active_id.as_ref().and_then(|id| id.get());
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        refresh_authoring_recipe(world, command.doc_id);
        let outcome = match world
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(command.doc_id)
            .map(|host| host.document())
        {
            None => Err(format!("unknown USD document {}", command.doc_id)),
            Some(document) => {
                let validation =
                    validate_proposal(document, command.scope, command.parent_gen, &command.ops);
                if !validation.is_valid() {
                    Err(format!(
                        "USD proposal validation failed: {}",
                        proposal_diagnostics(&validation.diagnostics)
                    ))
                } else {
                    let id = world.resource_mut::<UsdEditSessions>().insert(
                        command.doc_id,
                        command.scope,
                        command.label,
                        command.parent_gen,
                        validation,
                        command.ops,
                    );
                    let generation = world
                        .resource::<DocumentRegistry<UsdDocument>>()
                        .host(command.doc_id)
                        .map(|host| host.generation())
                        .unwrap_or_default();
                    Ok(proposal_ack(
                        id,
                        command.doc_id,
                        "created",
                        generation,
                        UsdProposalState::Pending,
                    ))
                }
            }
        };
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

#[on_command(ReviewUsdProposal)]
fn on_review_usd_proposal(
    trigger: On<ReviewUsdProposal>,
    mut commands: Commands,
    active_id: Option<Res<ActiveCommandId>>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let command = trigger.event().clone();
    let command_id = active_id.as_ref().and_then(|id| id.get());
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let outcome = match world
            .resource::<UsdEditSessions>()
            .proposal(command.proposal)
            .cloned()
        {
            None => Err(format!("unknown USD proposal {}", command.proposal.0)),
            Some(proposal) => match command.action {
                UsdProposalReviewAction::Reject => {
                    let removed = world
                        .resource_mut::<UsdEditSessions>()
                        .remove(command.proposal);
                    if removed.is_none() {
                        Err(format!("unknown USD proposal {}", command.proposal.0))
                    } else {
                        Ok(proposal_ack(
                            proposal.id,
                            proposal.doc,
                            "rejected",
                            proposal.parent_generation,
                            proposal.state,
                        ))
                    }
                }
                UsdProposalReviewAction::Mute => {
                    world
                        .resource_mut::<UsdEditSessions>()
                        .set_state(command.proposal, UsdProposalState::Muted)
                        .map(|_| {
                            proposal_ack(
                                proposal.id,
                                proposal.doc,
                                "muted",
                                proposal.parent_generation,
                                UsdProposalState::Muted,
                            )
                        })
                }
                UsdProposalReviewAction::Unmute => {
                    if proposal.state == UsdProposalState::Conflict {
                        Err(format!(
                            "USD proposal {} is conflicted; create a new proposal from the current generation",
                            proposal.id.0
                        ))
                    } else {
                        world
                            .resource_mut::<UsdEditSessions>()
                            .set_state(command.proposal, UsdProposalState::Pending)
                            .map(|_| {
                                proposal_ack(
                                    proposal.id,
                                    proposal.doc,
                                    "unmuted",
                                    proposal.parent_generation,
                                    UsdProposalState::Pending,
                                )
                            })
                    }
                }
            },
        };
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

#[on_command(CommitUsdProposal)]
fn on_commit_usd_proposal(
    trigger: On<CommitUsdProposal>,
    mut commands: Commands,
    active_id: Option<Res<ActiveCommandId>>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let proposal_id = trigger.event().proposal;
    let command_id = active_id.as_ref().and_then(|id| id.get());
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let outcome = match world
            .resource::<UsdEditSessions>()
            .proposal(proposal_id)
            .cloned()
        {
            None => Err(format!("unknown USD proposal {}", proposal_id.0)),
            Some(proposal) => {
                refresh_authoring_recipe(world, proposal.doc);
                let current = world
                    .resource::<DocumentRegistry<UsdDocument>>()
                    .host(proposal.doc)
                    .map(|host| {
                        (
                            host.generation(),
                            host.document().base_revision(),
                            host.document().origin().session_uri(),
                            validate_proposal(
                                host.document(),
                                proposal.scope,
                                proposal.parent_generation,
                                &proposal.ops,
                            ),
                        )
                    });
                let Some((generation, base_revision, origin, validation)) = current else {
                    let error = format!("unknown USD document {}", proposal.doc);
                    world.resource_mut::<UsdEditSessions>().remove(proposal.id);
                    return finish_command_result(
                        world,
                        command_id,
                        correlation_id,
                        Err(error),
                        ApiErrorCode::CommandRejected,
                    );
                };
                let externally_stale = world
                    .resource::<DocumentRegistry<UsdDocument>>()
                    .stale_docs()
                    .contains(&proposal.doc);
                let conflict = if proposal.state != UsdProposalState::Pending {
                    Some(format!(
                        "proposal {} is not pending (state={})",
                        proposal.id.0,
                        proposal.state.as_str()
                    ))
                } else if generation != proposal.parent_generation {
                    Some(format!(
                        "stale document generation: proposal parent {}, current {}",
                        proposal.parent_generation, generation
                    ))
                } else if base_revision != proposal.base_revision {
                    Some(format!(
                        "stale authored layer revision: proposal {}, current {}",
                        proposal.base_revision, base_revision
                    ))
                } else if origin != proposal.origin {
                    Some("document origin changed since proposal creation".to_owned())
                } else if externally_stale {
                    Some("backing USD file changed on disk since proposal creation".to_owned())
                } else if !validation.is_valid() {
                    Some(proposal_diagnostics(&validation.diagnostics))
                } else {
                    None
                };
                if let Some(conflict) = conflict {
                    world
                        .resource_mut::<UsdEditSessions>()
                        .mark_conflict(proposal.id, conflict.clone());
                    Err(format!(
                        "USD proposal {} conflict: {conflict}",
                        proposal.id.0
                    ))
                } else {
                    match apply_ops_as_change_set_result(
                        world,
                        proposal.doc,
                        proposal.label.clone(),
                        proposal.ops.clone(),
                        Some(proposal.parent_generation),
                    ) {
                        Ok((mut ack, _)) => {
                            claim_user_document_if_projected(world, proposal.doc);
                            let edit = ack.data.take().unwrap_or_default();
                            world.resource_mut::<UsdEditSessions>().remove(proposal.id);
                            ack.data = Some(lunco_api_core::api_value!({
                                "proposal": proposal.id.0,
                                "doc_id": proposal.doc.raw(),
                                "action": "committed",
                                "edit": edit,
                                "diagnostics": [],
                            }));
                            Ok(ack)
                        }
                        Err(error) => {
                            world
                                .resource_mut::<UsdEditSessions>()
                                .mark_conflict(proposal.id, error.clone());
                            Err(format!(
                                "USD proposal {} could not be committed: {error}",
                                proposal.id.0
                            ))
                        }
                    }
                }
            }
        };
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

#[on_command(ApplyUsdOps)]
fn on_apply_usd_ops(
    trigger: On<ApplyUsdOps>,
    mut commands: Commands,
    active_id: Res<ActiveCommandId>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let command = trigger.event().clone();
    let command_id = active_id.get();
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let total = command.ops.len();
        let outcome = match apply_ops_as_change_set_result(
            world,
            command.doc_id,
            command.label,
            command.ops,
            command.parent_gen,
        ) {
            Ok((ack, applied)) if applied == total => {
                claim_user_document_if_projected(world, command.doc_id);
                Ok(ack)
            }
            Ok((_, applied)) => Err(format!(
                "USD document {} applied {applied}/{total} operations",
                command.doc_id
            )),
            Err(error) => Err(error),
        };
        if let Err(error) = &outcome {
            bevy::log::warn!("[ApplyUsdOps] {} rejected: {error}", command.doc_id);
        }
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

#[on_command(ApplyUsdTransientOps)]
fn on_apply_usd_transient_ops(
    trigger: On<ApplyUsdTransientOps>,
    mut commands: Commands,
    active_id: Res<ActiveCommandId>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let command = trigger.event().clone();
    let command_id = active_id.get();
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let total = command.ops.len();
        let outcome = apply_transient_ops_result(
            world,
            command.doc_id,
            command.parent_gen,
            command.label,
            command.ops,
        )
        .and_then(|(ack, applied)| {
            if applied == total {
                Ok(ack)
            } else {
                Err(format!(
                    "USD document {} applied {applied}/{total} transient operations",
                    command.doc_id
                ))
            }
        });
        if let Err(error) = &outcome {
            bevy::log::warn!(
                "[ApplyUsdTransientOps] {} rejected: {error}",
                command.doc_id
            );
        }
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

#[on_command(ApplyUsdOp)]
fn on_apply_usd_op(
    trigger: On<ApplyUsdOp>,
    mut commands: Commands,
    active_id: Res<ActiveCommandId>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let doc = trigger.event().doc_id;
    let parent_gen = trigger.event().parent_gen;
    let op = trigger.event().op.clone();
    let target = op.edit_target().clone();
    let target_layers = vec![target.as_str().to_owned()];
    let paths = op.affected_paths();
    let command_id = active_id.get();
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let paths_for_error = paths.clone();
        refresh_authoring_recipe(world, doc);
        let result = match validate_live_attribute_types(world, doc, std::slice::from_ref(&op)) {
            Ok(()) => world
                .resource_mut::<DocumentRegistry<UsdDocument>>()
                .apply_mutation(
                    doc,
                    match parent_gen {
                        Some(parent) => lunco_doc::Mutation::local_against(parent, op),
                        None => lunco_doc::Mutation::local(op),
                    },
                )
                .map_err(|reject| lunco_doc::DocumentError::Internal(reject.to_string())),
            Err(error) => Err(lunco_doc::DocumentError::ValidationFailed(error)),
        };
        let outcome = result
            .map(|mut ack| {
                claim_user_document_if_projected(world, doc);
                ack.data = Some(usd_ack_data(
                    doc,
                    &target_layers,
                    &paths,
                    ack.new_gen.unwrap_or_default(),
                    None,
                    world.get_resource::<lunco_doc_bevy::JournalResource>(),
                ));
                bevy::log::debug!("[ApplyUsdOp] {} → gen {}", doc, ack.new_gen.unwrap_or(0));
                ack
            })
            .map_err(|reject| {
                format!(
                    "USD document {doc} edit at {:?} rejected: {reject}",
                    paths_for_error
                )
            });
        if let Err(error) = &outcome {
            bevy::log::warn!("[ApplyUsdOp] {error}");
        }
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

// ─────────────────────────────────────────────────────────────────────
// UndoDocument / RedoDocument — the ONE undo, per-domain
// ─────────────────────────────────────────────────────────────────────
//
// The VERB is generic and lives in `lunco-doc-bevy`; each domain observes it and acts
// only on documents its own registry owns (a Modelica document is handled by Modelica's
// observer in `lunco-modelica-ui/src/ui/commands/doc.rs`). These are USD's half, and they
// live HERE — in the crate that owns `DocumentRegistry<UsdDocument>` — not in the editor, so a
// headless binary with documents but no 3D editor can still undo.
//
// The generic undo verbs are handled by each document-owning domain. USD's
// handlers live here so the document registry remains the single owner.

/// Per-domain [`UndoDocument`] handler for **USD** documents: undo the document's last
/// history group by applying its typed inverses.
///
/// This is the **only** undo. Every authored edit — spawn, move, delete, terrain stroke,
/// waypoint, property — reaches the world as a [`UsdOp`] through [`ApplyUsdOp`], and
/// `UsdDocument::apply` hands back a typed inverse for each. So undo is a document
/// concern, not an editor one: apply the inverse group, and the projection re-derives
/// the ECS ([`crate::live_consume`]). It journals (undo/redo record through the same
/// `OpRecorder` seam) and replicates like any other op.
///
/// An editor-side transform stack cannot do this: it does not know about the
/// document, so an undone spawn could remain in the layer and journal. The
/// document-owned inverse group keeps both representations aligned.
///
/// No-ops for a `doc` this registry doesn't own, per the `UndoDocument` ownership
/// convention.
#[on_command(UndoDocument)]
pub fn on_undo_usd_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    mut commands: Commands,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let doc = trigger.event().doc_id;
    if registry.host(doc).is_none() {
        return;
    }
    let mut apply = || registry.host_mut(doc).map_or(Ok(false), |host| host.undo());
    let outcome = match journal {
        Some(journal) => journal
            .as_ref()
            .change_set(format!("Undo document {doc}"), apply),
        None => apply(),
    };
    match outcome {
        Ok(true) => {
            // `host_mut` bypasses the registry's `apply` funnel, so the Changed
            // notification has to be raised by hand (documented on `host_mut`). The twin
            // projection then re-derives the scene.
            registry.mark_changed(doc);
            if backed.claim_user(doc) {
                commands.trigger(lunco_usd_bevy_twin::UsdDocumentUserOwned { doc });
            }
            bevy::log::info!("[usd] undo applied on {doc}");
        }
        Ok(false) => bevy::log::info!("[usd] nothing to undo on {doc}"),
        Err(e) => bevy::log::warn!("[usd] undo failed on {doc}: {e:?}"),
    }
}

/// Per-domain [`RedoDocument`] handler for **USD** documents. The mirror of
/// [`on_undo_usd_document`]; same ownership rules.
#[on_command(RedoDocument)]
pub fn on_redo_usd_document(
    trigger: On<RedoDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    mut commands: Commands,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let doc = trigger.event().doc_id;
    if registry.host(doc).is_none() {
        return;
    }
    let mut apply = || registry.host_mut(doc).map_or(Ok(false), |host| host.redo());
    let outcome = match journal {
        Some(journal) => journal
            .as_ref()
            .change_set(format!("Redo document {doc}"), apply),
        None => apply(),
    };
    match outcome {
        Ok(true) => {
            registry.mark_changed(doc);
            if backed.claim_user(doc) {
                commands.trigger(lunco_usd_bevy_twin::UsdDocumentUserOwned { doc });
            }
            bevy::log::info!("[usd] redo applied on {doc}");
        }
        Ok(false) => bevy::log::info!("[usd] nothing to redo on {doc}"),
        Err(e) => bevy::log::warn!("[usd] redo failed on {doc}: {e:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// AttachComponent — build-from-parts (doc 48 §3.1)
// ─────────────────────────────────────────────────────────────────────

/// Apply a lowered [`UsdOp`] sequence to `doc` as **one document history group**
/// and one journal change set (H10).
///
/// A command that lowers to many primitive ops (`AttachComponent` → multiple base ops,
/// plus optional socket/rotation/axis ops; `realign_component_ops` → 4) must not
/// journal them as N independent entries:
/// one undo would then peel off ONE op and leave the object half-attached. The
/// journal's change-set API exists for exactly this
/// ([`lunco_doc_bevy::JournalResource::change_set`] → `Journal::begin_change_set`),
/// and the auto-recorder on the registry appends with `change_set: None`, so every
/// entry recorded inside the closure inherits the ambient set with no per-op code.
/// `UndoManager::take_undo_group` then undoes the whole group.
///
/// **Every multi-op USD handler should route through this** — including the
/// `realign_component_ops` call sites in `lunco-luncosim-edit-ui`.
///
/// The generic document host validates the complete sequence against a clone before
/// committing it. A malformed multi-op intent therefore applies zero operations;
/// a valid intent is committed as one history group even without a journal.
///
/// Returns `(applied, total)`.
fn usd_ack_data(
    doc: DocumentId,
    target_layers: &[String],
    paths: &[String],
    generation: u64,
    change_set_id: Option<lunco_twin_journal::ChangeSetId>,
    journal: Option<&lunco_doc_bevy::JournalResource>,
) -> lunco_api_core::ApiValue {
    let journal_cursor = journal.and_then(|journal| {
        journal.with_read(|journal| {
            journal
                .entries_for_doc(doc)
                .last()
                .map(|entry| entry.id.clone())
        })
    });
    let journal_cursor = journal_cursor
        .map(|cursor| {
            lunco_api_core::api_value!({
                "author": cursor.author.0,
                "lamport": cursor.lamport,
            })
        })
        .unwrap_or_default();
    lunco_api_core::api_value!({
        "status": "applied",
        "doc_id": doc.raw(),
        "target_layer": target_layers.first(),
        "target_layers": target_layers.to_vec(),
        "paths": paths.to_vec(),
        "generation": generation,
        "change_set_id": change_set_id.map(|id| id.0),
        "journal_cursor": journal_cursor,
        "diagnostics": [],
    })
}

fn apply_ops_as_change_set_result(
    world: &mut World,
    doc: DocumentId,
    label: impl Into<String>,
    ops: Vec<UsdOp>,
    parent_gen: Option<u64>,
) -> Result<(Ack, usize), String> {
    refresh_authoring_recipe(world, doc);
    validate_live_attribute_types(world, doc, &ops)?;
    let total = ops.len();
    let paths: Vec<String> = ops.iter().flat_map(UsdOp::affected_paths).collect();
    let mut target_layers = Vec::new();
    for target in ops.iter().map(UsdOp::edit_target) {
        if !target_layers.iter().any(|known| known == target.as_str()) {
            target_layers.push(target.as_str().to_owned());
        }
    }
    let journal = world
        .get_resource::<lunco_doc_bevy::JournalResource>()
        .cloned();
    let apply_all = |world: &mut World| {
        world
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .apply_group_against(doc, parent_gen, ops)
            .map_err(|reject| {
                format!(
                    "USD document {doc} compound edit at {:?} rejected: {reject}",
                    paths
                )
            })
    };
    let (change_set_id, result) = match journal.as_ref() {
        Some(journal) => {
            let (id, result) = journal.change_set_with_id(label, || apply_all(world));
            (Some(id), result)
        }
        None => (None, apply_all(world)),
    };
    let mut ack = result?;
    ack.data = Some(usd_ack_data(
        doc,
        &target_layers,
        &paths,
        ack.new_gen.unwrap_or_default(),
        change_set_id,
        journal.as_ref(),
    ));
    Ok((ack, total))
}

fn apply_transient_ops_result(
    world: &mut World,
    doc: DocumentId,
    parent_gen: Option<u64>,
    _label: String,
    ops: Vec<UsdOp>,
) -> Result<(Ack, usize), String> {
    refresh_authoring_recipe(world, doc);
    validate_live_attribute_types(world, doc, &ops)?;
    let total = ops.len();
    let result = {
        let mut registry = world.resource_mut::<DocumentRegistry<UsdDocument>>();
        let Some(host) = registry.host_mut(doc) else {
            return Err(format!("USD document {doc} is not open"));
        };
        if let Some(parent) = parent_gen {
            let current = host.generation();
            if parent != current {
                return Err(format!(
                    "USD document {doc} has stale parent generation {parent}; current is {current}"
                ));
            }
        }
        host.apply_group_transient(ops)
            .map_err(|reject| format!("USD transient edit rejected: {reject}"))?
    };
    world
        .resource_mut::<DocumentRegistry<UsdDocument>>()
        .mark_changed(doc);
    let mut ack = result;
    ack.data = Some(usd_ack_data(
        doc,
        &[],
        &[],
        ack.new_gen.unwrap_or_default(),
        None,
        world.get_resource::<lunco_doc_bevy::JournalResource>(),
    ));
    Ok((ack, total))
}

/// Validate typed attribute operations against the already-mounted composed
/// stage before changing the document layer. The document owns local SDF
/// declarations; this is the complementary check for referenced, payloaded,
/// and variant-composed declarations that do not exist in that document's
/// authored data. Keeping the check before the registry mutation preserves the
/// document/stage transaction when OpenUSD would reject a role or array-shape
/// mismatch (for example `color3f` versus `color3f[]`).
fn validate_live_attribute_types(
    world: &World,
    doc: DocumentId,
    ops: &[UsdOp],
) -> Result<(), String> {
    let planned_attributes: HashMap<(String, String), String> = ops
        .iter()
        .filter_map(|op| match op {
            UsdOp::SetAttribute {
                path,
                name,
                type_name,
                ..
            }
            | UsdOp::SetTimeSample {
                path,
                name,
                type_name,
                ..
            }
            | UsdOp::SetConnection {
                path,
                name,
                type_name,
                ..
            } => Some(((path.clone(), name.clone()), type_name.clone())),
            _ => None,
        })
        .collect();

    let Some(stage) = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc) else {
        let Some(composed) = world
            .get_resource::<DocumentRegistry<UsdDocument>>()
            .and_then(|registry| registry.host(doc))
            .map(|host| host.document().composed_arc())
        else {
            return Ok(());
        };
        return validate_authored_attribute_types(&composed, ops, &planned_attributes);
    };
    let view = stage.view();
    let stage_path = world
        .get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
        .and_then(|scenes| scenes.coords_of(doc))
        .map(|(name, rel)| lunco_assets_core::twin_uri(&name, &rel));
    let stage_id = stage_path
        .and_then(|path| {
            world
                .get_resource::<AssetServer>()
                .and_then(|server| server.get_handle::<UsdStageAsset>(path))
        })
        .map(|handle| handle.id());
    for op in ops {
        let (path, name, requested) = match op {
            UsdOp::SetAttribute {
                path,
                name,
                type_name,
                ..
            }
            | UsdOp::SetTimeSample {
                path,
                name,
                type_name,
                ..
            }
            | UsdOp::SetConnection {
                path,
                name,
                type_name,
                ..
            } => (path, name, type_name),
            _ => continue,
        };
        let sdf_path = openusd::sdf::Path::new(path).map_err(|error| {
            format!("typed USD edit `{path}.{name}` has an invalid path: {error}")
        })?;
        if let Some(declared) = view.attr_type_name(&sdf_path, name) {
            if declared != *requested {
                return Err(format!(
                    "typed USD edit `{path}.{name}` requests `{requested}`, but the composed stage declares `{declared}`"
                ));
            }
        }
        let UsdOp::SetConnection {
            sources, type_name, ..
        } = op
        else {
            continue;
        };
        for source in sources {
            let source_path = openusd::sdf::Path::new(source).map_err(|error| {
                format!("USD connection `{path}.{name}` has invalid source `{source}`: {error}")
            })?;
            let Some((source_prim, source_name)) = source_path.split_property() else {
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` is not a property path; author a source attribute such as `/Controller.outputs:throttle`"
                ));
            };
            let source_key = (source_prim.to_string(), source_name.to_string());
            let source_type = planned_attributes
                .get(&source_key)
                .cloned()
                .or_else(|| view.attr_type_name(&source_prim, source_name));
            if !view.has_prim(&source_prim)
                && !planned_attributes
                    .keys()
                    .any(|(path, _)| path == &source_prim.to_string())
            {
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` references missing prim `{source_prim}`"
                ));
            }
            let Some(source_type) = source_type else {
                if lunco_usd_bevy_stage::read::has_runtime_port_surface(&view, &source_prim)
                    && stage_id.is_some_and(|stage_id| {
                        live_runtime_port_exists(world, stage_id, &source_prim, source_name)
                    })
                {
                    continue;
                }
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` references missing property `{source_name}` on `{source_prim}`"
                ));
            };
            if source_type != *type_name {
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` declares `{source_type}`, but the sink declares `{type_name}`"
                ));
            }
        }
    }
    Ok(())
}

/// Check the exact dynamic endpoint against the projected registry. A provider
/// schema only says that a prim can publish runtime ports; it does not make
/// every spelling a valid port. This is intentionally a live-only check: the
/// file validator has no ECS/runtime registry to consult.
fn live_runtime_port_exists(
    world: &World,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    prim: &openusd::sdf::Path,
    property: &str,
) -> bool {
    let Some((direction, name)) = property
        .strip_prefix("outputs:")
        .map(|name| ("output", name))
        .or_else(|| property.strip_prefix("inputs:").map(|name| ("input", name)))
    else {
        return false;
    };
    let Some(registry) = world.get_resource::<lunco_port_core::ports::PortRegistry>() else {
        return false;
    };
    for entity in world.iter_entities() {
        let Some(path) = entity.get::<lunco_usd_bevy_scene::UsdPrimPath>() else {
            continue;
        };
        if path.stage_handle.id() != stage_id || path.path != prim.as_str() {
            continue;
        }
        return if direction == "output" {
            registry.has_output_port(world, entity.id(), name)
        } else {
            registry.has_input_port(world, entity.id(), name)
        };
    }
    false
}

/// The document-owned fallback for command preflight while a preview's
/// canonical composed stage is still settling. It covers self-contained and
/// newly opened documents; once a canonical stage exists the live composed
/// reader above remains authoritative for references, payloads, and variants.
fn validate_authored_attribute_types(
    composed: &openusd::sdf::Data,
    ops: &[UsdOp],
    planned_attributes: &HashMap<(String, String), String>,
) -> Result<(), String> {
    let attr_type = |path: &openusd::sdf::Path, name: &str| {
        let property = path.append_property(name).ok()?;
        match composed.field(&property, "typeName") {
            Some(openusd::sdf::Value::Token(type_name)) => Some(type_name.to_string()),
            Some(openusd::sdf::Value::String(type_name)) => Some(type_name.clone()),
            _ => None,
        }
    };
    let planned_prim = |path: &openusd::sdf::Path| {
        let path = path.to_string();
        planned_attributes.keys().any(|(prim, _)| prim == &path)
    };
    for op in ops {
        let (path, name, requested) = match op {
            UsdOp::SetAttribute {
                path,
                name,
                type_name,
                ..
            }
            | UsdOp::SetTimeSample {
                path,
                name,
                type_name,
                ..
            }
            | UsdOp::SetConnection {
                path,
                name,
                type_name,
                ..
            } => (path, name, type_name),
            _ => continue,
        };
        let target_path = openusd::sdf::Path::new(path).map_err(|error| {
            format!("typed USD edit `{path}.{name}` has an invalid path: {error}")
        })?;
        if let Some(declared) = attr_type(&target_path, name) {
            if declared != *requested {
                return Err(format!(
                    "typed USD edit `{path}.{name}` requests `{requested}`, but the composed document declares `{declared}`"
                ));
            }
        }
        let UsdOp::SetConnection { sources, .. } = op else {
            continue;
        };
        for source in sources {
            let source_path = openusd::sdf::Path::new(source).map_err(|error| {
                format!("USD connection `{path}.{name}` has invalid source `{source}`: {error}")
            })?;
            let Some((source_prim, source_name)) = source_path.split_property() else {
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` is not a property path; author a source attribute such as `/Controller.outputs:throttle`"
                ));
            };
            let source_key = (source_prim.to_string(), source_name.to_string());
            let source_type = planned_attributes
                .get(&source_key)
                .cloned()
                .or_else(|| attr_type(&source_prim, source_name));
            if composed.prim_type_name(&source_prim).is_none() && !planned_prim(&source_prim) {
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` references missing prim `{source_prim}`"
                ));
            }
            let Some(source_type) = source_type else {
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` references missing property `{source_name}` on `{source_prim}`"
                ));
            };
            if source_type != *requested {
                return Err(format!(
                    "USD connection `{path}.{name}` source `{source}` declares `{source_type}`, but the sink declares `{requested}`"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn apply_ops_as_change_set(
    world: &mut World,
    doc: DocumentId,
    label: impl Into<String>,
    ops: Vec<UsdOp>,
) -> (usize, usize) {
    let total = ops.len();
    let applied = match apply_ops_as_change_set_result(world, doc, label, ops, None) {
        Ok(_) => total,
        Err(error) => {
            bevy::log::warn!("[usd] {error}");
            0
        }
    };
    if applied == total && total != 0 {
        claim_user_document_if_projected(world, doc);
    }
    (applied, total)
}

/// Attach a component asset to a host body as a jointed child, deriving the
/// joint anchor from the placement so it is authored once, not twice. Lowers to
/// the primitive [`UsdOp`]s in [`lunco_usd_core::attach::attach_component_ops`].
///
/// The whole lowering is applied inside **one journal change set**
/// ([`apply_ops_as_change_set`]), so the attach is **one undo unit**: undo removes
/// the part, its placement, its joint, and the joint's anchors together.
///
/// The generic compound boundary validates the complete lowered sequence before
/// touching the live document. Socket attaches additionally validate the selected
/// socket, its schema, kind, joint/axis, asset plug, occupancy, and child
/// identity here so a stale or incompatible request cannot author a bad mount.
fn authored_text(
    data: &openusd::sdf::Data,
    prim: &openusd::sdf::Path,
    name: &str,
) -> Option<String> {
    data.field(
        &prim.append_property(name).ok()?,
        openusd::sdf::FieldKey::Default.as_str(),
    )?
    .as_str()
    .map(str::to_owned)
}

fn relationship_targets(
    data: &openusd::sdf::Data,
    prim: &openusd::sdf::Path,
    name: &str,
) -> Result<Vec<String>, String> {
    let property = prim
        .append_property(name)
        .map_err(|error| format!("invalid relationship {prim}.{name}: {error}"))?;
    path_list_targets(data, &property, "targetPaths", "relationship")
}

fn path_list_targets(
    data: &openusd::sdf::Data,
    property: &openusd::sdf::Path,
    field_name: &str,
    property_kind: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = data.field(property, field_name) else {
        return Ok(Vec::new());
    };
    let openusd::sdf::Value::PathListOp(op) = value else {
        return Err(format!(
            "{property_kind} {property} has a non-path {field_name} list"
        ));
    };
    Ok(op.iter().map(|path| path.as_str().to_string()).collect())
}

fn target_layer_authors_prim(
    document: &UsdDocument,
    edit_target: &LayerId,
    path: &openusd::sdf::Path,
) -> bool {
    let data = if edit_target.is_root() {
        document.data()
    } else if edit_target.is_runtime() {
        document.runtime_data()
    } else {
        return false;
    };
    data.spec(path)
        .is_some_and(|spec| spec.ty == openusd::sdf::SpecType::Prim)
}

fn validate_detach_component(
    world: &World,
    doc: DocumentId,
    spec: &lunco_usd_core::attach::DetachSpec,
) -> Result<(), String> {
    let component = openusd::sdf::Path::new(&spec.component_path)
        .map_err(|error| format!("invalid component path {}: {error}", spec.component_path))?;
    let joint = openusd::sdf::Path::new(&spec.joint_path)
        .map_err(|error| format!("invalid joint path {}: {error}", spec.joint_path))?;
    if component.is_property_path() || joint.is_property_path() {
        return Err("detach paths must name prims, not properties".into());
    }
    if component == joint
        || lunco_usd_bevy_stage::is_descendant_or_self(&joint, &spec.component_path)
    {
        return Err(format!(
            "joint {} must be separate from component subtree {}",
            spec.joint_path, spec.component_path
        ));
    }

    let registry = world
        .get_resource::<DocumentRegistry<UsdDocument>>()
        .ok_or_else(|| "USD document registry is unavailable".to_string())?;
    let host = registry
        .host(doc)
        .ok_or_else(|| format!("unknown USD document {doc}"))?;
    if !spec.edit_target.is_root() && !spec.edit_target.is_runtime() {
        return Err(format!("unknown edit target {}", spec.edit_target.as_str()));
    }
    if !target_layer_authors_prim(host.document(), &spec.edit_target, &component)
        || !target_layer_authors_prim(host.document(), &spec.edit_target, &joint)
    {
        return Err(format!(
            "component {} and joint {} must both be authored in edit target {}",
            spec.component_path,
            spec.joint_path,
            spec.edit_target.as_str()
        ));
    }

    let composed = host.document().composed();
    if composed.spec(&component).is_none() {
        return Err(format!("component {} does not exist", spec.component_path));
    }
    if composed.spec(&joint).is_none() {
        return Err(format!("joint {} does not exist", spec.joint_path));
    }
    if !lunco_usd_data::usd_data::has_authored_api_schema(
        &composed,
        &component,
        "LunCoMountAttachmentAPI",
    ) {
        return Err(format!(
            "component {} does not apply LunCoMountAttachmentAPI",
            spec.component_path
        ));
    }
    let attachment_joints =
        relationship_targets(&composed, &component, "lunco:mount:attachmentJoint")?;
    if attachment_joints != [spec.joint_path.clone()] {
        return Err(format!(
            "component {} records attachment joint {:?}, request supplied {}",
            spec.component_path, attachment_joints, spec.joint_path
        ));
    }
    let host_path = component
        .parent()
        .ok_or_else(|| format!("component {} has no host parent", spec.component_path))?;
    if joint.parent().as_ref() != Some(&host_path) {
        return Err(format!(
            "joint {} is not a sibling of component {} under host {}",
            spec.joint_path,
            spec.component_path,
            host_path.as_str()
        ));
    }
    if !lunco_usd_data::usd_data::has_authored_api_schema(
        &composed,
        &host_path,
        "PhysicsRigidBodyAPI",
    ) {
        return Err(format!(
            "host {} does not apply PhysicsRigidBodyAPI",
            host_path
        ));
    }
    let body0 = relationship_targets(&composed, &joint, "physics:body0")?;
    let body1 = relationship_targets(&composed, &joint, "physics:body1")?;
    if body0 != [host_path.as_str().to_string()] || body1 != [spec.component_path.clone()] {
        return Err(format!(
            "joint {} must relate body0={} and body1={}, got body0={body0:?}, body1={body1:?}",
            spec.joint_path, host_path, spec.component_path
        ));
    }

    let advertised_sockets = relationship_targets(&composed, &host_path, "lunco:mount:sockets")?;
    let mut occupied_by_component = Vec::new();
    for socket_target in &advertised_sockets {
        let socket = openusd::sdf::Path::new(socket_target).map_err(|error| {
            format!(
                "host {} advertises invalid socket {}: {error}",
                host_path, socket_target
            )
        })?;
        if !lunco_usd_data::usd_data::has_authored_api_schema(
            &composed,
            &socket,
            "LunCoMountSocketAPI",
        ) {
            continue;
        }
        let parts = relationship_targets(&composed, &socket, "lunco:mount:part")?;
        if parts.iter().any(|path| path == &spec.component_path) {
            occupied_by_component.push(socket_target.clone());
        }
    }
    match spec.socket_path.as_deref() {
        Some(socket_path) => {
            let socket = openusd::sdf::Path::new(socket_path)
                .map_err(|error| format!("invalid socket path {socket_path}: {error}"))?;
            if !advertised_sockets.iter().any(|path| path == socket_path) {
                return Err(format!(
                    "socket {socket_path} is not advertised by host {}",
                    host_path
                ));
            }
            let parts = relationship_targets(&composed, &socket, "lunco:mount:part")?;
            if parts != [spec.component_path.clone()] {
                return Err(format!(
                    "socket {socket_path} does not contain component {}",
                    spec.component_path
                ));
            }
            if occupied_by_component != [socket_path.to_string()] {
                return Err(format!(
                    "component {} has inconsistent socket occupancy {:?}",
                    spec.component_path, occupied_by_component
                ));
            }
        }
        None if !occupied_by_component.is_empty() => {
            return Err(format!(
                "component {} is occupied by socket(s) {:?}; detach requires the exact socket path",
                spec.component_path, occupied_by_component
            ));
        }
        None => {}
    }

    // Do not silently delete or rewrite external electrical/data/Modelica
    // relationships. Internal edges and the three known attachment edges are
    // safe; every other incoming edge is a caller-visible blocker.
    for (property_path, property) in composed.iter() {
        let (field_name, property_kind) = match property.ty {
            openusd::sdf::SpecType::Relationship => ("targetPaths", "relationship"),
            openusd::sdf::SpecType::Attribute => ("connectionPaths", "connection"),
            _ => continue,
        };
        let Some((owner, property_name)) = property_path.split_property() else {
            continue;
        };
        let targets = path_list_targets(&composed, property_path, field_name, property_kind)?;
        for target_raw in targets {
            let target = openusd::sdf::Path::new(&target_raw)
                .map_err(|error| format!("invalid {property_kind} target {target_raw}: {error}"))?;
            let target_prim = target.prim_path();
            let targets_removed = target_prim == joint
                || lunco_usd_bevy_stage::is_descendant_or_self(&target_prim, &spec.component_path);
            if !targets_removed {
                continue;
            }
            let internal =
                lunco_usd_bevy_stage::is_descendant_or_self(&owner, &spec.component_path)
                    || (owner == joint && property_name == "physics:body1" && target == component)
                    || (owner == component
                        && property_name == "lunco:mount:attachmentJoint"
                        && target == joint)
                    || (spec.socket_path.as_deref() == Some(owner.as_str())
                        && property_name == "lunco:mount:part"
                        && target == component);
            if !internal {
                return Err(format!(
                    "{property_kind} {property_path} points into detached component {}; remove that link first",
                    target_raw,
                ));
            }
        }
    }
    Ok(())
}

fn validate_attach_component(
    world: &World,
    doc: DocumentId,
    spec: &lunco_usd_core::attach::AttachSpec,
) -> Result<(), String> {
    if spec.placement.iter().any(|value| !value.is_finite())
        || spec.rotate_deg.iter().any(|value| !value.is_finite())
    {
        return Err("attachment transform contains a non-finite value".into());
    }
    let host_root = spec.host_path.trim_end_matches('/');
    let registry = world
        .get_resource::<DocumentRegistry<UsdDocument>>()
        .ok_or_else(|| "USD document registry is unavailable".to_string())?;
    let host = registry
        .host(doc)
        .ok_or_else(|| format!("unknown USD document {doc}"))?;
    if !spec.edit_target.is_root() && !spec.edit_target.is_runtime() {
        return Err(format!("unknown edit target {}", spec.edit_target.as_str()));
    }
    let composed = host.document().composed();

    let host_path = openusd::sdf::Path::new(host_root)
        .map_err(|error| format!("invalid host path {}: {error}", spec.host_path))?;
    if composed.spec(&host_path).is_none() {
        return Err(format!("host body {} does not exist", spec.host_path));
    }
    if !lunco_usd_data::usd_data::has_authored_api_schema(
        &composed,
        &host_path,
        "PhysicsRigidBodyAPI",
    ) {
        return Err(format!(
            "host {} does not apply PhysicsRigidBodyAPI",
            spec.host_path
        ));
    }

    if spec.name.is_empty()
        || spec.joint_name.is_empty()
        || spec.name.contains('/')
        || spec.joint_name.contains('/')
        || spec.name.contains('.')
        || spec.joint_name.contains('.')
    {
        return Err("component and joint names must be non-empty USD leaf identifiers".into());
    }
    let child = format!("{host_root}/{}", spec.name);
    let child_path = openusd::sdf::Path::new(&child)
        .map_err(|error| format!("invalid attached child path {child}: {error}"))?;
    if composed.spec(&child_path).is_some() {
        return Err(format!("attached child {child} already exists"));
    }
    let joint = format!("{host_root}/{}", spec.joint_name);
    let joint_path = openusd::sdf::Path::new(&joint)
        .map_err(|error| format!("invalid generated joint path {joint}: {error}"))?;
    if composed.spec(&joint_path).is_some() {
        return Err(format!("generated joint {joint} already exists"));
    }

    let Some(socket_path) = spec.socket_path.as_deref() else {
        return Ok(());
    };
    if !lunco_usd_data::usd_data::has_authored_api_schema(
        &composed,
        &host_path,
        "LunCoMountHostAPI",
    ) {
        return Err(format!(
            "host {} does not apply LunCoMountHostAPI",
            spec.host_path
        ));
    }
    let socket = openusd::sdf::Path::new(socket_path)
        .map_err(|error| format!("invalid socket path {socket_path}: {error}"))?;
    let advertised_sockets = relationship_targets(&composed, &host_path, "lunco:mount:sockets")?;
    if !advertised_sockets.iter().any(|path| path == socket_path) {
        return Err(format!(
            "socket {socket_path} is not advertised by host {}",
            spec.host_path
        ));
    }
    if composed.spec(&socket).is_none() {
        return Err(format!(
            "socket {socket_path} does not exist in document {doc}"
        ));
    }
    if !lunco_usd_data::usd_data::has_authored_api_schema(&composed, &socket, "LunCoMountSocketAPI")
    {
        return Err(format!(
            "socket {socket_path} does not apply LunCoMountSocketAPI"
        ));
    }
    let accepts = authored_text(&composed, &socket, "lunco:mount:socket")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("socket {socket_path} has no accepted plug kind"))?;
    let expected_joint = match &spec.joint {
        lunco_usd_core::attach::AttachJoint::Fixed => "fixed",
        lunco_usd_core::attach::AttachJoint::Revolute { .. } => "revolute",
        lunco_usd_core::attach::AttachJoint::Prismatic { .. } => "prismatic",
    };
    let actual_joint = authored_text(&composed, &socket, "lunco:mount:joint")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("socket {socket_path} has no lunco:mount:joint"))?;
    if actual_joint != expected_joint {
        return Err(format!(
            "socket {socket_path} requires joint {actual_joint}, request supplied {expected_joint}"
        ));
    }
    let requested_axis = match &spec.joint {
        lunco_usd_core::attach::AttachJoint::Fixed => None,
        lunco_usd_core::attach::AttachJoint::Revolute { axis }
        | lunco_usd_core::attach::AttachJoint::Prismatic { axis } => Some(match axis {
            lunco_usd_core::attach::Axis::X => "X",
            lunco_usd_core::attach::Axis::Y => "Y",
            lunco_usd_core::attach::Axis::Z => "Z",
        }),
    };
    let authored_axis =
        authored_text(&composed, &socket, "lunco:mount:axis").filter(|value| !value.is_empty());
    match (requested_axis, authored_axis.as_deref()) {
        (None, None) => {}
        (None, Some(axis)) => {
            return Err(format!(
                "fixed socket {socket_path} must not author axis {axis}"
            ));
        }
        (Some(expected), Some(actual)) if expected == actual => {}
        (Some(expected), actual) => {
            return Err(format!(
                "socket {socket_path} requires axis {actual:?}, request supplied {expected}"
            ));
        }
    }
    let occupied = relationship_targets(&composed, &socket, "lunco:mount:part")?;
    if occupied.len() > 1 {
        return Err(format!(
            "socket {socket_path} has multiple lunco:mount:part targets"
        ));
    }
    if let Some(existing) = occupied.into_iter().next() {
        let existing_path = openusd::sdf::Path::new(&existing).map_err(|error| {
            format!("socket {socket_path} has invalid lunco:mount:part target {existing}: {error}")
        })?;
        if !lunco_usd_bevy_stage::is_descendant_or_self(&existing_path, host_root) {
            return Err(format!(
                "socket {socket_path} points outside host body {host_root}"
            ));
        }
        return Err(format!(
            "socket {socket_path} is already occupied by {existing}"
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let schemes = world
            .get_resource::<lunco_assets_core::SchemeRegistry>()
            .ok_or_else(|| "asset scheme registry is unavailable".to_string())?;
        let local_asset = schemes
            .local_path(&spec.asset)
            .map_err(|error| format!("could not resolve attachment asset {}: {error}", spec.asset))?
            .ok_or_else(|| format!("attachment asset {} has no local file", spec.asset))?;
        let plug = lunco_usd_bevy_core::mount::read_asset_plug(&local_asset)
            .ok_or_else(|| format!("attachment asset {} has no valid mount plug", spec.asset))?;
        if plug.kind != accepts {
            return Err(format!(
                "attachment asset {} advertises plug {}, socket accepts {}",
                spec.asset, plug.kind, accepts
            ));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = accepts;
        return Err("socket attachment asset validation is unavailable on wasm".into());
    }
    Ok(())
}

#[on_command(AttachComponent)]
fn on_attach_component(
    trigger: On<AttachComponent>,
    mut commands: Commands,
    active_id: Option<Res<ActiveCommandId>>,
) {
    let doc = trigger.event().doc_id;
    let spec = trigger.event().spec.clone();
    let command_id = active_id.as_ref().and_then(|id| id.get());
    commands.queue(move |world: &mut World| {
        let outcome = match validate_attach_component(world, doc, &spec) {
            Err(error) => Err(error),
            Ok(()) => {
                let ops = lunco_usd_core::attach::attach_component_ops(&spec);
                let label = format!("Attach {} to {}", spec.name, spec.host_path);
                let (applied, total) = apply_ops_as_change_set(world, doc, label, ops);
                if applied != total {
                    Err(format!(
                        "attach {} applied {applied}/{total} operations",
                        spec.name
                    ))
                } else {
                    bevy::log::info!(
                        "[AttachComponent] {doc}: attached `{}` to `{}` ({total} ops, one change set)",
                        spec.name,
                        spec.host_path
                    );
                    Ok(Ack::with_data(
                        OpId::new(),
                        lunco_api_core::api_value!({
                            "component_path": format!("{}/{}", spec.host_path.trim_end_matches('/'), spec.name),
                            "joint_path": format!("{}/{}", spec.host_path.trim_end_matches('/'), spec.joint_name),
                        }),
                    ))
                }
            }
        };
        if let Err(error) = &outcome {
            bevy::log::warn!("[AttachComponent] attach rejected: {error}");
        }
        if let Some(command_id) = command_id {
            if let Some(mut results) = world.get_resource_mut::<CommandResults>() {
                results.record(command_id, outcome);
            }
        }
    });
}

#[on_command(DetachComponent)]
fn on_detach_component(
    trigger: On<DetachComponent>,
    mut commands: Commands,
    active_id: Option<Res<ActiveCommandId>>,
) {
    let command = trigger.event().clone();
    let command_id = active_id.as_ref().and_then(|id| id.get());
    commands.queue(move |world: &mut World| {
        let outcome = match validate_detach_component(world, command.doc_id, &command.spec) {
            Err(error) => Err(error),
            Ok(()) => {
                let ops = lunco_usd_core::attach::detach_component_ops(&command.spec);
                let label = format!("Detach {}", command.spec.component_path);
                let (applied, total) = apply_ops_as_change_set(world, command.doc_id, label, ops);
                if applied != total {
                    Err(format!(
                        "detach {} applied {applied}/{total} operations",
                        command.spec.component_path
                    ))
                } else {
                    bevy::log::info!(
                        "[DetachComponent] {}: removed `{}` and `{}` ({total} ops, one change set)",
                        command.doc_id,
                        command.spec.component_path,
                        command.spec.joint_path
                    );
                    Ok(Ack::with_data(
                        OpId::new(),
                        lunco_api_core::api_value!({
                            "component_path": command.spec.component_path,
                            "joint_path": command.spec.joint_path,
                            "socket_path": command.spec.socket_path,
                        }),
                    ))
                }
            }
        };
        if let Err(error) = &outcome {
            bevy::log::warn!("[DetachComponent] detach rejected: {error}");
        }
        if let Some(command_id) = command_id {
            if let Some(mut results) = world.get_resource_mut::<CommandResults>() {
                results.record(command_id, outcome);
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// AttachProgram — source-backed simulation program authoring
// ─────────────────────────────────────────────────────────────────────

#[on_command(AttachProgram)]
fn on_attach_program(trigger: On<AttachProgram>, mut commands: Commands) {
    let command = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let ops = match lunco_usd_core::program::program_attach_ops(&command.spec) {
            Ok(ops) => ops,
            Err(error) => {
                bevy::log::warn!(
                    "[AttachProgram] {} rejected before authoring: {}",
                    command.doc_id,
                    error
                );
                return;
            }
        };
        let label = format!(
            "Attach program {} to {}",
            command.spec.name, command.spec.host_path
        );
        let (applied, total) = apply_ops_as_change_set(world, command.doc_id, label, ops);
        if applied == total {
            bevy::log::info!(
                "[AttachProgram] {}: attached `{}` to `{}` ({total} ops, one change set)",
                command.doc_id,
                command.spec.name,
                command.spec.host_path
            );
        } else {
            bevy::log::warn!(
                "[AttachProgram] {} rejected during authoring: {applied}/{total} ops applied",
                command.doc_id
            );
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// SetDomeLight — HDRI environment, authored as a UsdLuxDomeLight
// ─────────────────────────────────────────────────────────────────────

/// Author the scene's HDRI environment: a `UsdLuxDomeLight` carrying
/// `inputs:texture:file`. Projected by `lunco_usd_bevy_light::dome` into a skybox +
/// image-based lighting.
///
/// **This is the only way to change the environment at runtime.** It lowers to
/// [`UsdOp`]s and goes through [`apply_ops_as_change_set`], so the edit saves,
/// journals, undoes as ONE unit, and replicates — exactly like any other USD
/// edit. Writing to the `Skybox`/`GeneratedEnvironmentMapLight` components
/// directly would light the local viewport and be invisible to all four of
/// those, which is the failure mode this command exists to prevent.
///
/// Idempotent: `AddPrim` is a `define_prim`, so re-issuing hot-replaces the
/// dome rather than stacking duplicates. Every field is `Option` — `None`
/// leaves the authored value alone, so a lighting tweak need not restate the
/// texture.
#[Command(default)]
pub struct SetDomeLight {
    /// Document to author into. `None` = the workspace's active document.
    pub doc_id: Option<DocumentId>,
    /// Prim path of the dome. `None` = `/World/Sky`.
    ///
    /// It must live **under the stage's `defaultPrim` subtree** (`/World` in
    /// every scene here) — a prim authored outside it composes into the layer
    /// but is never mounted, so the sky would silently not appear.
    pub path: Option<String>,
    /// `inputs:texture:file` — the HDRI, resolved relative to the stage layer
    /// (e.g. `../hdri/lunar_horizon_2k.hdr`). Equirectangular (`.hdr`, `.png`)
    /// or a `.ktx2` cubemap.
    pub texture: Option<String>,
    /// `inputs:intensity` — multiplier on the image (1.0 = as authored).
    pub intensity: Option<f32>,
    /// `inputs:exposure` — stops, applied as intensity × 2^exposure.
    pub exposure: Option<f32>,
    /// `inputs:color` — linear RGB tint multiplied into the image.
    pub color: Option<[f32; 3]>,
    /// `xformOp:rotateXYZ`, **degrees** — spins the environment. The usual case
    /// is yaw only (`[0, heading, 0]`).
    pub rotation: Option<[f32; 3]>,
    /// `lunco:dome:skybox` — `false` lights the scene from the HDRI but leaves
    /// the sky black. The lunar case: real bounce light, no visible sky.
    pub skybox: Option<bool>,
}

#[on_command(SetDomeLight)]
fn on_set_dome_light(
    trigger: On<SetDomeLight>,
    backed: Option<Res<lunco_usd_bevy_twin::DocBackedTwinScenes>>,
    asset_server: Res<AssetServer>,
    roots: Query<&UsdPrimPath, With<UsdSceneRoot>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // The running scene's root is the single entity that knows both things this
    // command needs: which document to author into, and which prim to author
    // under. Ask it for both, rather than counting registry entries (the
    // registry also holds terrain and script documents, so "the only one" is not
    // a thing that exists) or hardcoding `/World` (the sandbox scene is rooted
    // at `/SandboxScene`, and a prim authored under a non-existent parent
    // composes into the layer and is then never mounted — an invisible sky).
    let root = match roots.iter().collect::<Vec<_>>()[..] {
        [root] => root,
        [] => {
            bevy::log::warn!("[SetDomeLight] no scene is loaded — nothing to author a dome onto");
            return;
        }
        _ => {
            bevy::log::warn!(
                "[SetDomeLight] several scenes are mounted — pass `doc` and `path` explicitly"
            );
            return;
        }
    };

    let doc = match cmd.doc_id {
        Some(doc) => doc,
        None => {
            let Some(doc) = backed.as_ref().and_then(|b| {
                lunco_usd_bevy_twin::scene_document_for(b, &asset_server, root.stage_handle.id())
            }) else {
                bevy::log::warn!(
                    "[SetDomeLight] the running scene is a raw-file scene (not doc-backed), so it \
                     has no document to journal into — open it as a Twin, or pass `doc`"
                );
                return;
            };
            doc
        }
    };

    // Default the dome to a `Sky` prim directly under the scene's *mounted root*
    // — `/SandboxScene/Sky`, `/World/Sky`, … — which is inside the subtree the
    // stage actually mounts, and so is the one place a new prim is guaranteed to
    // compose AND appear.
    let path = cmd.path.clone().unwrap_or_else(|| {
        let root_path = root.path.trim_end_matches('/');
        format!("{root_path}/Sky")
    });

    // Split `/SandboxScene/Sky` → parent `/SandboxScene`, name `Sky`: `AddPrim`
    // takes them separately.
    let Some((parent, name)) = path.rsplit_once('/') else {
        bevy::log::warn!("[SetDomeLight] `{path}` is not an absolute prim path");
        return;
    };
    let parent = if parent.is_empty() { "/" } else { parent }.to_string();
    let name = name.to_string();
    if name.is_empty() {
        bevy::log::warn!("[SetDomeLight] `{path}` has no prim name");
        return;
    }

    let cmd = cmd.clone();
    commands.queue(move |world: &mut World| {
        let root = LayerId::root();
        let mut ops = vec![UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: parent,
            name,
            type_name: Some(ltok::T_DOME_LIGHT.into()),
            reference: None,
            reference_prim_path: None,
        }];

        // `SetAttribute`'s non-string branch parses `value` as a USDA literal,
        // so an asset path is spelled with its `@…@` delimiters and a color3f
        // as `(r, g, b)`. See the op's docs — this is the one place the
        // encoding is decided, and no call site hand-escapes.
        let mut attr = |name: &str, ty: &str, value: String| {
            ops.push(UsdOp::SetAttribute {
                edit_target: root.clone(),
                path: path.clone(),
                name: name.into(),
                type_name: ty.into(),
                value,
            });
        };
        if let Some(t) = &cmd.texture {
            attr(ltok::A_TEXTURE_FILE, "asset", format!("@{t}@"));
            // Be explicit rather than leaning on USD's `automatic`: it makes the
            // authored intent legible in the .usda, and `automatic` is what a
            // reader has to *guess* at.
            attr(ltok::A_TEXTURE_FORMAT, "token", "\"latlong\"".into());
        }
        if let Some(i) = cmd.intensity {
            attr(ltok::A_INTENSITY, "float", i.to_string());
        }
        if let Some(e) = cmd.exposure {
            attr(ltok::A_EXPOSURE, "float", e.to_string());
        }
        if let Some([r, g, b]) = cmd.color {
            attr(ltok::A_COLOR, "color3f", format!("({r}, {g}, {b})"));
        }
        if let Some(s) = cmd.skybox {
            attr("lunco:dome:skybox", "bool", s.to_string());
        }
        // Rotation is an xformOp, not a plain attribute: `SetRotate` also
        // authors `xformOpOrder` when the prim has none, which a bare
        // `SetAttribute` would not — the sky would then simply not rotate.
        if let Some([x, y, z]) = cmd.rotation {
            ops.push(UsdOp::SetRotate {
                edit_target: root.clone(),
                path: path.clone(),
                value: [x as f64, y as f64, z as f64],
            });
        }

        let (applied, n) = apply_ops_as_change_set(world, doc, "Set dome light", ops);
        bevy::log::info!(
            "[SetDomeLight] {doc}: authored `{path}` ({applied}/{n} ops, one change set)"
        );
    });
}

/// A3 auto-bridge: hand the [`JournalResource`](lunco_doc_bevy::JournalResource)
/// to the USD registry the moment it appears, so it fits a
/// [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder) onto existing and
/// future hosts. Edits — **including undo/redo** — then record losslessly with
/// no per-op code.
///
/// Reactive, not per-frame: gated by `resource_added`, so it runs once (the
/// frame the journal is installed) and never again. Headless builds without a
/// journal never run it.
fn wire_usd_journal_handle(
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    journal: Res<lunco_doc_bevy::JournalResource>,
) {
    registry.set_journal(journal.clone());
}

// ─────────────────────────────────────────────────────────────────────
// Pending-event drain — registry rings → trigger events
// ─────────────────────────────────────────────────────────────────────

/// Each frame, drain the registry's pending-event rings into the
/// canonical [`lunco_doc_bevy`] notification triggers.
///
/// Mirrors the publish-events system in `lunco-modelica-core`. Cheap
/// no-op when nothing is pending; gated implicitly by the
/// `Vec::is_empty` checks inside `drain_pending`.
fn drain_usd_pending_events(
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut commands: Commands,
) {
    let pending = registry.drain_pending();
    if pending.opened.is_empty() && pending.changed.is_empty() && pending.closed.is_empty() {
        return;
    }
    for doc in pending.opened {
        commands.trigger(DocumentOpened::local(doc));
    }
    for doc in pending.changed {
        commands.trigger(DocumentChanged::local(doc));
    }
    for doc in pending.closed {
        commands.trigger(DocumentClosed::local(doc));
    }
}

// ─────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod change_set_tests {
    //! **H10** — a multi-op command undoes as ONE unit.
    //!
    //! `AttachComponent` lowers to a compound `UsdOp` sequence. Each operation retains its
    //! lossless `(forward, inverse)` entry, while the change-set ID makes the
    //! complete attach one undo unit.
    use super::*;
    use lunco_doc_bevy::JournalResource;
    use lunco_twin_journal::{AuthorTag, UndoManager, UndoScope};
    use lunco_usd_core::attach::{attach_component_ops, AttachJoint, AttachSpec, Axis};
    use lunco_usd_document::document::LayerId;

    const RIG: &str =
        "#usda 1.0\ndef Xform \"Rig\"\n{\n    def Xform \"Chassis\"\n    {\n    }\n}\n";

    fn wheel_spec() -> AttachSpec {
        AttachSpec::new(
            LayerId::root(),
            "/Rig/Chassis",
            "Wheel",
            "constraint_47",
            "components/mobility/wheel.usda",
            [0.5, -0.3, 1.2],
            AttachJoint::Revolute { axis: Axis::X },
        )
    }

    /// A journal-wired world holding one open USD document.
    fn world_with_doc() -> (World, DocumentId, JournalResource) {
        let mut world = World::new();
        let journal = JournalResource::default();
        world.insert_resource(journal.clone());

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        // The A3 auto-bridge, done by hand (the system that does this in-app is
        // `wire_usd_journal_handle`): the recorder is what journals each op.
        registry.set_journal(journal.clone());
        let (doc, _) = registry.open_file("/tmp/lunco_h10_attach.usda", RIG.to_string());
        world.insert_resource(registry);
        (world, doc, journal)
    }

    #[test]
    fn attach_component_journals_one_change_set_and_undoes_as_one_unit() {
        let (mut world, doc, journal) = world_with_doc();
        let spec = wheel_spec();
        let ops = attach_component_ops(&spec);
        let n = ops.len();
        assert!(
            n > 1,
            "the attach lowering is multi-op — that is the whole finding"
        );

        let (applied, total) = apply_ops_as_change_set(&mut world, doc, "Attach Wheel", ops);
        assert_eq!(
            (applied, total),
            (n, n),
            "every op applies onto a valid host"
        );
        assert_eq!(
            world
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(doc)
                .expect("document host")
                .undo_depth(),
            1,
            "the lowered attach must occupy one document history group"
        );

        journal.with_read(|j| {
            let entries: Vec<_> = j.entries_for_doc(doc).collect();
            assert_eq!(entries.len(), n, "one journal entry per op (unchanged)");

            // THE FIX: they all belong to ONE change set.
            let cs = entries[0]
                .change_set
                .expect("the handler must open a change set — this is H10");
            assert!(
                entries.iter().all(|e| e.change_set == Some(cs)),
                "every op of the command must join the SAME change set"
            );
            assert_eq!(
                j.change_set_entries(cs).len(),
                n,
                "the change set groups all {n} ops"
            );

            // And the undo view takes the whole group: one undo, whole attach.
            let mut um = UndoManager::new(AuthorTag::local_user());
            for e in &entries {
                um.record_local(e.id.clone());
            }
            let group = um.take_undo_group(&UndoScope::Document(doc), j);
            assert_eq!(
                group.len(),
                n,
                "one undo must peel off the WHOLE attach, not 1-of-{n}"
            );
            assert!(
                !um.can_undo(),
                "nothing left behind — the attach was one unit"
            );
        });
    }

    #[test]
    fn rejected_compound_operation_does_not_partially_apply() {
        let (mut world, doc, journal) = world_with_doc();
        let ops = vec![
            UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: "/Rig/Chassis".to_owned(),
                name: "test:compoundValue".to_owned(),
                type_name: "float".to_owned(),
                value: "1.0".to_owned(),
            },
            UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: "/Rig/Missing".to_owned(),
                name: "test:compoundValue".to_owned(),
                type_name: "float".to_owned(),
                value: "2.0".to_owned(),
            },
        ];

        assert_eq!(
            apply_ops_as_change_set(&mut world, doc, "Invalid compound", ops),
            (0, 2)
        );
        assert_eq!(
            world
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(doc)
                .expect("document host")
                .generation(),
            0,
            "validation must happen before the live host is mutated"
        );
        journal.with_read(|j| {
            assert_eq!(j.entries_for_doc(doc).count(), 0);
        });
    }

    /// No journal (headless) — the ops still apply; there is simply nothing to
    /// group. The helper must not require a `JournalResource`.
    #[test]
    fn applies_without_a_journal() {
        let mut world = World::new();
        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file("/tmp/lunco_h10_nojournal.usda", RIG.to_string());
        world.insert_resource(registry);

        let ops = attach_component_ops(&wheel_spec());
        let n = ops.len();
        let (applied, total) = apply_ops_as_change_set(&mut world, doc, "Attach Wheel", ops);
        assert_eq!((applied, total), (n, n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_browser_load_is_cancelled_with_its_closed_twin() {
        let root = Path::new("/twins/rover");
        let path = root.join("scenes/rover.usda");
        assert!(pending_load_belongs_to_closed_twin(Some(root), &path, root));
        assert!(pending_load_belongs_to_closed_twin(None, &path, root));
        assert!(!pending_load_belongs_to_closed_twin(
            Some(Path::new("/twins/other")),
            &path,
            Path::new("/twins/new")
        ));
    }

    #[test]
    fn duplicate_usd_loads_share_one_pending_read_and_owner() {
        let root = PathBuf::from("/twins/rover");
        let path = root.join("scene.usda");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        spawn_usd_load(app.world_mut(), path.clone(), None);
        spawn_usd_load(app.world_mut(), path, Some(root.clone()));

        let pending = app.world().resource::<PendingUsdLoads>();
        assert_eq!(pending.tasks.len(), 1);
        assert_eq!(pending.tasks[0].twin_root.as_deref(), Some(root.as_path()));
    }

    #[test]
    fn closed_twin_drops_its_pending_browser_read() {
        let root = PathBuf::from("/twins/rover");
        let path = root.join("scene.usda");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();
        spawn_usd_load(app.world_mut(), path, Some(root.clone()));
        assert_eq!(app.world().resource::<PendingUsdLoads>().tasks.len(), 1);

        app.world_mut().trigger(TwinClosed {
            twin: lunco_workspace::TwinId::new(1),
            root,
            was_active: false,
        });
        assert!(app.world().resource::<PendingUsdLoads>().tasks.is_empty());
    }
}
