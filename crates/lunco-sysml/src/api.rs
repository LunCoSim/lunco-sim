//! Transport-neutral SysML document editing and inspection.
//!
//! The command is intentionally small: SysML source remains the canonical
//! artifact and the document host owns parsing, generation checks, journaling,
//! and undo/redo. Rhai tools provide the authoring UX; this module only lowers
//! their typed intent into the generic document registry.

use bevy::prelude::*;
use lunco_api::diagnostics::diagnostic_api_value;
use lunco_api::{ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult, api_param_u64};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_command_contracts::Ack;
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Document, DocumentId, FileBacked, OpenOutcome};
use lunco_doc_bevy::{
    CloseDocument, DocumentRegistry, DocumentSaved, NewDocument, OpenFile, RedoDocument,
    SaveAsDocument, SaveDocument, UndoDocument,
};
use lunco_storage::Storage;

use crate::{SysmlDocument, SysmlOp};

/// One source-level SysML edit admitted by the Rhai authoring tools.
///
/// Byte offsets are deliberately carried as unsigned integers at the API
/// boundary and checked before conversion to `usize`; a source edit never
/// travels through `f64` or a signed sentinel.
#[derive(Reflect, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SysmlApiOp {
    /// Replace the complete source buffer.
    ReplaceSource {
        /// New UTF-8 source text.
        source: String,
    },
    /// Replace one UTF-8 byte range in the current source.
    EditText {
        /// Inclusive-start byte offset.
        range_start: u64,
        /// Exclusive-end byte offset.
        range_end: u64,
        /// Replacement UTF-8 text.
        replacement: String,
    },
}

/// Apply a validated SysML source edit group as one journal/undo unit.
#[Command(default)]
pub struct ApplySysmlOps {
    /// Explicit SysML document id.
    pub doc_id: DocumentId,
    /// Ordered source operations; the document host validates the complete
    /// group before committing any of them.
    pub ops: Vec<SysmlApiOp>,
    /// Optional optimistic-concurrency cursor from `InspectSysmlDocument`.
    #[serde(default)]
    pub parent_generation: Option<u64>,
}

/// Persist one SysML document without colliding with the domain-generic
/// `SaveDocument` command.  The shared verb is still useful to UI code, but
/// the transport-facing Rhai editor needs an owner-specific terminal command
/// while multiple document domains observe the same generic event.
#[Command(default)]
pub struct SaveSysmlDocument {
    /// Explicit SysML document id.
    pub doc_id: DocumentId,
}

/// Register the SysML command and query adapters.
pub struct SysmlApiPlugin;

#[derive(Resource, Default)]
struct PendingSysmlOpens {
    tasks: Vec<PendingSysmlOpen>,
}

#[derive(PartialEq, Eq)]
enum SysmlOpenRequest {
    File {
        path: std::path::PathBuf,
        admission: lunco_workspace::FileDocumentAdmission,
    },
    Browser {
        request: Entity,
    },
}

enum PreparedSysmlOpen {
    File(lunco_workspace::ResolvedFileDocument, String),
    Browser {
        id: DocumentId,
        document: SysmlDocument,
    },
}

struct PendingSysmlOpen {
    request: SysmlOpenRequest,
    task: bevy::tasks::Task<Result<PreparedSysmlOpen, String>>,
}

register_commands!(
    on_close_sysml_document,
    on_open_sysml_file,
    on_new_sysml_document,
    on_apply_sysml_ops,
    on_save_sysml_document_explicit,
    on_undo_sysml_document,
    on_redo_sysml_document,
    on_save_sysml_document,
    on_save_as_sysml_document
);

impl Plugin for SysmlApiPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SysmlApiOp>();
        register_all_commands(app);
        lunco_api::add_plugin_once::<lunco_api::ApiQueryRegistryPlugin>(
            app,
            lunco_api::ApiQueryRegistryPlugin,
        );
        app.init_resource::<PendingSysmlOpens>()
            .add_systems(Update, drain_pending_sysml_opens)
            .add_observer(on_browser_sysml_file);
        app.world_mut()
            .resource_mut::<ApiQueryRegistry>()
            .register(InspectSysmlDocumentProvider);
    }
}

/// Route a filesystem `.sysml`/`.kerml` open through the async storage path.
/// Other URI schemes and extensions belong to their owning domain observers.
#[on_command(OpenFile)]
fn on_open_sysml_file(
    trigger: On<OpenFile>,
    mut pending: ResMut<PendingSysmlOpens>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    let raw = &trigger.event().path;
    let path = match lunco_storage::file_uri_to_path(raw) {
        Ok(Some(path)) => path,
        Ok(None) if lunco_assets_core::has_scheme(raw) => return,
        Ok(None) => std::path::PathBuf::from(raw),
        Err(error) => {
            warn!("[SysmlOpenFile] {error}");
            return;
        }
    };
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(extension.as_deref(), Some("sysml" | "kerml")) {
        return;
    }
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let admission = lunco_workspace::FileDocumentAdmission::capture(
        workspace.as_deref().map(|workspace| &workspace.0),
        replication.as_ref(),
    );
    if pending
        .tasks
        .iter()
        .any(|load| matches!(&load.request, SysmlOpenRequest::File { path: captured_path, admission: captured } if captured_path == &path && captured == &admission))
    {
        return;
    }
    let task_path = path.clone();
    let task_admission = admission.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let (resolved, bytes) = task_admission.read(&task_path).await?;
        let source = String::from_utf8(bytes)
            .map_err(|error| format!("invalid UTF-8 in {}: {error}", resolved.path.display()))?;
        Ok(PreparedSysmlOpen::File(resolved, source))
    });
    pending.tasks.push(PendingSysmlOpen {
        request: SysmlOpenRequest::File { path, admission },
        task,
    });
}

fn prepare_browser_sysml_document(
    id: DocumentId,
    display_name: String,
    bytes: &[u8],
) -> Result<SysmlDocument, String> {
    let source = std::str::from_utf8(bytes)
        .map_err(|error| format!("invalid UTF-8 in browser file `{display_name}`: {error}"))?
        .to_owned();
    Ok(SysmlDocument::with_origin(
        id,
        source,
        lunco_doc::PathlessOrigin::untitled(display_name).into(),
    ))
}

fn on_browser_sysml_file(
    trigger: On<lunco_workbench_file_dialog::PickResolved>,
    requests: Query<&lunco_workbench_file_dialog::PickInFlight>,
    registry: Res<DocumentRegistry<SysmlDocument>>,
    mut pending: ResMut<PendingSysmlOpens>,
) {
    use lunco_workbench_file_dialog::{PickFollowUp, PickedPath};
    let event = trigger.event();
    let Ok(request) = requests.get(event.request) else {
        return;
    };
    if !matches!(event.follow_up, PickFollowUp::OpenFile)
        || !matches!(request.follow_up, PickFollowUp::OpenFile)
    {
        return;
    }
    let PickedPath::BrowserFile {
        display_name,
        bytes,
    } = &event.result
    else {
        return;
    };
    let extension = std::path::Path::new(display_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(extension.as_deref(), Some("sysml" | "kerml")) {
        return;
    }
    let identity = SysmlOpenRequest::Browser {
        request: event.request,
    };
    if pending.tasks.iter().any(|load| load.request == identity) {
        return;
    }
    let id = registry.reserve_id();
    let display_name = display_name.clone();
    let bytes = bytes.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        std::panic::catch_unwind(|| prepare_browser_sysml_document(id, display_name, &bytes))
            .unwrap_or_else(|_| Err("browser SysML source preparation panicked".to_owned()))
            .map(|document| PreparedSysmlOpen::Browser { id, document })
    });
    pending.tasks.push(PendingSysmlOpen {
        request: identity,
        task,
    });
}

/// Close the SysML document for every shared `CloseDocument` request,
/// including Twin teardown and workspace replacement.
#[on_command(CloseDocument)]
fn on_close_sysml_document(
    trigger: On<CloseDocument>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
) {
    registry.remove_document(trigger.event().doc_id);
}

/// Finish pending source reads on the ECS thread and let the registry decide
/// whether a clean open document may refresh or a dirty one must be retained.
fn drain_pending_sysml_opens(
    mut pending: ResMut<PendingSysmlOpens>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut commands: Commands,
) {
    if pending.tasks.is_empty() {
        return;
    }
    let tasks = std::mem::take(&mut pending.tasks);
    let mut waiting = Vec::new();
    for mut load in tasks {
        match bevy::tasks::futures_lite::future::block_on(
            bevy::tasks::futures_lite::future::poll_once(&mut load.task),
        ) {
            None => waiting.push(load),
            Some(Err(error)) => {
                error!("[sysml] {error}");
                if matches!(load.request, SysmlOpenRequest::Browser { .. }) {
                    lunco_core::trigger_runtime_error(
                        &mut commands,
                        "sysml-browser-open-failed",
                        error,
                    );
                }
            }
            Some(Ok(PreparedSysmlOpen::File(resolved, source))) => {
                let replication = lunco_core_session::current_replication_owner(
                    connection.as_deref(),
                    replica.as_deref(),
                );
                if !resolved.runtime.is_current(
                    workspace.as_deref().map(|workspace| &workspace.0),
                    replication.as_ref(),
                ) {
                    warn!(
                        "[sysml] {} belongs to a retired runtime owner",
                        resolved.path.display()
                    );
                    continue;
                }
                if let Some(doc) = registry.doc_for_file(&resolved.path)
                    && registry
                        .host(doc)
                        .is_some_and(|host| host.document().is_dirty())
                    && workspace.as_deref().is_some_and(|workspace| {
                        workspace
                            .document(doc)
                            .is_none_or(|entry| entry.runtime_context != resolved.runtime)
                    })
                {
                    warn!(
                        "[sysml] refusing to rebind dirty document {doc} to a different runtime owner"
                    );
                    continue;
                }
                let (doc, outcome) = registry.open_file(resolved.path.clone(), source);
                if outcome != OpenOutcome::KeptUnparsable
                    && let Some(workspace) = workspace.as_deref_mut()
                    && let Some(host) = registry.host(doc)
                {
                    let origin = host.document().origin().clone();
                    workspace.add_document(lunco_workspace::DocumentEntry {
                        id: doc,
                        kind: lunco_workspace::DocumentKindId::new("sysml"),
                        title: origin.display_name(),
                        origin,
                        runtime_context: resolved.runtime,
                        dirty: host.document().is_dirty(),
                    });
                }
                match outcome {
                    OpenOutcome::Allocated => {
                        info!("[sysml] opened {} as {doc}", resolved.path.display())
                    }
                    OpenOutcome::Refreshed => {
                        info!("[sysml] refreshed {} ({doc})", resolved.path.display())
                    }
                    OpenOutcome::KeptDirty => {
                        warn!("[sysml] kept dirty document {doc}; disk not reloaded")
                    }
                    OpenOutcome::KeptUnparsable => {
                        warn!("[sysml] kept {doc}; source is not valid SysML")
                    }
                }
            }
            Some(Ok(PreparedSysmlOpen::Browser { id, document })) => {
                if let Err(error) = registry.install_prebuilt(id, document) {
                    warn!("[SysmlBrowserOpen] {error}");
                    lunco_core::trigger_runtime_error(
                        &mut commands,
                        "sysml-browser-open-failed",
                        error.to_string(),
                    );
                    continue;
                }
                if let Some(workspace) = workspace.as_deref_mut()
                    && let Some(host) = registry.host(id)
                {
                    let origin = host.document().origin().clone();
                    workspace.add_document(lunco_workspace::DocumentEntry {
                        id,
                        kind: lunco_workspace::DocumentKindId::new("sysml"),
                        title: origin.display_name(),
                        origin,
                        runtime_context: lunco_workspace::DocumentRuntimeOwner::Application,
                        dirty: host.document().is_dirty(),
                    });
                }
            }
        }
    }
    pending.tasks = waiting;
}

/// Create a minimal editable SysML source document for File → New.
#[on_command(NewDocument)]
fn on_new_sysml_document(
    trigger: On<NewDocument>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    if trigger.event().kind != "sysml" {
        return;
    }
    let next = registry.ids().count() + 1;
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let runtime = workspace.as_deref().map_or(
        lunco_workspace::DocumentRuntimeOwner::Application,
        |workspace| workspace.new_document_runtime_owner(replication.as_ref()),
    );
    if !runtime.is_current(
        workspace.as_deref().map(|workspace| &workspace.0),
        replication.as_ref(),
    ) {
        warn!("[sysml] document creation owner is retired");
        return;
    }
    let doc = registry.allocate(
        "package Untitled {\n}\n".to_owned(),
        lunco_doc::PathlessOrigin::untitled(format!("Untitled-{next}.sysml")),
    );
    if let Some(mut workspace) = workspace {
        if let Some(host) = registry.host(doc) {
            let origin = host.document().origin().clone();
            workspace.add_document(lunco_workspace::DocumentEntry {
                id: doc,
                kind: lunco_workspace::DocumentKindId::new("sysml"),
                title: origin.display_name(),
                origin,
                runtime_context: runtime,
                dirty: host.document().is_dirty(),
            });
        }
    }
}

/// Lower one Rhai/API batch into the generic reversible SysML document host.
#[on_command(ApplySysmlOps)]
fn on_apply_sysml_ops(
    trigger: On<ApplySysmlOps>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
) -> Result<Ack, String> {
    let request = trigger.event();
    let doc_id = request.doc_id;
    let parent_generation = request.parent_generation;
    let mut ops = Vec::with_capacity(request.ops.len());
    for (index, op) in request.ops.iter().enumerate() {
        let converted = match op {
            SysmlApiOp::ReplaceSource { source } => SysmlOp::ReplaceSource {
                new: source.clone(),
            },
            SysmlApiOp::EditText {
                range_start,
                range_end,
                replacement,
            } => {
                let start = usize::try_from(*range_start).map_err(|_| {
                    format!("ApplySysmlOps: operation {index} start offset exceeds usize")
                })?;
                let end = usize::try_from(*range_end).map_err(|_| {
                    format!("ApplySysmlOps: operation {index} end offset exceeds usize")
                })?;
                SysmlOp::EditText {
                    range: start..end,
                    replacement: replacement.clone(),
                }
            }
        };
        ops.push(converted);
    }
    if !registry.contains(doc_id) {
        return Err(format!("ApplySysmlOps: unknown SysML document {doc_id}"));
    }
    let count = ops.len();
    let mut ack = registry
        .apply_group_against(doc_id, parent_generation, ops)
        .map_err(|reject| format!("ApplySysmlOps: {reject}"))?;
    ack.data = Some(lunco_api_core::api_value!({
        "operations": count,
        "doc_id": doc_id.raw(),
    }));
    Ok(ack)
}

/// Persist a SysML document through its owning registry and return a typed
/// command result to Rhai/API callers.
#[on_command(SaveSysmlDocument)]
fn on_save_sysml_document_explicit(
    trigger: On<SaveSysmlDocument>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let doc_id = trigger.event().doc_id;
    let Some(host) = registry.host(doc_id) else {
        return Err(format!(
            "SaveSysmlDocument: unknown SysML document {doc_id}"
        ));
    };
    let document = host.document();
    let Some(path) = document
        .origin()
        .canonical_path()
        .map(std::path::Path::to_path_buf)
    else {
        return Err(format!(
            "SaveSysmlDocument: {doc_id} has no file path; SaveAsDocument is required"
        ));
    };
    if !document.origin().is_writable() {
        return Err(format!("SaveSysmlDocument: {doc_id} is read-only"));
    }
    let source = document.source().to_owned();
    let generation = document.generation();
    let storage = lunco_storage::FileStorage::new();
    let handle = lunco_storage::StorageHandle::File(path.clone());
    storage
        .write_sync(&handle, source.as_bytes())
        .map_err(|error| {
            format!(
                "SaveSysmlDocument: save {} failed: {error:?}",
                path.display()
            )
        })?;
    if let Some(host) = registry.host_mut(doc_id) {
        host.document_mut().mark_saved();
    }
    registry.note_saved(doc_id);
    commands.trigger(DocumentSaved::local(doc_id));
    Ok(Ack {
        data: Some(lunco_api_core::api_value!({
            "doc_id": doc_id.raw(),
            "path": path.display().to_string(),
            "generation": generation,
            "action": "saved",
        })),
        ..Default::default()
    })
}

/// Undo the most recent SysML history group through the shared document verb.
#[on_command(UndoDocument)]
fn on_undo_sysml_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
) {
    let doc_id = trigger.event().doc_id;
    let Some(host) = registry.host_mut(doc_id) else {
        return;
    };
    match host.undo() {
        Ok(true) => registry.mark_changed(doc_id),
        Ok(false) => info!("[sysml] nothing to undo on {doc_id}"),
        Err(error) => warn!("[sysml] undo failed on {doc_id}: {error:?}"),
    }
}

/// Redo the most recently undone SysML history group through the shared verb.
#[on_command(RedoDocument)]
fn on_redo_sysml_document(
    trigger: On<RedoDocument>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
) {
    let doc_id = trigger.event().doc_id;
    let Some(host) = registry.host_mut(doc_id) else {
        return;
    };
    match host.redo() {
        Ok(true) => registry.mark_changed(doc_id),
        Ok(false) => info!("[sysml] nothing to redo on {doc_id}"),
        Err(error) => warn!("[sysml] redo failed on {doc_id}: {error:?}"),
    }
}

/// Persist a writable file-backed SysML document through `lunco-storage`.
#[on_command(SaveDocument)]
fn on_save_sysml_document(
    trigger: On<SaveDocument>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
    mut commands: Commands,
) {
    let doc_id = trigger.event().doc_id;
    let Some(host) = registry.host(doc_id) else {
        return;
    };
    let document = host.document();
    let Some(path) = document
        .origin()
        .canonical_path()
        .map(std::path::Path::to_path_buf)
    else {
        warn!("[sysml] {doc_id} has no file path; Save-As is required");
        return;
    };
    if !document.origin().is_writable() {
        warn!("[sysml] {doc_id} is read-only");
        return;
    }
    let source = document.source().to_owned();
    let storage = lunco_storage::FileStorage::new();
    let handle = lunco_storage::StorageHandle::File(path.clone());
    if let Err(error) = storage.write_sync(&handle, source.as_bytes()) {
        error!("[sysml] save {} failed: {error:?}", path.display());
        return;
    }
    if let Some(host) = registry.host_mut(doc_id) {
        host.document_mut().mark_saved();
    }
    registry.note_saved(doc_id);
    commands.trigger(DocumentSaved::local(doc_id));
}

/// Persist a SysML document to an explicit new path and rebind its identity.
/// An empty path is rejected here; a UI may resolve a picker and reissue the
/// same command with the chosen path.
#[on_command(SaveAsDocument)]
fn on_save_as_sysml_document(
    trigger: On<SaveAsDocument>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
    mut commands: Commands,
) {
    let doc_id = trigger.event().doc_id;
    let target = trigger.event().path.trim();
    if target.is_empty() {
        warn!("[sysml] Save-As for {doc_id} requires an explicit destination path");
        return;
    }
    let Some(host) = registry.host(doc_id) else {
        return;
    };
    if host.document().origin().is_read_only() {
        warn!("[sysml] Save-As blocked for read-only document {doc_id}");
        return;
    }
    let source = host.document().source().to_owned();
    let path = std::path::PathBuf::from(target);
    let storage = lunco_storage::FileStorage::new();
    let handle = lunco_storage::StorageHandle::File(path.clone());
    if let Err(error) = storage.write_sync(&handle, source.as_bytes()) {
        error!("[sysml] Save-As {} failed: {error:?}", path.display());
        return;
    }
    if let Some(host) = registry.host_mut(doc_id) {
        host.document_mut()
            .set_origin(lunco_doc::DocumentOrigin::File {
                path: path.clone(),
                writable: true,
            });
        host.document_mut().mark_saved();
    }
    registry.note_saved(doc_id);
    commands.trigger(DocumentSaved::local(doc_id));
}

/// Inspect one open SysML document without copying the source into a second
/// registry. The semantic report remains available through `ValidateSysml`;
/// this query supplies the document identity/generation needed by editors.
struct InspectSysmlDocumentProvider;

impl ApiQueryProvider for InspectSysmlDocumentProvider {
    fn name(&self) -> &'static str {
        "InspectSysmlDocument"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(doc_id) = api_param_u64(params, "doc_id").map(DocumentId::new) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectSysmlDocument requires an explicit numeric `doc_id`",
            ));
        };
        let Some(registry) = world.get_resource::<DocumentRegistry<SysmlDocument>>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "InspectSysmlDocument requires the SysML document registry",
            ));
        };
        let Some(host) = registry.host(doc_id) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("SysML document {} is not open", doc_id.raw()),
            ));
        };
        let document = host.document();
        let origin = document.origin();
        let origin_uri = origin.session_uri();
        let Some(analyses) = world.get_resource::<crate::SysmlDocumentAnalyses>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "InspectSysmlDocument requires the SysML analysis owner",
            ));
        };
        let state = analyses.state_for(doc_id, document.generation(), &origin_uri);
        let (analysis_state, analysis_generation, diagnostics, semantic_errors, analysis_error) =
            match state {
                crate::SysmlDocumentAnalysisState::Pending { generation, .. } => (
                    "pending",
                    api_value!(generation),
                    api_value!(null),
                    api_value!(null),
                    api_value!(null),
                ),
                crate::SysmlDocumentAnalysisState::Ready {
                    generation,
                    analysis,
                    ..
                } => {
                    let Some(report) = world
                        .get_resource::<lunco_doc_bevy::DocumentDiagnostics>()
                        .and_then(|store| store.get(doc_id))
                        .and_then(|entry| entry.sources.get("sysml.analysis"))
                        .filter(|report| report.generation == generation)
                    else {
                        return Err(ApiQueryError::new(
                            ApiErrorCode::InternalError,
                            "SysML analysis completed without publishing its shared diagnostics report",
                        ));
                    };
                    let diagnostics = report
                        .diagnostics
                        .iter()
                        .map(|diagnostic| {
                            diagnostic_api_value(diagnostic, Some("sysml"), Some("sysml.analysis"))
                        })
                        .collect::<Vec<_>>();
                    (
                        "ready",
                        api_value!(generation),
                        api_value!(diagnostics),
                        api_value!(analysis.has_errors()),
                        api_value!(null),
                    )
                }
                crate::SysmlDocumentAnalysisState::Failed {
                    generation, error, ..
                } => (
                    "failed",
                    api_value!(generation),
                    api_value!(null),
                    api_value!(null),
                    api_value!(error),
                ),
            };
        Ok(Some(api_value!({
            "doc_id": doc_id.raw(),
            "kind": "sysml",
            "source": document.source(),
            "generation": document.generation(),
            "dirty": document.is_dirty(),
            "read_only": origin.is_read_only(),
            "origin": {
                "uri": origin_uri,
                "title": origin.display_name(),
                "writable": origin.is_writable(),
            },
            "analysis_state": analysis_state,
            "analysis_generation": analysis_generation,
            "diagnostics": diagnostics,
            "semantic_errors": semantic_errors,
            "analysis_error": analysis_error,
        })))
    }
}

#[cfg(test)]
mod browser_payload_tests {
    use super::*;

    #[test]
    fn browser_sysml_payload_preparation_preserves_pathless_source_identity() {
        let source = b"package Fixture {}";
        let first =
            prepare_browser_sysml_document(DocumentId::new(201), "same # %.sysml".into(), source)
                .unwrap();
        let second =
            prepare_browser_sysml_document(DocumentId::new(202), "same # %.sysml".into(), source)
                .unwrap();
        assert_ne!(first.id(), second.id());
        assert!(first.origin().is_untitled());
        assert!(first.origin().canonical_path().is_none());
        assert_eq!(first.origin().display_name(), "same # %.sysml");
        assert_eq!(first.source(), "package Fixture {}");
        assert!(first.is_dirty());
        assert!(
            prepare_browser_sysml_document(DocumentId::new(203), "invalid.sysml".into(), &[0xff])
                .err()
                .unwrap()
                .contains("invalid UTF-8")
        );
    }
}
