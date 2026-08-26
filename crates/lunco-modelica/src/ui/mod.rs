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
use lunco_workbench::{
    MenuCtx, PanelId, Perspective, PerspectiveId, UndoProbeCtx, WorkbenchAppExt, WorkbenchLayout,
};
// Core document/library/compile state moved out of `ui` into `crate::state`.
use crate::state::{ModelicaDocumentRegistry, WorkbenchState};
use lunco_doc_bevy::DocumentDiagnostics;

/// The [`PanelId`] under which `ModelViewPanel` is registered. Lives in the
/// `ui` module because `PanelId` is a workbench (UI) panel-registry key — the
/// core tab types in [`crate::model_tabs_types`] don't depend on it.
pub const MODEL_VIEW_KIND: PanelId = PanelId("modelica_model_view");

pub mod class_source;
pub mod document_openings;
/// Source extraction and rewriting for the UI's "Duplicate to edit" command.
pub mod duplicate;

pub mod commands;
/// Reactive UI observers of core domain state (status-bus mirrors, etc.).
pub mod core_observers;
/// Bevy/UI integration for shareable model links (clipboard + boot loader).
pub mod model_share;
/// UI→core bridge: workbench rename events → `RenameModelicaClass`.
pub mod rename_chain;
pub use commands::{CompileModel, CreateNewScratchModel, ModelicaCommandsPlugin};

pub mod class_display;
pub mod icon_paint;
pub mod image_loader;
/// Debounced AST reparse driver — see module docs.
pub mod input_activity;
pub mod panels;
pub mod solver_picker;
pub mod text_node;
pub mod theme;
pub mod uri_handler;
pub mod viz;
pub mod wasm_autosave;
pub mod wasm_clipboard;
pub mod welcome_progress;
pub mod wire_router;

/// Modelica section of the Twin Browser — class-tree contributed by
/// this crate to `lunco-workbench`'s `BrowserSectionRegistry`.
pub mod browser_section;
/// Twin-scoped downloadable resources shown in the Twin Browser.
pub mod twin_datasets;

/// Drains the workbench's `BrowserActions` outbox and routes
/// section-emitted intents (open file, open Modelica class) into the
/// existing document-load and drill-in pipelines.
pub mod browser_dispatch;

/// Per-panel "pin to model" overrides for singleton inspector panels.
pub mod doc_pin;

/// Document hot-exit codec — persists & restores open Modelica buffers.
pub mod session_codec;

use crate::ModelicaModel;

/// Shadow-sync observer: Modelica doc opened → register entry in the
/// Workspace session. The Workspace list is populated from the Modelica
/// document lifecycle, while the Modelica registry remains the domain owner.
/// Invalidate every source-derived memo when any doc changes — the merged icons in
/// `ModelicaEngine` and the decoded bitmap textures on the paint side.
///
/// Costs one atomic increment; the memos drop themselves lazily on next access, so
/// a change nobody paints after is free. This used to clear a paint-side port-icon
/// cache that was, in fact, dead — while the bitmap textures it *should* have been
/// clearing were never invalidated at all. See [`crate::icon_memo`].
fn invalidate_source_memos_on_doc_changed(_trigger: On<lunco_doc_bevy::DocumentChanged>) {
    crate::icon_memo::invalidate_source_memos();
}

/// Per-doc generation watermark for the
/// [`close_drilled_tabs_on_class_removed`] observer. Tracks the last
/// `ModelicaDocument::generation` we processed so each
/// `DocumentChanged` fire only walks new entries in the change ring
/// buffer. Falls back to a re-anchor when the retention window has
/// rolled over (`changes_since` returns `None`).
#[derive(Resource, Default)]
struct ClassRemovedWatermark(std::collections::HashMap<lunco_doc::DocumentId, u64>);

/// Cross-truth rule R4 (see `docs/architecture/20-domain-modelica.md` §5):
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
    let Some(host) = registry.host(doc) else {
        return;
    };
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

// `world` module deleted as part of the A2 single-struct migration:
// `ClassEntry` is now the canonical class record consumed everywhere,
// and per-doc `ModelicaIndex.classes` already holds it, so a separate
// `ModelicaWorld` resource just duplicates state. The unified
// read-side resolver (`class_metadata::resolve_metadata`) consults
// the pre-baked MSL library + the live per-doc index directly.

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
/// Per-doc index generation the title was last derived for (CQ-209).
/// The derived title is a pure function of the doc's index (+ origin),
/// so an unchanged generation can't change it — gating on this skips the
/// `derive_title_from_doc` String allocation on the (overwhelmingly
/// common) no-change frame.
#[derive(Resource, Default)]
struct DocTitleGenCache(std::collections::HashMap<lunco_doc::DocumentId, u64>);

fn derive_doc_title(
    registry: Res<ModelicaDocumentRegistry>,
    mut ws: ResMut<lunco_workspace::WorkspaceResource>,
    mut cache: ResMut<DocTitleGenCache>,
) {
    for (doc_id, host) in registry.docs() {
        let document = host.document();
        let generation = document.index().generation;
        // Skip docs whose index hasn't changed since we last derived —
        // no String alloc, no workspace write.
        if cache.0.get(&doc_id) == Some(&generation) {
            continue;
        }
        let derived = derive_title_from_doc(document);
        let Some(entry) = ws.document_mut(doc_id) else {
            // Not in the workspace yet — retry next frame; don't cache,
            // or we'd never set the title once it appears.
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
        cache.0.insert(doc_id, generation);
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

/// Purge per-**entity** state when a `ModelicaModel` entity despawns.
///
/// Entity-scoped only, deliberately: a despawn must never drop the document.
/// `clear_scene_entities` despawns every `WorldGrid` child on any scene
/// clear/reload, and USD-sourced cosim models are `WorldGrid` children — so
/// dropping the document here meant loading another scene (or a script firing
/// `ClearScene`) silently destroyed the user's open Modelica document, unsaved
/// edits and undo stack included, with no prompt.
///
/// Documents outlive entities (edit-without-running, cosim re-spawn). They are
/// dropped only by the explicit `CloseDocument` path, which owns every
/// doc-scoped purge; an entity dying says nothing about whether the user is
/// done with the source.
fn cleanup_removed_simulators(
    mut removed: RemovedComponents<ModelicaModel>,
    registry: Option<ResMut<ModelicaDocumentRegistry>>,
    signals: Option<ResMut<lunco_viz::SignalRegistry>>,
    viz_registry: Option<ResMut<lunco_viz::VisualizationRegistry>>,
) {
    let mut registry = registry;
    let mut signals = signals;
    let mut viz_registry = viz_registry;
    for entity in removed.read() {
        // Drop the entity→doc link — and ONLY the link. This system is the
        // sole unlinker, so skipping it would leave the index handing panels
        // despawned entities via `simulator_for`. The document itself stays.
        if let Some(reg) = registry.as_mut() {
            reg.unlink_entity(entity);
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

/// Link any freshly-spawned `ModelicaModel` into the
/// [`ModelicaDocumentRegistry`] doc→entity map. The mirror image of
/// [`cleanup_removed_simulators`]: removal is centralised via
/// `RemovedComponents`, so addition is too. The Interactive Live row,
/// [`crate::state::simulator_for`], and every doc-scoped panel resolve their
/// entity through this map, so a spawn path that forgets the explicit
/// `registry.link()` would silently drop the live sim from those surfaces —
/// this closes that gap structurally. Idempotent with the explicit links the
/// compile / lunica spawn sites still perform (re-linking the same
/// entity→doc pair is a no-op).
fn link_added_simulators(
    added: Query<(Entity, &ModelicaModel), Added<ModelicaModel>>,
    registry: Option<ResMut<ModelicaDocumentRegistry>>,
) {
    let Some(mut registry) = registry else { return };
    for (entity, model) in &added {
        // `DocumentId::default()` (0) means "no document assigned yet"
        // (reflect-deserialised / placeholder entities); a later assignment
        // plus its own explicit link covers those. Only link once the host
        // exists, so a transient pre-allocation spawn can't trip `link`'s
        // unknown-id debug assertion.
        if model.document != lunco_doc::DocumentId::default()
            && registry.host(model.document).is_some()
        {
            registry.link(entity, model.document);
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
    fn default() -> Self {
        Self { seed_welcome: true }
    }
}

impl Perspective for AnalyzePerspective {
    fn id(&self) -> PerspectiveId {
        PerspectiveId("modelica_analyze")
    }
    fn title(&self) -> String {
        // Keep the Modelica workbench recognisable beside the simulator's
        // `◉ View` and `⚒ Build` perspectives. The equation-wave glyph is
        // part of the authored title contract; an unadorned label makes the
        // third perspective look like a missing-icon button.
        "∿ Lunica".into()
    }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        // Side dock = Twin Browser only. Modelica contributes its library
        // section to that browser, so there is one authoritative browse
        // surface for workspace classes and standard libraries.
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
        // Start with the browse-and-open task only. Context panels and output
        // docks are opened by the operation that has content for them, or from
        // View; empty telemetry, inspector, plots, and log tabs should not take
        // half of a fresh workbench.
        layout.set_right_inspector(None);
        layout.set_bottom(None);
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
        // not just in the 3D `lunco-luncosim` binary that originally added it.
        if !app.is_plugin_added::<lunco_ui::LuncoUiPlugin>() {
            app.add_plugins(lunco_ui::LuncoUiPlugin);
        }

        // Twin-level change journal subscribes to the generic document
        // lifecycle events this plugin fires. The journal is now CORE substrate
        // (added by `SandboxCorePlugin` so the headless server + clients journal
        // too), but a standalone Modelica workbench that never adds the sandbox
        // core still needs it — so add it here only if absent. Guarded because
        // `TwinJournalPlugin` registers lifecycle observers: a double-add would
        // record every open/save/close twice.
        if !app.is_plugin_added::<lunco_doc_bevy::TwinJournalPlugin>() {
            app.add_plugins(lunco_doc_bevy::TwinJournalPlugin);
        }

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

        // The tutorial launcher is the SHARED `lunco-tutorial` engine (🎓 menu +
        // panel + `StartTutorial` + onboarding + F1). Lunica just registers its
        // rhai lessons (`assets/tutorials/lunica/*.rhai`) into it; the launcher
        // runs each as a scenario on a host entity. Apps that embed the Modelica
        // workbench as a *secondary* workspace (sandbox's Design tab) pre-insert
        // `ModelicaUiConfig { include_help_overlay: false, .. }` to suppress it.
        // Tutorials compose from assets/tutorials/lunica.usda (data, not code).
        #[cfg(feature = "scripting")]
        if config.include_help_overlay {
            app.add_plugins(lunco_tutorial::TutorialPlugin {
                app: "lunica".into(),
            });
        }

        // Edit events (`ModelicaApiEditPlugin`), the doc registry, the journal
        // wire, and `drain_document_changes` now live in `ModelicaCorePlugin`
        // (build_modelica_core) so a headless server journals Modelica edits
        // too. UI adds core first, so they're already present here.

        app.init_resource::<WorkbenchState>()
            .init_resource::<ModelicaDocumentRegistry>()
            .init_resource::<DocumentDiagnostics>()
            .init_resource::<crate::model_tabs::ModelTabs>()
            .init_resource::<crate::sim_default::RunTargetOverrides>()
            .init_resource::<crate::model_tabs_types::TabRenderContext>()
            .init_resource::<panels::code_editor::EditorBufferState>()
            .add_observer(panels::code_editor::on_editor_settings_changed)
            .add_observer(panels::code_editor::on_code_editor_menu_action)
            .add_observer(panels::code_editor::on_editor_buffer_changed)
            .add_observer(panels::code_editor::on_commit_editor_buffer_requested)
            .add_observer(panels::code_editor::on_ensure_editor_buffer_state)
            .add_observer(panels::canvas_diagram::on_apply_ops_requested)
            .add_observer(panels::canvas_diagram::on_drill_into_class_requested)
            .add_observer(crate::ui::commands::sim::on_set_model_input_requested)
            .add_observer(panels::palette::on_clear_component_drag_payload)
            .add_observer(panels::palette::on_place_component_requested)
            .add_observer(panels::console::on_clear_console_requested)
            .add_observer(panels::diagnostics::on_clear_diagnostics_requested)
            .add_observer(panels::diagnostics::on_diagnostic_jump_requested)
            .add_observer(panels::package_browser::on_open_package_class_requested)
            .add_observer(panels::experiments::on_load_experiment_requested)
            .add_observer(panels::experiments::on_export_experiment_requested)
            .add_observer(panels::experiments::on_rerun_experiment_requested)
            .add_observer(panels::experiments::on_set_experiment_run_target_requested)
            .add_observer(panels::inspector::on_plot_binding_requested)
            .add_observer(panels::inspector::on_plot_title_requested)
            .add_observer(panels::inspector::on_diagram_text_requested)
            .add_observer(panels::graphs::on_export_graph_requested)
            .add_observer(panels::model_view::on_sync_model_tab_requested)
            .add_observer(panels::model_view::on_fast_run_setup_requested)
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
            .init_resource::<crate::canvas_feedback::PendingApiFocusQueue>()
            .init_resource::<crate::canvas_feedback::PendingApiConnectionQueue>()
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
            // UI→core: workbench Untitled-draft rename → RenameModelicaClass.
            .add_observer(rename_chain::on_rename_open_document_chain_to_modelica)
            .init_resource::<panels::canvas_projection::DiagramAutoLayoutSettings>()
            .init_resource::<panels::palette::PaletteState>()
            .init_resource::<panels::palette::ComponentDragPayload>()
            // WP-8 / CQ-208: the Telemetry panel's active-class parameter
            // list is a pure view over `TelemetryViewModel`, re-flattened
            // only when the active doc / class / index generation changes.
            .init_resource::<panels::telemetry::TelemetryViewModel>()
            .add_systems(Update, panels::telemetry::populate_telemetry_view_model)
            // WP-8 / CQ-207: the experiments plot reads shared `Arc` sample
            // arrays + the variable catalog from `ExperimentsViewModel`,
            // rebuilt only when the twin's run set / sample totals change.
            .init_resource::<panels::experiments::ExperimentsViewModel>()
            .add_systems(Update, panels::experiments::populate_experiments_view_model)
            .insert_resource(crate::package_tree::PackageTreeCache::new())
            .add_systems(Update, browser_dispatch::drain_browser_actions)
            .add_systems(Update, panels::package_browser::handle_package_loading_tasks)
            .add_systems(Update, panels::package_browser::reconcile_library_roots_on_ready)
            // Reactive: fires once, the frame MSL enters the engine session —
            // re-projects open canvas tabs (so std-lib icons resolve), rebuilds
            // the bundled-examples tree, reconciles library roots. Previously
            // lived in the never-added `PackageBrowserPlugin`; wired here so it
            // actually runs (the bug behind "Modelica files not updated after
            // MSL loaded": restored/auto-opened tabs projected empty pre-MSL
            // and never recovered).
            .add_observer(panels::package_browser::on_msl_became_ready)
            .add_observer(panels::package_browser::on_modelica_library_became_ready)
            .add_systems(Update, cleanup_removed_simulators)
            .add_systems(Update, link_added_simulators)
            // `drain_document_changes` + the A3 journal-wire auto-bridge moved
            // to `ModelicaCorePlugin` (so headless journals too).
            .add_systems(Update, commands::drain_open_file_results)
            // Coarse cache invalidation: any doc edit can shift
            // cross-file inheritance chains, so every source-derived
            // memo flushes wholesale. Re-fills lazily on next paint
            // via rumoca's content-hash cache — unchanged classes
            // return the same icon instantly.
            .add_observer(invalidate_source_memos_on_doc_changed)
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
            .init_resource::<DocTitleGenCache>()
            .add_systems(Update, derive_doc_title)
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
            .register_panel(lunco_workbench::TwinBrowserPanel)
            .register_panel(lunco_workbench::FilesPanel)
            .insert_resource(panels::welcome::LearningPathRegistry::with_builtins())
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
                    title: "Lunica",
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
                    // Only offer the tour where the tutorial launcher runs: the
                    // shared `lunco-tutorial` engine consumes this perspective's
                    // `HelpTourRequest` → the onboarding tutorial. Gated on the
                    // same flag; embedded-as-secondary hosts (sandbox's Design
                    // tab) set it false → no dead button.
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
            .register(browser_section::ModelicaSection);
        app.world_mut()
            .resource_mut::<lunco_workbench::BrowserSectionRegistry>()
            .register(twin_datasets::TwinDatasetsSection);
    }
}

/// Push Modelica editor preferences onto the application-wide
/// Settings menu. Lives in the workbench Settings dropdown rather
/// than a per-panel gear button — keeps editor toolbar tidy and
/// all prefs discoverable in one place.
fn register_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, ctx| {
        ui.label(egui::RichText::new("Code Editor").weak().small());
        let Some((original_word_wrap, original_auto_indent)) = ctx
            .resource::<panels::code_editor::EditorBufferState>()
            .map(|buf| (buf.word_wrap, buf.auto_indent))
        else {
            return;
        };
        let mut word_wrap = original_word_wrap;
        let mut auto_indent = original_auto_indent;
        ui.checkbox(&mut word_wrap, "Word wrap")
            .on_hover_text("Wrap long lines at editor width");
        ui.checkbox(&mut auto_indent, "Auto indent")
            .on_hover_text("Copy previous line's indent on Enter");
        if word_wrap != original_word_wrap || auto_indent != original_auto_indent {
            ctx.trigger(panels::code_editor::EditorSettingsChanged {
                word_wrap,
                auto_indent,
            });
        }
        ui.separator();
        ui.label(egui::RichText::new("Component Palette").weak().small());
        let Some(mut palette) = ctx
            .resource::<panels::canvas_diagram::PaletteSettings>()
            .cloned()
        else {
            return;
        };
        let original_palette = palette.clone();
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
        if palette != original_palette {
            ctx.set_resource(palette);
        }
        ui.separator();
        ui.label(egui::RichText::new("Diagram").weak().small());
        let Some(mut limits) = ctx
            .resource::<panels::canvas_diagram::DiagramProjectionLimits>()
            .cloned()
        else {
            return;
        };
        let original_limits = limits.clone();
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
        if limits != original_limits {
            ctx.set_resource(limits);
        }
        ui.add_space(4.0);
        // ── Drag snap ────────────────────────────────────────────
        // Off by default — a lot of Modelica source uses
        // hand-placed non-grid positions and the user shouldn't
        // have their authored placements auto-rounded unless they
        // opted in. When on, drags quantise *live* (visible during
        // the drag itself) to multiples of `step` Modelica units.
        let Some((original_snap_enabled, original_snap_step)) = ctx
            .resource::<panels::canvas_diagram::CanvasSnapSettings>()
            .map(|snap| (snap.enabled, snap.step))
        else {
            return;
        };
        let mut snap_enabled = original_snap_enabled;
        let mut snap_step = original_snap_step;
        ui.checkbox(&mut snap_enabled, "Snap to grid on drag")
            .on_hover_text(
                "When on, dragging an icon quantises its position to a \
             grid. Applies live during the drag and at commit. Off \
             by default.",
            );
        ui.horizontal(|ui| {
            ui.label("Grid step");
            ui.add_enabled(
                snap_enabled,
                egui::DragValue::new(&mut snap_step)
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
        if snap_enabled != original_snap_enabled || snap_step != original_snap_step {
            ctx.set_resource(panels::canvas_diagram::CanvasSnapSettings {
                enabled: snap_enabled,
                step: snap_step,
            });
        }
    });
    layout.register_settings_submenu("Modelica", render_assets_settings);
}

/// Settings rows for the Modelica section — MSL readiness and local override.
/// Native dataset download actions live in the generic Data & libraries panel.
fn render_assets_settings(ui: &mut bevy_egui::egui::Ui, ctx: &mut MenuCtx) {
    use bevy_egui::egui;
    use lunco_assets::msl::{MslLoadPhase, MslLoadState};

    // Current state line.
    let state = ctx.resource::<MslLoadState>().cloned();

    // If the Modelica UI is active, the MslSettings resource MUST exist
    // by architectural design (ModelicaPlugin adds ModelicaCorePlugin adds MslRemotePlugin).
    let Some(mut settings) = ctx.resource::<crate::msl_settings::MslSettings>().cloned() else {
        return;
    };
    let original_settings = settings.clone();

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
            let phase_label = match phase {
                MslLoadPhase::FetchingManifest => "fetching manifest",
                MslLoadPhase::FetchingBundle => "downloading",
                MslLoadPhase::LoadingCache => "loading from cache",
                MslLoadPhase::Decompressing => "extracting",
                MslLoadPhase::Parsing => "loading",
            };
            // The `Parsing` phase carries file counts in `bytes_done`/`bytes_total`;
            // every other phase carries bytes. Rendering the count as MB showed a
            // frozen "0.0 / 0.0 MB", so branch on the phase.
            if *bytes_total == 0 {
                ui.label(format!("Status: {phase_label}"));
            } else if matches!(phase, MslLoadPhase::Parsing) {
                ui.label(format!(
                    "Status: {phase_label} · {bytes_done} / {bytes_total} files",
                ));
            } else {
                ui.label(format!(
                    "Status: {phase_label} · {:.1} / {:.1} MB",
                    *bytes_done as f64 / 1_048_576.0,
                    *bytes_total as f64 / 1_048_576.0,
                ));
            }
        }
        Some(MslLoadState::Failed(msg)) => {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Status: failed — {msg}"));
        }
        Some(MslLoadState::NotStarted) | None => {
            ui.label("Status: not started");
        }
    }

    // Resolved on-disk path. May be the explicit-install destination, the
    // workspace `.cache/msl/`, or a user-supplied override.
    let root = lunco_assets::msl_source_root_path();
    match root.as_ref() {
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

    #[cfg(not(target_arch = "wasm32"))]
    if root.is_some() && matches!(state, Some(MslLoadState::Failed(_))) {
        if ui
            .button("Rebuild editor index")
            .on_hover_text(
                "Re-scan the installed Modelica source and rebuild its editor index. \
                 This does not download anything.",
            )
            .clicked()
        {
            ctx.trigger(crate::msl_remote::NativeMslIndexAction::Rebuild);
        }
    }

    // Local-root override — wins over an explicit download. Restart needed
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
                 subdirectory. Takes precedence over a downloaded copy. \
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
    if settings != original_settings {
        ctx.set_resource(settings);
    }

    // Native downloads are dispatched by the generic dataset panel so MSL,
    // terrain and Twin resources share one lifecycle and retry UX.
    #[cfg(not(target_arch = "wasm32"))]
    ui.label("Download MSL from Settings ▸ Data & libraries.");

    #[cfg(target_arch = "wasm32")]
    {
        // Web MSL is a host-served bundle rather than a native dataset, so its
        // platform-specific fetch controls remain here.
        let load_state = ctx.resource::<lunco_assets::msl::MslLoadState>().cloned();
        let install_running = matches!(
            load_state,
            Some(lunco_assets::msl::MslLoadState::Loading { .. })
        );
        let install_failed = matches!(load_state, Some(lunco_assets::msl::MslLoadState::Failed(_)));
        let install_ready = matches!(
            load_state,
            Some(lunco_assets::msl::MslLoadState::Ready { .. })
        );
        ui.horizontal(|ui| {
            // While an install is in flight, show Cancel. Before the first
            // install, show Install. Once finished, show Reinstall/Retry.
            if install_running {
                ui.label("MSL bundle loading…");
            } else if matches!(
                load_state,
                Some(lunco_assets::msl::MslLoadState::NotStarted) | None
            ) {
                if ui
                    .button("Install MSL")
                    .on_hover_text(
                        "Download and index the Modelica Standard Library. \
                     Nothing is downloaded until you click this button.",
                    )
                    .clicked()
                {
                    ctx.trigger(crate::msl_remote::MslInstallAction::Install);
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
                    ctx.trigger(crate::msl_remote::MslInstallAction::Reinstall);
                }
            } else if install_ready
                && ui
                    .button("Reinstall")
                    .on_hover_text(
                        "Force-redownload MSL and rebuild the bincode cache. \
                 Wipes the current cache directory first.",
                    )
                    .clicked()
            {
                ctx.trigger(crate::msl_remote::MslInstallAction::Reinstall);
            }
        });
    }
}

/// Contribute Cut/Copy/Paste/Select-All entries to the workbench's
/// global Edit menu. The entries flip flags on
/// [`panels::code_editor::CodeEditorMenuRequest`]; the code-editor
/// render reads & clears them next frame, OR-merging into the same
/// flags the in-panel toolbar uses. Keeps clipboard/selection
/// handling in one place while letting the menu drive it.
fn register_edit_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    // Undo/redo availability for the workbench's global Edit menu —
    // answers only for documents this registry owns (same ownership
    // check as `resolve_editor_intent`), so Undo greys out with a
    // "Nothing to undo" hint instead of firing a no-op intent.
    layout.register_undo_probe(|ctx: &UndoProbeCtx| {
        let doc = ctx
            .resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|workspace| workspace.0.active_document)?;
        let host = ctx
            .resource::<crate::state::ModelicaDocumentRegistry>()?
            .host(doc)?;
        Some((host.can_undo(), host.can_redo()))
    });
    layout.register_edit_menu(|ui, ctx| {
        // TODO: promote Cut / Copy / Paste / Select All to typed
        // `#[Command]` events so the HTTP API can drive them too
        // (mirrors the existing `Undo` / `Redo` commands in
        // `ui/commands.rs`). Today they only flow through the menu /
        // toolbar / keyboard since the operation is scoped to the
        // currently-focused egui TextEdit, which has no
        // representation on the API side — a typed command would
        // need an explicit `doc` + range/text payload.
        if ui.button("Cut\tCtrl+X").clicked() {
            ctx.trigger(panels::code_editor::CodeEditorMenuAction::Cut);
            ui.close();
        }
        if ui.button("Copy\tCtrl+C").clicked() {
            ctx.trigger(panels::code_editor::CodeEditorMenuAction::Copy);
            ui.close();
        }
        if ui.button("Paste\tCtrl+V").clicked() {
            ctx.trigger(panels::code_editor::CodeEditorMenuAction::Paste);
            ui.close();
        }
        ui.separator();
        if ui.button("Select All\tCtrl+A").clicked() {
            ctx.trigger(panels::code_editor::CodeEditorMenuAction::SelectAll);
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
    bevy::log::info!("[ModelicaImageLoader] installed egui_extras loaders + modelica:// loader");

    commands.insert_resource(ImageLoadersInstalled);
}

/// Forward newly-pushed [`lunco_workbench::status_bus::StatusBus`]
/// events to the [`panels::console::ConsoleLog`].
///
/// We track how many *discrete* history entries we've already mirrored
/// so progress ticks (which mutate the bus seq but don't append to
/// history) don't show up as console spam. We diff against the bus's
/// monotonic [`StatusBus::history_total`] watermark — NOT
/// `history().count()`, which plateaus at the ring's capacity and would
/// make this early-return forever once the buffer filled, silently
/// freezing the console audit trail (CQ-523).
fn fan_status_bus_to_console(
    bus: bevy::prelude::Res<lunco_workbench::status_bus::StatusBus>,
    mut console: bevy::prelude::ResMut<panels::console::ConsoleLog>,
    mut last_total: bevy::prelude::Local<u64>,
) {
    let total = bus.history_total();
    if total == *last_total {
        return;
    }
    // Forward only what's new since we last looked. If more entries
    // arrived than the ring can hold (a burst between frames), `delta`
    // exceeds the live length — clamp to what we still have so we take
    // exactly the retained tail.
    let delta = total.saturating_sub(*last_total) as usize;
    let take = delta.min(bus.history().count());
    for ev in bus
        .history()
        .rev()
        .take(take)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let level = match ev.level {
            lunco_workbench::status_bus::StatusLevel::Info => panels::console::ConsoleLevel::Info,
            lunco_workbench::status_bus::StatusLevel::Warn => panels::console::ConsoleLevel::Warn,
            lunco_workbench::status_bus::StatusLevel::Error => panels::console::ConsoleLevel::Error,
            lunco_workbench::status_bus::StatusLevel::Attention => {
                panels::console::ConsoleLevel::Info
            }
            // Progress events shouldn't be in `history` (they live in
            // active_progress), but if one ever sneaks in, surface as Info.
            lunco_workbench::status_bus::StatusLevel::Progress => {
                panels::console::ConsoleLevel::Info
            }
        };
        console.push(level, format!("[{}] {}", ev.source, ev.message));
    }
    *last_total = total;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_perspective_publishes_its_tab_icon() {
        assert_eq!(AnalyzePerspective::default().title(), "∿ Lunica");
    }

    /// THE DATA-LOSS REGRESSION. A despawn must not destroy the document.
    ///
    /// `clear_scene_entities` despawns every `WorldGrid` child on any scene
    /// clear/reload, and USD-sourced cosim models are `WorldGrid` children.
    /// This system used to resolve `entity → doc` and call `remove_document`,
    /// so loading a second scene — or a rhai script firing `ClearScene` —
    /// silently destroyed the user's open Modelica document, unsaved edits
    /// and undo stack included, with no prompt.
    ///
    /// The unit-level invariant was never in doubt: `unlink_entity` already
    /// kept the document, and a test already asserted it. The defect was that
    /// this system did more than unlink. So the test has to drive the SYSTEM,
    /// through a real despawn — asserting on the registry API alone passes
    /// while the app destroys your work.
    #[test]
    fn despawning_a_simulator_keeps_its_document() {
        let mut app = App::new();
        app.init_resource::<ModelicaDocumentRegistry>();
        app.add_systems(Update, cleanup_removed_simulators);

        let entity = app.world_mut().spawn(ModelicaModel::default()).id();
        let doc = {
            let mut reg = app.world_mut().resource_mut::<ModelicaDocumentRegistry>();
            let doc = reg.allocate("model M end M;".into());
            reg.link(entity, doc);
            doc
        };

        // A scene clear/reload despawns the entity out from under the document.
        app.world_mut().entity_mut(entity).despawn();
        app.update();

        let reg = app.world().resource::<ModelicaDocumentRegistry>();
        assert!(
            reg.host(doc).is_some(),
            "a scene reload despawned the entity and took the user's document with it"
        );
        assert_eq!(
            reg.document_of(entity),
            None,
            "the dead entity must still be unlinked — this system is the only unlinker, \
             so leaking it would hand panels a despawned entity via `simulator_for`"
        );
    }
}
