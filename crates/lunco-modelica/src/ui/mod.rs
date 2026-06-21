//! Modelica workbench UI — panels as entity viewers.
//!
//! ## Architecture: Panels Are Entity Viewers
//!
//! Each panel watches a `ModelicaModel` entity and renders its data.
//! Panels don't know if they're in a standalone workbench, a floating overlay
//! on a 3D viewport, or a mission dashboard — they just watch the selected entity.
//!
//! ```text
//!                    ModelicaModel entity
//!                    (attached to 3D objects
//!                     or standalone workbench)
//!                              │
//!           ┌──────────────────┼──────────────────┐
//!           ▼                  ▼                  ▼
//!     DiagramPanel      CodeEditorPanel    TelemetryPanel
//!     (lunco-canvas)    (text editor)      (params/inputs)
//! ```
//!
//! ## Selection Bridge
//!
//! `WorkbenchState.selected_entity` is the single source of truth.
//! Any context can trigger an editor by setting it:
//! - Package Browser: click a model in the tree
//! - 3D viewport: click a rover's solar panel
//! - Colony tree: select a subsystem node
//!
//! ```rust,ignore
//! // Anywhere in the codebase:
//! fn open_modelica_editor(world: &mut World, entity: Entity) {
//!     if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
//!         state.selected_entity = Some(entity);
//!     }
//!     // Panels auto-update because they watch WorkbenchState
//! }
//! ```
//!
//! ## Panel Layout
//!
//! bevy_workbench auto-assigns panel slots by ID convention:
//!
//! | ID Pattern         | Auto-Slot | Default Position  |
//! |--------------------|-----------|-------------------|
//! | contains "inspector" | Right   | Right dock        |
//! | contains "console"   | Bottom  | Bottom dock       |
//! | contains "preview"   | Center  | Center tab        |
//! | (no match)           | Left    | Left dock         |
//!
//! Users can drag, split, tab, and float panels freely.
//! Layout persists across sessions via bevy_workbench persistence.
//!
//! ## Panels
//!
//! - **Package Browser** (left dock) — Dymola-style library tree, click to open
//! - **Code Editor** (center tab) — source code editing, compile & run
//! - **Diagram** (center tab) — component block diagram on `lunco-canvas`
//! - **Telemetry** (right dock) — parameters, inputs, variable toggles
//! - **Graphs** (bottom dock) — time-series plots of simulation variables

use bevy::prelude::*;
use lunco_workbench::{Perspective, PerspectiveId, WorkbenchAppExt, WorkbenchLayout, PanelId};
// Core document/library/compile state moved out of `ui` into `crate::state`.
use crate::state::{CompileStates, ModelicaDocumentRegistry, WorkbenchState};

pub mod document_openings;

pub mod commands;
/// Reactive UI observers of core domain state (status-bus mirrors, etc.).
pub mod core_observers;
pub use commands::{CompileModel, CreateNewScratchModel, ModelicaCommandsPlugin};

pub mod icon_paint;
pub mod image_loader;
pub mod panels;
pub mod viz;
pub mod theme;
pub mod uri_handler;
pub mod loaded_classes;
pub mod text_node;
pub mod wasm_autosave;
pub mod wasm_clipboard;
pub mod welcome_progress;
pub mod help_overlay;
/// Debounced AST reparse driver — see module docs.
pub mod input_activity;
pub mod wire_router;
/// Modelica section of the Twin Browser — class-tree contributed by
/// this crate to `lunco-workbench`'s `BrowserSectionRegistry`.
pub mod browser_section;

/// Drains the workbench's `BrowserActions` outbox and routes
/// section-emitted intents (open file, open Modelica class) into the
/// existing document-load and drill-in pipelines.
pub mod browser_dispatch;

/// Per-panel "pin to model" overrides for singleton inspector panels.
pub mod doc_pin;

/// Document hot-exit codec — persists & restores open Modelica buffers.
pub mod session_codec;

use crate::ModelicaModel;

/// Fan queued document lifecycle notifications out as observer triggers.
///
/// The registry accumulates ids on every mutation (allocate → Opened +
/// Changed, `checkpoint_source` with new text → Changed, explicit
/// `mark_changed` after `host_mut` undo/redo → Changed, `remove_document`
/// → Closed). This system drains all three queues once per frame and
/// emits the matching generic events from [`lunco_doc_bevy`] so any
/// observer (panel re-render, diagram re-parse, plot variable-list
/// refresh, Twin journal, …) reacts without polling generation
/// counters.
///
/// Fire order per frame: Opened, Changed, Closed. Opened-before-Changed
/// means subscribers that key on "track docs I've seen Opened for" can
/// safely skip Changed events for unknown ids.
fn drain_document_changes(
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    for doc in registry.drain_pending_opened() {
        commands.trigger(lunco_doc_bevy::DocumentOpened::local(doc));
    }
    for doc in registry.drain_pending_changes() {
        commands.trigger(lunco_doc_bevy::DocumentChanged::local(doc));
    }
    for doc in registry.drain_pending_closed() {
        commands.trigger(lunco_doc_bevy::DocumentClosed::local(doc));
    }
}

/// Shadow-sync observer: Modelica doc opened → register entry in the
/// Workspace session.
///
/// Runs alongside (not instead of) the existing open paths during the
/// 5b.1 migration. Once step 5c retires the legacy `ModelicaDocumentRegistry`
/// / `ModelTabs` / the registry-by-doc lookup triad, this observer
/// becomes the sole population point for the Workspace's document list.
/// Wholesale-clear the canvas paint-side port-icon cache when any
/// doc changes. Cheap on rumoca's content-hash cache; the next
/// paint refills.
fn invalidate_port_icon_cache_on_doc_changed(
    _trigger: On<lunco_doc_bevy::DocumentChanged>,
) {
    crate::ui::panels::canvas_diagram::invalidate_port_icon_cache();
}

/// Per-doc generation watermark for the
/// [`close_drilled_tabs_on_class_removed`] observer. Tracks the last
/// `ModelicaDocument::generation` we processed so each
/// `DocumentChanged` fire only walks new entries in the change ring
/// buffer. Falls back to a re-anchor when the retention window has
/// rolled over (`changes_since` returns `None`).
#[derive(Resource, Default)]
struct ClassRemovedWatermark(std::collections::HashMap<lunco_doc::DocumentId, u64>);

/// Cross-truth rule R4 (see `docs/architecture/B0_CROSS_TRUTH_POLICY.md`):
/// when a `RemoveClass` op lands, every tab drilled into the
/// removed class — or a descendant of it — closes. Without this
/// observer the dangling tab falls through to first-tab behaviour
/// and renders a blank or unrelated-class canvas.
///
/// Reads new entries from `ModelicaDocument::changes_since` between
/// observer fires; the per-doc watermark resource keeps it O(new
/// changes) rather than O(history).
fn close_drilled_tabs_on_class_removed(
    trigger: On<lunco_doc_bevy::DocumentChanged>,
    registry: Res<crate::state::ModelicaDocumentRegistry>,
    mut tabs: ResMut<crate::model_tabs::ModelTabs>,
    mut watermark: ResMut<ClassRemovedWatermark>,
    mut experiments: Option<ResMut<lunco_experiments::ExperimentRegistry>>,
    mut drafts: Option<ResMut<crate::experiments_runner::ExperimentDrafts>>,
    mut steppers: Query<&mut crate::ModelicaModel>,
) {
    use lunco_doc::Document as _;
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else { return };
    let document = host.document();
    let last_seen = watermark.0.get(&doc).copied().unwrap_or(0);
    // `changes_since` returns None when the retention ring rolled
    // past `last_seen`. Re-anchor and bail; drilled tabs that lost
    // their class survive (corner case — accepted, the alternative
    // is closing every drilled tab on rollover, which is worse).
    let Some(changes) = document.changes_since(last_seen) else {
        watermark.0.insert(doc, document.generation());
        return;
    };
    let mut highest_gen = last_seen;
    let mut to_close: Vec<String> = Vec::new();
    let mut to_rename: Vec<(String, String)> = Vec::new();
    for (gen, change) in changes {
        highest_gen = highest_gen.max(*gen);
        match change {
            crate::document::ModelicaChange::ClassRemoved { qualified } => {
                to_close.push(qualified.clone());
            }
            crate::document::ModelicaChange::ClassRenamed { old, new } => {
                to_rename.push((old.clone(), new.clone()));
            }
            _ => {}
        }
    }
    for qualified in to_close {
        let closed = tabs.close_drilled_into(doc, &qualified);
        if !closed.is_empty() {
            bevy::log::info!(
                "[R4] RemoveClass({qualified}) closed {} drilled tab(s)",
                closed.len()
            );
        }
    }
    // Identity-preserving rename: tabs / experiments / drafts /
    // running stepper entities that referenced the old class name
    // re-bind to the new one. Keeps the user's open canvas / run
    // history / setup / live simulator intact when they retype a
    // class header in the text editor.
    for (old, new) in to_rename {
        let touched_tabs = tabs.rename_drilled_class(doc, &old, &new);
        let touched_experiments = experiments
            .as_mut()
            .map(|r| {
                r.rename_model_ref(
                    &crate::ui::doc_pin::twin_id_for_doc(doc),
                    &lunco_experiments::ModelRef(old.clone()),
                    &lunco_experiments::ModelRef(new.clone()),
                )
            })
            .unwrap_or(0);
        let touched_drafts = drafts
            .as_mut()
            .map(|d| {
                d.rename_model_ref(
                    doc,
                    &lunco_experiments::ModelRef(old.clone()),
                    &lunco_experiments::ModelRef(new.clone()),
                )
            })
            .unwrap_or(false);
        // Update any compiled stepper entities linked to this doc
        // so subsequent telemetry / model-name queries see the new
        // identity without forcing a recompile. The actual rumoca
        // session keys off the AST class definition, not this
        // string, so the running simulation continues uninterrupted.
        let mut touched_steppers = 0usize;
        for (e, d) in registry.iter_doc_for_entity() {
            if d != doc {
                continue;
            }
            if let Ok(mut model) = steppers.get_mut(e) {
                if model.model_name == old {
                    model.model_name = new.clone();
                    touched_steppers += 1;
                }
            }
        }
        bevy::log::info!(
            "[R4] ClassRenamed({old} → {new}): {touched_tabs} tab(s), \
             {touched_experiments} experiment(s), drafts={touched_drafts}, \
             {touched_steppers} stepper(s)"
        );
    }
    watermark.0.insert(doc, highest_gen);
}

// `mirror_open_model_on_doc_changed` deleted cleanup.

// `world` module deleted as part of the A2 single-struct migration:
// `ClassEntry` is now the canonical class record consumed everywhere,
// and per-doc `ModelicaIndex.classes` already holds it, so a separate
// `ModelicaWorld` resource just duplicates state. The unified
// read-side resolver (`class_metadata::resolve_metadata`) consults
// the pre-baked MSL library + the live per-doc index directly.

fn sync_workspace_on_doc_opened(
    trigger: On<lunco_doc_bevy::DocumentOpened>,
    registry: Res<ModelicaDocumentRegistry>,
    mut ws: ResMut<lunco_workspace::WorkspaceResource>,
    mut source_roots: Option<ResMut<crate::source_roots::SourceRootRegistry>>,
) {
    let id = trigger.event().doc;
    // Dedupe — `DocumentOpened` can fire multiple times per id during
    // the race between allocate/install_prebuilt and later reconcile
    // passes. Treat a second Opened as a no-op so the Workspace
    // document list stays a set, not a multiset.
    let Some(host) = registry.host(id) else {
        return;
    };
    let doc = host.document();
    let origin = doc.origin().clone();
    // Register every top-level class the opened doc declares so the
    // pre-Compile gate treats them as Ready (engine_resource syncs
    // doc ASTs into the rumoca session on install, so the types
    // resolve without a worker round-trip).
    if let Some(roots) = source_roots.as_deref_mut() {
        let path = match &origin {
            lunco_doc::DocumentOrigin::File { path, .. } => Some(path.clone()),
            _ => None,
        };
        for class in doc.index().classes.values() {
            // Top-level classes only: qualified name with no `.`.
            if !class.name.contains('.') {
                roots.register_open_doc_root(class.name.clone(), path.clone());
            }
        }
    }
    if ws.document(id).is_some() {
        return;
    }
    let title = origin.display_name();
    ws.add_document(lunco_workspace::DocumentEntry {
        id,
        kind: lunco_workspace::DocumentKind::Modelica,
        origin,
        // Default to `None`; when the UI supports "New Model from
        // active Twin" the caller will set this explicitly before the
        // add_document fires.
        context_twin: None,
        title,
    });
}

/// Shadow-sync observer: Modelica doc closed → drop entry from Workspace.
fn sync_workspace_on_doc_closed(
    trigger: On<lunco_doc_bevy::DocumentClosed>,
    mut ws: ResMut<lunco_workspace::WorkspaceResource>,
) {
    ws.close_document(trigger.event().doc);
}

/// Shadow-sync observer: a save (regular or Save-As) can change a
/// document's origin (Untitled → File on Save-As). Re-read the
/// document and update the Workspace entry's `origin` + `title`.
///
/// `DocumentSaved` fires for every save, not only Save-As; the update
/// is idempotent for regular Save (origin unchanged, title unchanged)
/// so no gate is needed.
fn sync_workspace_on_doc_saved(
    trigger: On<lunco_doc_bevy::DocumentSaved>,
    registry: Res<ModelicaDocumentRegistry>,
    mut ws: ResMut<lunco_workspace::WorkspaceResource>,
) {
    let id = trigger.event().doc;
    let Some(host) = registry.host(id) else { return };
    let doc = host.document();
    let new_origin = doc.origin().clone();
    let new_title = new_origin.display_name();
    // Push to recents on every File-saved event. `push_loose` dedupes
    // to the front, so re-saving an existing file simply hoists it to
    // the top — matches VS Code behaviour and is what makes Save-As
    // of an Untitled draft show up in "Open Recent File" next session
    // (the rebind from Untitled → File doesn't otherwise re-trigger
    // `add_document`, which is the only other recents push site).
    if let Some(p) = new_origin.canonical_path() {
        ws.recents.push_loose(p.to_path_buf());
    }
    if let Some(entry) = ws.document_mut(id) {
        entry.origin = new_origin;
        entry.title = new_title;
    }
}

/// Derive `WorkspaceResource.DocumentEntry.title` from the AST's
/// first top-level class name. Modelica's class-first identity model
/// (Dymola / OMEdit) means the tab label should follow the class, not
/// the original Untitled-N or filename — see
/// `docs/architecture/20-domain-modelica.md` § 7a.
///
/// Fallback ladder: AST first-class name → `origin.display_name()`
/// (file stem or `Untitled-N`).
///
/// Untitled docs also get their `origin.name` rewritten to match the
/// class name, so subsequent Save-As prompts default to
/// `<class>.mo` and the Files browser groups consistently.
///
/// TODO(modelica.naming.tab_title_source) — make the choice between
/// "ClassName" (current behaviour) vs "FileName" (VS Code) settings-
/// driven. Today the rule is hardcoded to ClassName.
///
/// TODO(ui.italic_for_unsaved) — italic styling on the tab label is
/// the renderer's job (lunco-workbench tab widget); not implemented
/// yet. Dirty-dot `●` likewise.
///
/// TODO(multi-class breadcrumb) — for `package P; model A; model B; end P;`
/// docs, this currently shows `P` (the first top-level class). Once
/// drilled-in tracking is per-doc-tab (it's per-canvas today), the
/// derived title should become `P.<drilled>` to match Dymola.
fn derive_doc_title(
    registry: Res<ModelicaDocumentRegistry>,
    mut ws: ResMut<lunco_workspace::WorkspaceResource>,
) {
    // Cheap when nothing changed: each iteration is a HashMap lookup +
    // a string compare, write only on diff. No per-doc generation
    // tracking yet — add one if profiling shows this in a hot frame.
    for (doc_id, host) in registry.docs() {
        let document = host.document();
        let derived = derive_title_from_doc(document);
        let Some(entry) = ws.document_mut(doc_id) else {
            continue;
        };
        if entry.title != derived {
            entry.title = derived.clone();
        }
        // For Untitled docs, also keep the origin in sync so Save-As
        // suggestions and other origin-readers see the new identity.
        if let lunco_doc::DocumentOrigin::Untitled { name } = &entry.origin {
            if name.as_str() != derived.as_str() {
                entry.origin = lunco_doc::DocumentOrigin::untitled(derived);
            }
        }
    }
}

/// Pure helper: read the first class name out of the per-doc Index,
/// fall back to the origin's display name. Kept separate so future
/// drilled-in / multi-class logic plugs in without re-deriving the
/// fallback chain.
fn derive_title_from_doc(doc: &crate::document::ModelicaDocument) -> String {
    if let Some(name) = doc.index().classes.keys().next() {
        if !name.is_empty() {
            return name.clone();
        }
    }
    doc.origin().display_name()
}

/// React to a Twin being added (Open Folder / Open Twin / promotion)
/// by spawning a background scan task that builds the package-browser
/// tree for that Twin's `.mo` content.
///
/// The scan was previously inlined into the welcome panel's "Open
/// Folder" button. Hoisting it onto the canonical `TwinAdded` event
/// means menu / picker / HTTP / scripts all converge on one path —
/// the welcome button is now just another fire-and-forget caller.
fn scan_twin_on_added(
    trigger: On<lunco_workspace::TwinAdded>,
    ws: Res<lunco_workspace::WorkspaceResource>,
    mut cache: ResMut<crate::package_tree::PackageTreeCache>,
) {
    let twin_id = trigger.event().twin;
    let Some(twin) = ws.twin(twin_id) else {
        return;
    };
    let folder = twin.root.clone();
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    let task = pool.spawn(async move { crate::package_tree::scan_twin_folder(folder) });
    cache.twin = None;
    cache.twin_scan_task = Some(task);
}

/// Drop the document linked to a despawned `ModelicaModel` entity, and
/// any compile-state bookkeeping attached to that document.
///
/// Behavior preserved from the entity-keyed era: when an entity is
/// despawned, its backing [`crate::document::ModelicaDocument`](crate::document::ModelicaDocument)
/// is also removed. The long-term design lets documents outlive entities
/// (edit-without-running, cosim re-spawn), so this will become opt-in
/// once the tab/view layer can explicitly unload a document.
fn cleanup_removed_documents(
    mut removed: RemovedComponents<ModelicaModel>,
    registry: Option<ResMut<ModelicaDocumentRegistry>>,
    compile_states: Option<ResMut<CompileStates>>,
    canvas_state: Option<ResMut<panels::canvas_diagram::CanvasDiagramState>>,
    signals: Option<ResMut<lunco_viz::SignalRegistry>>,
    viz_registry: Option<ResMut<lunco_viz::VisualizationRegistry>>,
    bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
) {
    let Some(mut registry) = registry else { return };
    let mut compile_states = compile_states;
    let mut canvas_state = canvas_state;
    let mut signals = signals;
    let mut viz_registry = viz_registry;
    let mut bus = bus;
    for entity in removed.read() {
        if let Some(doc) = registry.unlink_entity(entity) {
            registry.remove_document(doc);
            if let Some(states) = compile_states.as_mut() {
                states.remove(doc);
            }
            // Drop the per-doc canvas entry (viewport, selection,
            // in-flight projection task) so a later tab reusing the
            // id starts fresh. Matches how CompileStates is cleaned.
            if let Some(canvas) = canvas_state.as_mut() {
                canvas.drop_doc(doc);
            }
            // Drop the bus's terminal-outcome cache for this doc so
            // `last_outcome` doesn't accumulate dead entries across
            // long sessions. The doc id is monotonic — won't be
            // reused — but bounded growth is the right hygiene.
            if let Some(b) = bus.as_mut() {
                b.clear_outcomes_for(
                    lunco_workbench::status_bus::BusyScope::Document(doc.0),
                );
            }
            // tab-removal handled by `ModelTabs::close` /
            // `close_all_for_doc` already.
        }
        // Drop every registered signal + plot binding for this entity
        // so stale plots don't keep reading the last values forever.
        if let Some(sigs) = signals.as_mut() {
            sigs.drop_entity(entity);
        }
        if let Some(reg) = viz_registry.as_mut() {
            crate::ui::viz::drop_entity_bindings(reg, entity);
        }
    }
}

/// The Modelica workbench's default workspace preset.
///
/// Mirrors the "Analyze — Modelica deep dive" slot map from the workbench
/// design doc (`docs/architecture/11-workbench.md` § 4).
pub struct AnalyzePerspective {
    /// When true, seed the centre with the Welcome tab so a freshly
    /// switched-into Design workspace has *some* visible content.
    /// Sandbox-class embeds disable this (`ModelicaUiConfig
    /// { include_welcome_panel: false }`) so the Design tab opens
    /// empty — the user is expected to drill into a model first.
    pub seed_welcome: bool,
}

impl Default for AnalyzePerspective {
    fn default() -> Self { Self { seed_welcome: true } }
}

impl Perspective for AnalyzePerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("modelica_analyze") }
    fn title(&self) -> String { "📐 Design".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        // Side dock = Twin Browser only. The legacy
        // `PackageBrowserPanel` stays registered (View → Panels can
        // re-dock it) but is not docked by default — its remaining
        // unique features (MSL palette, drag-to-instantiate) will
        // migrate into the Twin Browser as a future `MslSection`.
        // Side-by-side dock would just present users with two
        // browsers solving the same job.
        // Two sibling tabs in the side dock — Twin (everything you
        // browse by name: workspace classes, MSL, bundled, future
        // USD/SysML — matches Dymola/OMEdit's single-Package-Browser
        // pattern) and Files (raw FS). Twin is leftmost so it's the
        // default active tab on first launch.
        layout.set_side_browser_tabs(vec![
            lunco_workbench::TWIN_BROWSER_PANEL_ID,
            lunco_workbench::FILES_PANEL_ID,
        ]);
        // Center is seeded with no singleton tab — model views are
        // multi-instance tabs opened dynamically by the Package Browser
        // (one tab per open document). An app that boots with a
        // default model can pre-open a tab after setup via
        // `WorkbenchLayout::open_instance(MODEL_VIEW_KIND, doc.raw())`.
        //
        // Keep a placeholder center tab so the dock's cross layout
        // still builds on apps with nothing open yet. When the first
        // real model tab opens, the placeholder stays docked next
        // to it — users can close it.
        if self.seed_welcome {
            layout.set_center(vec![PanelId("modelica_welcome")]);
            layout.set_active_center_tab(0);
        } else {
            layout.set_center(vec![]);
        }
        // Right dock — Telemetry (parameters, inputs, variable
        // toggles), Inspector (selected node's modifications), and
        // Component Palette (MSL instantiation). The Telemetry panel
        // is registered under the historical id `modelica_inspector`
        // for layout-stability reasons; the new selection-driven
        // Inspector uses `modelica_diagram_inspector`.
        layout.set_right_inspector_tabs(vec![
            PanelId("modelica_inspector"),
            PanelId("modelica_diagram_inspector"),
            PanelId("modelica_component_palette"),
        ]);
        // Bottom dock: Graphs first so it's the default active tab —
        // the simulation plot is what a user running a model wants
        // to see on landing, not the log stream. Console stays one
        // click away for compile / save / error output (VS Code's
        // Terminal/Output/Problems pattern, just with a different
        // default active tab).
        // Modelica plot is now a multi-instance kind; the first
        // instance is opened here, pinned to the well-known
        // `DEFAULT_MODELICA_GRAPH` VizId, and lands in the Bottom
        // slot alongside these singletons. Telemetry-panel checkboxes
        // bind to that same default VizId, preserving the historical
        // behaviour of "tick a variable → it appears in the Graphs tab".
        layout.set_bottom_tabs(vec![
            PanelId("modelica_experiments"),
            PanelId("modelica_diagnostics"),
            PanelId("modelica_console"),
            PanelId("modelica_journal"),
        ]);
        layout.open_instance(
            crate::ui::panels::graphs::MODELICA_PLOT_KIND,
            crate::ui::viz::DEFAULT_MODELICA_GRAPH.0,
        );
        // Graphs is the most-used bottom tab — pin it leftmost.
        layout.move_instance_to_front(
            crate::ui::panels::graphs::MODELICA_PLOT_KIND,
            crate::ui::viz::DEFAULT_MODELICA_GRAPH.0,
        );
    }
}

/// Plugin that registers all Modelica workbench UI panels.
///
/// Panels are entity viewers — they watch `WorkbenchState.selected_entity`
/// and render data for the active `ModelicaModel`. They work in any context:
/// standalone workbench, 3D overlay, or mission dashboard.
pub struct ModelicaUiPlugin;

impl Plugin for ModelicaUiPlugin {
    fn build(&self, app: &mut App) {
        // Read embed config once. Defaults to "everything on" (lunica).
        // Sandbox-class embeds insert `ModelicaUiConfig { include_*: false }`
        // before adding this plugin. See `lib.rs::ModelicaUiConfig`.
        let config = app
            .world()
            .get_resource::<crate::ModelicaUiConfig>()
            .cloned()
            .unwrap_or_default();

        // ModalQueue + modal host live in lunco-ui. `render_close_dialogs`
        // (added below by ModelicaCommandsPlugin) consumes ModalQueue, so
        // LuncoUiPlugin must be present whenever Modelica UI is mounted —
        // not just in the 3D `lunco-client` binary that originally added it.
        if !app.is_plugin_added::<lunco_ui::LuncoUiPlugin>() {
            app.add_plugins(lunco_ui::LuncoUiPlugin);
        }

        // Twin-level change journal subscribes to the generic document
        // lifecycle events this plugin fires. One journal per App —
        // adding the plugin multiple times is a no-op on `init_resource`.
        app.add_plugins(lunco_doc_bevy::TwinJournalPlugin);

        // Per AGENTS.md §3 (Tunability): Journal panel display knobs
        // go through `lunco-settings`. Registered here so `settings.json`
        // round-trips them.
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<panels::journal::JournalPanelSettings>();

        // Document hot-exit: persist every open Modelica buffer into the
        // per-Twin workspace-state and restore it (with unsaved edits) on
        // next launch. The workbench owns the file + lifecycle; this
        // codec just reads/writes Modelica buffers (AGENTS.md §3 + the
        // VS Code `workspaceStorage` model in 11-workbench §9).
        use lunco_workbench::AppDocumentSessionExt;
        app.register_document_session_codec(session_codec::ModelicaSessionCodec);

        // MSL class cache lives inside `class_cache::msl_engine` —
        // no Bevy plugin / resource needed. `peek_or_load_msl_class_blocking`
        // routes through the static engine; drill-in spawns its own
        // task that ultimately consults the same session.

        // Long-lived workspace `ModelicaEngine` mirrored from
        // `ModelicaDocumentRegistry`. Panel render code, API
        // observers, and async tasks query the same warm session
        // instead of rebuilding one per call.
        app.add_plugins(crate::engine_resource::ModelicaEnginePlugin);

        // Off-thread icon pre-warmer: on every DocumentOpened, walk
        // the doc's AST for cross-package type references and prime
        // rumoca's caches in the background. Drill-in projection
        // sees a populated cache instead of paying the cold-walk
        // seconds per first-time MSL chain.
        app.add_plugins(crate::icon_warmer::IconWarmerPlugin);

        // Intent layer: key chords → EditorIntent. Domain resolvers
        // (installed by ModelicaCommandsPlugin below) translate intents
        // into concrete commands for the docs they own.
        app.add_plugins(lunco_doc_bevy::EditorIntentPlugin);

        // Command bus for Modelica documents — Undo / Redo / Save /
        // Close (generic) + Compile (domain-specific) — plus the
        // EditorIntent resolver. UI buttons, keyboard shortcuts,
        // scripts, and the remote API all funnel through these.
        app.add_plugins(ModelicaCommandsPlugin);

        // Welcome-panel open-counter ledger. Loads the persisted
        // JSON at startup and bumps counts whenever `OpenClass`
        // fires — drives the progress dots on the learning paths.
        app.add_plugins(welcome_progress::WelcomeProgressPlugin);

        // Multi-screen help/tour overlay. Pops on first launch (per
        // `HelpOverlaySettings.seen` in settings.json), reachable
        // thereafter from Help → Show Tour or F1. Apps that embed the
        // Modelica workbench as a *secondary* workspace (sandbox's
        // Design tab) pre-insert `ModelicaUiConfig { include_help_overlay:
        // false, .. }` to suppress the tour — there's no point coaching
        // a sandbox user through lunica's onboarding.
        if config.include_help_overlay {
            app.add_plugins(help_overlay::HelpOverlayPlugin);
        }

        // Reflect-registered query providers exposed over the
        // ApiQueryRegistry (cf. spec 032). Feature-gated because the
        // registry only exists when `lunco-api` is enabled.
        #[cfg(feature = "lunco-api")]
        app.add_plugins(crate::api_queries::ModelicaApiQueriesPlugin);

        // Edit events — always registered so the GUI and tests can
        // dispatch them. External API exposure is gated separately
        // inside the plugin via `ApiVisibility` (off by default; pass
        // `--api-expose-edits` to expose). See
        // `crates/lunco-modelica/src/api_edits.rs` for the rationale.
        app.add_plugins(crate::api::ModelicaApiEditPlugin);

        app.init_resource::<WorkbenchState>()
            .init_resource::<ModelicaDocumentRegistry>()
            .init_resource::<CompileStates>()
            .init_resource::<crate::model_tabs::ModelTabs>()
            .init_resource::<crate::sim_default::RunTargetOverrides>()
            .init_resource::<crate::model_tabs_types::TabRenderContext>()
            .init_resource::<panels::code_editor::EditorBufferState>()
            .init_resource::<panels::console::ConsoleLog>()
            .init_resource::<panels::diagnostics::DiagnosticsLog>()
            // Journal panel reads directly from the canonical
            // `JournalResource` in `lunco-doc-bevy`; no local cache.
            // Registration of `JournalResource` happens in
            // `TwinJournalPlugin`, added as part of the workbench plugin.
            // Canvas animation: API-driven AddComponent calls queue a
            // pending camera focus; this system applies it via
            // `viewport.set_target` (which auto-eases) once the new
            // node has landed in the projected scene. See
            // `docs/architecture/20-domain-modelica.md` § 9c.
            .init_resource::<panels::canvas_diagram::PendingApiFocusQueue>()
            .init_resource::<panels::canvas_diagram::PendingApiConnectionQueue>()
            .add_systems(
                Update,
                (
                    panels::canvas_diagram::drive_pending_api_focus,
                    panels::canvas_diagram::drive_pending_api_connections,
                )
                    .chain(),
            )
            // Forward StatusBus events to the Console panel so the
            // user has a chronological audit trail of every status
            // event from every subsystem (MSL, compile, sim, …).
            .add_systems(Update, fan_status_bus_to_console)
            // Reactive UI observer of core `MslLoadState` → status bus (moved
            // here from the core MSL plugin; core no longer touches the bus).
            .add_systems(Update, core_observers::mirror_msl_state_to_status_bus)
            // Reactive UI observer: drain core live-sim samples → viz plots.
            // The core worker no longer references lunco_viz.
            .add_systems(Update, core_observers::drain_sim_samples_to_viz)
            // Reactive UI observers: core notices → Console; source-root load
            // state → status bar. Core emits events/state; these project them.
            .add_systems(Update, core_observers::drain_notices_to_console)
            .add_systems(Update, core_observers::mirror_source_roots_to_status_bus)
            // Reactive UI: relay core compile requests → CompileModel command.
            .add_systems(Update, core_observers::relay_compile_requests)
            // Reactive UI: feed input/workspace pacing hints into the core
            // parse scheduler (before it reads them this frame).
            .add_systems(
                Update,
                core_observers::feed_parse_pacing
                    .before(crate::engine_resource::drive_engine_sync),
            )
            // Reactive UI: project terminal experiment-run events into console,
            // plot auto-pick, and SignalRegistry playback.
            .add_systems(Update, core_observers::project_run_results_to_ui)
            .init_resource::<panels::canvas_projection::DiagramAutoLayoutSettings>()
            .init_resource::<panels::palette::PaletteState>()
            .init_resource::<panels::palette::ComponentDragPayload>()
            .insert_resource(crate::package_tree::PackageTreeCache::new())
            .add_systems(Update, browser_dispatch::drain_browser_actions)
            .add_systems(Update, panels::package_browser::handle_package_loading_tasks)
            .add_systems(Update, cleanup_removed_documents)
            .add_systems(Update, drain_document_changes)
            .add_systems(Update, commands::drain_open_file_results)
            // Mirror the active document's volatile fields (source,
            // detected_name) into the registry-by-doc lookup
            // B.3 phase 6 (2026-05-08): the
            // `mirror_active_open_model` Update system + the
            // `mirror_open_model_on_doc_changed` observer were
            // deleted with the `OpenModel` cache. All readers now
            // derive source/metadata from
            // `ModelicaDocumentRegistry::host(doc).document()`
            // directly — no mirror needed.
            //
            // Workspace shadow-sync: keep `WorkspaceResource` populated
            // from the existing document-registry lifecycle.
            .add_observer(sync_workspace_on_doc_opened)
            .add_observer(sync_workspace_on_doc_closed)
            .add_observer(sync_workspace_on_doc_saved)
            // Coarse cache invalidation: any doc edit can shift
            // cross-file inheritance chains, so the paint-hot
            // port-icon cache flushes wholesale. Re-fills lazily
            // on next paint via rumoca's content-hash cache —
            // unchanged classes return the same icon instantly.
            .add_observer(invalidate_port_icon_cache_on_doc_changed)
            // Cross-truth rule R4: close tabs drilled into a removed
            // class. Watermark resource keeps the observer O(new
            // changes) per fire.
            .init_resource::<ClassRemovedWatermark>()
            .add_observer(close_drilled_tabs_on_class_removed)
            // Push-driven editor buffer sync — replaces the old
            // per-frame generation poll in `CodeEditorPanel::render`.
            .add_observer(panels::code_editor::editor_on_doc_changed)
            // Structural ops that arrive against a stale syntax
            // cache are deferred here and applied once the async
            // engine sync lands a fresh parse — removes the last
            // sync-reparse from the write path.
            .init_resource::<panels::canvas_diagram::PendingStructuralOps>()
            .add_systems(
                Update,
                panels::canvas_diagram::drain_pending_structural_ops,
            )
            .add_systems(Update, derive_doc_title)
            // Twin panel reads docs directly from `ModelicaDocumentRegistry`
            // now (PR4); no separate `LoadedModelicaClasses` registry +
            // observer pair to keep in sync.
            // Kick off a background scan whenever the workbench
            // announces a new Twin (Open Folder / Open Twin / "Save
            // as Twin" promotion). The scan populates the package
            // browser's Twin tree; until this lands, opening a Twin
            // would update WorkspaceResource but the Modelica
            // sidebar wouldn't reflect it.
            .add_observer(scan_twin_on_added)
            .add_systems(Update, panels::diagnostics::refresh_diagnostics)
            // Input activity timestamp — read by `drive_engine_sync`
            // to gate edit-debounced reparses (replaces the prior
            // standalone `ast_refresh` system).
            .init_resource::<input_activity::InputActivity>()
            .add_systems(bevy::prelude::PreUpdate, input_activity::stamp_user_input)
            .add_systems(Startup, register_settings_menu)
            .add_systems(Startup, register_edit_menu)
            .init_resource::<panels::code_editor::CodeEditorMenuRequest>()
            // Image-loader install is a first-frame one-shot — runs
            // in the egui primary-context pass until the context is
            // ready and the loaders land, then the marker resource
            // `ImageLoadersInstalled` short-circuits the run_if and
            // Bevy stops calling us entirely.
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                install_image_loaders_once.run_if(
                    bevy::ecs::schedule::common_conditions::not(
                        bevy::ecs::schedule::common_conditions::resource_exists::<
                            ImageLoadersInstalled,
                        >,
                    ),
                ),
            )
            .register_panel(panels::package_browser::PackageBrowserPanel)
            .register_panel(lunco_workbench::TwinBrowserPanel)
            .register_panel(lunco_workbench::FilesPanel)
            .register_panel(panels::welcome::WelcomePanel)
            .register_panel(panels::telemetry::TelemetryPanel)
            .register_instance_panel(panels::graphs::ModelicaPlotPanel)
            .register_panel(panels::console::ConsolePanel)
            .register_panel(panels::diagnostics::DiagnosticsPanel)
            .register_panel(panels::journal::JournalPanel)
            .register_panel(panels::experiments::ExperimentsPanel)
            .init_resource::<panels::experiments::ExperimentVisibility>()
            .init_resource::<panels::experiments::PlotPanelStates>()
            .init_resource::<doc_pin::DocPinState>()
            .init_resource::<panels::experiments::ActivePlot>()
            .register_panel(panels::canvas_diagram::CanvasDiagramPanel)
            .init_resource::<panels::canvas_diagram::CanvasDiagramState>()
            .init_resource::<panels::canvas_diagram::PaletteSettings>()
            .init_resource::<panels::canvas_diagram::DiagramProjectionLimits>()
            .init_resource::<document_openings::DocumentOpenings>()
            .init_resource::<document_openings::AstReparseBusyHandles>()
            .init_resource::<document_openings::CompileBusyHandles>()
            .init_resource::<document_openings::SimulateBusyHandle>()
            .init_resource::<panels::canvas_diagram::CanvasSnapSettings>()
            .add_systems(Update, document_openings::drive_file_load_openings)
            .add_systems(Update, document_openings::track_ast_reparse_busy)
            .add_systems(Update, document_openings::track_compile_busy)
            .add_systems(Update, document_openings::track_simulate_busy)
            .add_systems(Update, panels::canvas_diagram::drive_drill_in_loads)
            .add_systems(Update, panels::canvas_diagram::drive_duplicate_loads)
            // Flip `cancel` on every non-active tab's in-flight
            // canvas projection. On wasm `AsyncCompute` runs
            // cooperatively on the main thread; uncancelled stale
            // projections steal cycles the active tab's projection
            // needs. See `cancel_inactive_projections` rustdoc.
            .add_systems(Update, panels::canvas_diagram::cancel_inactive_projections)
            .register_panel(panels::inspector::InspectorPanel)
            .register_panel(panels::palette::ComponentPalettePanel)
            // Multi-instance: one tab per open document. Instances are
            // opened at runtime by the Package Browser.
            .register_instance_panel(panels::model_view::ModelViewPanel::default())
            .register_perspective(AnalyzePerspective {
                seed_welcome: config.include_welcome_panel,
            })
            .register_perspective_help(
                lunco_workbench::PerspectiveId("modelica_analyze"),
                lunco_workbench::PerspectiveHelp {
                    title: "📐 Design",
                    description: "Modelica engineering workbench. Author models as \
                                  text or wired diagrams, then compile and simulate.",
                    shortcuts: vec![
                        lunco_workbench::HelpShortcut { keys: "F5", description: "Compile & run the active model" },
                        lunco_workbench::HelpShortcut { keys: "Ctrl+N", description: "New untitled model" },
                        lunco_workbench::HelpShortcut { keys: "Ctrl+S", description: "Save the active model" },
                        lunco_workbench::HelpShortcut { keys: "Ctrl+Z", description: "Undo" },
                        lunco_workbench::HelpShortcut { keys: "Ctrl+Shift+Z", description: "Redo" },
                        lunco_workbench::HelpShortcut { keys: "F2", description: "Rename selected item in browser" },
                    ],
                    mouse: vec![
                        lunco_workbench::HelpMouse { interaction: "Drag", description: "Move components · drag a part onto the diagram" },
                        lunco_workbench::HelpMouse { interaction: "Drag port → port", description: "Connect two component ports" },
                        lunco_workbench::HelpMouse { interaction: "Scroll", description: "Zoom the diagram canvas" },
                    ],
                    // Only offer the tour where it actually exists: the
                    // HelpOverlayPlugin (and its tour-request consumer) is
                    // gated on this same flag. Embedded-as-secondary hosts
                    // (sandbox's Design tab) set it false → no dead button.
                    has_tour: config.include_help_overlay,
                },
            );

        // Contribute the Modelica section to the Twin Browser's
        // section registry. The workbench's WorkbenchPlugin already
        // installed the registry resource and the built-in Files
        // section; we just append. ensure it exists first to avoid
        // panics during mixed-mode or deferred plugin builds.
        app.init_resource::<lunco_workbench::BrowserSectionRegistry>();
        // One section per domain — `ModelicaSection` reads system
        // libraries straight from `PackageTreeCache::roots` and
        // workspace docs from `ModelicaDocumentRegistry`. No parallel
        // registry to keep in sync. Adding a new library is a one-line
        // `roots.push(...)` in `PackageTreeCache::new`; future domain
        // crates (`UsdSection`, `SysmlSection`, ...) follow the same
        // outer pattern with their own per-domain section.
        app.world_mut()
            .resource_mut::<lunco_workbench::BrowserSectionRegistry>()
            .register(browser_section::ModelicaSection::default());
    }
}

/// Push Modelica editor preferences onto the application-wide
/// Settings menu. Lives in the workbench Settings dropdown rather
/// than a per-panel gear button — keeps editor toolbar tidy and
/// all prefs discoverable in one place.
fn register_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world
        .get_resource_mut::<lunco_workbench::WorkbenchLayout>()
    else {
        return;
    };
    layout.register_settings(|ui, world| {
        ui.label(egui::RichText::new("Code Editor").weak().small());
        let mut buf = world.resource_mut::<panels::code_editor::EditorBufferState>();
        ui.checkbox(&mut buf.word_wrap, "Word wrap")
            .on_hover_text("Wrap long lines at editor width");
        ui.checkbox(&mut buf.auto_indent, "Auto indent")
            .on_hover_text("Copy previous line's indent on Enter");
        drop(buf);
        ui.separator();
        ui.label(egui::RichText::new("Component Palette").weak().small());
        let mut palette =
            world.resource_mut::<panels::canvas_diagram::PaletteSettings>();
        ui.checkbox(
            &mut palette.show_icon_only_classes,
            "Show icon-only classes",
        )
        .on_hover_text(
            "Include decorative classes from `Modelica.*.Icons.*` \
             subpackages in the add-component menu. Off by default \
             because they have no connectors and typically aren't \
             what a user wants to drop on a diagram.",
        );
        drop(palette);
        ui.separator();
        ui.label(egui::RichText::new("Diagram").weak().small());
        let mut limits =
            world.resource_mut::<panels::canvas_diagram::DiagramProjectionLimits>();
        ui.horizontal(|ui| {
            ui.label("Max nodes");
            ui.add(
                egui::DragValue::new(&mut limits.max_nodes)
                    .range(10..=100_000)
                    .speed(10.0),
            )
            .on_hover_text(
                "Upper bound on component count before the projector \
                 bails out with a warning. Raise for large models; \
                 lower if projections feel slow on modest hardware.",
            );
        });
        ui.horizontal(|ui| {
            ui.label("Timeout (s)");
            let mut secs = limits.max_duration.as_secs();
            if ui
                .add(
                    egui::DragValue::new(&mut secs)
                        .range(1_u64..=3600)
                        .speed(1.0),
                )
                .on_hover_text(
                    "Wall-clock deadline for a single projection. \
                     If the background parse + build takes longer, \
                     the task is cancelled and the canvas stays empty \
                     with a log warning. Default 60 s — only huge or \
                     pathological models get close.",
                )
                .changed()
            {
                limits.max_duration = std::time::Duration::from_secs(secs);
            }
        });
        drop(limits);
        ui.add_space(4.0);
        // ── Drag snap ────────────────────────────────────────────
        // Off by default — a lot of Modelica source uses
        // hand-placed non-grid positions and the user shouldn't
        // have their authored placements auto-rounded unless they
        // opted in. When on, drags quantise *live* (visible during
        // the drag itself) to multiples of `step` Modelica units.
        let mut snap =
            world.resource_mut::<panels::canvas_diagram::CanvasSnapSettings>();
        ui.checkbox(&mut snap.enabled, "Snap to grid on drag").on_hover_text(
            "When on, dragging an icon quantises its position to a \
             grid. Applies live during the drag and at commit. Off \
             by default.",
        );
        ui.horizontal(|ui| {
            ui.label("Grid step");
            ui.add_enabled(
                snap.enabled,
                egui::DragValue::new(&mut snap.step)
                    .range(0.5..=50.0)
                    .speed(0.5)
                    .suffix(" units"),
            )
            .on_hover_text(
                "Snap granularity in Modelica diagram-coordinate \
                 units (the 200-unit standard system). Common: 2 \
                 (fine), 5 (medium), 10 (coarse).",
            );
        });
        drop(snap);
        ui.separator();
        render_assets_settings(ui, world);
    });
}

/// Settings rows for the "Assets" section — MSL load state, bundle URL,
/// local override, last-fetched bookkeeping, and quick actions
/// (open cache folder, clear cache).
fn render_assets_settings(ui: &mut bevy_egui::egui::Ui, world: &mut World) {
    use bevy_egui::egui;
    use lunco_assets::msl::{MslLoadPhase, MslLoadState};

    // Current state line.
    let state = world.get_resource::<MslLoadState>().cloned();

    // If the Modelica UI is active, the MslSettings resource MUST exist
    // by architectural design (ModelicaPlugin adds ModelicaCorePlugin adds MslRemotePlugin).
    let mut settings = world.resource_mut::<crate::msl_settings::MslSettings>();

    ui.label(egui::RichText::new("Assets — MSL").weak().small());

    match state.as_ref() {
        Some(MslLoadState::Ready {
            file_count,
            uncompressed_bytes,
            ..
        }) => {
            ui.label(format!(
                "Status: ready · {file_count} files · {:.1} MB",
                *uncompressed_bytes as f64 / 1_048_576.0,
            ));
        }
        Some(MslLoadState::Loading {
            phase,
            bytes_done,
            bytes_total,
        }) => {
            let phase = match phase {
                MslLoadPhase::FetchingManifest => "fetching manifest",
                MslLoadPhase::FetchingBundle => "downloading",
                MslLoadPhase::LoadingCache => "loading from cache",
                MslLoadPhase::Decompressing => "extracting",
                MslLoadPhase::Parsing => "loading",
            };
            if *bytes_total > 0 {
                ui.label(format!(
                    "Status: {phase} · {:.1} / {:.1} MB",
                    *bytes_done as f64 / 1_048_576.0,
                    *bytes_total as f64 / 1_048_576.0,
                ));
            } else {
                ui.label(format!("Status: {phase}"));
            }
        }
        Some(MslLoadState::Failed(msg)) => {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Status: failed — {msg}"));
        }
        Some(MslLoadState::NotStarted) | None => {
            ui.label("Status: not started");
        }
    }

    // Resolved on-disk path. May be the auto-fetch destination, the
    // workspace `.cache/msl/`, or a user-supplied override.
    let root = lunco_assets::msl_source_root_path();
    match root {
        Some(p) => {
            ui.horizontal(|ui| {
                ui.label("Root:");
                ui.monospace(p.display().to_string());
            });
        }
        None => {
            ui.label("Root: (not materialised yet)");
        }
    }

    // Local-root override — wins over auto-download. Restart needed
    // for changes to take effect (the resolution happens once at
    // plugin build).
    let mut local = settings
        .local_root_override
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Local root");
        if ui
            .add(
                egui::TextEdit::singleline(&mut local)
                    .desired_width(360.0)
                    .hint_text("/path/to/msl (parent of Modelica/)"),
            )
            .on_hover_text(
                "Absolute path to a Modelica Standard Library tree on \
                 disk. The directory must contain a `Modelica/` \
                 subdirectory. Takes precedence over the auto-download. \
                 Restart required.",
            )
            .changed()
        {
            settings.local_root_override = if local.trim().is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(local.trim()))
            };
        }
    });
    if let Some(v) = settings.last_fetched_version.clone() {
        ui.label(
            egui::RichText::new(format!("Installed version: {v}"))
                .weak()
                .small(),
        );
    }
    drop(settings);

    // Actions.
    let load_state = world
        .get_resource::<lunco_assets::msl::MslLoadState>()
        .cloned();
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    let install_running = matches!(
        load_state,
        Some(lunco_assets::msl::MslLoadState::Loading { .. })
    );
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    let install_failed = matches!(
        load_state,
        Some(lunco_assets::msl::MslLoadState::Failed(_))
    );
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    let install_ready = matches!(
        load_state,
        Some(lunco_assets::msl::MslLoadState::Ready { .. })
    );
    ui.horizontal(|ui| {
        // While an install is in flight, show Cancel. When it's
        // finished (Ready/Failed) show Reinstall/Retry instead so the
        // user can always pick "do it again" without restarting.
        #[cfg(not(target_arch = "wasm32"))]
        {
        if install_running {
            if let Some(cancel) = world.get_resource::<crate::msl_remote::MslInstallCancel>() {
                if ui
                    .button("Cancel")
                    .on_hover_text(
                        "Stop the in-flight MSL download/index. The \
                         download aborts within one chunk; the indexer \
                         aborts at the next phase boundary.",
                    )
                    .clicked()
                {
                    cancel.0.store(true, std::sync::atomic::Ordering::Relaxed);
                    bevy::log::info!("[MSL] cancel requested by user");
                }
            }
        } else if install_failed {
            if ui
                .button("Retry")
                .on_hover_text(
                    "Re-run the MSL download + indexer. Clears the \
                     previous cache so a partial install is wiped.",
                )
                .clicked()
            {
                crate::msl_remote::reinstall_msl(world);
            }
        } else if install_ready {
            if ui
                .button("Reinstall")
                .on_hover_text(
                    "Force-redownload MSL and rebuild the bincode cache. \
                     Wipes the current cache directory first.",
                )
                .clicked()
            {
                crate::msl_remote::reinstall_msl(world);
            }
        }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if ui
            .button("Open cache folder")
            .on_hover_text("Reveal the MSL cache directory in the system file manager.")
            .clicked()
        {
            let path = lunco_assets::cache_subdir("msl");
            if let Err(e) = open_in_file_manager(&path) {
                bevy::log::warn!("[Assets] could not open {}: {e}", path.display());
            }
        }
        if ui
            .button("Clear cache")
            .on_hover_text(
                "Delete the MSL cache directory. Use when a previous \
                 fetch left a partial tree. Restart to re-download.",
            )
            .clicked()
        {
            let path = lunco_assets::cache_subdir("msl");
            match std::fs::remove_dir_all(&path) {
                Ok(()) => bevy::log::info!("[Assets] cleared {}", path.display()),
                Err(e) => {
                    bevy::log::warn!("[Assets] could not clear {}: {e}", path.display())
                }
            }
        }
    });

    // ── Optional libraries ────────────────────────────────────────
    // Other entries in Assets.toml (e.g. ThermofluidStream). Each row
    // shows current state (installed / missing) and an Install /
    // Reinstall button. Click fires `download_asset` on
    // AsyncComputeTaskPool — no fine-grained progress, just a log
    // line on completion. The indexer picks these up on its next
    // run (or on app restart).
    #[cfg(not(target_arch = "wasm32"))]
    render_optional_libraries(ui, world);
}

#[cfg(not(target_arch = "wasm32"))]
fn render_optional_libraries(ui: &mut bevy_egui::egui::Ui, _world: &mut World) {
    use bevy_egui::egui;
    use lunco_assets::download::AssetManifest;

    let manifest = match AssetManifest::from_str(crate::msl_remote::BUNDLED_ASSETS_TOML) {
        Ok(m) => m,
        Err(e) => {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("Could not parse Assets.toml: {e}"),
            );
            return;
        }
    };
    let optional: Vec<_> = manifest
        .assets
        .iter()
        .filter(|(k, _)| k.as_str() != "msl")
        .collect();
    if optional.is_empty() {
        return;
    }
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Assets — Optional libraries")
            .weak()
            .small(),
    );
    for (key, entry) in optional {
        let dest = lunco_assets::cache_dir().join(&entry.dest);
        let installed = dest.exists()
            && std::fs::read_dir(&dest)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&entry.name).strong());
            if let Some(v) = entry.version.as_ref() {
                ui.label(egui::RichText::new(format!("v{v}")).weak().small());
            }
            ui.label(
                egui::RichText::new(if installed {
                    "installed"
                } else {
                    "not installed"
                })
                .weak()
                .small(),
            );
            let label = if installed { "Reinstall" } else { "Install" };
            if ui
                .button(label)
                .on_hover_text(format!(
                    "Download {} from {}.\nLanding zone: {}",
                    entry.name,
                    entry.url,
                    dest.display()
                ))
                .clicked()
            {
                spawn_optional_library_install(key.clone(), entry.clone(), installed);
            }
        });
    }
}

/// Kick off a one-shot download for a non-MSL library entry. No state
/// is published to ECS — the next render of the panel re-reads the
/// disk to determine "installed". Errors are logged via bevy::log.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_optional_library_install(
    key: String,
    entry: lunco_assets::download::AssetEntry,
    is_reinstall: bool,
) {
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    pool.spawn(async move {
        // For a reinstall, wipe the existing dest first so the
        // version-file cache-hit check doesn't short-circuit.
        if is_reinstall {
            let dest = lunco_assets::cache_dir().join(&entry.dest);
            if let Err(e) = std::fs::remove_dir_all(&dest) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    bevy::log::warn!(
                        "[Assets] could not clear {} before reinstall: {e}",
                        dest.display()
                    );
                }
            }
        }
        bevy::log::info!("[Assets] installing {}…", entry.name);
        match lunco_assets::download::download_asset(&entry, &key) {
            Ok(()) => bevy::log::info!("[Assets] {} installed", entry.name),
            Err(e) => bevy::log::warn!("[Assets] {} install failed: {e}", entry.name),
        }
    })
    .detach();
}

/// Best-effort "reveal in file manager" — spawns the platform's
/// default file browser at `path`. Returns an io error if the spawn
/// fails outright (the file manager itself may still pop up an error
/// dialog; we don't try to capture that).
fn open_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
    }
    Ok(())
}

/// Contribute Cut/Copy/Paste/Select-All entries to the workbench's
/// global Edit menu. The entries flip flags on
/// [`panels::code_editor::CodeEditorMenuRequest`]; the code-editor
/// render reads & clears them next frame, OR-merging into the same
/// flags the in-panel toolbar uses. Keeps clipboard/selection
/// handling in one place while letting the menu drive it.
fn register_edit_menu(world: &mut World) {
    let Some(mut layout) = world
        .get_resource_mut::<lunco_workbench::WorkbenchLayout>()
    else {
        return;
    };
    layout.register_edit_menu(|ui, world| {
        // TODO: promote Cut / Copy / Paste / Select All to typed
        // `#[Command]` events so the HTTP API can drive them too
        // (mirrors the existing `Undo` / `Redo` commands in
        // `ui/commands.rs`). Today they only flow through the menu /
        // toolbar / keyboard since the operation is scoped to the
        // currently-focused egui TextEdit, which has no
        // representation on the API side — a typed command would
        // need an explicit `doc` + range/text payload.
        let mut req = world
            .resource_mut::<panels::code_editor::CodeEditorMenuRequest>();
        if ui.button("Cut\tCtrl+X").clicked() {
            req.cut = true;
            ui.close();
        }
        if ui.button("Copy\tCtrl+C").clicked() {
            req.copy = true;
            ui.close();
        }
        if ui.button("Paste\tCtrl+V").clicked() {
            req.paste = true;
            ui.close();
        }
        ui.separator();
        if ui.button("Select All\tCtrl+A").clicked() {
            req.select_all = true;
            ui.close();
        }
    });
}

/// Marker resource — inserted by
/// [`install_image_loaders_once`] once the egui context is ready and
/// the loaders are wired. The system's `run_if(not(resource_exists))`
/// condition means Bevy stops scheduling the system after this
/// resource appears, so we pay exactly one successful install plus
/// however many frames we had to wait for the context to come up
/// (typically one or two).
#[derive(bevy::prelude::Resource)]
struct ImageLoadersInstalled;

/// First-frame egui image-loader registration. Gated by a `run_if`
/// so Bevy stops scheduling it after the first successful install —
/// no per-frame cost at all, not even a function-call return.
fn install_image_loaders_once(
    mut commands: bevy::prelude::Commands,
    mut contexts: bevy_egui::EguiContexts,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        // Context not ready yet — the run_if keeps scheduling us so
        // we get another shot next frame.
        return;
    };
    // Built-in loaders for file://, http(s)://, raw paths, bytes://,
    // etc. Covers everything the Modelica Documentation HTML can
    // reference through normal URIs.
    egui_extras::install_image_loaders(ctx);
    // Custom loader for `modelica://Package/Resources/…` URIs used
    // throughout MSL Documentation blocks.
    let loader = std::sync::Arc::new(image_loader::ModelicaImageLoader::new());
    ctx.add_bytes_loader(loader);
    bevy::log::info!(
        "[ModelicaImageLoader] installed egui_extras loaders + modelica:// loader"
    );

    commands.insert_resource(ImageLoadersInstalled);
}

/// Forward newly-pushed [`lunco_workbench::status_bus::StatusBus`]
/// events to the [`panels::console::ConsoleLog`].
///
/// We track the count of *discrete* history entries we've already
/// mirrored so progress ticks (which mutate the bus seq but don't
/// append to history) don't show up as console spam. New entries
/// arrive at the back of the ring buffer; old ones drop off the front
/// when capacity is hit. We use a (last_seen_seq, last_back_message)
/// pair to detect "new entries since we last looked" without needing
/// per-event sequence numbers.
fn fan_status_bus_to_console(
    bus: bevy::prelude::Res<lunco_workbench::status_bus::StatusBus>,
    mut console: bevy::prelude::ResMut<panels::console::ConsoleLog>,
    mut last_count: bevy::prelude::Local<usize>,
) {
    let count = bus.history().count();
    if count == 0 {
        *last_count = 0;
        return;
    }
    if count == *last_count {
        return;
    }
    // The history ring buffer can lose entries from the front when
    // capacity hits. We only forward what's *new* at the back since
    // last we looked. Skip the first `(count - delta).min(count)`
    // events; forward the rest.
    let delta = count.saturating_sub(*last_count);
    for ev in bus.history().rev().take(delta).collect::<Vec<_>>().into_iter().rev() {
        let level = match ev.level {
            lunco_workbench::status_bus::StatusLevel::Info => panels::console::ConsoleLevel::Info,
            lunco_workbench::status_bus::StatusLevel::Warn => panels::console::ConsoleLevel::Warn,
            lunco_workbench::status_bus::StatusLevel::Error => panels::console::ConsoleLevel::Error,
            // Progress events shouldn't be in `history` (they live in
            // active_progress), but if one ever sneaks in, surface as Info.
            lunco_workbench::status_bus::StatusLevel::Progress => panels::console::ConsoleLevel::Info,
        };
        console.push(level, format!("[{}] {}", ev.source, ev.message));
    }
    *last_count = count;
}
