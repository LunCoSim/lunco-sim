//! Document-level operations: Undo, Redo, Save, and Format.

use bevy::prelude::*;
use lunco_core::{on_command, Command};
use lunco_doc::DocumentId;
use lunco_doc_bevy::{DocumentSaved, RedoDocument, SaveAsDocument, SaveDocument, UndoDocument};

use crate::state::{ModelicaDocumentRegistry, WorkbenchState};
use crate::ui::panels::code_editor::EditorBufferState;

// ─── Command Structs ─────────────────────────────────────────────────────────

/// Undo the most recent edit on the active document.
#[Command(default)]
pub struct Undo {
    pub doc: DocumentId,
}

/// Redo the most recently undone edit.
#[Command(default)]
pub struct Redo {
    pub doc: DocumentId,
}

/// Run rumoca-tool-fmt on the active document.
#[Command(default)]
pub struct FormatDocument {
    pub doc: DocumentId,
}

/// API shim for Save.
#[Command(default)]
pub struct SaveActiveDocument {
    pub doc: DocumentId,
}

/// API shim for SaveAs.
#[Command(default)]
pub struct SaveActiveDocumentAs {
    pub doc: DocumentId,
    pub path: String,
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(Undo)]
pub fn on_undo(trigger: On<Undo>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        }) else {
            bevy::log::warn!("[Undo] no active document");
            return;
        };
        world.commands().trigger(UndoDocument { doc });
    });
}

#[on_command(Redo)]
pub fn on_redo(trigger: On<Redo>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        }) else {
            bevy::log::warn!("[Redo] no active document");
            return;
        };
        world.commands().trigger(RedoDocument { doc });
    });
}

#[on_command(UndoDocument)]
pub fn on_undo_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut editor: ResMut<EditorBufferState>,
    mut workbench: ResMut<WorkbenchState>,
) {
    let doc = trigger.event().doc;
    apply_undo_or_redo(doc, true, &mut registry, &mut editor, &mut workbench);
}

#[on_command(RedoDocument)]
pub fn on_redo_document(
    trigger: On<RedoDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut editor: ResMut<EditorBufferState>,
    mut workbench: ResMut<WorkbenchState>,
) {
    apply_undo_or_redo(
        trigger.event().doc,
        false,
        &mut registry,
        &mut editor,
        &mut workbench,
    );
}

fn apply_undo_or_redo(
    doc: DocumentId,
    is_undo: bool,
    registry: &mut ModelicaDocumentRegistry,
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
    registry: &ModelicaDocumentRegistry,
    editor: &mut EditorBufferState,
    workbench: &mut WorkbenchState,
) {
    editor.text = source.to_string();
    editor.generation = registry.host(doc).map(|h| h.generation()).unwrap_or(0);
    workbench.editor_buffer = source.to_string();
}

#[on_command(SaveDocument)]
pub fn on_save_document(
    trigger: On<SaveDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;

    // No writable filesystem in the browser — every save is a
    // download. Delegate to Save-As, which picks a sensible file name
    // and triggers the browser download.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (&registry, &console);
        commands.trigger(SaveAsDocument {
            doc,
            path: String::new(),
        });
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let to_save = {
            let Some(host) = registry.host(doc) else {
                return;
            };
            let document = host.document();
            if document.origin().is_untitled() {
                commands.trigger(SaveAsDocument {
                    doc,
                    path: String::new(),
                });
                return;
            }
            let Some(path) = document.canonical_path() else {
                console.warn(format!("Save skipped — doc {doc} has no canonical path"));
                return;
            };
            if document.is_read_only() {
                let name = document.origin().display_name();
                let msg =
                    format!("Save blocked — '{name}' is read-only (library / bundled example).");
                warn!("[Save] {msg}");
                console.warn(msg);
                return;
            }
            (path.to_path_buf(), document.source().to_string())
        };

        let (path, source) = to_save;
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path.clone());
        if let Err(e) = futures_lite::future::block_on(
            <lunco_storage::FileStorage as lunco_storage::Storage>::write(
                &storage,
                &handle,
                source.as_bytes(),
            ),
        ) {
            let msg = format!("Save failed: {}: {e}", path.display());
            error!("[Save] {msg}");
            console.error(msg);
            return;
        }
        let msg = format!("Saved {} bytes to {}", source.len(), path.display());
        info!("[Save] {msg}");
        console.info(msg);

        registry.mark_document_saved(doc);
        commands.trigger(DocumentSaved::local(doc));
    }
}

#[on_command(SaveAsDocument)]
pub fn on_save_as_document(
    trigger: On<SaveAsDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    let target_path = trigger.event().path.clone();

    // wasm: no filesystem, and the browser's save flow can't hand back
    // a writable path — so Save-As is a download triggered right here,
    // never a two-phase picker round-trip.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = &workspace;
        let (name, source) = {
            let Some(host) = registry.host(doc) else {
                return;
            };
            let document = host.document();
            let name = if !target_path.is_empty() {
                target_path.clone()
            } else {
                let raw = document.origin().display_name();
                if raw.ends_with(".mo") {
                    raw
                } else {
                    format!("{raw}.mo")
                }
            };
            (name, document.source().to_string())
        };
        lunco_workbench::picker::download_file(&name, &source);
        registry.mark_document_saved(doc);
        let msg = format!("Downloaded {} ({} bytes)", name, source.len());
        info!("[SaveAs] {msg}");
        console.info(msg);
        commands.trigger(DocumentSaved::local(doc));
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if target_path.is_empty() {
            let Some(host) = registry.host(doc) else {
                return;
            };
            let document = host.document();
            let suggested_name = {
                let raw = document.origin().display_name();
                if raw.ends_with(".mo") {
                    raw
                } else {
                    format!("{raw}.mo")
                }
            };
            let start_dir = workspace
                .active_twin
                .and_then(|id| workspace.twin(id))
                .map(|t| lunco_storage::StorageHandle::File(t.root.clone()));
            commands.trigger(lunco_workbench::picker::PickHandle {
                mode: lunco_workbench::picker::PickMode::SaveFile(
                    lunco_workbench::picker::SaveHint {
                        suggested_name: Some(suggested_name),
                        start_dir,
                        filters: vec![lunco_workbench::picker::OpenFilter::new(
                            "Modelica models",
                            &["mo"],
                        )],
                    },
                ),
                on_resolved: lunco_workbench::picker::PickFollowUp::SaveAs(doc),
            });
            return;
        }

        let path = std::path::PathBuf::from(&target_path);
        let source = {
            let Some(host) = registry.host(doc) else {
                return;
            };
            host.document().source().to_string()
        };

        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path.clone());
        if let Err(e) = futures_lite::future::block_on(
            <lunco_storage::FileStorage as lunco_storage::Storage>::write(
                &storage,
                &handle,
                source.as_bytes(),
            ),
        ) {
            let msg = format!("Save-As failed: {}: {e}", path.display());
            error!("[SaveAs] {msg}");
            console.error(msg);
            return;
        }

        if let Some(host) = registry.host_mut(doc) {
            host.document_mut()
                .set_origin(lunco_doc::DocumentOrigin::File {
                    path: path.clone(),
                    writable: true,
                });
        }
        registry.mark_document_saved(doc);
        let msg = format!("Saved {} bytes to {}", source.len(), path.display());
        info!("[SaveAs] {msg}");
        console.info(msg);

        commands.trigger(DocumentSaved::local(doc));
    }
}

#[on_command(FormatDocument)]
pub fn on_format_document(trigger: On<FormatDocument>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        use crate::document::ModelicaOp;
        let doc = if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        };
        let Some(doc) = doc else {
            bevy::log::warn!("[FormatDocument] no active document");
            return;
        };
        let workbench_read_only = crate::state::read_only_for(world, doc);
        if workbench_read_only {
            bevy::log::info!("[FormatDocument] tab is read-only — skipping");
            return;
        }
        let Some(registry) = world.get_resource::<ModelicaDocumentRegistry>() else {
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
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        if let Some(host) = registry.host_mut(doc) {
            if let Err(e) = host.apply(ModelicaOp::ReplaceSource { new: formatted }) {
                bevy::log::warn!("[FormatDocument] apply failed: {e:?}");
            }
        }
    });
}

#[on_command(SaveActiveDocument)]
pub fn on_save_active_document(trigger: On<SaveActiveDocument>, mut commands: Commands) {
    let raw = trigger.event().doc;
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
        world.commands().trigger(SaveDocument { doc });
    });
}

#[on_command(SaveActiveDocumentAs)]
pub fn on_save_active_document_as(trigger: On<SaveActiveDocumentAs>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let doc = if ev.doc.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(ev.doc)
        };
        let Some(doc) = doc else {
            bevy::log::warn!("[SaveActiveDocumentAs] no active document");
            return;
        };
        let path = std::path::PathBuf::from(&ev.path);
        let source = {
            let registry = world.resource::<ModelicaDocumentRegistry>();
            let Some(host) = registry.host(doc) else {
                return;
            };
            host.document().source().to_string()
        };
        // Through `lunco-storage` — atomic tmp+rename on native, localStorage on
        // wasm — so "Save As" is a real, working command in the browser instead
        // of a `std::fs::write` that always fails there.
        if let Err(e) = crate::source_asset::write_text_sync(&path, &source) {
            bevy::log::warn!(
                "[SaveActiveDocumentAs] write failed {}: {}",
                path.display(),
                e
            );
            return;
        }
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        if let Some(host) = registry.host_mut(doc) {
            host.document_mut()
                .set_origin(lunco_doc::DocumentOrigin::File {
                    path: path.clone(),
                    writable: true,
                });
        }
        registry.mark_document_saved(doc);
        bevy::log::info!(
            "[SaveActiveDocumentAs] saved {} ({} bytes)",
            path.display(),
            source.len(),
        );
        world.commands().trigger(DocumentSaved::local(doc));
    });
}
