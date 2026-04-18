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
use crate::ui::panels::{
    canvas_diagram::CanvasDiagramPanel, code_editor::CodeEditorPanel, diagram::DiagramPanel,
};
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
    /// Block-diagram canvas (lunco-canvas rewrite). Parallel to
    /// `Diagram` during the transition; retires the snarl variant
    /// once the canvas path covers every feature we ship.
    Canvas,
    /// The class's own `Icon` annotation rendering — what the
    /// component looks like when instantiated in a parent diagram.
    /// OMEdit/Dymola have Icon + Diagram as sibling views.
    Icon,
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
    canvas: CanvasDiagramPanel,
}

impl Default for ModelViewPanel {
    fn default() -> Self {
        Self {
            code: CodeEditorPanel,
            diagram: DiagramPanel,
            canvas: CanvasDiagramPanel,
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
        // Tab title mirrors VS Code's pattern:
        //   `●` prefix   → unsaved changes
        //   `🔒` prefix  → read-only (Example / library — edits won't save)
        // Both can stack: `🔒 ● Battery` = read-only model the user tried to edit.
        let doc = DocumentId::new(instance);
        let (base, dirty, read_only) = resolve_tab_title(world, doc);
        let mut prefix = String::new();
        if read_only {
            prefix.push_str("🔒 ");
        }
        if dirty {
            prefix.push_str("● ");
        }
        if prefix.is_empty() {
            base
        } else {
            format!("{prefix}{base}")
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
            ModelViewMode::Canvas => self.canvas.render(ui, world),
            ModelViewMode::Icon => render_icon_view(ui, world),
        }
    }
}

/// Compute `(base, dirty, read_only)` for `doc`. The tab's
/// `InstancePanel::title` prefixes icons accordingly.
fn resolve_tab_title(world: &World, doc: DocumentId) -> (String, bool, bool) {
    if let Some(host) = world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
    {
        let document = host.document();
        return (
            document.origin().display_name(),
            document.is_dirty(),
            document.is_read_only(),
        );
    }

    // Fall back to any live `open_model.display_name` if it matches.
    if let Some(state) = world.get_resource::<WorkbenchState>() {
        if let Some(open) = state.open_model.as_ref() {
            if open.doc == Some(doc) {
                return (open.display_name.clone(), false, open.read_only);
            }
        }
    }
    (format!("Model #{}", doc.raw()), false, false)
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
            let display_name = document.origin().display_name();
            let path_str = document
                .canonical_path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("mem://{display_name}"));
            // Classify for Package Browser / UI badges — we've lost
            // the MSL-vs-Bundled distinction at the doc level (both
            // are just read-only files now); Package Browser-side
            // code that *needs* that distinction should consult its
            // own origin-tracking.
            let library = match document.origin() {
                lunco_doc::DocumentOrigin::Untitled { .. } => {
                    crate::ui::state::ModelLibrary::InMemory
                }
                lunco_doc::DocumentOrigin::File { writable: true, .. } => {
                    crate::ui::state::ModelLibrary::User
                }
                lunco_doc::DocumentOrigin::File { writable: false, .. } => {
                    crate::ui::state::ModelLibrary::Bundled
                }
            };
            (
                path_str,
                display_name,
                document.source().to_string(),
                document.is_read_only(),
                library,
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
        let canv_sel = view_mode == ModelViewMode::Canvas;
        if ui.selectable_label(text_sel, "📝 Text").clicked() {
            new_view_mode = ModelViewMode::Text;
        }
        if ui.selectable_label(diag_sel, "🔗 Diagram").clicked() {
            new_view_mode = ModelViewMode::Diagram;
        }
        if ui.selectable_label(canv_sel, "🧩 Canvas").clicked() {
            new_view_mode = ModelViewMode::Canvas;
        }
        let icon_sel = view_mode == ModelViewMode::Icon;
        if ui.selectable_label(icon_sel, "🎨 Icon").clicked() {
            new_view_mode = ModelViewMode::Icon;
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
        // Compile is independent of writability — simulating a
        // read-only Example is a valid (and common) workflow. Save
        // stays gated on writable; Compile only waits for an
        // in-flight compile to settle.
        let _ = is_read_only;
        let compile_enabled = !matches!(compile_state, CompileState::Compiling);
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
                // Diagram-mode compile regenerates source from the
                // visual-node graph. That only makes sense when the
                // user actually composed something visually — for an
                // equation-only model (e.g. RocketEngine) the graph
                // is empty and the generated source is a bare
                // `model X end X;`, which trips EmptySystem in the
                // solver. Fall back to compiling the document source
                // in that case so Compile always "does the obvious
                // thing" regardless of view.
                let diagram_is_empty = world
                    .get_resource::<crate::ui::panels::diagram::DiagramState>()
                    .map(|s| s.diagram.nodes.is_empty())
                    .unwrap_or(true);
                if diagram_is_empty {
                    world.commands().trigger(crate::ui::CompileModel { doc });
                } else {
                    crate::ui::panels::diagram::do_compile(world);
                }
            }
            ModelViewMode::Canvas => {
                // Canvas is a read-only view in B2 — compile just
                // routes through the document source, same as Text.
                // B3 (doc write-back) will emit real ops from drag /
                // connect; compile can then stay the same.
                world.commands().trigger(crate::ui::CompileModel { doc });
            }
            ModelViewMode::Icon => {
                // Icon is a pure display view — compile-from-icon
                // doesn't mean anything, route through the document
                // source the same as Text does.
                world.commands().trigger(crate::ui::CompileModel { doc });
            }
        }
    }
    new_view_mode
}

/// Render the class's icon. Priority order:
///
/// 1. MSL-registered class: look up its `icon_asset` in
///    `msl_component_library` by qualified name (from
///    `open_model.model_path` when it's an `msl://…` id, or from
///    `detected_name` for plain-short-name matches) and render the
///    SVG if present.
/// 2. Class with an inline `Icon` annotation: TBD — needs an Icon-
///    primitives renderer. Currently shows the placeholder.
/// 3. Everything else: a friendly "no icon defined" placeholder so
///    the tab doesn't appear broken.
///
/// Always centred in the available rect, aspect-preserving.
fn render_icon_view(ui: &mut egui::Ui, world: &mut World) {
    let (qualified, _source) = {
        let ws = world.resource::<WorkbenchState>();
        let Some(open) = ws.open_model.as_ref() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("No model open").weak(),
                );
            });
            return;
        };
        // Prefer the msl://Full.Qualified.Name path if we have one
        // (drill-in sets this); otherwise fall back to detected
        // short name, which matches MSL entries by suffix.
        let from_path = open
            .model_path
            .strip_prefix("msl://")
            .map(|s| s.to_string());
        let short = open.detected_name.clone().unwrap_or_default();
        (
            from_path.unwrap_or_else(|| short.clone()),
            open.source.clone(),
        )
    };

    // Look up MSL entry. Qualified match first (exact), then any
    // entry whose msl_path ends in `.<short>` for best-effort
    // resolution of local models that reference an MSL class.
    let icon_asset = {
        let lib = crate::visual_diagram::msl_component_library();
        lib.iter()
            .find(|c| c.msl_path == qualified)
            .or_else(|| {
                // Try short-name tail match.
                let short = qualified.rsplit('.').next().unwrap_or(&qualified);
                lib.iter()
                    .find(|c| c.msl_path.rsplit('.').next() == Some(short))
            })
            .and_then(|c| c.icon_asset.clone())
    };

    let painter = ui.painter();
    let rect = ui.available_rect_before_wrap();

    // Diagram frame (Dymola's square coordinate box).
    painter.rect_stroke(
        rect.shrink(12.0),
        4.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_premultiplied(90, 100, 120, 120),
        ),
        egui::StrokeKind::Outside,
    );

    if let Some(path) = icon_asset {
        if let Some(bytes) = svg_bytes_for_icon(&path) {
            // Render into a centred square that's at most ~60 % of
            // the smaller rect dimension — leaves breathing room
            // and matches Dymola's Icon tab sizing.
            let side = (rect.width().min(rect.height()) * 0.6).max(100.0);
            let icon_rect = egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(side, side),
            );
            crate::ui::panels::svg_renderer::draw_svg_to_egui(
                painter, icon_rect, &bytes,
            );
            // Class name under the icon.
            painter.text(
                egui::pos2(icon_rect.center().x, icon_rect.max.y + 16.0),
                egui::Align2::CENTER_TOP,
                &qualified,
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(200, 210, 225),
            );
            return;
        }
    }

    // Fallback placeholder — the class has no known icon.
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("🎨")
                    .size(48.0)
                    .color(egui::Color32::from_rgb(120, 130, 150)),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("No icon defined for this class")
                    .color(egui::Color32::from_rgb(180, 190, 210)),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Add an `annotation(Icon(graphics={…}))` clause \
                     in the Text tab, or instantiate this class in a \
                     parent diagram.",
                )
                .italics()
                .size(11.0)
                .color(egui::Color32::from_rgb(140, 155, 175)),
            );
        });
    });
}

/// SVG byte cache shared with the canvas panel — same lookup path
/// as `canvas_diagram::svg_bytes_for`. Duplicated here rather than
/// exposed as `pub` because the panel module graph is a flat
/// `src/ui/panels/` listing and I'd prefer not to expose a cache
/// function just for this.
fn svg_bytes_for_icon(asset_path: &str) -> Option<std::sync::Arc<Vec<u8>>> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Option<std::sync::Arc<Vec<u8>>>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().expect("svg icon cache poisoned");
    if let Some(cached) = map.get(asset_path) {
        return cached.clone();
    }
    let full = lunco_assets::msl_dir().join(asset_path);
    let loaded = std::fs::read(&full).ok().map(std::sync::Arc::new);
    map.insert(asset_path.to_string(), loaded.clone());
    loaded
}
