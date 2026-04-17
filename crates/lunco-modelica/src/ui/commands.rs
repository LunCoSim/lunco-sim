//! Command bus for Modelica documents.
//!
//! Every user intent that mutates a [`ModelicaDocument`] is a Bevy event
//! fired via `commands.trigger(...)`; the observers in this module are
//! the single write surface. UI buttons, keyboard shortcuts, the remote
//! API, and scripting all funnel through the same path.
//!
//! The generic commands ([`lunco_doc_bevy::UndoDocument`] /
//! [`RedoDocument`](lunco_doc_bevy::RedoDocument) /
//! [`SaveDocument`](lunco_doc_bevy::SaveDocument) /
//! [`CloseDocument`](lunco_doc_bevy::CloseDocument)) carry a
//! [`DocumentId`] without naming a domain. Each observer here checks
//! whether [`ModelicaDocumentRegistry`] owns the id and acts or
//! no-ops — USD, scripting, SysML can install parallel observers that
//! handle *their* ids with no coordination needed.
//!
//! Modelica-specific intents live here too. [`CompileModel`] is the
//! big one: it replaces the old `dispatch_compile_from_buffer` helper
//! and reads source directly from the Document (the buffer is already
//! kept in sync via focus-loss / commit-on-switch).

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_doc_bevy::{
    CloseDocument, DocumentSaved, EditorIntent, RedoDocument, SaveDocument, UndoDocument,
};
use std::collections::HashMap;

use crate::ast_extract::{
    extract_input_names, extract_inputs_with_defaults, extract_model_name,
    extract_parameters, hash_content,
};
use crate::ui::panels::code_editor::EditorBufferState;
use crate::ui::panels::diagram::DiagramState;
use crate::ui::{CompileState, CompileStates, ModelicaDocumentRegistry, WorkbenchState};
use crate::{ModelicaChannels, ModelicaCommand, ModelicaModel};

// ─────────────────────────────────────────────────────────────────────────────
// Modelica-specific commands
// ─────────────────────────────────────────────────────────────────────────────

/// Request to compile a Modelica document and run the resulting
/// simulation.
///
/// Reads the document's *current* source (not any editor buffer — the
/// buffer is expected to have been flushed by the caller via
/// [`ModelicaDocumentRegistry::checkpoint_source`] before firing), parses
/// parameters / inputs, spawns or updates the [`ModelicaModel`] entity
/// linked to the document, marks the [`CompileState`] as
/// [`CompileState::Compiling`], and sends a
/// [`ModelicaCommand::Compile`] to the worker.
///
/// Unknown / foreign ids are no-ops.
#[derive(Event, Clone, Debug)]
pub struct CompileModel {
    /// The document to compile.
    pub doc: DocumentId,
}

// ─────────────────────────────────────────────────────────────────────────────
// Observers
// ─────────────────────────────────────────────────────────────────────────────

/// Plugin that installs all Modelica command observers.
///
/// `ModelicaUiPlugin` adds this automatically. Keeping the registration
/// in its own plugin makes it easy for headless tests (or another shell
/// that doesn't want the rest of the UI plugin) to opt in to the
/// command path alone.
pub struct ModelicaCommandsPlugin;

impl Plugin for ModelicaCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_undo_document)
            .add_observer(on_redo_document)
            .add_observer(on_save_document)
            .add_observer(on_close_document)
            .add_observer(on_compile_model)
            .add_observer(resolve_editor_intent);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Intent resolver — EditorIntent → concrete command for Modelica docs
// ─────────────────────────────────────────────────────────────────────────────

/// Translate an abstract [`EditorIntent`] into the concrete Modelica
/// command(s) it maps to, targeting the currently-active document.
///
/// **Ownership-aware**: only resolves when the active document is
/// owned by [`ModelicaDocumentRegistry`]. If another domain (USD,
/// scripting, SysML) owns the active doc, its own resolver handles
/// the intent and this observer no-ops — both resolvers fire on
/// every intent and each picks the ones that belong to it.
///
/// This is the "intent → command" layer. Keybindings map keys to
/// intents in `lunco-doc-bevy`; resolvers like this one map intents
/// to concrete commands per domain. Users reconfiguring hotkeys
/// never touch this function; they edit their `Keybindings`.
fn resolve_editor_intent(
    trigger: On<EditorIntent>,
    workbench: Res<WorkbenchState>,
    registry: Res<ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    let Some(doc) = workbench.open_model.as_ref().and_then(|m| m.doc) else {
        return;
    };
    // Ownership check — is this doc in the Modelica registry?
    if registry.host(doc).is_none() {
        return;
    }

    match *trigger.event() {
        EditorIntent::Undo => commands.trigger(UndoDocument { doc }),
        EditorIntent::Redo => commands.trigger(RedoDocument { doc }),
        EditorIntent::Save => commands.trigger(SaveDocument { doc }),
        EditorIntent::SaveAs => {
            // Save-As dialog isn't wired up yet — log so the user sees
            // the intent landed, at least.
            warn!("[Intent] SaveAs not implemented yet for doc {doc}");
        }
        EditorIntent::Close => commands.trigger(CloseDocument { doc }),
        EditorIntent::Compile => commands.trigger(CompileModel { doc }),
    }
}

fn on_undo_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut editor: ResMut<EditorBufferState>,
    mut workbench: ResMut<WorkbenchState>,
) {
    apply_undo_or_redo(
        trigger.event().doc,
        /*is_undo=*/ true,
        &mut registry,
        &mut editor,
        &mut workbench,
    );
}

fn on_redo_document(
    trigger: On<RedoDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut editor: ResMut<EditorBufferState>,
    mut workbench: ResMut<WorkbenchState>,
) {
    apply_undo_or_redo(
        trigger.event().doc,
        /*is_undo=*/ false,
        &mut registry,
        &mut editor,
        &mut workbench,
    );
}

/// Shared body for Undo / Redo — runs the op on the `DocumentHost`,
/// then mirrors the reverted source into the editor buffer so the
/// text view shows it on the next frame.
///
/// No-op if the registry doesn't own `doc`, if there's nothing to
/// undo/redo, or if the document is read-only.
fn apply_undo_or_redo(
    doc: DocumentId,
    is_undo: bool,
    registry: &mut ModelicaDocumentRegistry,
    editor: &mut EditorBufferState,
    workbench: &mut WorkbenchState,
) {
    // Ownership + writability check.
    let is_read_only = match registry.host(doc) {
        Some(h) => h.document().is_read_only(),
        None => return,
    };
    if is_read_only {
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
        // Undo/redo goes directly through `host_mut` — record it so the
        // Bevy observer drain sees the change.
        if result.is_some() {
            registry.mark_changed(doc);
        }
        result
    };

    let Some(source) = new_source else { return };
    sync_editor_buffer_to_source(&source, editor, workbench);
}

/// Write the given source into [`EditorBufferState`] (including line
/// starts, detected name, hash) and [`WorkbenchState::editor_buffer`]
/// so both the text view and any mirror consumers see the new content
/// on the next frame.
fn sync_editor_buffer_to_source(
    source: &str,
    editor: &mut EditorBufferState,
    workbench: &mut WorkbenchState,
) {
    let mut new_starts = vec![0usize];
    for (i, b) in source.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            new_starts.push(i + 1);
        }
    }
    editor.text = source.to_string();
    editor.line_starts = new_starts.into();
    editor.detected_name = extract_model_name(source);
    editor.source_hash = hash_content(source);
    workbench.editor_buffer = source.to_string();
}

fn on_save_document(
    trigger: On<SaveDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;

    // Validate + snapshot what we need to write.
    let to_save = {
        let Some(host) = registry.host(doc) else {
            return; // Foreign / unknown id.
        };
        let document = host.document();
        let Some(path) = document.canonical_path() else {
            warn!(
                "[Save] Document {} has no canonical path — Save-As not implemented yet.",
                doc
            );
            return;
        };
        // `mem://` paths are our in-memory marker, not real filesystem
        // paths. Treat them as "needs Save-As" until that dialog lands.
        if path.to_string_lossy().starts_with("mem://") {
            warn!(
                "[Save] Document {} is in-memory (path {:?}); Save-As required.",
                doc, path
            );
            return;
        }
        if document.is_read_only() {
            warn!(
                "[Save] Document {} is read-only ({:?} library) — refusing to overwrite.",
                doc,
                document.library()
            );
            return;
        }
        (path.to_path_buf(), document.source().to_string())
    };

    let (path, source) = to_save;
    if let Err(e) = std::fs::write(&path, &source) {
        error!("[Save] Failed to write {:?}: {e}", path);
        return;
    }
    info!("[Save] Wrote {} bytes to {:?}", source.len(), path);

    registry.mark_document_saved(doc);
    commands.trigger(DocumentSaved { doc });
}

fn on_close_document(
    trigger: On<CloseDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
) {
    let doc = trigger.event().doc;
    if registry.host(doc).is_none() {
        return; // Foreign or already-closed.
    }
    registry.remove_document(doc);
}

fn on_compile_model(
    trigger: On<CompileModel>,
    mut commands: Commands,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut workbench: ResMut<WorkbenchState>,
    mut compile_states: ResMut<CompileStates>,
    diagram_state: Res<DiagramState>,
    channels: Option<Res<ModelicaChannels>>,
    mut q_models: Query<&mut ModelicaModel>,
) {
    let doc = trigger.event().doc;

    // Ownership check.
    let (source, is_read_only) = match registry.host(doc) {
        Some(h) => (
            h.document().source().to_string(),
            h.document().is_read_only(),
        ),
        None => return,
    };
    if is_read_only {
        warn!("[Compile] Document {} is read-only.", doc);
        return;
    }
    let Some(model_name) = extract_model_name(&source) else {
        workbench.compilation_error =
            Some("Could not find a valid model declaration.".to_string());
        return;
    };
    let params = extract_parameters(&source);
    let inputs_with_defaults = extract_inputs_with_defaults(&source);
    let runtime_inputs = extract_input_names(&source);

    // Find or spawn the entity linked to this document.
    let linked = registry.entities_linked_to(doc);

    let target_entity = if let Some(&entity) = linked.first() {
        // Update existing entity in place.
        if let Ok(mut model) = q_models.get_mut(entity) {
            let old_inputs = std::mem::take(&mut model.inputs);
            model.session_id += 1;
            model.is_stepping = true;
            model.model_name = model_name.clone();
            model.parameters = params.clone();
            model.inputs.clear();
            for (name, val) in &inputs_with_defaults {
                let existing = old_inputs.get(name).copied();
                model
                    .inputs
                    .entry(name.clone())
                    .or_insert_with(|| existing.unwrap_or(*val));
            }
            for name in &runtime_inputs {
                let existing = old_inputs.get(name).copied();
                model
                    .inputs
                    .entry(name.clone())
                    .or_insert_with(|| existing.unwrap_or(0.0));
            }
            model.variables.clear();
            model.paused = false;
            model.current_time = 0.0;
            model.last_step_time = 0.0;
        }
        entity
    } else {
        // No entity yet — spawn one linked to this doc. Spawning goes
        // through `Commands` (deferred), so we can't immediately
        // query the new entity in this system — initial fields are
        // set on the component at spawn time instead.
        let session_id = diagram_state.model_counter as u64 + 1;
        let entity = commands
            .spawn((
                Name::new(model_name.clone()),
                ModelicaModel {
                    model_path: "".into(),
                    model_name: model_name.clone(),
                    current_time: 0.0,
                    last_step_time: 0.0,
                    session_id,
                    paused: false,
                    parameters: params,
                    inputs: runtime_inputs.into_iter().map(|n| (n, 0.0)).collect(),
                    variables: HashMap::new(),
                    document: doc,
                    is_stepping: true,
                },
            ))
            .id();
        registry.link(entity, doc);
        workbench.selected_entity = Some(entity);
        entity
    };

    // Resolve the session_id for the command we're about to send. For
    // the updated-in-place branch this is whatever we just bumped to;
    // for the newly-spawned branch the entity doesn't exist yet (spawn
    // is deferred), so fall back to the DiagramState counter we used.
    let session_id = q_models
        .get(target_entity)
        .map(|m| m.session_id)
        .unwrap_or_else(|_| diagram_state.model_counter as u64 + 1);

    compile_states.set(doc, CompileState::Compiling);

    if let Some(channels) = channels {
        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity: target_entity,
            session_id,
            model_name,
            source,
        });
    }
}
