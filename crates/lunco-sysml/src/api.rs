//! Transport-neutral SysML document editing and inspection.
//!
//! The command is intentionally small: SysML source remains the canonical
//! artifact and the document host owns parsing, generation checks, journaling,
//! and undo/redo. Rhai tools provide the authoring UX; this module only lowers
//! their typed intent into the generic document registry.

use bevy::prelude::*;
use lunco_api::{ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult, api_param_u64};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_command_contracts::Ack;
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Document, DocumentId, FileBacked, OpenOutcome};
use lunco_doc_bevy::{
    DocumentRegistry, DocumentSaved, NewDocument, OpenFile, RedoDocument, SaveAsDocument,
    SaveDocument, UndoDocument,
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

struct PendingSysmlOpen {
    path: std::path::PathBuf,
    task: bevy::tasks::Task<Result<String, String>>,
}

register_commands!(
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
        app.init_resource::<ApiQueryRegistry>();
        app.init_resource::<PendingSysmlOpens>()
            .add_systems(Update, drain_pending_sysml_opens);
        app.world_mut()
            .resource_mut::<ApiQueryRegistry>()
            .register(InspectSysmlDocumentProvider);
    }
}

/// Route a filesystem `.sysml`/`.kerml` open through the async storage path.
/// Other URI schemes and extensions belong to their owning domain observers.
#[on_command(OpenFile)]
fn on_open_sysml_file(trigger: On<OpenFile>, mut pending: ResMut<PendingSysmlOpens>) {
    let raw = trigger.event().path.trim();
    let path = raw.strip_prefix("file://").unwrap_or(raw);
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if lunco_assets_core::has_scheme(path)
        || !matches!(extension.as_deref(), Some("sysml" | "kerml"))
    {
        return;
    }
    let path = std::path::PathBuf::from(path);
    if pending.tasks.iter().any(|load| load.path == path) {
        return;
    }
    let task_path = path.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(task_path.clone());
        let bytes = storage
            .read(&handle)
            .await
            .map_err(|error| format!("failed to read {}: {error:?}", task_path.display()))?;
        String::from_utf8(bytes)
            .map_err(|error| format!("invalid UTF-8 in {}: {error}", task_path.display()))
    });
    pending.tasks.push(PendingSysmlOpen { path, task });
}

/// Finish pending source reads on the ECS thread and let the registry decide
/// whether a clean open document may refresh or a dirty one must be retained.
fn drain_pending_sysml_opens(
    mut pending: ResMut<PendingSysmlOpens>,
    mut registry: ResMut<DocumentRegistry<SysmlDocument>>,
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
            Some(Err(error)) => error!("[sysml] {error}"),
            Some(Ok(source)) => {
                let (doc, outcome) = registry.open_file(load.path.clone(), source);
                match outcome {
                    OpenOutcome::Allocated => {
                        info!("[sysml] opened {} as {doc}", load.path.display())
                    }
                    OpenOutcome::Refreshed => {
                        info!("[sysml] refreshed {} ({doc})", load.path.display())
                    }
                    OpenOutcome::KeptDirty => {
                        warn!("[sysml] kept dirty document {doc}; disk not reloaded")
                    }
                    OpenOutcome::KeptUnparsable => {
                        warn!("[sysml] kept {doc}; source is not valid SysML")
                    }
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
) {
    if trigger.event().kind != "sysml" {
        return;
    }
    let next = registry.ids().count() + 1;
    registry.allocate(
        "package Untitled {\n}\n".to_owned(),
        lunco_doc::PathlessOrigin::untitled(format!("Untitled-{next}.sysml")),
    );
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
        let diagnostics = document
            .analysis()
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                api_value!({
                    "kind": format!("{:?}", diagnostic.kind),
                    "message": diagnostic.message.clone(),
                    "file": diagnostic.file.clone(),
                    "start": diagnostic.start,
                    "end": diagnostic.end,
                })
            })
            .collect::<Vec<_>>();
        Ok(Some(api_value!({
            "doc_id": doc_id.raw(),
            "kind": "sysml",
            "source": document.source(),
            "generation": document.generation(),
            "dirty": document.is_dirty(),
            "read_only": origin.is_read_only(),
            "origin": {
                "uri": origin.session_uri(),
                "title": origin.display_name(),
                "writable": origin.is_writable(),
            },
            "diagnostics": diagnostics,
            "semantic_errors": document.has_diagnostics(),
        })))
    }
}
