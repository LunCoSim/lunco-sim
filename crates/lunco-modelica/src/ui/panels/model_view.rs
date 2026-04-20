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
    canvas_diagram::CanvasDiagramPanel, code_editor::CodeEditorPanel,
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
    /// Block-diagram canvas (lunco-canvas). Replaced the old
    /// egui-snarl-based variant.
    Canvas,
    /// The class's own `Icon` annotation rendering — what the
    /// component looks like when instantiated in a parent diagram.
    /// OMEdit/Dymola have Icon + Diagram as sibling views.
    Icon,
    /// The class's `Documentation(info="…", revisions="…")`
    /// annotation rendered as text. HTML is shown as-is (no
    /// Markdown conversion yet) — reads like rendered plain text
    /// with tags visible, which is honest about what's in source
    /// and avoids guessing at formatting.
    Docs,
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

    /// Iterate the document ids of every currently-open tab.
    /// Used by drill-in to avoid re-allocating a document when the
    /// same class is already open in a tab.
    pub fn iter_docs(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.tabs.keys().copied()
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
    canvas: CanvasDiagramPanel,
}

impl Default for ModelViewPanel {
    fn default() -> Self {
        Self {
            code: CodeEditorPanel,
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
        // Diagnostic: log on first render per tab (view switches
        // don't re-log — one-shot per tab open) so we can see which
        // body path the freeze is hitting. Throw-away; promoted to
        // a Diagnostics event if this turns out to be the culprit.
        {
            use std::sync::{Mutex, OnceLock};
            static SEEN: OnceLock<Mutex<std::collections::HashSet<(u64, u8)>>> =
                OnceLock::new();
            let seen = SEEN.get_or_init(|| Mutex::new(Default::default()));
            let tag = match new_view_mode {
                ModelViewMode::Text => 0u8,
                ModelViewMode::Canvas => 1,
                ModelViewMode::Icon => 2,
                ModelViewMode::Docs => 3,
            };
            if let Ok(mut s) = seen.lock() {
                if s.insert((doc.raw(), tag)) {
                    bevy::log::info!(
                        "[ModelView] rendering tab doc={:?} mode={:?}",
                        doc,
                        new_view_mode,
                    );
                }
            }
        }

        match new_view_mode {
            ModelViewMode::Text => self.code.render(ui, world),
            ModelViewMode::Canvas => self.canvas.render(ui, world),
            ModelViewMode::Icon => render_icon_view(ui, world),
            ModelViewMode::Docs => render_docs_view(ui, world),
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

    // Fall back to any live `open_model.display_name` when it's the
    // current active tab. Active-doc identity is the Workspace's
    // concern; `open_model` is only a display cache of the same.
    let active_doc = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document);
    if active_doc == Some(doc) {
        if let Some(state) = world.get_resource::<WorkbenchState>() {
            if let Some(open) = state.open_model.as_ref() {
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
pub(crate) fn sync_active_tab_to_doc(world: &mut World, doc: DocumentId) {
    // Already active AND the cached snapshot is from the real doc
    // (not a placeholder that filled in while a drill-in load was
    // still in flight). The check on non-empty source distinguishes:
    //   - Real snapshot: source is the file contents → nothing to do.
    //   - Placeholder: source is "" because host was still missing
    //     when we last synced → refresh now that the registry has
    //     the real document.
    //
    // Without the second condition, drill-in tabs could get stuck
    // showing an empty Text view forever: sync runs with a placeholder,
    // `already_active` fires, we short-circuit, and the real
    // source never lands until the user manually switches tabs.
    let active_matches = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document)
        == Some(doc);
    let (already_active, source_is_placeholder) = {
        let ws = world.resource::<WorkbenchState>();
        match ws.open_model.as_ref() {
            Some(open) => (active_matches, open.source.is_empty()),
            None => (false, false),
        }
    };
    if already_active && !source_is_placeholder {
        // Still refresh selected_entity in case an entity linked to
        // this doc was spawned since the last switch.
        refresh_selected_entity_for(world, doc);
        return;
    }

    // Gather read-side data up front so we don't hold two borrows
    // at once. `detected_name` comes from the doc's cached AST —
    // MUST NOT re-parse here. Previously this called
    // `ast_extract::extract_model_name(source)` which kicked off an
    // uncached rumoca parse on the main thread; on a 184 KB
    // drill-in source that froze the UI for ~200 s in debug builds.
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
            // `document.is_read_only()` means "can't Save without
            // Save-As" — true for Untitled docs despite Untitled
            // being fully editable. For UI purposes (right-click
            // menu, apply_ops gate) "read-only" means "library
            // class the user isn't allowed to mutate", so tie it
            // to the library classification instead: only Bundled
            // (MSL, drill-in target) is read-only; Untitled and
            // User files are both editable.
            let read_only =
                matches!(library, crate::ui::state::ModelLibrary::Bundled);
            let detected_name = document
                .ast()
                .ast()
                .and_then(crate::ast_extract::extract_model_name_from_ast);
            (
                path_str,
                display_name,
                document.source().to_string(),
                read_only,
                library,
                detected_name,
            )
        })
    };

    // Fallback: the doc is a placeholder reserved by drill-in and
    // its bg load hasn't finished yet (so `registry.host(doc)` is
    // still None). We still need to flip `open_model.doc` to this
    // tab's id — otherwise every per-doc lookup downstream (canvas
    // state, loading overlay, read-only gate) keeps routing to the
    // PREVIOUS tab's doc and the new tab visually mirrors it until
    // the parse completes. Use the DrillInLoads entry for a
    // display name; the source stays empty until the real document
    // is installed.
    let snapshot = snapshot.or_else(|| {
        // Drill-in tab still loading? Use the qualified name as
        // the placeholder identity.
        if let Some(loads) = world
            .get_resource::<crate::ui::panels::canvas_diagram::DrillInLoads>()
        {
            if let Some(qualified) = loads.detail(doc) {
                let qualified = qualified.to_string();
                let short = qualified
                    .rsplit('.')
                    .next()
                    .map(str::to_string)
                    .unwrap_or_else(|| qualified.clone());
                return Some((
                    format!("msl://{qualified}"),
                    short.clone(),
                    String::new(),
                    true,
                    crate::ui::state::ModelLibrary::Bundled,
                    Some(short),
                ));
            }
        }
        // Duplicate-to-workspace tab still building? Use the target
        // display name; the copy is editable (not read-only).
        if let Some(dup) = world
            .get_resource::<crate::ui::panels::canvas_diagram::DuplicateLoads>()
        {
            if let Some(display) = dup.detail(doc) {
                let display = display.to_string();
                return Some((
                    format!("mem://{display}"),
                    display.clone(),
                    String::new(),
                    false,
                    crate::ui::state::ModelLibrary::InMemory,
                    Some(display),
                ));
            }
        }
        None
    });
    let Some((path_str, display_name, source, read_only, library, detected_name)) =
        snapshot
    else {
        return;
    };

    // Compute line starts for the editor buffer + open_model.
    let mut line_starts: Vec<usize> = vec![0];
    for (i, b) in source.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }

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
        });
        state.editor_buffer = source_arc.to_string();
        state.diagram_dirty = true;
    }

    // Mirror active-document into the Workspace session. Workspace is
    // the single source of truth for "which doc has focus"; open_model
    // stays as a UI-side cache of derived fields (source Arc, line
    // starts, galley) that aren't worth putting into the session type.
    {
        let mut ws = world.resource_mut::<lunco_workbench::WorkspaceResource>();
        ws.active_document = Some(doc);
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
    // Icon-only class (MSL `Modelica.*.Icons.*` subtree): no
    // connectors, nothing to diagram. `model_path` carries the
    // `msl://<qualified>` URI for drill-in tabs, so the
    // path-based helper works on it directly.
    let is_icon_only_tab = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| {
            crate::class_cache::is_icon_only_class(&m.model_path)
                || m.model_path.contains("/Icons/")
        })
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
    let mut duplicate_clicked = false;
    let mut auto_arrange_clicked = false;
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
        let canv_sel = view_mode == ModelViewMode::Canvas;
        let icon_sel = view_mode == ModelViewMode::Icon;
        let docs_sel = view_mode == ModelViewMode::Docs;
        // All four views are always available — OMEdit/Dymola
        // pattern. A partial or icon-only class has a legitimately
        // empty Diagram layer, and users should be able to view it.
        // The smart "land in the right view by default" happens at
        // install time (see `drive_drill_in_loads`), not by hiding
        // buttons.
        let _ = (is_read_only, is_icon_only_tab);
        if ui.selectable_label(text_sel, "📝 Text").clicked() {
            new_view_mode = ModelViewMode::Text;
        }
        if ui.selectable_label(canv_sel, "🔗 Diagram").clicked() {
            new_view_mode = ModelViewMode::Canvas;
        }
        if ui.selectable_label(icon_sel, "🎨 Icon").clicked() {
            new_view_mode = ModelViewMode::Icon;
        }
        if ui.selectable_label(docs_sel, "📖 Docs").clicked() {
            new_view_mode = ModelViewMode::Docs;
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
        let compile_enabled = !matches!(compile_state, CompileState::Compiling);
        compile_clicked = ui
            .add_enabled(compile_enabled, egui::Button::new("🚀 Compile"))
            .on_hover_text("Compile the current model and run it (F5)")
            .clicked();

        // Auto-Arrange: batch SetPlacement on every component in the
        // active class to a clean grid. Only useful on the Diagram
        // view and only on editable docs. Dymola's "Edit → Auto
        // Arrange" in one button.
        if view_mode == ModelViewMode::Canvas && !is_read_only {
            ui.separator();
            auto_arrange_clicked = ui
                .button("🧹 Auto-Arrange")
                .on_hover_text(
                    "Lay out all components in a grid and write the \
                     positions back into the source as Placement \
                     annotations. Undo-able.",
                )
                .clicked();
        }

        // Auto-Arrange: batch SetPlacement on every component in the
        // active class to a clean grid. Only useful on the Diagram
        // view and only on editable docs. Dymola's "Edit → Auto
        // Arrange" in one button.
        if view_mode == ModelViewMode::Canvas && !is_read_only {
            ui.separator();
            auto_arrange_clicked = ui
                .button("🧹 Auto-Arrange")
                .on_hover_text(
                    "Lay out all components in a grid and write the \
                     positions back into the source as Placement \
                     annotations. Undo-able.",
                )
                .clicked();
        }

        // Duplicate-to-workspace: only offered on read-only tabs.
        // Users browsing an MSL Example who want to tweak parameters
        // hit this to get an editable copy in a new tab; the library
        // original stays untouched. Mirrors Dymola's "make your own
        // copy" workflow for example models.
        if is_read_only {
            ui.separator();
            duplicate_clicked = ui
                .button("📄 Duplicate to Workspace")
                .on_hover_text(
                    "Copy this library class into a new editable Untitled \
                     model so you can tweak parameters / connections \
                     without modifying the MSL source.",
                )
                .clicked();
        }
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
    if duplicate_clicked {
        world
            .commands()
            .trigger(crate::ui::commands::DuplicateModelFromReadOnly {
                source_doc: doc,
            });
    }
    if auto_arrange_clicked {
        world
            .commands()
            .trigger(crate::ui::commands::AutoArrangeDiagram {
                doc: doc.raw(),
            });
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
            ModelViewMode::Docs => {
                // Docs is pure display — compile routes through the
                // document source like Text.
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
/// Render the active class's `Documentation(info="…", revisions="…")`
/// annotation. HTML is shown raw — no Markdown conversion, no tag
/// stripping. Most Modelica docs are short prose with light HTML
/// (paragraphs, the occasional `<strong>` or `<code>`); the tags
/// read fine inline for a workbench built for engineers. Upgrading
/// to a Markdown-converted render is a follow-up.
fn render_docs_view(ui: &mut egui::Ui, world: &mut World) {
    use crate::ui::state::WorkbenchState;
    let doc_id = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document);
    if doc_id.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No model open").weak());
        });
        return;
    }

    // Resolve the class: drill-in target (qualified), or first non-
    // package class in the AST as fallback. Same picker the canvas's
    // target resolver uses.
    let (class_name, info, revisions): (
        Option<String>,
        Option<String>,
        Option<String>,
    ) = match doc_id {
        None => (None, None, None),
        Some(doc) => {
            let drilled = world
                .get_resource::<crate::ui::panels::canvas_diagram::DrilledInClassNames>()
                .and_then(|m| m.get(doc).map(str::to_string));
            let ast = world
                .resource::<crate::ui::state::ModelicaDocumentRegistry>()
                .host(doc)
                .and_then(|h| h.document().ast().result.as_ref().ok().cloned());
            match ast {
                Some(ast) => {
                    let class = if let Some(q) = drilled.as_deref() {
                        walk_qualified_class(ast.as_ref(), q)
                    } else {
                        use rumoca_session::parsing::ClassType;
                        ast.classes
                            .iter()
                            .find(|(_, c)| !matches!(c.class_type, ClassType::Package))
                            .map(|(n, c)| (n.clone(), c))
                    };
                    class
                        .map(|(name, class)| {
                            let (info, revs) =
                                extract_documentation(&class.annotation);
                            (Some(name), info, revs)
                        })
                        .unwrap_or((None, None, None))
                }
                None => (None, None, None),
            }
        }
    };

    // Typography: constrain reading width and centre in the panel.
    // Modelica docs open in whatever width the panel is — often
    // 1000+ px, which drops text line-length to 140+ characters. The
    // eye can't scan that; standard book / web typography caps at
    // ~65–75 characters (≈ 720 px at 13 px body), matching MDN, Rust
    // docs, and Obsidian's reading view.
    const READING_WIDTH: f32 = 760.0;
    const SIDE_MARGIN: f32 = 24.0;

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let avail = ui.available_width();
            let target_width = READING_WIDTH.min(avail - SIDE_MARGIN * 2.0);
            let inset = ((avail - target_width) * 0.5).max(SIDE_MARGIN);

            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(inset as i8, 16))
                .show(ui, |ui| {
                    ui.set_max_width(target_width);

                    if let Some(name) = &class_name {
                        ui.label(
                            egui::RichText::new(name)
                                .size(22.0)
                                .strong()
                                .color(egui::Color32::from_rgb(230, 235, 245)),
                        );
                        ui.add_space(12.0);
                    }
                    match info.as_deref().filter(|s| !s.trim().is_empty()) {
                        Some(html) => {
                            render_html_as_markdown(ui, target_width, html);
                        }
                        None => {
                            ui.label(
                                egui::RichText::new("(no documentation)")
                                    .italics()
                                    .weak(),
                            );
                        }
                    }
                    if let Some(revs) =
                        revisions.as_deref().filter(|s| !s.trim().is_empty())
                    {
                        ui.add_space(24.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Revisions")
                                .strong()
                                .size(15.0)
                                .color(egui::Color32::from_rgb(200, 210, 225)),
                        );
                        ui.add_space(6.0);
                        render_html_as_markdown(ui, target_width, revs);
                    }
                });
        });
}

/// Convert a Modelica-documentation HTML blob into Markdown with
/// [`htmd`] and render it via [`egui_commonmark::CommonMarkViewer`].
///
/// `target_width` is the reading-width cap applied to images so a
/// full-res MSL plot (often 1200+ px) doesn't blow past the column
/// and force the reader to scroll sideways. Keeping the Markdown
/// render cache static means repeated frames don't re-tokenise the
/// same text.
fn render_html_as_markdown(ui: &mut egui::Ui, target_width: f32, html: &str) {
    use std::sync::Mutex;
    static CACHE: std::sync::OnceLock<
        Mutex<egui_commonmark::CommonMarkCache>,
    > = std::sync::OnceLock::new();
    let cache = CACHE
        .get_or_init(|| Mutex::new(egui_commonmark::CommonMarkCache::default()));
    // `htmd::convert` is pure CPU, sub-millisecond on typical MSL
    // docs. Caching the Markdown conversion would shave frames in
    // pathological cases; skipping for simplicity — CommonMarkCache
    // covers the render-side reuse.
    let md = htmd::convert(html).unwrap_or_else(|_| html.to_string());
    if let Ok(mut c) = cache.lock() {
        egui_commonmark::CommonMarkViewer::new()
            .max_image_width(Some(target_width as usize))
            .show(ui, &mut c, &md);
    }
}

/// Walk a dotted qualified-name path into nested classes and return
/// `(short_name, class)` for the final segment. Mirrors the canvas
/// resolver but keeps the short name for the heading.
fn walk_qualified_class<'a>(
    ast: &'a rumoca_session::parsing::ast::StoredDefinition,
    qualified: &str,
) -> Option<(String, &'a rumoca_session::parsing::ast::ClassDef)> {
    let mut segments = qualified.split('.');
    let first = segments.next()?;
    let (first_name, first_class) = ast
        .classes
        .iter()
        .find(|(n, _)| n.as_str() == first)
        .map(|(n, c)| (n.clone(), c))?;
    let mut current_name = first_name;
    let mut current_class = first_class;
    for seg in segments {
        let next = current_class.classes.get_key_value(seg)?;
        current_name = next.0.clone();
        current_class = next.1;
    }
    Some((current_name, current_class))
}

/// Un-escape a Modelica string literal's body per MLS §2.4.6. The
/// subset we handle covers what Documentation HTML actually uses:
///   `\"`  → `"`    (attribute quotes)
///   `\\`  → `\`    (literal backslash)
///   `\n`  → LF     (line break)
///   `\t`  → tab
///   `\r`  → CR
/// Unknown `\x` sequences fall through as two chars so we don't
/// accidentally destroy source that htmd or commonmark might still
/// handle gracefully.
fn unescape_modelica_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                match n {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '\'' => out.push('\''),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract `Documentation(info="…", revisions="…")` — both are HTML
/// string payloads. Returns `(info, revisions)`.
fn extract_documentation(
    annotations: &[rumoca_session::parsing::ast::Expression],
) -> (Option<String>, Option<String>) {
    use rumoca_session::parsing::ast::{Expression, TerminalType};
    // Find the Documentation(...) call.
    let call = annotations.iter().find(|e| match e {
        Expression::FunctionCall { comp, .. } | Expression::ClassModification { target: comp, .. } => {
            comp.parts
                .first()
                .map(|p| p.ident.text.as_ref() == "Documentation")
                .unwrap_or(false)
        }
        _ => false,
    });
    let Some(call) = call else { return (None, None) };
    let args: &[Expression] = match call {
        Expression::FunctionCall { args, .. } => args.as_slice(),
        Expression::ClassModification { modifications, .. } => modifications.as_slice(),
        _ => return (None, None),
    };
    let str_arg = |name: &str| -> Option<String> {
        for a in args {
            let (arg_name, value) = match a {
                Expression::NamedArgument { name, value } => {
                    (name.text.as_ref(), value.as_ref())
                }
                Expression::Modification { target, value } => (
                    target.parts.first().map(|p| p.ident.text.as_ref()).unwrap_or(""),
                    value.as_ref(),
                ),
                _ => continue,
            };
            if arg_name != name {
                continue;
            }
            if let Expression::Terminal { terminal_type: TerminalType::String, token } = value {
                // Rumoca keeps the raw source slice on the token, which
                // still includes the surrounding `"…"` *and* the
                // Modelica-spec escape sequences (`\"` for a literal
                // quote, `\\` for a backslash, `\n` for a newline). For
                // Documentation HTML the `\"` attribute-quotes are the
                // loudest — un-escaping turns `<img src=\"…\"/>` back
                // into the literal `<img src="…"/>` so htmd + the
                // renderer see proper HTML.
                let raw = token.text.as_ref();
                let inner = raw.trim_start_matches('"').trim_end_matches('"');
                return Some(unescape_modelica_string(inner));
            }
        }
        None
    };
    (str_arg("info"), str_arg("revisions"))
}

fn render_icon_view(ui: &mut egui::Ui, world: &mut World) {
    let theme = world
        .get_resource::<lunco_theme::Theme>()
        .cloned()
        .unwrap_or_else(lunco_theme::Theme::dark);
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

    let frame_stroke_src = theme.colors.overlay1;
    painter.rect_stroke(
        rect.shrink(12.0),
        4.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(
                frame_stroke_src.r(),
                frame_stroke_src.g(),
                frame_stroke_src.b(),
                120,
            ),
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
                theme.tokens.text,
            );
            return;
        }
    }

    // Fallback placeholder — the class has no known icon. Same
    // centered-card pattern the empty-diagram overlay uses.
    use crate::ui::theme::ModelicaThemeExt;
    crate::ui::panels::placeholder::render_centered_card(
        ui,
        rect,
        egui::vec2(380.0, 170.0),
        &theme,
        |ui| {
            ui.label(
                egui::RichText::new("🎨")
                    .size(36.0)
                    .color(theme.text_muted()),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("No icon defined for this class")
                    .strong()
                    .color(theme.text_heading()),
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
                .color(theme.text_muted()),
            );
        },
    );
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