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
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_doc_bevy::{
    CloseDocument, DocumentSaved, EditorIntent, RedoDocument, SaveAsDocument, SaveDocument,
    UndoDocument,
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

/// Request to create a new untitled Modelica model and open its tab.
///
/// Matches VS Code's "New File" flow — no name dialog, no Save-As
/// prompt. The observer picks the next free `Untitled<N>` name,
/// allocates an in-memory [`ModelicaDocument`](crate::document::ModelicaDocument)
/// with a `mem://Untitled<N>` marker path, records it in the Package
/// Browser's in-memory list, and triggers an [`OpenTab`](lunco_workbench::OpenTab)
/// so the user lands on the editable tab immediately.
#[derive(Event, Clone, Debug)]
pub struct CreateNewScratchModel;

/// Request to duplicate a read-only (library) model into a new
/// editable Untitled document.
///
/// The "play with examples" workflow: user drills into
/// `Modelica.Blocks.Examples.PID_Controller`, looks at the diagram,
/// wants to tweak a parameter. Because the MSL class is read-only,
/// we need a second, editable model. This command creates one —
/// same source, stripped of the `within` clause so the copy doesn't
/// claim to live inside `Modelica.*`, opens a fresh tab, leaves the
/// original MSL tab untouched.
///
/// For classes backed by package-aggregated files (e.g.
/// `Blocks/package.mo`), only the target class's source is
/// extracted — otherwise users would get a 150 KB copy of the
/// whole Blocks package as their "Untitled" starting point.
#[derive(Event, Clone, Debug)]
pub struct DuplicateModelFromReadOnly {
    pub source_doc: DocumentId,
}

/// Open an MSL example as a fresh editable copy in the workspace
/// without the user needing to first drill into it.
///
/// The Welcome page's examples strip dispatches this on click.
/// Same effect as `DuplicateModelFromReadOnly` but sourced from a
/// qualified MSL class name rather than an already-open read-only
/// doc — the observer resolves the file path via the MSL class
/// index and runs the whole extract + rewrite + parse pipeline on
/// a background task so the UI stays responsive even for
/// multi-hundred-KB package files.
///
/// The duplicated copy lands in Canvas view by default (examples
/// are composed models — users want to see the diagram, not the
/// source).
#[derive(Event, Clone, Debug)]
pub struct OpenExampleInWorkspace {
    pub qualified: String,
}

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

/// Run the Auto-Arrange layout: assign each component of the active
/// class a deterministic grid position and persist it via a batch of
/// `SetPlacement` ops (undo-able as one group). Matches Dymola's
/// **Edit → Auto Arrange** command. The passive open-time fallback
/// stacks components at origin so nothing jumps around; users invoke
/// this to lay out an imported model cleanly in one click.
///
/// Exposed to the LunCo API: `POST /api/commands` with
/// `{"command": "AutoArrangeDiagram", "params": {"doc": 0}}` where
/// `doc = 0` targets the currently-active tab. Kept as a raw `u64`
/// (not `DocumentId`) so the generic `lunco-doc` crate stays free of
/// the bevy-reflect dependency required to cross the API boundary.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct AutoArrangeDiagram {
    /// Raw `DocumentId::raw()` value, or `0` for "the currently-active
    /// Model tab" (useful from API / tests / scripts that don't track
    /// document ids).
    pub doc: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// API navigation commands — reflect-registered so scripts / tests /
// remote agents can drive the UI over HTTP without a mouse. Each is a
// fire-and-forget event with a tiny observer; all follow the same
// convention as `AutoArrangeDiagram` (doc=0 means "the active tab").
// ─────────────────────────────────────────────────────────────────────────────

/// Focus (open + bring to front) the tab whose title contains the
/// given substring. Case-sensitive; first match wins.
///
/// Useful from the API because the raw `DocumentId` is server-minted
/// and not discoverable from outside; the tab title is. A future
/// `ListDocuments` query will return the ids directly for exact
/// targeting.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct FocusDocumentByName {
    pub pattern: String,
}

/// Switch the active tab's view mode. `mode` is one of
/// `"text"`, `"diagram"`, `"icon"`, `"docs"` (case-insensitive).
/// Unknown modes are ignored.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct SetViewMode {
    /// Doc id, or `0` for the active tab.
    pub doc: u64,
    /// `"text"` | `"diagram"` | `"icon"` | `"docs"`.
    pub mode: String,
}

/// Set the canvas zoom level for a specific diagram. `1.0` = 100 %.
/// `0.0` = fit-all (same as [`FitCanvas`]).
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct SetZoom {
    /// Doc id, or `0` for the active tab.
    pub doc: u64,
    /// Absolute zoom. Clamped to the canvas's configured min/max.
    pub zoom: f32,
}

/// Frame the scene so the whole diagram fits in the viewport.
/// Equivalent to the `F` keyboard shortcut.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct FitCanvas {
    /// Doc id, or `0` for the active tab.
    pub doc: u64,
}

/// Open (or focus, if already open) an MSL class as a fresh editable
/// copy. `qualified` is the full dot-path,
/// e.g. `"Modelica.Electrical.Analog.Examples.ChuaCircuit"`.
/// Reflect-registered shim over the existing `OpenExampleInWorkspace`
/// event so scripts can open examples without knowing the internal
/// event name.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct OpenExample {
    pub qualified: String,
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
        app.init_resource::<CloseDialogState>()
            .init_resource::<PendingCloseAfterSave>()
            .add_observer(on_undo_document)
            .add_observer(on_redo_document)
            .add_observer(on_save_document)
            .add_observer(on_save_as_document)
            .add_observer(finish_close_after_save)
            .add_observer(on_close_document)
            .add_observer(on_document_closed_cleanup)
            .add_observer(on_compile_model)
            .add_observer(on_create_new_scratch_model)
            .add_observer(on_duplicate_model_from_read_only)
            .add_observer(on_open_example_in_workspace)
            // Auto-Arrange: reflect-registered so the LunCo API can
            // fire it via `ExecuteCommand { command: "AutoArrangeDiagram" }`.
            .register_type::<AutoArrangeDiagram>()
            .add_observer(crate::ui::panels::canvas_diagram::on_auto_arrange_diagram)
            // Navigation commands — same reflect-registered pattern so
            // the HTTP API can drive the UI (focus a tab, switch view
            // mode, zoom / fit, drill into an MSL example).
            .register_type::<FocusDocumentByName>()
            .register_type::<SetViewMode>()
            .register_type::<SetZoom>()
            .register_type::<FitCanvas>()
            .register_type::<OpenExample>()
            .register_type::<OpenClass>()
            .register_type::<MoveComponent>()
            .register_type::<PanCanvas>()
            .register_type::<Undo>()
            .register_type::<Redo>()
            .register_type::<Exit>()
            .register_type::<GetFile>()
            .register_type::<FormatDocument>()
            .register_type::<OpenFile>()
            .register_type::<InspectActiveDoc>()
            .add_observer(on_focus_document_by_name)
            .add_observer(on_set_view_mode)
            .add_observer(on_set_zoom)
            .add_observer(on_fit_canvas)
            .add_observer(on_open_example)
            .add_observer(on_open_class)
            .add_observer(on_move_component)
            .add_observer(on_pan_canvas)
            .add_observer(on_undo)
            .add_observer(on_redo)
            .add_observer(on_exit)
            .add_observer(on_get_file)
            .add_observer(on_format_document)
            .add_observer(on_open_file)
            .add_observer(on_inspect_active_doc)
            .add_observer(resolve_editor_intent)
            .add_observer(resolve_new_document_intent)
            .add_systems(
                bevy::prelude::Update,
                (
                    drain_pending_tab_closes,
                    update_status_bar,
                    publish_unsaved_modelica_docs,
                ),
            )
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                render_close_dialogs,
            );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unsaved-changes close prompt
// ─────────────────────────────────────────────────────────────────────────────

/// Per-doc confirmation state for "close tab with unsaved changes".
///
/// The [`CloseTab`](lunco_workbench::CloseTab) event on a dirty doc is
/// gated by this queue: the workbench's on-close hook pushes the tab
/// id into `PendingTabCloses`, `drain_pending_tab_closes` inspects the
/// dirty flag, and dirty tabs land here to await a user decision. The
/// `render_close_dialogs` system draws a modal per entry.
#[derive(Resource, Default)]
pub struct CloseDialogState {
    /// Docs with an open close-confirmation modal.
    pub pending: Vec<DocumentId>,
}

/// Drain `PendingTabCloses` from `lunco_workbench`. Clean docs close
/// immediately; dirty docs get queued for the user-confirmation modal.
///
/// Documents for which the user chose **Save** in the close
/// confirmation dialog. Once each doc fires its `DocumentSaved`, the
/// close completes; if the save is cancelled (Save-As picker dismissed
/// for an Untitled) the doc stays in place and the tab keeps living,
/// matching VS Code's behaviour.
#[derive(Resource, Default)]
pub struct PendingCloseAfterSave {
    docs: std::collections::HashSet<DocumentId>,
}

impl PendingCloseAfterSave {
    fn queue(&mut self, doc: DocumentId) {
        self.docs.insert(doc);
    }
    fn take(&mut self, doc: DocumentId) -> bool {
        self.docs.remove(&doc)
    }
}

/// Observer: after a `DocumentSaved`, finish any close that was
/// waiting on this save. Fires `CloseTab` + `CloseDocument` in order.
fn finish_close_after_save(
    trigger: On<lunco_doc_bevy::DocumentSaved>,
    pending: Option<ResMut<PendingCloseAfterSave>>,
    mut commands: Commands,
) {
    let Some(mut pending) = pending else { return };
    let doc = trigger.event().doc;
    if pending.take(doc) {
        commands.trigger(lunco_workbench::CloseTab {
            kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
            instance: doc.raw(),
        });
        commands.trigger(CloseDocument { doc });
    }
}

/// Runs on `Update`, so it picks up both the tab × button (queued by
/// the workbench's `on_close`) and Ctrl+W (pushed by the
/// EditorIntent::Close resolver below).
fn drain_pending_tab_closes(
    mut pending: ResMut<lunco_workbench::PendingTabCloses>,
    registry: Res<ModelicaDocumentRegistry>,
    mut dialogs: ResMut<CloseDialogState>,
    mut commands: Commands,
) {
    for tab in pending.drain() {
        let lunco_workbench::TabId::Instance { kind, instance } = tab else {
            continue; // Singleton — not our concern.
        };
        if kind != crate::ui::panels::model_view::MODEL_VIEW_KIND {
            continue; // Another domain's tab.
        }
        let doc = DocumentId::new(instance);
        let is_dirty = registry
            .host(doc)
            .map(|h| h.document().is_dirty())
            .unwrap_or(false);
        if is_dirty {
            if !dialogs.pending.contains(&doc) {
                dialogs.pending.push(doc);
            }
        } else {
            // Clean — go straight through.
            commands.trigger(lunco_workbench::CloseTab { kind, instance });
            commands.trigger(CloseDocument { doc });
        }
    }
}

/// Render one modal per entry in [`CloseDialogState`]. Three choices:
/// **Save** (disabled for Untitled until Save-As lands), **Don't save**,
/// **Cancel**. The Save path fires `SaveDocument` + full close; Don't
/// save fires the close directly; Cancel dismisses the dialog.
fn render_close_dialogs(
    mut egui_ctx: bevy_egui::EguiContexts,
    registry: Res<ModelicaDocumentRegistry>,
    mut dialogs: ResMut<CloseDialogState>,
    // `Option<ResMut>` rather than `ResMut` — the system is registered
    // in one of the `EguiPrimaryContextPass` passes which, in Bevy
    // 0.18, can be polled before plugin-level `init_resource`s have
    // taken effect on a world that was externally-constructed (e.g.
    // the minimal-app CI path). Missing resource is a no-op; normal
    // runs always populate it from `ModelicaCommandsPlugin::build`.
    mut pending_save_close: Option<ResMut<PendingCloseAfterSave>>,
    mut commands: Commands,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    // Drain-and-reinsert pattern so we can mutate individual entries
    // without fighting the Vec during iteration.
    let pending = std::mem::take(&mut dialogs.pending);
    let mut survivors = Vec::with_capacity(pending.len());
    for doc in pending {
        let Some(host) = registry.host(doc) else {
            // Doc vanished (another system closed it). Drop the dialog.
            continue;
        };
        let document = host.document();
        let display_name = document.origin().display_name();
        let is_untitled = document.origin().is_untitled();
        let is_read_only = document.is_read_only();
        // Read-only library classes can't be saved at all; the user's
        // only honest options are Don't Save or Cancel. Untitled docs
        // route their Save through Save-As → the picker.
        let can_save = !is_read_only;

        enum DialogAction {
            None,
            Save,
            DontSave,
            Cancel,
        }
        let mut action = DialogAction::None;

        let window_id = egui::Id::new(("unsaved_close_prompt", doc.raw()));
        let mut open = true;
        egui::Window::new(format!("Save changes to '{}'?", display_name))
            .id(window_id)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Your changes will be lost if you don't save them.",
                    )
                    .size(12.0),
                );
                if is_untitled {
                    ui.add_space(4.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 180, 200),
                        "This model has never been saved — picking Save \
                         will open a Save-As dialog to bind it to a file.",
                    );
                }
                if is_read_only {
                    ui.add_space(4.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 150, 50),
                        "This is a read-only library class; Save is \
                         unavailable. Use Duplicate to Workspace if you \
                         want to keep your edits.",
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let save_btn = ui.add_enabled(
                        can_save,
                        egui::Button::new(egui::RichText::new("Save").strong()),
                    );
                    if save_btn.clicked() {
                        action = DialogAction::Save;
                    }
                    if ui.button("Don't save").clicked() {
                        action = DialogAction::DontSave;
                    }
                    if ui.button("Cancel").clicked() {
                        action = DialogAction::Cancel;
                    }
                });
            });
        // Close via the title-bar X also dismisses — treat as Cancel.
        if !open {
            action = DialogAction::Cancel;
        }

        match action {
            DialogAction::None => {
                survivors.push(doc);
            }
            DialogAction::Save => {
                // Queue the close to run *after* the save completes —
                // for Untitled docs the save opens a picker that the
                // user may cancel, in which case the close must NOT
                // proceed. `finish_close_after_save` observer fires
                // CloseTab+CloseDocument when DocumentSaved lands.
                if let Some(q) = pending_save_close.as_mut() {
                    q.queue(doc);
                }
                commands.trigger(SaveDocument { doc });
            }
            DialogAction::DontSave => {
                commands.trigger(lunco_workbench::CloseTab {
                    kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
                    instance: doc.raw(),
                });
                commands.trigger(CloseDocument { doc });
            }
            DialogAction::Cancel => { /* drop from pending */ }
        }
    }
    dialogs.pending = survivors;
}

/// Observer fired after a document is removed from the registry.
/// Cleans up the domain-side state that trailed the document:
/// `ModelTabs` entry, `PackageTreeCache.in_memory_models` entry,
/// `CompileStates` entry.
fn on_document_closed_cleanup(
    trigger: On<lunco_doc_bevy::DocumentClosed>,
    mut model_tabs: ResMut<crate::ui::panels::model_view::ModelTabs>,
    mut cache: ResMut<crate::ui::panels::package_browser::PackageTreeCache>,
    mut compile_states: ResMut<CompileStates>,
    mut workbench: ResMut<WorkbenchState>,
    mut workspace: ResMut<lunco_workbench::WorkspaceResource>,
) {
    let doc = trigger.event().doc;
    model_tabs.close(doc);
    cache.in_memory_models.retain(|e| e.doc != doc);
    compile_states.remove(doc);
    // If the closed doc was active, clear the slot so the welcome
    // panel / another tab's sync can take over. Drive the check off
    // `workspace.active_document` (the source of truth) and reset
    // both the Workspace pointer and the UI cache in lockstep.
    if workspace.active_document == Some(doc) {
        workspace.active_document = None;
        workbench.open_model = None;
        workbench.editor_buffer.clear();
        workbench.compilation_error = None;
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
    workspace: Res<lunco_workbench::WorkspaceResource>,
    registry: Res<ModelicaDocumentRegistry>,
    mut pending_closes: ResMut<lunco_workbench::PendingTabCloses>,
    mut commands: Commands,
) {
    let Some(doc) = workspace.active_document else {
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
        EditorIntent::SaveAs => commands.trigger(SaveAsDocument { doc }),
        EditorIntent::Close => {
            // Ctrl+W goes through the same dirty-check + modal-prompt
            // pipeline as the tab × button. Push the tab id into the
            // workbench's close-request queue; `drain_pending_tab_closes`
            // decides whether to close immediately or prompt.
            pending_closes.push(lunco_workbench::TabId::Instance {
                kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
                instance: doc.raw(),
            });
        }
        EditorIntent::Compile => commands.trigger(CompileModel { doc }),
        // `NewDocument` doesn't need an active doc — it's handled by
        // `NewDocumentNoDoc` resolver below (the resolver that runs
        // even when there's no active doc).
        EditorIntent::NewDocument => {}
    }
}

/// Second EditorIntent resolver that fires regardless of whether an
/// active document is owned by Modelica — handles the intents that
/// have no existing-doc target, currently just `NewDocument`.
///
/// Kept separate from [`resolve_editor_intent`] so the active-doc
/// ownership check there can stay a hard precondition for all other
/// intent variants.
fn resolve_new_document_intent(trigger: On<EditorIntent>, mut commands: Commands) {
    if matches!(*trigger.event(), EditorIntent::NewDocument) {
        commands.trigger(CreateNewScratchModel);
    }
}

fn on_undo_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut editor: ResMut<EditorBufferState>,
    mut workbench: ResMut<WorkbenchState>,
) {
    let doc = trigger.event().doc;
    apply_undo_or_redo(
        doc,
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
    // Ownership check only — `Document::is_read_only()` here means
    // "can't save without Save-As", which is true for every Untitled
    // doc (Duplicate-to-Workspace copies, freshly-typed scratch
    // models). Those are fully editable; the predicate's name is
    // misleading. The canvas's apply_ops gates on
    // `WorkbenchState.open_model.read_only` (true only for
    // bundled / library tabs); we mirror that here so undo/redo
    // works on Untitled docs.
    if registry.host(doc).is_none() {
        return;
    }
    let workbench_read_only = workbench
        .open_model
        .as_ref()
        .map(|m| m.read_only)
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
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;

    // Validate + snapshot what we need to write.
    let to_save = {
        let Some(host) = registry.host(doc) else {
            return; // Foreign / unknown id.
        };
        let document = host.document();
        // Untitled → route through Save-As so the user picks a path.
        // Matches VS Code's behaviour (Ctrl+S on an Untitled buffer
        // opens the Save-As dialog).
        if document.origin().is_untitled() {
            commands.trigger(SaveAsDocument { doc });
            return;
        }
        let Some(path) = document.canonical_path() else {
            console.warn(format!(
                "Save skipped — doc {doc} has no canonical path"
            ));
            return;
        };
        if document.is_read_only() {
            let name = document.origin().display_name();
            let msg = format!("Save blocked — '{name}' is read-only (library / bundled example).");
            warn!("[Save] {msg}");
            console.warn(msg);
            return;
        }
        (path.to_path_buf(), document.source().to_string())
    };

    let (path, source) = to_save;
    // Write through `lunco-storage` so the backend seam is exercised
    // (native today, OPFS / IndexedDB / HTTP tomorrow — same trait).
    let storage = lunco_storage::FileStorage::new();
    let handle = lunco_storage::StorageHandle::File(path.clone());
    if let Err(e) = <lunco_storage::FileStorage as lunco_storage::Storage>::write(
        &storage,
        &handle,
        source.as_bytes(),
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

/// Observer for [`SaveAsDocument`] — fires the native save picker,
/// writes the chosen file, rebinds the document's origin to the new
/// path, and emits [`DocumentSaved`] on success. Cancelling the
/// picker is a silent no-op.
fn on_save_as_document(
    trigger: On<SaveAsDocument>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    workspace: Res<lunco_workbench::WorkspaceResource>,
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    // Snapshot everything we need before the modal picker — `ResMut`s
    // are held across the rfd call but the dialog is blocking on
    // native, so keep the held set small.
    let (source, suggested_name, start_dir) = {
        let Some(host) = registry.host(doc) else { return };
        let document = host.document();
        let suggested = {
            let raw = document.origin().display_name();
            // Attach `.mo` if the user hasn't already chosen a full
            // filename (Untitled<N> is the common case).
            if raw.ends_with(".mo") {
                raw.to_string()
            } else {
                format!("{raw}.mo")
            }
        };
        // Start in the active Twin's folder so Save-As of a scratch
        // doc lands inside the project the user is working on by
        // default. Falls through to the picker's default when no
        // active Twin is set.
        let start = workspace
            .active_twin
            .and_then(|id| workspace.twin(id))
            .map(|t| lunco_storage::StorageHandle::File(t.root.clone()));
        (document.source().to_string(), suggested, start)
    };

    let storage = lunco_storage::FileStorage::new();
    let hint = lunco_storage::SaveHint {
        suggested_name: Some(suggested_name),
        start_dir,
        filters: vec![lunco_storage::OpenFilter::new(
            "Modelica models",
            &["mo"],
        )],
    };
    let handle = match <lunco_storage::FileStorage as lunco_storage::Storage>::pick_save(
        &storage, &hint,
    ) {
        Ok(Some(h)) => h,
        Ok(None) => {
            // User cancelled the picker. Not an error.
            return;
        }
        Err(e) => {
            let msg = format!("Save-As picker failed: {e}");
            error!("[SaveAs] {msg}");
            console.error(msg);
            return;
        }
    };
    let Some(path) = handle.as_file_path().map(std::path::Path::to_path_buf) else {
        // FileStorage only produces `File(..)` handles; defensive.
        console.warn("Save-As returned a non-file handle".to_string());
        return;
    };
    if let Err(e) = <lunco_storage::FileStorage as lunco_storage::Storage>::write(
        &storage,
        &handle,
        source.as_bytes(),
    ) {
        let msg = format!("Save-As failed: {}: {e}", path.display());
        error!("[SaveAs] {msg}");
        console.error(msg);
        return;
    }

    // Rebind the document's origin to the new writable path and mark
    // it saved. `set_origin` does not touch source or generation.
    if let Some(host) = registry.host_mut(doc) {
        host.document_mut().set_origin(
            lunco_doc::DocumentOrigin::File {
                path: path.clone(),
                writable: true,
            },
        );
    }
    registry.mark_document_saved(doc);
    let msg = format!("Saved {} bytes to {}", source.len(), path.display());
    info!("[SaveAs] {msg}");
    console.info(msg);

    commands.trigger(DocumentSaved::local(doc));
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
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    diagram_state: Res<DiagramState>,
    channels: Option<Res<ModelicaChannels>>,
    mut q_models: Query<&mut ModelicaModel>,
) {
    let doc = trigger.event().doc;

    // Ownership check. Read-only docs are fair game to compile —
    // the Save button is what's gated on writability, not compile.
    // Users *simulate* examples; they just can't overwrite them.
    //
    // Use the document's already-parsed AST for the metadata
    // extraction. Calling the `_source` variants here re-parses
    // via rumoca on the main thread — a 152 KB MSL package file
    // costs ~30 s per call in debug builds, and there are four
    // calls, so clicking Compile on an MSL example would lock the
    // UI for minutes. Pulling from the cached AST is constant-time.
    let (source, ast_for_extract) = match registry.host(doc) {
        Some(h) => {
            let doc = h.document();
            let ast = doc.ast().result.as_ref().ok().cloned();
            (doc.source().to_string(), ast)
        }
        None => return,
    };
    let Some(ast) = ast_for_extract else {
        // Parse failure on this doc (rare — rumoca is
        // error-recovering). Fall back to the source-based
        // extractors, which at least try once; if they also fail,
        // the error message below fires.
        let msg = "Could not parse Modelica source for compile.".to_string();
        workbench.compilation_error = Some(msg.clone());
        console.error(format!("Compile failed: {msg}"));
        return;
    };
    let Some(model_name) =
        crate::ast_extract::extract_model_name_from_ast(&ast)
    else {
        let msg = "Could not find a valid model declaration.".to_string();
        workbench.compilation_error = Some(msg.clone());
        console.error(format!("Compile failed: {msg}"));
        return;
    };
    let params = crate::ast_extract::extract_parameters_from_ast(&ast);
    let inputs_with_defaults =
        crate::ast_extract::extract_inputs_with_defaults_from_ast(&ast);
    let runtime_inputs = crate::ast_extract::extract_input_names_from_ast(&ast);

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
                    descriptions: HashMap::new(),
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

    compile_states.mark_started(doc);
    console.info(format!("⏵ Compile started: '{model_name}'"));

    if let Some(channels) = channels {
        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity: target_entity,
            session_id,
            model_name,
            source,
        });
    } else {
        console.error("Modelica worker channel not available — compile dispatch dropped.");
    }
}

fn on_create_new_scratch_model(
    _trigger: On<CreateNewScratchModel>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut cache: ResMut<crate::ui::panels::package_browser::PackageTreeCache>,
    mut model_tabs: ResMut<crate::ui::panels::model_view::ModelTabs>,
    mut workbench: ResMut<WorkbenchState>,
    mut workspace: ResMut<lunco_workbench::WorkspaceResource>,
    mut commands: Commands,
) {
    // Find the lowest `Untitled<N>` not already taken — matches VS
    // Code's `Untitled-1`, `Untitled-2` … semantics.
    let taken: std::collections::HashSet<String> = cache
        .in_memory_models
        .iter()
        .map(|e| e.display_name.clone())
        .collect();
    let mut n: u32 = 1;
    let name = loop {
        let candidate = format!("Untitled{n}");
        if !taken.contains(&candidate) {
            break candidate;
        }
        n += 1;
    };

    let source = format!("model {name}\n\nend {name};\n");
    let mem_id = format!("mem://{name}");
    let doc_id = registry.allocate_with_origin(
        source.clone(),
        lunco_doc::DocumentOrigin::untitled(name.clone()),
    );

    cache.in_memory_models.retain(|e| e.id != mem_id);
    cache
        .in_memory_models
        .push(crate::ui::panels::package_browser::InMemoryEntry {
            display_name: name.clone(),
            id: mem_id.clone(),
            doc: doc_id,
        });

    let source_arc: std::sync::Arc<str> = source.into();
    workbench.open_model = Some(crate::ui::OpenModel {
        model_path: mem_id,
        display_name: name.clone(),
        source: source_arc.clone(),
        line_starts: vec![0].into(),
        detected_name: Some(name),
        cached_galley: None,
        read_only: false,
        library: crate::ui::state::ModelLibrary::InMemory,
    });
    workbench.editor_buffer = source_arc.to_string();
    workbench.diagram_dirty = true;

    // Sync into the Workspace session. The sync observer adds the
    // DocumentEntry on its own; what we need here is the
    // "active-document" pointer.
    workspace.active_document = Some(doc_id);

    model_tabs.ensure(doc_id);
    commands.trigger(lunco_workbench::OpenTab {
        kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
        instance: doc_id.raw(),
    });
}

fn on_duplicate_model_from_read_only(
    trigger: On<DuplicateModelFromReadOnly>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut cache: ResMut<crate::ui::panels::package_browser::PackageTreeCache>,
    mut model_tabs: ResMut<crate::ui::panels::model_view::ModelTabs>,
    class_names: Option<Res<crate::ui::panels::canvas_diagram::DrilledInClassNames>>,
    mut duplicate_loads: ResMut<
        crate::ui::panels::canvas_diagram::DuplicateLoads,
    >,
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut commands: Commands,
) {
    let source_doc = trigger.event().source_doc;

    // UI-thread work only: cheap lookups (registry host, string
    // clones, name collision scan). All heavy work — source text
    // extraction via regex, rewriting, and especially the rumoca
    // parse in `ModelicaDocument::with_origin` — goes to a bg task
    // below. Per the architectural rule: no O(source_bytes) work
    // on the UI thread.
    let (source_full, origin_class_short, origin_fqn) = {
        let Some(host) = registry.host(source_doc) else {
            console.error("Duplicate failed: source doc not found in registry");
            return;
        };
        let doc = host.document();
        let fqn = class_names
            .as_ref()
            .and_then(|m| m.get(source_doc))
            .map(String::from);
        let short = fqn
            .as_ref()
            .and_then(|q| q.rsplit('.').next().map(String::from))
            .unwrap_or_else(|| doc.origin().display_name());
        (doc.source().to_string(), short, fqn)
    };

    // Pick a new Untitled name. Try `<ClassName>Copy` first; fall
    // back to `<ClassName>CopyN` on collision.
    let taken: std::collections::HashSet<String> = cache
        .in_memory_models
        .iter()
        .map(|e| e.display_name.clone())
        .collect();
    let base_name = format!("{origin_class_short}Copy");
    let mut name = base_name.clone();
    let mut n: u32 = 2;
    while taken.contains(&name) {
        name = format!("{base_name}{n}");
        n += 1;
    }

    // Reserve a doc id. No parse, no allocation of a Document —
    // the document is built on the bg task and installed via
    // `install_prebuilt` when ready.
    let doc_id = registry.reserve_id();

    // Register the tab immediately so the user sees a new tab
    // appear in the dock even though content is still being
    // prepared. The drive system fills in the doc when the
    // bg task completes; until then the canvas overlay shows
    // "Loading resource..." for the display name.
    let mem_id = format!("mem://{name}");
    cache.in_memory_models.retain(|e| e.id != mem_id);
    cache
        .in_memory_models
        .push(crate::ui::panels::package_browser::InMemoryEntry {
            display_name: name.clone(),
            id: mem_id,
            doc: doc_id,
        });
    model_tabs.ensure(doc_id);
    // Duplicated copies land in Canvas view — the whole point of
    // "make a playable copy of an MSL example" is to see the
    // diagram. Text view for editing is one toolbar click away.
    if let Some(tab) = model_tabs.get_mut(doc_id) {
        tab.view_mode = crate::ui::panels::model_view::ModelViewMode::Canvas;
    }
    commands.trigger(lunco_workbench::OpenTab {
        kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
        instance: doc_id.raw(),
    });

    // Spawn the heavy work off-thread. Task captures owned data
    // only; no world access from the task.
    let origin_short_for_task = origin_class_short.clone();
    let name_for_task = name.clone();
    let origin_fqn_for_task = origin_fqn.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        // 1. Extract just the target class's source from a
        //    multi-class package file (falls through to the full
        //    source for own-file classes).
        let class_src = extract_class_source(&source_full, &origin_short_for_task)
            .unwrap_or(source_full);
        // 2. Rewrite: rename the class + strip `within` so the
        //    copy is standalone.
        let renamed = rewrite_duplicated_source(
            &class_src,
            &origin_short_for_task,
            &name_for_task,
        );
        // 2b. Inject parent-package imports so scope-dependent refs
        //     like `SI.Angle` still resolve after extraction. No-op
        //     for non-MSL sources (FQN unknown → no path → empty).
        let imports = origin_fqn_for_task
            .as_deref()
            .and_then(crate::class_cache::resolve_msl_class_path)
            .map(|p| collect_parent_imports(&p))
            .unwrap_or_default();
        let renamed = inject_class_imports(&renamed, &imports);
        // 2c. Re-attach a `within <origin package>;` so within-
        //     relative type references in the copy (e.g. PID's
        //     `Blocks.Math.Gain` which is short for
        //     `Modelica.Blocks.Math.Gain`) keep resolving via the
        //     projector's scope-chain fallback. The copy's class name
        //     is new (PIDCopy), so this doesn't collide with the
        //     original. No-op when the origin FQN is unknown
        //     (non-MSL source).
        let copy_src = match origin_fqn_for_task.as_deref() {
            Some(fqn) => {
                let mut parts: Vec<&str> = fqn.split('.').collect();
                parts.pop();
                let origin_pkg = parts.join(".");
                if origin_pkg.is_empty() {
                    renamed
                } else {
                    format!("within {origin_pkg};\n{renamed}")
                }
            }
            None => renamed,
        };
        // 3. Build the document. `with_origin` runs rumoca to
        //    populate the AST cache — bg thread, so the UI stays
        //    responsive even on multi-KB sources.
        crate::document::ModelicaDocument::with_origin(
            doc_id,
            copy_src,
            lunco_doc::DocumentOrigin::untitled(name_for_task),
        )
    });

    duplicate_loads.insert(
        doc_id,
        crate::ui::panels::canvas_diagram::DuplicateBinding {
            display_name: name.clone(),
            origin_short: origin_class_short.clone(),
            started: std::time::Instant::now(),
            task,
        },
    );
    console.info(format!(
        "📄 Duplicating `{origin_class_short}` → `{name}` (building…)"
    ));
}

fn on_open_example_in_workspace(
    trigger: On<OpenExampleInWorkspace>,
    mut cache: ResMut<crate::ui::panels::package_browser::PackageTreeCache>,
    mut model_tabs: ResMut<crate::ui::panels::model_view::ModelTabs>,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut duplicate_loads: ResMut<
        crate::ui::panels::canvas_diagram::DuplicateLoads,
    >,
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut commands: Commands,
) {
    let qualified = trigger.event().qualified.clone();
    let origin_short = qualified
        .rsplit('.')
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| qualified.clone());

    // Pick a new Untitled name, same collision strategy as the
    // sibling `on_duplicate_model_from_read_only`.
    let taken: std::collections::HashSet<String> = cache
        .in_memory_models
        .iter()
        .map(|e| e.display_name.clone())
        .collect();
    let base_name = format!("{origin_short}Copy");
    let mut name = base_name.clone();
    let mut n: u32 = 2;
    while taken.contains(&name) {
        name = format!("{base_name}{n}");
        n += 1;
    }

    // Reserve id + open the tab now so the user sees immediate
    // feedback; the canvas will show "Loading resource…" until
    // the bg build lands via `drive_duplicate_loads`.
    let doc_id = registry.reserve_id();
    let mem_id = format!("mem://{name}");
    cache.in_memory_models.retain(|e| e.id != mem_id);
    cache
        .in_memory_models
        .push(crate::ui::panels::package_browser::InMemoryEntry {
            display_name: name.clone(),
            id: mem_id,
            doc: doc_id,
        });
    // Examples are composed models — land in Canvas view so users
    // see the diagram on open, not the raw source.
    model_tabs.ensure(doc_id);
    if let Some(tab) = model_tabs.get_mut(doc_id) {
        tab.view_mode = crate::ui::panels::model_view::ModelViewMode::Canvas;
    }
    commands.trigger(lunco_workbench::OpenTab {
        kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
        instance: doc_id.raw(),
    });

    // Bg task: resolve path → read file → extract target class →
    // rewrite → build `ModelicaDocument`. All off UI thread.
    let qualified_for_task = qualified.clone();
    let origin_short_for_task = origin_short.clone();
    let name_for_task = name.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        // 1. Resolve MSL file path (static HashMap probe). If the
        //    class isn't indexed, build an empty doc so the user
        //    still gets a tab with a clear error marker.
        let Some(path) = crate::class_cache::resolve_msl_class_path(&qualified_for_task) else {
            return crate::document::ModelicaDocument::with_origin(
                doc_id,
                format!("// Could not locate MSL file for {qualified_for_task}\n"),
                lunco_doc::DocumentOrigin::untitled(name_for_task),
            );
        };
        // 2. Read file. I/O — fine off-thread.
        let source_full = std::fs::read_to_string(&path).unwrap_or_default();
        // 3. Extract just the target class (same helpers used by
        //    `DuplicateModelFromReadOnly`).
        let class_src = extract_class_source(&source_full, &origin_short_for_task)
            .unwrap_or(source_full);
        // 4. Rewrite: rename + strip `within` so the copy is
        //    standalone.
        let renamed = rewrite_duplicated_source(
            &class_src,
            &origin_short_for_task,
            &name_for_task,
        );
        // 4b. Inject parent-package imports (e.g. `import
        //     Modelica.Units.SI;`) so scope-dependent references
        //     resolve once the class is standalone.
        let imports = collect_parent_imports(&path);
        let renamed = inject_class_imports(&renamed, &imports);
        // 4c. Re-attach a `within <origin package>;` clause so the
        //     copy's enclosing-package context is preserved for
        //     scope-chain resolution of bare `extends` refs. The
        //     origin package is `qualified` minus its leaf; falling
        //     back to an empty (unqualified) `within` if the class
        //     was top-level.
        let origin_pkg: String = {
            let mut parts: Vec<&str> = qualified_for_task.split('.').collect();
            parts.pop();
            parts.join(".")
        };
        let copy_src = if origin_pkg.is_empty() {
            renamed
        } else {
            format!("within {origin_pkg};\n{renamed}")
        };
        // 5. Build doc (runs rumoca parse on the bg thread).
        crate::document::ModelicaDocument::with_origin(
            doc_id,
            copy_src,
            lunco_doc::DocumentOrigin::untitled(name_for_task),
        )
    });

    duplicate_loads.insert(
        doc_id,
        crate::ui::panels::canvas_diagram::DuplicateBinding {
            display_name: name.clone(),
            origin_short: origin_short.clone(),
            started: std::time::Instant::now(),
            task,
        },
    );
    console.info(format!(
        "📄 Opening example `{qualified}` → editable `{name}` (building…)"
    ));
}

/// Pull the source text for a named class out of a (possibly
/// multi-class) `.mo` file. Scans for `^\s*(model|block|class|
/// connector|function|record|package|type)\s+<Name>\b` as the
/// opener and `^\s*end\s+<Name>\s*;` as the matching closer.
///
/// Works for the common MSL shapes (own-file class; single target
/// class inside a package file with no shadowing nested class of
/// the same name). Returns `None` if the opener or closer can't be
/// found — caller should fall back to copying the whole source.
fn extract_class_source(source: &str, class_name: &str) -> Option<String> {
    let safe = regex::escape(class_name);
    // Single-line pattern — the earlier multi-line raw-string form
    // contained a literal `\<newline>` (raw strings don't honour
    // line continuations), which made regex compile fail and the
    // caller fall through to copying the whole 152 KB file. Found
    // via a debug session where the duplicated doc was identical
    // to the whole package.mo.
    let opener_pat = format!(
        r"(?m)^\s*(?:partial\s+)?(?:encapsulated\s+)?(?:model|block|class|connector|function|record|package|type)\s+{safe}\b",
        safe = safe,
    );
    let opener = regex::Regex::new(&opener_pat).ok()?;
    let closer_pat = format!(r"(?m)^\s*end\s+{safe}\s*;", safe = safe);
    let closer = regex::Regex::new(&closer_pat).ok()?;
    let start = opener.find(source)?.start();
    // Find the first matching `end <ClassName>;` at or after
    // `start`. Multi-class files can have identically-named nested
    // classes, but in MSL practice `end <Name>;` pairs cleanly
    // with the outer opener we just found.
    let rel_end = closer.find(&source[start..])?.end();
    let end = start + rel_end;
    Some(source[start..end].to_string())
}

/// Walk from a class file's directory up through the filesystem,
/// collecting `import` statements from every `package.mo` on the
/// way. These are the imports that were in scope for the class at
/// its original location — once the class is extracted into a
/// standalone workspace file, it loses that scope, so the imports
/// must be injected into the class body itself (Modelica allows
/// class-local imports).
///
/// Stops walking as soon as a directory has no `package.mo` — that
/// marks the boundary of the enclosing package hierarchy. Returns
/// imports in outer-to-inner order, deduplicated while preserving
/// first-seen position.
///
/// Covers the SI/unit shortcuts that break duplication of MSL
/// examples: e.g. `Modelica/Blocks/package.mo` declares
/// `import Modelica.Units.SI;` which is why `SI.Angle` resolves
/// inside `Modelica.Blocks.Examples.PID_Controller` but not in a
/// naïvely extracted copy.
fn collect_parent_imports(class_file: &std::path::Path) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut dir = class_file.parent();
    while let Some(d) = dir {
        let pkg = d.join("package.mo");
        if !pkg.exists() {
            break;
        }
        if let Ok(src) = std::fs::read_to_string(&pkg) {
            // Only scan the **package preamble** — the region
            // between the enclosing `package Foo` header and the
            // first nested class declaration. MSL package.mo files
            // routinely inline whole example models, whose own
            // local `import` statements must NOT be promoted into
            // the duplicated class (seen in the wild:
            // `Blocks/Examples/package.mo` contains multiple
            // nested examples each doing `import distribution =
            // Modelica.Math.Distributions.X.density;` — lifting
            // two of those into one class yields a duplicate-alias
            // parse error).
            let class_opener = regex::Regex::new(
                r"^\s*(?:partial\s+)?(?:encapsulated\s+)?(?:model|block|class|connector|function|record|package|type)\s+",
            );
            let mut entered_package = false;
            let mut level: Vec<String> = Vec::new();
            for line in src.lines() {
                let is_opener = class_opener
                    .as_ref()
                    .map(|re| re.is_match(line))
                    .unwrap_or(false);
                if is_opener {
                    if entered_package {
                        break;
                    }
                    entered_package = true;
                    continue;
                }
                if entered_package {
                    let t = line.trim();
                    if t.starts_with("import ") && t.ends_with(';') {
                        level.push(t.to_string());
                    }
                }
            }
            // Level is the outer-relative-to-previous step. Prepend
            // so the final chain is outer-first, inner-last.
            let mut merged = level;
            merged.extend(chain.drain(..));
            chain = merged;
        }
        dir = d.parent();
    }
    let mut seen = std::collections::HashSet::new();
    chain.retain(|s| seen.insert(s.clone()));
    chain
}

/// Insert a block of `import` statements right after the class
/// header line. Used after `rewrite_duplicated_source` so the
/// header has already been renamed. Returns the input unmodified
/// when the list is empty or the header can't be located — a copy
/// that still needs an import fix is strictly better than a copy
/// with a broken header.
fn inject_class_imports(src: &str, imports: &[String]) -> String {
    if imports.is_empty() {
        return src.to_string();
    }
    // Match the first class header line (including any trailing
    // description string) and capture through to its newline. Same
    // header shapes as `extract_class_source` / `rewrite_*`.
    let header_re = regex::Regex::new(
        r"(?m)^(\s*(?:partial\s+)?(?:encapsulated\s+)?(?:model|block|class|connector|function|record|package|type)\s+[A-Za-z_]\w*[^\n]*)\n",
    )
    .ok();
    let Some(re) = header_re else {
        return src.to_string();
    };
    let Some(m) = re.find(src) else {
        return src.to_string();
    };
    let mut insert_at = m.end();
    // Per MLS the class name may be followed by a description string
    // (optionally split across lines, or even broken over multiple
    // adjacent quoted strings). Imports must land *after* it — the
    // grammar forbids anything between the class name and its
    // description. Advance past whitespace and any leading quoted
    // string(s) before injecting.
    let bytes = src.as_bytes();
    let skip_ws = |mut i: usize| {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        i
    };
    let mut scan = skip_ws(insert_at);
    while scan < bytes.len() && bytes[scan] == b'"' {
        let mut j = scan + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' if j + 1 < bytes.len() => j += 2,
                b'"' => { j += 1; break; }
                _ => j += 1,
            }
        }
        insert_at = j;
        scan = skip_ws(j);
    }
    // Inject on its own new line so the imports don't glue to the
    // description's trailing `"`.
    let needs_leading_newline = insert_at > 0 && bytes[insert_at - 1] != b'\n';
    let block: String = imports
        .iter()
        .map(|i| format!("  {i}\n"))
        .collect();
    let mut out = String::with_capacity(src.len() + block.len() + 1);
    out.push_str(&src[..insert_at]);
    if needs_leading_newline {
        out.push('\n');
    }
    out.push_str(&block);
    out.push_str(&src[insert_at..]);
    out
}

/// Rename the class and strip any `within` clause so the copy is a
/// standalone Untitled model. Conservative: if anything doesn't
/// match exactly once, returns the input unmodified — a user-
/// visible but working "not quite renamed" copy beats a mangled
/// source.
fn rewrite_duplicated_source(
    src: &str,
    old_name: &str,
    new_name: &str,
) -> String {
    let safe_old = regex::escape(old_name);
    // Single-line patterns for the same reason noted in
    // `extract_class_source` — raw strings don't do line
    // continuation.
    let header_pat = format!(
        r"(?m)^(\s*(?:partial\s+)?(?:encapsulated\s+)?(?:model|block|class|connector|function|record|package|type)\s+){safe}\b",
        safe = safe_old,
    );
    let header_re = regex::Regex::new(&header_pat).ok();
    let footer_pat = format!(r"(?m)^(\s*end\s+){safe}(\s*;)", safe = safe_old);
    let footer_re = regex::Regex::new(&footer_pat).ok();
    let within_re =
        regex::Regex::new(r"(?m)^\s*within\s*[A-Za-z_][\w\.]*\s*;\s*\n?").ok();

    let mut out: std::borrow::Cow<'_, str> = src.into();
    if let Some(re) = within_re {
        out = re.replace(&out, "").into_owned().into();
    }
    if let (Some(hr), Some(fr)) = (header_re, footer_re) {
        if hr.find(&out).is_some() && fr.find(&out).is_some() {
            let stepped = hr.replace(&out, |caps: &regex::Captures| {
                format!("{}{new_name}", &caps[1])
            });
            let stepped = fr.replace(&stepped, |caps: &regex::Captures| {
                format!("{}{new_name}{}", &caps[1], &caps[2])
            });
            out = stepped.into_owned().into();
        }
    }
    out.into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// API navigation observers
// ─────────────────────────────────────────────────────────────────────────────
//
// Each is a tiny, predictable translator from a reflect-registered
// event to the domain-specific action. `doc=0` means "active tab"
// across all of them (see also `AutoArrangeDiagram`). Observers can't
// take `&mut World` in Bevy 0.18, so the ones that need it defer via
// `commands.queue(|world| ...)` — same trick Auto-Arrange uses.

fn resolve_active_doc(world: &World) -> Option<DocumentId> {
    world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document)
}

fn on_focus_document_by_name(
    trigger: On<FocusDocumentByName>,
    mut commands: Commands,
) {
    let pattern = trigger.event().pattern.clone();
    if pattern.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        // Case-insensitive substring match across the session's open
        // documents. First match wins.
        let hit = {
            let ws = world.resource::<lunco_workbench::WorkspaceResource>();
            let needle = pattern.to_lowercase();
            ws.documents()
                .iter()
                .find(|d| d.title.to_lowercase().contains(&needle))
                .map(|d| d.id)
        };
        let Some(doc) = hit else {
            bevy::log::info!(
                "[FocusDocumentByName] no tab matches '{}'",
                pattern
            );
            return;
        };
        world.commands().trigger(lunco_workbench::OpenTab {
            kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
            instance: doc.raw(),
        });
    });
}

fn on_set_view_mode(trigger: On<SetViewMode>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let mode_str = trigger.event().mode.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw == 0 {
            resolve_active_doc(world)
        } else {
            Some(DocumentId::new(raw))
        }) else {
            return;
        };
        use crate::ui::panels::model_view::{ModelTabs, ModelViewMode};
        let new_mode = match mode_str.to_lowercase().as_str() {
            "text" | "source" => ModelViewMode::Text,
            "diagram" | "canvas" => ModelViewMode::Canvas,
            "icon" => ModelViewMode::Icon,
            "docs" | "documentation" => ModelViewMode::Docs,
            other => {
                bevy::log::warn!(
                    "[SetViewMode] unknown mode '{other}' — expected text|diagram|icon|docs"
                );
                return;
            }
        };
        if let Some(mut tabs) = world.get_resource_mut::<ModelTabs>() {
            if let Some(state) = tabs.get_mut(doc) {
                state.view_mode = new_mode;
            }
        }
    });
}

/// Approximate screen rect used by the API-side fit command. The
/// real canvas rect is only known at render time; picking 800×600
/// here matches the Fit-All menu button and produces a reasonable
/// zoom for API-driven workflows where the window size varies.
fn approx_screen_rect() -> lunco_canvas::Rect {
    lunco_canvas::Rect::from_min_max(
        lunco_canvas::Pos::new(0.0, 0.0),
        lunco_canvas::Pos::new(800.0, 600.0),
    )
}

fn on_set_zoom(trigger: On<SetZoom>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let zoom = trigger.event().zoom;
    commands.queue(move |world: &mut World| {
        let doc = if raw == 0 {
            resolve_active_doc(world)
        } else {
            Some(DocumentId::new(raw))
        };
        use crate::ui::panels::canvas_diagram::CanvasDiagramState;
        let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get_mut(doc);
        if zoom <= 0.0 {
            // zoom = 0 → fit-all. Keeps the API callable by scripts
            // that don't want to distinguish Fit from SetZoom.
            if let Some(bounds) = docstate.canvas.scene.bounds() {
                let sr = approx_screen_rect();
                let (c, z) = docstate.canvas.viewport.fit_values(bounds, sr, 40.0);
                docstate.canvas.viewport.set_target(c, z);
            }
        } else {
            let vp = &mut docstate.canvas.viewport;
            let c = vp.center;
            vp.set_target(c, zoom);
        }
    });
}

fn on_fit_canvas(trigger: On<FitCanvas>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let doc = if raw == 0 {
            resolve_active_doc(world)
        } else {
            Some(DocumentId::new(raw))
        };
        use crate::ui::panels::canvas_diagram::CanvasDiagramState;
        let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        // Defer to next render so Fit uses the canvas widget's
        // actual rect, not a hardcoded approximation. Without this
        // the observer-side fit picks zoom for an 800×600 viewport
        // even when the real one is 1700×800, leaving content
        // clipped at the top under the toolbar.
        state.get_mut(doc).pending_fit = true;
    });
}

fn on_open_example(
    trigger: On<OpenExample>,
    mut commands: Commands,
) {
    // Shim over `OpenExampleInWorkspace` with a simpler name for the
    // public API surface. Internal callers can keep using the old
    // event directly; this observer just re-fires.
    let qualified = trigger.event().qualified.clone();
    commands.trigger(OpenExampleInWorkspace { qualified });
}

/// Open a class in a **read-only** tab — the same path the canvas's
/// double-click-to-drill-in gesture uses. Unlike [`OpenExample`] (which
/// duplicates into an editable Untitled doc), this opens the class
/// directly as an `msl://` tab for exploration. Reuses an existing
/// tab if the same class is already open.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct OpenClass {
    pub qualified: String,
}

fn on_open_class(trigger: On<OpenClass>, mut commands: Commands) {
    let qualified = trigger.event().qualified.clone();
    commands.queue(move |world: &mut World| {
        crate::ui::panels::canvas_diagram::drill_into_class(world, &qualified);
    });
}

/// Move a component instance to a new `(x, y)` position in Modelica
/// diagram coordinates (-100..100, +Y up). Same code path the mouse
/// drag uses — emits a `SetPlacement` op so undo/redo + source
/// rewrite work uniformly. `class` empty ⇒ active editing class on
/// the active tab.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct MoveComponent {
    pub class: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    /// Optional explicit extent. Empty (0.0, 0.0) means "preserve
    /// the existing extent" — reads it from the live scene the same
    /// way mouse-drag does.
    pub width: f32,
    pub height: f32,
}

/// Undo the most recent edit on the active document. Reflect-
/// registered so automation can drive the same undo path the
/// Ctrl+Z keybinding / toolbar arrow uses. `doc=0` ⇒ active tab.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct Undo {
    pub doc: u64,
}

/// Redo the most recently undone edit. Mirror of [`Undo`].
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct Redo {
    pub doc: u64,
}

fn on_undo(trigger: On<Undo>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw == 0 {
            resolve_active_doc(world)
        } else {
            Some(DocumentId::new(raw))
        }) else {
            bevy::log::warn!("[Undo] no active document");
            return;
        };
        world.commands().trigger(UndoDocument { doc });
    });
}

fn on_redo(trigger: On<Redo>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw == 0 {
            resolve_active_doc(world)
        } else {
            Some(DocumentId::new(raw))
        }) else {
            bevy::log::warn!("[Redo] no active document");
            return;
        };
        world.commands().trigger(RedoDocument { doc });
    });
}

/// Pan the canvas viewport to centre on `(x, y)` in canvas world
/// coords (+Y down — same frame the projector emits node positions
/// in). Use it from API tests / automation to position the
/// viewport before screenshotting.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct PanCanvas {
    /// 0 ⇒ active document.
    pub doc: u64,
    pub x: f32,
    pub y: f32,
}

/// Gracefully shut down the application. Exposed so automation can
/// stop the workbench without the operator having to confirm a kill
/// signal each time.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct Exit {}

/// Run rumoca-tool-fmt on the active document and replace its
/// source with the formatted text. Single undo step. No-op on
/// read-only tabs or when formatting fails (parse errors etc.).
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct FormatDocument {
    /// 0 ⇒ active document.
    pub doc: u64,
}

fn on_format_document(trigger: On<FormatDocument>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        use crate::document::ModelicaOp;
        let doc = if raw == 0 {
            resolve_active_doc(world)
        } else {
            Some(DocumentId::new(raw))
        };
        let Some(doc) = doc else {
            bevy::log::warn!("[FormatDocument] no active document");
            return;
        };
        let workbench_read_only = world
            .get_resource::<crate::ui::WorkbenchState>()
            .and_then(|s| s.open_model.as_ref().map(|m| m.read_only))
            .unwrap_or(false);
        if workbench_read_only {
            bevy::log::info!("[FormatDocument] tab is read-only — skipping");
            return;
        }
        let Some(registry) = world.get_resource::<crate::ui::state::ModelicaDocumentRegistry>()
        else {
            return;
        };
        let Some(host) = registry.host(doc) else { return };
        let original = host.document().source().to_string();
        let opts = rumoca_tool_fmt::FormatOptions::default();
        let formatted = match rumoca_tool_fmt::format_with_source_name(
            &original, &opts, "<editor>",
        ) {
            Ok(s) => s,
            Err(e) => {
                bevy::log::warn!("[FormatDocument] format failed: {}", e);
                return;
            }
        };
        if formatted == original {
            return;
        }
        // Route through the document op pipeline so undo/redo +
        // canvas reprojection both work the same way as a manual
        // edit.
        let mut registry = world.resource_mut::<crate::ui::state::ModelicaDocumentRegistry>();
        if let Some(host) = registry.host_mut(doc) {
            let _ = host.apply(ModelicaOp::ReplaceSource { new: formatted });
        }
    });
}

/// Publish every Untitled (in-memory, not yet saved) Modelica
/// document into the cross-domain `UnsavedDocs` resource the Files
/// browser section reads.
///
/// **Change-driven, not per-frame.** Bevy's `Res::is_changed()` flips
/// only on the tick when something mutated the registry (allocate,
/// install_prebuilt, remove_document, set_origin, …). When neither
/// the registry nor the cross-domain resource has ticked since our
/// last write, bail without recomputing — saves walking the doc
/// list every frame for a UI surface that changes a few times per
/// session.
fn publish_unsaved_modelica_docs(
    registry: Res<crate::ui::state::ModelicaDocumentRegistry>,
    unsaved: Option<ResMut<lunco_workbench::UnsavedDocs>>,
) {
    let Some(mut unsaved) = unsaved else { return };
    if !registry.is_changed() && !unsaved.is_added() {
        return;
    }
    unsaved.entries = registry
        .iter()
        // Workspace = user content. Read-only library docs (MSL
        // classes the user clicked into) aren't part of the
        // workspace — same filter the Modelica section uses.
        .filter(|(_, host)| {
            let o = host.document().origin();
            o.is_writable() || o.is_untitled()
        })
        .map(|(_, host)| {
            let origin = host.document().origin();
            lunco_workbench::UnsavedDocEntry {
                display_name: origin.display_name(),
                kind: "Modelica".into(),
                is_unsaved: origin.is_untitled(),
            }
        })
        .collect();
}

/// Surface the active document's compile state + workspace activity
/// in the workbench status bar so users can tell at a glance what's
/// running. Reads-only — runs every frame, writes via
/// `WorkbenchLayout::set_status`.
///
/// Status priority (first-match wins):
///   1. Compile in flight on active doc → "Compiling <model>…".
///   2. Compile error on active doc → "Compile error".
///   3. Compile ready on active doc → "Compiled <model>".
///   4. No active doc → "ready".
fn update_status_bar(
    workbench: Res<crate::ui::WorkbenchState>,
    workspace: Option<Res<lunco_workbench::WorkspaceResource>>,
    compile_states: Res<crate::ui::CompileStates>,
    layout: Option<ResMut<lunco_workbench::WorkbenchLayout>>,
) {
    let Some(mut layout) = layout else { return };
    // Re-render only when something a status reader cares about
    // ticked: the active document changed, the compile state
    // transitioned, the open model swapped. Cheap idle path —
    // most frames have no change.
    let any_change = workbench.is_changed()
        || compile_states.is_changed()
        || workspace.as_ref().map(|w| w.is_changed()).unwrap_or(false);
    if !any_change && !layout.is_added() {
        return;
    }
    let active_doc = workspace.as_ref().and_then(|w| w.active_document);
    let model_name = workbench
        .open_model
        .as_ref()
        .and_then(|m| m.detected_name.clone())
        .or_else(|| {
            workbench
                .open_model
                .as_ref()
                .map(|m| m.model_path.clone())
        })
        .unwrap_or_else(|| "(untitled)".to_string());

    let text = match active_doc {
        None => "ready".to_string(),
        Some(doc) => match compile_states.state_of(doc) {
            crate::ui::CompileState::Compiling => format!("⏳ Compiling {model_name}…"),
            crate::ui::CompileState::Error => format!("⚠ Compile error in {model_name}"),
            crate::ui::CompileState::Ready => format!("✓ Compiled {model_name}"),
            crate::ui::CompileState::Idle => format!("● {model_name}"),
        },
    };
    layout.set_status(text);
}

/// Inspect the active document's parsed AST and log the results
/// (top-level class names, parse error if any). API automation
/// uses this to diagnose why a drill-in or projection produced
/// zero nodes — if the AST is empty, the file failed strict parse.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct InspectActiveDoc {}

fn on_inspect_active_doc(_trigger: On<InspectActiveDoc>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        let doc = resolve_active_doc(world);
        let Some(doc) = doc else {
            bevy::log::warn!("[InspectActiveDoc] no active document");
            return;
        };
        let registry = world.resource::<crate::ui::state::ModelicaDocumentRegistry>();
        let Some(host) = registry.host(doc) else {
            bevy::log::warn!("[InspectActiveDoc] doc {} not in registry", doc.raw());
            return;
        };
        let document = host.document();
        let cache = document.ast();
        let origin = document.origin();
        bevy::log::info!(
            "[InspectActiveDoc] doc={} origin={:?} source_len={} gen={}",
            doc.raw(),
            origin.display_name(),
            document.source().len(),
            cache.generation,
        );
        match cache.result.as_ref() {
            Ok(ast) => {
                bevy::log::info!(
                    "[InspectActiveDoc]   parse OK; within={:?}",
                    ast.within.as_ref().map(|w| w.to_string()),
                );
                fn dump(
                    name: &str,
                    class: &rumoca_session::parsing::ast::ClassDef,
                    depth: usize,
                ) {
                    let indent = "  ".repeat(depth + 1);
                    let comps: Vec<String> = class
                        .components
                        .iter()
                        .map(|(n, c)| format!("{}: {}", n, c.type_name))
                        .collect();
                    bevy::log::info!(
                        "[InspectActiveDoc]{}{} ({:?}) extends={} components=[{}]",
                        indent,
                        name,
                        class.class_type,
                        class.extends.len(),
                        comps.join(", "),
                    );
                    for (cn, child) in &class.classes {
                        dump(cn, child, depth + 1);
                    }
                }
                for (n, c) in &ast.classes {
                    dump(n, c, 0);
                }
            }
            Err(e) => {
                bevy::log::warn!("[InspectActiveDoc]   parse ERR: {}", e);
            }
        }
    });
}

/// Open an arbitrary `.mo` file from disk into a new workspace
/// tab as an Untitled document seeded from the file's contents.
/// Used by API automation to load bundled examples or external
/// files without a Twin folder being open.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct OpenFile {
    pub path: String,
}

fn on_open_file(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    commands.queue(move |world: &mut World| {
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                bevy::log::warn!("[OpenFile] {} read failed: {}", path, e);
                return;
            }
        };
        let path_buf = std::path::PathBuf::from(&path);
        let stem = path_buf
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Opened")
            .to_string();
        let mut registry =
            world.resource_mut::<crate::ui::state::ModelicaDocumentRegistry>();
        let doc_id = registry.allocate_with_origin(
            source.clone(),
            lunco_doc::DocumentOrigin::File {
                path: path_buf,
                writable: true,
            },
        );
        // Land in Canvas view so the user sees the diagram.
        let mut tabs = world.resource_mut::<crate::ui::panels::model_view::ModelTabs>();
        tabs.ensure(doc_id);
        if let Some(tab) = tabs.get_mut(doc_id) {
            tab.view_mode = crate::ui::panels::model_view::ModelViewMode::Canvas;
        }
        world.commands().trigger(lunco_workbench::OpenTab {
            kind: crate::ui::panels::model_view::MODEL_VIEW_KIND,
            instance: doc_id.raw(),
        });
        bevy::log::info!("[OpenFile] opened `{}` as `{}`", path, stem);
    });
}

/// Read a file from the filesystem and log its contents to the
/// console at INFO level. Useful for automation that wants to
/// fetch a file's content via the API without spawning a separate
/// shell. Resolves `path` relative to the workbench's current
/// working directory.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct GetFile {
    pub path: String,
}

fn on_get_file(trigger: On<GetFile>) {
    let path = trigger.event().path.clone();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            bevy::log::info!(
                "[GetFile] {} ({} bytes) -- BEGIN --\n{}\n-- END --",
                path,
                content.len(),
                content,
            );
        }
        Err(e) => {
            bevy::log::warn!("[GetFile] {} read failed: {}", path, e);
        }
    }
}

fn on_exit(_trigger: On<Exit>, mut commands: Commands) {
    bevy::log::info!("[Exit] AppExit triggered via API");
    commands.queue(|world: &mut World| {
        if let Some(mut messages) =
            world.get_resource_mut::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
        {
            messages.write(bevy::app::AppExit::Success);
        }
    });
}

fn on_pan_canvas(trigger: On<PanCanvas>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let doc = if ev.doc == 0 {
            resolve_active_doc(world)
        } else {
            Some(DocumentId::new(ev.doc))
        };
        use crate::ui::panels::canvas_diagram::CanvasDiagramState;
        let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get_mut(doc);
        let z = docstate.canvas.viewport.zoom;
        docstate.canvas.viewport.set_target(lunco_canvas::Pos::new(ev.x, ev.y), z);
    });
}

fn on_move_component(trigger: On<MoveComponent>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use crate::document::ModelicaOp;
        use crate::pretty::Placement;
        let active_doc = world
            .get_resource::<lunco_workbench::WorkspaceResource>()
            .and_then(|ws| ws.active_document);
        let Some(doc_id) = active_doc else {
            bevy::log::warn!("[MoveComponent] no active document");
            return;
        };
        let class = if ev.class.is_empty() {
            // Mirror canvas_diagram::resolve_doc_context: read the
            // active editing class from the Workbench's open_model
            // detected name if we don't have one explicitly.
            world
                .get_resource::<crate::ui::panels::canvas_diagram::DrilledInClassNames>()
                .and_then(|m| m.get(doc_id).map(str::to_string))
                .or_else(|| {
                    world.get_resource::<crate::ui::WorkbenchState>()
                        .and_then(|s| s.open_model.as_ref().map(|m| m.detected_name.clone()))
                        .flatten()
                })
                .unwrap_or_default()
        } else {
            ev.class.clone()
        };
        if class.is_empty() {
            bevy::log::warn!("[MoveComponent] could not resolve target class for doc");
            return;
        }
        // Use specified extent if provided, otherwise preserve the
        // node's current rect from the canvas scene (same logic as
        // the mouse-drag path).
        let (width, height) = if ev.width > 0.0 && ev.height > 0.0 {
            (ev.width, ev.height)
        } else {
            use crate::ui::panels::canvas_diagram::CanvasDiagramState;
            world
                .get_resource::<CanvasDiagramState>()
                .and_then(|state| {
                    let docstate = state.get(Some(doc_id));
                    docstate.canvas.scene.nodes().find_map(|(_id, n)| {
                        if n.origin.as_deref() == Some(ev.name.as_str()) {
                            Some((n.rect.width().max(1.0), n.rect.height().max(1.0)))
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or((20.0, 20.0))
        };
        let op = ModelicaOp::SetPlacement {
            class: class.clone(),
            name: ev.name.clone(),
            placement: Placement {
                x: ev.x,
                y: ev.y,
                width,
                height,
            },
        };
        crate::ui::panels::canvas_diagram::apply_ops_public(world, doc_id, vec![op]);
    });
}
