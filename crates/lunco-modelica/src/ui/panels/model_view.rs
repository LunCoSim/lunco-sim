//! `ModelViewPanel` — multi-instance center tab, one per open document.
//!
//! Implements [`InstancePanel`] so the workbench can host arbitrarily
//! many model tabs in the center dock. Each tab's instance id is the
//! raw [`DocumentId`] it views; per-tab state (current view mode,
//! future: text cursor, pan/zoom) lives in the [`ModelTabs`] resource.
//!
//! Rendering strategy for v1: the active tab writes
//! `WorkbenchState.open_model` and sets `diagram_dirty` on every
//! render pass. That keeps the existing side panels (Telemetry,
//! Inspector, Graphs), the code editor body, and the diagram body
//! working unchanged — they all read the singleton `open_model` /
//! `EditorBufferState` / `DiagramState` as before. The cost is that
//! split views (two tabs visible at once) currently flicker as each
//! render pass overwrites the singletons; real per-tab view state
//! will move the buffer and diagram state into `ModelTabs` in a
//! follow-up pass.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_workbench::{InstancePanel, Panel, PanelId, PanelSlot};

use crate::ui::panels::code_editor::EditorBufferState;
use crate::ui::panels::{code_editor::CodeEditorPanel, diagram::DiagramPanel};
use crate::ui::{CompileState, CompileStates, ModelicaDocumentRegistry, WorkbenchState};

/// The `PanelId` under which `ModelViewPanel` is registered as an
/// instance-panel kind. Instance ids are [`DocumentId::raw`] values.
pub const MODEL_VIEW_KIND: PanelId = PanelId("modelica_model_view");

/// Which rendering mode a model tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelViewMode {
    /// Raw Modelica source (egui TextEdit).
    #[default]
    Text,
    /// Block-diagram canvas (egui-snarl).
    Diagram,
}

/// Per-tab state for a [`ModelViewPanel`] instance. One entry per
/// currently-open document.
///
/// Kept minimal for v1 — currently just the view mode. Future fields
/// (text cursor, scroll, pan, zoom, selection) land here when we move
/// to truly independent split views.
#[derive(Debug, Clone)]
pub struct ModelTabState {
    /// The Document this tab is viewing. Redundant with the
    /// [`ModelTabs`] map key but kept on the value for ergonomic
    /// access from render code.
    pub doc: DocumentId,
    /// Text vs Diagram.
    pub view_mode: ModelViewMode,
}

/// Registry of open [`ModelViewPanel`] tabs, keyed by [`DocumentId`].
///
/// "One tab per document" — opening the same `.mo` file twice focuses
/// the existing tab instead of spawning a duplicate. Closing a tab
/// drops the entry here but *does not* remove the underlying
/// `ModelicaDocument` from [`ModelicaDocumentRegistry`]; the document
/// survives in the Package Browser's "Your Models" list so the user
/// can reopen it later.
#[derive(Resource, Default)]
pub struct ModelTabs {
    tabs: HashMap<DocumentId, ModelTabState>,
}

impl ModelTabs {
    /// Ensure a tab exists for `doc` and return its instance id.
    ///
    /// Call this together with
    /// [`WorkbenchLayout::open_instance`](lunco_workbench::WorkbenchLayout::open_instance)
    /// — that adds/focuses the dock tab, this records the per-tab
    /// view state.
    pub fn ensure(&mut self, doc: DocumentId) -> u64 {
        self.tabs.entry(doc).or_insert_with(|| ModelTabState {
            doc,
            view_mode: ModelViewMode::default(),
        });
        doc.raw()
    }

    /// Drop the per-tab state for `doc`. Pair with
    /// [`WorkbenchLayout::close_instance`](lunco_workbench::WorkbenchLayout::close_instance).
    pub fn close(&mut self, doc: DocumentId) {
        self.tabs.remove(&doc);
    }

    /// Immutable lookup by document id.
    pub fn get(&self, doc: DocumentId) -> Option<&ModelTabState> {
        self.tabs.get(&doc)
    }

    /// Mutable lookup.
    pub fn get_mut(&mut self, doc: DocumentId) -> Option<&mut ModelTabState> {
        self.tabs.get_mut(&doc)
    }

    /// Whether a tab exists for `doc`.
    pub fn contains(&self, doc: DocumentId) -> bool {
        self.tabs.contains_key(&doc)
    }
}

/// The Modelica model-view panel. Zero-sized — per-tab state lives in
/// [`ModelTabs`], the render body delegates to the existing code /
/// diagram panels.
pub struct ModelViewPanel {
    /// Reused renderers for the tab body. The unified toolbar is
    /// rendered by [`render_unified_toolbar`] before dispatching to
    /// one of these based on the tab's current view mode.
    code: CodeEditorPanel,
    diagram: DiagramPanel,
}

impl Default for ModelViewPanel {
    fn default() -> Self {
        Self {
            code: CodeEditorPanel,
            diagram: DiagramPanel,
        }
    }
}

impl InstancePanel for ModelViewPanel {
    fn kind(&self) -> PanelId {
        MODEL_VIEW_KIND
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        true
    }

    fn title(&self, world: &World, instance: u64) -> String {
        // Tab title mirrors VS Code: base name + a leading `●` when
        // the underlying document has unsaved changes. The dot is the
        // only save-status indicator — there's no explicit Save button.
        let doc = DocumentId::new(instance);
        let (base, dirty) = resolve_tab_title(world, doc);
        if dirty {
            format!("● {}", base)
        } else {
            base
        }
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World, instance: u64) {
        let doc = DocumentId::new(instance);

        // Make sure the tab has a state entry (idempotent).
        world.resource_mut::<ModelTabs>().ensure(doc);

        // Sync the singleton `open_model` / editor buffer / diagram
        // state to *this* tab before rendering. Side panels and the
        // legacy body renderers all read these singletons; this is
        // the v1 "active tab" signal. Splits of multiple tabs still
        // share the singletons — per-tab buffers are a follow-up.
        sync_active_tab_to_doc(world, doc);

        // Read the tab's desired view mode so the toolbar can reflect
        // (and, on click, mutate) it.
        let view_mode = world
            .resource::<ModelTabs>()
            .get(doc)
            .map(|s| s.view_mode)
            .unwrap_or_default();

        let new_view_mode = render_unified_toolbar(doc, view_mode, ui, world);
        if new_view_mode != view_mode {
            if let Some(state) = world.resource_mut::<ModelTabs>().get_mut(doc) {
                state.view_mode = new_view_mode;
            }
        }

        ui.separator();

        // Body — delegate to the existing code / diagram panels
        // (both of which still read `open_model` / `EditorBufferState`
        // / `DiagramState`, which `sync_active_tab_to_doc` just
        // pointed at this tab's document).
        match new_view_mode {
            ModelViewMode::Text => self.code.render(ui, world),
            ModelViewMode::Diagram => self.diagram.render(ui, world),
        }
    }
}

/// Compute the base tab title (no dirty dot) and dirty flag for
/// `doc`. The tab's `InstancePanel::title` uses these to render.
fn resolve_tab_title(world: &World, doc: DocumentId) -> (String, bool) {
    if let Some(host) = world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
    {
        let document = host.document();
        let base = document
            .canonical_path()
            .and_then(|p| {
                // `mem://Untitled1` → "Untitled1"; real paths → file stem.
                let s = p.to_string_lossy();
                if let Some(name) = s.strip_prefix("mem://") {
                    Some(name.to_string())
                } else {
                    p.file_stem().map(|x| x.to_string_lossy().into_owned())
                }
            })
            .unwrap_or_else(|| format!("Untitled"));
        return (base, document.is_dirty());
    }

    // Fall back to any live `open_model.display_name` if it matches.
    if let Some(state) = world.get_resource::<WorkbenchState>() {
        if let Some(open) = state.open_model.as_ref() {
            if open.doc == Some(doc) {
                return (open.display_name.clone(), false);
            }
        }
    }
    (format!("Model #{}", doc.raw()), false)
}

/// Point the singleton `WorkbenchState.open_model` / `editor_buffer`
/// / `diagram_dirty` / `selected_entity` at `doc`, loading source and
/// display info from the registry.
///
/// No-op if `open_model` already targets this doc (avoids a
/// `diagram_dirty = true` spam when rendering a tab that's already
/// the active one). Mutates `selected_entity` to one of the entities
/// linked to `doc`, if any — that's what the Telemetry / Inspector
/// / Graphs side panels filter by.
fn sync_active_tab_to_doc(world: &mut World, doc: DocumentId) {
    // Already active? Nothing to do.
    let already_active = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .and_then(|m| m.doc)
        == Some(doc);
    if already_active {
        // Still refresh selected_entity in case an entity linked to
        // this doc was spawned since the last switch.
        refresh_selected_entity_for(world, doc);
        return;
    }

    // Gather read-side data up front so we don't hold two borrows
    // at once.
    let snapshot = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        registry.host(doc).map(|h| {
            let document = h.document();
            let path_str = document
                .canonical_path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("doc://{}", doc.raw()));
            let display_name = document
                .canonical_path()
                .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_else(|| format!("Model #{}", doc.raw()));
            (
                path_str,
                display_name,
                document.source().to_string(),
                document.is_read_only(),
                document.library().clone(),
            )
        })
    };

    let Some((path_str, display_name, source, read_only, library)) = snapshot else {
        return;
    };

    // Compute line starts for the editor buffer + open_model.
    let mut line_starts: Vec<usize> = vec![0];
    for (i, b) in source.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let detected_name = crate::ast_extract::extract_model_name(&source);

    // Update WorkbenchState.
    {
        let source_arc: std::sync::Arc<str> = source.clone().into();
        let mut state = world.resource_mut::<WorkbenchState>();
        let prev_path = state.open_model.as_ref().map(|m| m.model_path.clone());
        if let Some(p) = prev_path {
            state.navigation_stack.push(p);
        }
        state.open_model = Some(crate::ui::OpenModel {
            model_path: path_str,
            display_name,
            source: source_arc.clone(),
            line_starts: line_starts.clone().into(),
            detected_name: detected_name.clone(),
            cached_galley: None,
            read_only,
            library,
            doc: Some(doc),
        });
        state.editor_buffer = source_arc.to_string();
        state.diagram_dirty = true;
    }

    // Update the editor buffer state (used by the code-editor body).
    let model_path = world
        .get_resource::<WorkbenchState>()
        .and_then(|s| s.open_model.as_ref().map(|m| m.model_path.clone()))
        .unwrap_or_default();
    {
        let mut buf = world.resource_mut::<EditorBufferState>();
        buf.text = source;
        buf.line_starts = line_starts.into();
        buf.detected_name = detected_name;
        buf.model_path = model_path;
    }

    // Reset any stale in-progress diagram canvas — the diagram body
    // reparses from the fresh source on next render.
    if let Some(mut ds) = world.get_resource_mut::<crate::ui::panels::diagram::DiagramState>() {
        ds.diagram = crate::visual_diagram::VisualDiagram::default();
        ds.snarl = egui_snarl::Snarl::default();
        ds.compile_status = None;
    }

    refresh_selected_entity_for(world, doc);
}

/// Point `WorkbenchState.selected_entity` at one of the entities
/// linked to `doc`, if any. No-op if nothing is linked yet — the
/// side panels will show empty state until a compile spawns one.
fn refresh_selected_entity_for(world: &mut World, doc: DocumentId) {
    let entity = world
        .resource::<ModelicaDocumentRegistry>()
        .entities_linked_to(doc)
        .into_iter()
        .next();
    if let Some(entity) = entity {
        if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
            if state.selected_entity != Some(entity) {
                state.selected_entity = Some(entity);
            }
        }
    }
}

/// Render the unified per-tab toolbar. Returns the (possibly updated)
/// view mode the caller should persist into [`ModelTabs`].
fn render_unified_toolbar(
    doc: DocumentId,
    view_mode: ModelViewMode,
    ui: &mut egui::Ui,
    world: &mut World,
) -> ModelViewMode {
    // Snapshot everything we need before the closure so we don't
    // fight the borrow checker mid-layout.
    let display_name = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| m.display_name.clone())
        .unwrap_or_else(|| format!("Model #{}", doc.raw()));

    let compile_state = world.resource::<CompileStates>().state_of(doc);
    let is_read_only = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| m.read_only)
        .unwrap_or(false);
    let compilation_error = world.resource::<WorkbenchState>().compilation_error.clone();

    let undo_redo = world
        .resource::<ModelicaDocumentRegistry>()
        .host(doc)
        .map(|h| (h.can_undo(), h.can_redo(), h.undo_depth(), h.redo_depth()));

    // Collect button presses without touching world inside the closure.
    let mut compile_clicked = false;
    let mut undo_clicked = false;
    let mut redo_clicked = false;
    let mut dismiss_error = false;
    let mut new_view_mode = view_mode;

    ui.horizontal(|ui| {
        // Identity is on the tab title now (dirty dot there too);
        // the toolbar shows just the view switcher + status + actions
        // so the header stays tight like VS Code.
        let _ = display_name;
        if is_read_only {
            ui.colored_label(egui::Color32::from_rgb(200, 150, 50), "👁 read-only");
            ui.separator();
        }

        let text_sel = view_mode == ModelViewMode::Text;
        let diag_sel = view_mode == ModelViewMode::Diagram;
        if ui.selectable_label(text_sel, "📝 Text").clicked() {
            new_view_mode = ModelViewMode::Text;
        }
        if ui.selectable_label(diag_sel, "🔗 Diagram").clicked() {
            new_view_mode = ModelViewMode::Diagram;
        }
        ui.separator();

        if let Some(ref err) = compilation_error {
            let chip = ui
                .colored_label(egui::Color32::LIGHT_RED, "⚠ Error")
                .on_hover_text(err);
            if chip.clicked() {
                dismiss_error = true;
            }
        } else {
            match compile_state {
                CompileState::Compiling => {
                    ui.colored_label(egui::Color32::from_rgb(220, 200, 80), "⏳ Compiling…");
                }
                CompileState::Ready => {
                    ui.colored_label(egui::Color32::GREEN, "✓ Ready");
                }
                CompileState::Error => {
                    ui.colored_label(egui::Color32::LIGHT_RED, "⚠ Error");
                }
                CompileState::Idle => {
                    ui.colored_label(egui::Color32::GRAY, "◌ Idle");
                }
            }
        }

        if let Some((can_undo, can_redo, undo_n, redo_n)) = undo_redo {
            ui.separator();
            undo_clicked = ui
                .add_enabled(can_undo, egui::Button::new("↶"))
                .on_hover_text(format!("Undo — {undo_n} available (Ctrl+Z)"))
                .clicked();
            redo_clicked = ui
                .add_enabled(can_redo, egui::Button::new("↷"))
                .on_hover_text(format!("Redo — {redo_n} available (Ctrl+Shift+Z)"))
                .clicked();
        }

        ui.separator();
        let compile_enabled =
            !is_read_only && !matches!(compile_state, CompileState::Compiling);
        compile_clicked = ui
            .add_enabled(compile_enabled, egui::Button::new("🚀 Compile"))
            .on_hover_text("Compile the current model and run it (F5)")
            .clicked();
    });

    // Apply side effects after the closure.
    if dismiss_error {
        if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
            s.compilation_error = None;
        }
    }
    if undo_clicked {
        world.commands().trigger(lunco_doc_bevy::UndoDocument { doc });
    }
    if redo_clicked {
        world.commands().trigger(lunco_doc_bevy::RedoDocument { doc });
    }
    if compile_clicked {
        match new_view_mode {
            ModelViewMode::Text => {
                let buffer = world.resource::<EditorBufferState>().text.clone();
                if !buffer.is_empty() {
                    world
                        .resource_mut::<ModelicaDocumentRegistry>()
                        .checkpoint_source(doc, buffer);
                }
                world.commands().trigger(crate::ui::CompileModel { doc });
            }
            ModelViewMode::Diagram => {
                crate::ui::panels::diagram::do_compile(world);
            }
        }
    }
    new_view_mode
}
