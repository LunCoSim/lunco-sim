//! Document-level operations: Undo, Redo, Save, and Format.

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command};
use lunco_doc::DocumentId;
use lunco_doc_bevy::{DocumentSaved, RedoDocument, SaveAsDocument, SaveDocument, UndoDocument};

use crate::ui::document_context::ModelicaDocuments;
use crate::ui::panels::code_editor::EditorBufferState;
use crate::ui::workbench_state::WorkbenchState;

// ─── Command Structs ─────────────────────────────────────────────────────────

/// Undo the most recent edit on the active document.
#[Command(default)]
pub struct Undo {
    pub doc_id: DocumentId,
}

/// Redo the most recently undone edit.
#[Command(default)]
pub struct Redo {
    pub doc_id: DocumentId,
}

/// Run rumoca-tool-fmt on the active document.
#[Command(default)]
pub struct FormatDocument {
    pub doc_id: DocumentId,
}

/// Save the document — the one save verb, in-process and over the API alike.
/// Unassigned `doc_id` (`0` over the API) means the active document.
#[Command(default)]
pub struct SaveActiveDocument {
    pub doc_id: DocumentId,
}

/// Save the document to `path`. Unassigned `doc_id` means the active document.
#[Command(default)]
pub struct SaveActiveDocumentAs {
    pub doc_id: DocumentId,
    pub path: String,
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(Undo)]
pub fn on_undo(trigger: On<Undo>, mut commands: Commands) {
    let raw = trigger.event().doc_id;
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        }) else {
            bevy::log::warn!("[Undo] no active document");
            return;
        };
        world.commands().trigger(UndoDocument { doc_id: doc });
    });
}

#[on_command(Redo)]
pub fn on_redo(trigger: On<Redo>, mut commands: Commands) {
    let raw = trigger.event().doc_id;
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        }) else {
            bevy::log::warn!("[Redo] no active document");
            return;
        };
        world.commands().trigger(RedoDocument { doc_id: doc });
    });
}

#[on_command(UndoDocument)]
pub fn on_undo_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<ModelicaDocuments>,
    mut editor: ResMut<EditorBufferState>,
    mut workbench: ResMut<WorkbenchState>,
) {
    let doc = trigger.event().doc_id;
    apply_undo_or_redo(doc, true, &mut registry, &mut editor, &mut workbench);
}

#[on_command(RedoDocument)]
pub fn on_redo_document(
    trigger: On<RedoDocument>,
    mut registry: ResMut<ModelicaDocuments>,
    mut editor: ResMut<EditorBufferState>,
    mut workbench: ResMut<WorkbenchState>,
) {
    apply_undo_or_redo(
        trigger.event().doc_id,
        false,
        &mut registry,
        &mut editor,
        &mut workbench,
    );
}

fn apply_undo_or_redo(
    doc: DocumentId,
    is_undo: bool,
    registry: &mut ModelicaDocuments,
    editor: &mut EditorBufferState,
    workbench: &mut WorkbenchState,
) {
    if registry.host(doc).is_none() {
        return;
    }
    let workbench_read_only = registry
        .host(doc)
        .map(|h| h.document().is_read_only())
        .unwrap_or(false);

    if workbench_read_only {
        return;
    }

    let new_source = {
        let result = registry.host_mut(doc).and_then(|host| {
            let changed = if is_undo {
                host.undo().ok().unwrap_or(false)
            } else {
                host.redo().ok().unwrap_or(false)
            };
            changed.then(|| host.document().source().to_string())
        });
        if result.is_some() {
            registry.mark_changed(doc);
        }
        result
    };

    let Some(source) = new_source else { return };
    sync_editor_buffer_to_source(doc, &source, registry, editor, workbench);
}

pub fn sync_editor_buffer_to_source(
    doc: DocumentId,
    source: &str,
    registry: &ModelicaDocuments,
    editor: &mut EditorBufferState,
    workbench: &mut WorkbenchState,
) {
    editor.text = source.to_string();
    editor.generation = registry.host(doc).map(|h| h.generation()).unwrap_or(0);
    workbench.editor_buffer = source.to_string();
}

fn finish_modelica_save(
    command_id: Option<u64>,
    result: Result<Ack, String>,
    results: &mut lunco_core::CommandResults,
    console: &mut lunco_ui::log::LogBuffer,
    commands: &mut Commands,
) {
    if let Err(message) = &result {
        warn!("[Modelica save] {message}");
        console.error(message.clone());
        commands.trigger(lunco_core::RuntimeError {
            name: "modelica-save-failed".into(),
            message: message.clone(),
        });
    }
    if let Some(id) = command_id {
        results.record(id, result);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn write_modelica_source(
    registry: &ModelicaDocuments,
    doc: DocumentId,
    path: &std::path::Path,
) -> Result<usize, String> {
    let document = registry
        .host(doc)
        .ok_or("Modelica save source document is closed")?
        .document();
    let source = document.source();
    let storage = lunco_storage::FileStorage::new();
    lunco_storage::Storage::write_sync(
        &storage,
        &lunco_storage::StorageHandle::File(path.to_path_buf()),
        source.as_bytes(),
    )
    .map_err(|error| format!("Save failed: {}: {error}", path.display()))?;
    Ok(source.len())
}

#[cfg(not(target_arch = "wasm32"))]
fn request_modelica_save_picker(
    doc: DocumentId,
    document: &lunco_modelica_document::ModelicaDocument,
    workspace: &lunco_workspace::Workspace,
    replication: Option<&lunco_workspace::ReplicationOwner>,
    commands: &mut Commands,
) -> Result<Ack, String> {
    let name = document.origin().display_name();
    let suggested_name = if name.ends_with(".mo") {
        name
    } else {
        format!("{name}.mo")
    };
    let start_dir = workspace
        .active_twin
        .and_then(|id| workspace.twin(id))
        .map(|twin| lunco_storage::StorageHandle::File(twin.root.clone()));
    let pin = lunco_workspace::PinnedDocumentRuntimeOwner::for_document(doc, Some(workspace))?;
    if !pin.is_current(Some(workspace), replication) {
        return Err(format!(
            "Save As document {doc} belongs to a retired source owner"
        ));
    }
    lunco_workbench_file_dialog::request_pick(
        commands,
        lunco_workbench_file_dialog::PickMode::SaveFile(lunco_workbench_file_dialog::SaveHint {
            suggested_name: Some(suggested_name),
            start_dir,
            filters: vec![lunco_workbench_file_dialog::OpenFilter::new(
                "Modelica models",
                &["mo"],
            )],
        }),
        lunco_workbench_file_dialog::PickFollowUp::SaveAs(doc),
        pin,
    );
    Ok(Ack::with_data(
        OpId::new(),
        lunco_hooks::HookValue::map([("picker_requested", lunco_hooks::HookValue::Bool(true))]),
    ))
}

#[cfg(target_arch = "wasm32")]
fn download_modelica_source(
    document: &lunco_modelica_document::ModelicaDocument,
    requested_name: &str,
    picker: Option<&lunco_workbench_file_dialog::BrowserPicker>,
) -> Result<String, String> {
    let name = if requested_name.is_empty() {
        let name = document.origin().display_name();
        if name.ends_with(".mo") {
            name
        } else {
            format!("{name}.mo")
        }
    } else {
        requested_name.to_owned()
    };
    let picker = picker.ok_or("browser download capability is not installed")?;
    lunco_workbench_file_dialog::download_file(picker, &name, document.source())?;
    Ok(format!(
        "Downloaded {name} ({} bytes)",
        document.source().len()
    ))
}

#[on_command(SaveDocument)]
pub fn on_save_document(
    trigger: On<SaveDocument>,
    mut registry: ResMut<ModelicaDocuments>,
    mut console: ResMut<lunco_ui::log::LogBuffer>,
    mut commands: Commands,
    active: Res<lunco_core::ActiveCommandId>,
    mut results: ResMut<lunco_core::CommandResults>,
    #[cfg(target_arch = "wasm32")] picker: Option<
        NonSend<lunco_workbench_file_dialog::BrowserPicker>,
    >,
    #[cfg(not(target_arch = "wasm32"))] workspace: Res<lunco_workspace::WorkspaceResource>,
    #[cfg(not(target_arch = "wasm32"))] connection: Option<
        Res<lunco_core_session::ClientConnection>,
    >,
    #[cfg(not(target_arch = "wasm32"))] replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    let doc = trigger.event().doc_id;
    if registry.host(doc).is_none() {
        return;
    }
    let result = (|| -> Result<Ack, String> {
        let document = registry
            .host(doc)
            .ok_or("Modelica save source document is closed")?
            .document();
        #[cfg(target_arch = "wasm32")]
        let message = download_modelica_source(document, "", picker.as_deref())?;
        #[cfg(not(target_arch = "wasm32"))]
        let message = {
            if document.origin().is_untitled() {
                let replication = lunco_core_session::current_replication_owner(
                    connection.as_deref(),
                    replica.as_deref(),
                );
                return request_modelica_save_picker(
                    doc,
                    document,
                    &workspace.0,
                    replication.as_ref(),
                    &mut commands,
                );
            }
            if document.is_read_only() {
                return Err(format!(
                    "Save blocked: '{}' is read-only; use SaveAsDocument",
                    document.origin().display_name()
                ));
            }
            let path = document
                .canonical_path()
                .ok_or("Modelica save source has no canonical file path")?
                .to_path_buf();
            let bytes = write_modelica_source(&registry, doc, &path)?;
            format!("Saved {bytes} bytes to {}", path.display())
        };
        registry.mark_document_saved(doc);
        info!("[Save] {message}");
        console.info(message);
        commands.trigger(DocumentSaved::local(doc));
        Ok(Ack::new(OpId::new()))
    })();
    finish_modelica_save(
        active.get(),
        result,
        &mut results,
        &mut console,
        &mut commands,
    );
}

#[on_command(SaveAsDocument)]
pub fn on_save_as_document(
    trigger: On<SaveAsDocument>,
    mut registry: ResMut<ModelicaDocuments>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    mut console: ResMut<lunco_ui::log::LogBuffer>,
    mut commands: Commands,
    active: Res<lunco_core::ActiveCommandId>,
    mut results: ResMut<lunco_core::CommandResults>,
    #[cfg(target_arch = "wasm32")] picker: Option<
        NonSend<lunco_workbench_file_dialog::BrowserPicker>,
    >,
    #[cfg(not(target_arch = "wasm32"))] connection: Option<
        Res<lunco_core_session::ClientConnection>,
    >,
    #[cfg(not(target_arch = "wasm32"))] replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    let doc = trigger.event().doc_id;
    let target_path = trigger.event().path.clone();
    if registry.host(doc).is_none() {
        return;
    }
    let result = (|| -> Result<Ack, String> {
        #[cfg(target_arch = "wasm32")]
        let message = {
            let _ = &workspace;
            let document = registry
                .host(doc)
                .ok_or("Modelica save source document is closed")?
                .document();
            download_modelica_source(document, &target_path, picker.as_deref())?
        };
        #[cfg(not(target_arch = "wasm32"))]
        let message = {
            if target_path.is_empty() {
                let document = registry
                    .host(doc)
                    .ok_or("Modelica save source document is closed")?
                    .document();
                let replication = lunco_core_session::current_replication_owner(
                    connection.as_deref(),
                    replica.as_deref(),
                );
                return request_modelica_save_picker(
                    doc,
                    document,
                    &workspace.0,
                    replication.as_ref(),
                    &mut commands,
                );
            }
            let path = std::path::PathBuf::from(&target_path);
            let bytes = write_modelica_source(&registry, doc, &path)?;
            registry
                .host_mut(doc)
                .ok_or("Modelica save source document is closed")?
                .document_mut()
                .set_origin(lunco_doc::DocumentOrigin::File {
                    path: path.clone(),
                    writable: true,
                });
            format!("Saved {bytes} bytes to {}", path.display())
        };
        registry.mark_document_saved(doc);
        info!("[SaveAs] {message}");
        console.info(message);
        commands.trigger(DocumentSaved::local(doc));
        Ok(Ack::new(OpId::new()))
    })();
    finish_modelica_save(
        active.get(),
        result,
        &mut results,
        &mut console,
        &mut commands,
    );
}

#[on_command(FormatDocument)]
pub fn on_format_document(trigger: On<FormatDocument>, mut commands: Commands) {
    let raw = trigger.event().doc_id;
    commands.queue(move |world: &mut World| {
        use lunco_modelica_document::ModelicaOp;
        let doc = if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        };
        let Some(doc) = doc else {
            bevy::log::warn!("[FormatDocument] no active document");
            return;
        };
        let workbench_read_only = crate::ui::document_context::read_only_for(world, doc);
        if workbench_read_only {
            bevy::log::info!("[FormatDocument] tab is read-only — skipping");
            return;
        }
        let Some(registry) = world.get_resource::<ModelicaDocuments>() else {
            return;
        };
        let Some(host) = registry.host(doc) else {
            return;
        };
        let original = host.document().source().to_string();
        let opts = rumoca_tool_fmt::FormatOptions::default();
        let formatted = match rumoca_tool_fmt::format_with_source_name(&original, &opts, "<editor>")
        {
            Ok(s) => s,
            Err(e) => {
                bevy::log::warn!("[FormatDocument] format failed: {}", e);
                return;
            }
        };
        if formatted == original {
            return;
        }
        let result = crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::ReplaceSource { new: formatted },
            lunco_twin_journal::AuthorTag::for_tool("format-document"),
        );
        if let Err(error) = result {
            bevy::log::warn!("[FormatDocument] apply failed: {error:?}");
        }
    });
}

#[on_command(SaveActiveDocument)]
pub fn on_save_active_document(trigger: On<SaveActiveDocument>, mut commands: Commands) {
    let raw = trigger.event().doc_id;
    commands.queue(move |world: &mut World| {
        let doc = if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        };
        let Some(doc) = doc else {
            bevy::log::warn!("[SaveActiveDocument] no active document");
            return;
        };
        world.commands().trigger(SaveDocument { doc_id: doc });
    });
}

#[on_command(SaveActiveDocumentAs)]
pub fn on_save_active_document_as(trigger: On<SaveActiveDocumentAs>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let doc = if ev.doc_id.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(ev.doc_id)
        };
        let Some(doc) = doc else {
            bevy::log::warn!("[SaveActiveDocumentAs] no active document");
            return;
        };
        world.commands().trigger(SaveAsDocument {
            doc_id: doc,
            path: ev.path,
        });
    });
}
