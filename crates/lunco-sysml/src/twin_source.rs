//! Twin-native SysML source loading and document lifecycle.
//!
//! A mounted Twin already owns an indexed file list and a `twin://` asset
//! authority. Its Rhai loading policy selects each source path; this module
//! validates that typed request, loads the file through `AssetServer`, and
//! opens the resulting bytes in the canonical SysML document registry. It
//! never walks the filesystem or parses a second copy of the source.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::asset::{AssetEvent, AssetLoadFailedEvent, AssetServer, Assets, Handle};
use bevy::prelude::*;

use lunco_assets_core::twin_uri;
use lunco_doc::{DocumentId, FileBacked, OpenOutcome};
use lunco_doc_bevy::{
    DocumentChanged, DocumentClosed, DocumentOpened, DocumentRegistry, DocumentSaved,
};
use lunco_workspace::{DocumentEntry, TwinClosed, WorkspaceResource};

use crate::{SysmlDocument, SysmlSource};

/// Structured runtime fault emitted when a selected Twin SysML source cannot
/// be loaded or opened. An absent optional SysML source set is not a fault.
pub const SYSML_TWIN_SOURCE_LOAD_FAILED: &str = "sysml-twin-source-load-failed";

/// One source file requested from a mounted Twin.
struct PendingSysmlSource {
    handle: Handle<SysmlSource>,
    twin_name: String,
    relative_path: String,
    absolute_path: PathBuf,
    twin_root: PathBuf,
}

/// Event-driven source/document state for mounted Twins.
///
/// `items` stays pending until the asset pipeline emits a terminal signal.
/// `owned_documents` records only clean documents allocated by this automatic
/// loader; a document already opened by a user is never claimed or discarded
/// by Twin teardown.
#[derive(Resource, Default)]
pub struct PendingSysmlSources {
    items: Vec<PendingSysmlSource>,
    ready: HashSet<bevy::asset::AssetId<SysmlSource>>,
    failed: HashMap<bevy::asset::AssetId<SysmlSource>, String>,
    owned_documents: HashMap<DocumentId, PathBuf>,
}

impl PendingSysmlSources {
    fn mark_ready(&mut self, id: bevy::asset::AssetId<SysmlSource>) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.ready.insert(id);
        }
    }

    fn mark_failed(&mut self, id: bevy::asset::AssetId<SysmlSource>, error: String) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.failed.insert(id, error);
        }
    }

    fn release_root(&mut self, root: &Path) {
        self.items
            .retain(|item| !lunco_doc::same_file(&item.twin_root, root));
        let live: HashSet<_> = self.items.iter().map(|item| item.handle.id()).collect();
        self.ready.retain(|id| live.contains(id));
        self.failed.retain(|id, _| live.contains(id));
    }
}

/// Request one Twin-relative SysML source selected by the active Rhai loading
/// policy. Rust validates the path and performs the asynchronous asset/document
/// operation; source-set selection belongs to the Twin policy.
#[lunco_core::Command(default)]
pub struct LoadTwinSysmlSource {
    /// Workspace identity of the Twin whose source authority was mounted.
    pub twin_id: u64,
    /// Exact `twin://` authority returned by the asset owner.
    pub name: String,
    /// Indexed `.sysml` or `.kerml` path relative to the Twin root.
    pub relative_path: String,
}

#[lunco_core::on_command(LoadTwinSysmlSource)]
fn load_twin_sysml_source(
    trigger: On<LoadTwinSysmlSource>,
    workspace: Option<Res<WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<SysmlSource>>>,
    mut pending: ResMut<PendingSysmlSources>,
) -> Result<lunco_command_contracts::Ack, String> {
    let request = trigger.event();
    let workspace = workspace.ok_or_else(|| "WorkspaceResource is not installed".to_owned())?;
    let twin_id = lunco_workspace::TwinId::new(request.twin_id);
    let twin = workspace
        .twin(twin_id)
        .ok_or_else(|| format!("workspace Twin {} is unavailable", request.twin_id))?;
    if workspace.active_twin != Some(twin_id) {
        return Err(format!("Twin {} is not active", request.twin_id));
    }
    let relative = Path::new(&request.relative_path);
    let extension = relative
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if !lunco_assets_path::is_safe_relative_path(&request.relative_path)
        || relative == Path::new(".")
        || !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        || !matches!(extension.as_deref(), Some("sysml" | "kerml"))
    {
        return Err(format!(
            "Twin SysML path `{}` must be a safe Twin-relative .sysml or .kerml file",
            request.relative_path
        ));
    }
    if !twin
        .files()
        .iter()
        .any(|entry| entry.relative_path.as_path() == relative)
    {
        return Err(format!(
            "Twin SysML path `{}` is not indexed",
            request.relative_path
        ));
    }
    if !roots
        .as_deref()
        .and_then(|roots| roots.name_for_root(&twin.root).ok().flatten())
        .is_some_and(|name| name == request.name)
    {
        return Err(format!(
            "Twin asset authority `{}` does not belong to Twin {}",
            request.name, request.twin_id
        ));
    }
    let asset_server = asset_server.ok_or_else(|| "AssetServer is not installed".to_owned())?;
    let twin_root = twin.root.clone();
    if pending
        .items
        .iter()
        .any(|item| item.twin_root == twin_root && item.relative_path == request.relative_path)
    {
        return Ok(lunco_command_contracts::Ack::new(
            lunco_command_contracts::OpId::new(),
        ));
    }

    let uri = twin_uri(&request.name, &request.relative_path);
    let handle = asset_server.load::<SysmlSource>(uri);
    let id = handle.id();
    if assets
        .as_deref()
        .is_some_and(|loaded| loaded.get(id).is_some())
    {
        pending.ready.insert(id);
    }
    let source_failed = asset_server
        .get_load_state(id)
        .is_some_and(|state| state.is_failed());
    pending.items.push(PendingSysmlSource {
        handle,
        twin_name: request.name.clone(),
        relative_path: request.relative_path.clone(),
        absolute_path: twin_root.join(relative),
        twin_root,
    });
    if source_failed {
        pending.mark_failed(id, "the source asset had already failed to load".into());
    }
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

lunco_core::register_commands!(load_twin_sysml_source);

pub(crate) fn register_twin_source_commands(app: &mut App) {
    register_all_commands(app);
}

/// Convert source asset lifecycle messages into terminal pending state.
pub(crate) fn mark_pending_sysml_sources(
    mut pending: ResMut<PendingSysmlSources>,
    mut events: MessageReader<AssetEvent<SysmlSource>>,
    mut failures: MessageReader<AssetLoadFailedEvent<SysmlSource>>,
) {
    for event in events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => pending.mark_ready(*id),
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => pending.mark_failed(
                *id,
                "the Twin SysML source asset was removed before opening".to_owned(),
            ),
        }
    }
    for failure in failures.read() {
        pending.mark_failed(failure.id, failure.error.to_string());
    }
}

/// Open ready Twin sources through the one file-backed document registry.
pub(crate) fn drain_pending_sysml_sources(
    mut pending: ResMut<PendingSysmlSources>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
    assets: Res<Assets<SysmlSource>>,
    mut commands: Commands,
) {
    if pending.items.is_empty() {
        return;
    }
    let ready = std::mem::take(&mut pending.ready);
    let failed = std::mem::take(&mut pending.failed);
    let items = std::mem::take(&mut pending.items);
    let mut still_pending = Vec::new();

    for item in items {
        let id = item.handle.id();
        if let Some(error) = failed.get(&id) {
            report_source_error(
                &mut commands,
                &item.twin_name,
                format!("{}: {error}", item.relative_path),
            );
            continue;
        }
        if !ready.contains(&id) {
            still_pending.push(item);
            continue;
        }
        let Some(source) = assets.get(&item.handle) else {
            report_source_error(
                &mut commands,
                &item.twin_name,
                format!(
                    "{} emitted a ready event without a stored SysML source asset",
                    item.relative_path
                ),
            );
            continue;
        };

        let (document, outcome) =
            registry.open_file(item.absolute_path.clone(), source.text.to_string());
        info!(
            "[sysml] opened Twin source `{}/{}` as document {:?} ({outcome:?})",
            item.twin_name, item.relative_path, document
        );
        match outcome {
            OpenOutcome::KeptUnparsable => {
                report_source_error(
                    &mut commands,
                    &item.twin_name,
                    format!("{} is not a valid SysML source", item.relative_path),
                );
            }
            OpenOutcome::KeptDirty => {
                warn!(
                    "[sysml] `{}/{}` has unsaved edits; keeping the dirty document",
                    item.twin_name, item.relative_path
                );
            }
            OpenOutcome::Allocated => {
                pending
                    .owned_documents
                    .insert(document, item.twin_root.clone());
            }
            OpenOutcome::Refreshed => {}
        }
    }
    pending.items = still_pending;
}

/// Close clean documents allocated by the Twin source loader when that Twin
/// closes. Dirty documents remain available as loose user work.
pub(crate) fn release_twin_sysml_sources(
    trigger: On<TwinClosed>,
    mut pending: ResMut<PendingSysmlSources>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
) {
    let root = &trigger.event().root;
    pending.release_root(root);
    let owned: Vec<_> = pending
        .owned_documents
        .iter()
        .filter(|(_, document_root)| lunco_doc::same_file(document_root, root))
        .map(|(document, _)| *document)
        .collect();
    for document in owned {
        let remove = registry
            .host(document)
            .map(|host| !host.document().is_dirty())
            .unwrap_or(true);
        if remove {
            registry.remove(document);
            pending.owned_documents.remove(&document);
        } else {
            warn!(
                "[sysml] retaining dirty Twin-owned document {:?} after Twin close",
                document
            );
            // The user now owns this loose document. Do not retain an automatic
            // lease that could delete it later if it becomes clean.
            pending.owned_documents.remove(&document);
        }
    }
}

/// Drain the SysML registry's lifecycle ring into the shared document events.
pub(crate) fn drain_sysml_document_events(
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
    mut commands: Commands,
) {
    let pending = registry.drain_pending();
    for document in pending.opened {
        commands.trigger(DocumentOpened::local(document));
    }
    for document in pending.changed {
        commands.trigger(DocumentChanged::local(document));
    }
    for document in pending.closed {
        commands.trigger(DocumentClosed::local(document));
    }
}

pub(crate) fn sync_workspace_on_sysml_doc_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<SysmlDocument>>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let document = trigger.event().doc;
    let Some(host) = registry.host(document) else {
        return;
    };
    if workspace.document(document).is_some() {
        return;
    }
    let origin = host.document().origin().clone();
    let context_twin = origin
        .is_untitled()
        .then_some(workspace.active_twin)
        .flatten();
    workspace.add_document(DocumentEntry {
        id: document,
        kind: lunco_workspace::DocumentKindId::new("sysml"),
        origin: origin.clone(),
        context_twin,
        title: origin.display_name(),
        dirty: host.document().is_dirty(),
    });
}

pub(crate) fn sync_workspace_on_sysml_doc_changed(
    trigger: On<DocumentChanged>,
    registry: Res<DocumentRegistry<SysmlDocument>>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let document = trigger.event().doc;
    let Some(entry) = workspace.document_mut(document) else {
        return;
    };
    if let Some(host) = registry.host(document) {
        entry.dirty = host.document().is_dirty();
        entry.origin = host.document().origin().clone();
        entry.title = entry.origin.display_name();
    }
}

pub(crate) fn sync_workspace_on_sysml_doc_saved(
    trigger: On<DocumentSaved>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    if let Some(entry) = workspace.document_mut(trigger.event().doc) {
        entry.dirty = false;
    }
}

pub(crate) fn sync_workspace_on_sysml_doc_closed(
    trigger: On<DocumentClosed>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    if let Some(mut workspace) = workspace {
        workspace.close_document(trigger.event().doc);
    }
}

fn report_source_error(commands: &mut Commands, twin_name: &str, detail: impl Into<String>) {
    let detail = detail.into();
    let message = format!("Twin `{twin_name}` SysML source load failed: {detail}");
    error!("[sysml] {message}");
    lunco_core::trigger_runtime_error(commands, SYSML_TWIN_SOURCE_LOAD_FAILED, message);
}
