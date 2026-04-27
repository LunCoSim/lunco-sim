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
//!     (egui-snarl)      (text editor)      (params/inputs)
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
//! - **Diagram** (center tab) — component block diagram via egui-snarl
//! - **Telemetry** (right dock) — parameters, inputs, variable toggles
//! - **Graphs** (bottom dock) — time-series plots of simulation variables

use bevy::prelude::*;
use lunco_workbench::{Perspective, PerspectiveId, WorkbenchAppExt, WorkbenchLayout, PanelId};

pub mod state;
pub use state::*;

pub mod commands;
pub use commands::{CompileModel, CreateNewScratchModel, ModelicaCommandsPlugin};

pub mod image_loader;
pub mod panels;
pub mod viz;
pub mod theme;
pub mod uri_handler;
pub mod welcome_progress;
/// Debounced AST reparse driver — see module docs.
pub mod ast_refresh;
pub mod input_activity;

/// Modelica section of the Twin Browser — class-tree contributed by
/// this crate to `lunco-workbench`'s `BrowserSectionRegistry`.
pub mod browser_section;

/// Drains the workbench's `BrowserActions` outbox and routes
/// section-emitted intents (open file, open Modelica class) into the
/// existing document-load and drill-in pipelines.
pub mod browser_dispatch;

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
/// / `ModelTabs` / `WorkbenchState.open_model` triad, this observer
/// becomes the sole population point for the Workspace's document list.
fn sync_workspace_on_doc_opened(
    trigger: On<lunco_doc_bevy::DocumentOpened>,
    registry: Res<ModelicaDocumentRegistry>,
    mut ws: ResMut<lunco_workbench::WorkspaceResource>,
) {
    let id = trigger.event().doc;
    // Dedupe — `DocumentOpened` can fire multiple times per id during
    // the race between allocate/install_prebuilt and later reconcile
    // passes. Treat a second Opened as a no-op so the Workspace
    // document list stays a set, not a multiset.
    if ws.document(id).is_some() {
        return;
    }
    let Some(host) = registry.host(id) else {
        return;
    };
    let doc = host.document();
    let origin = doc.origin().clone();
    let title = match &origin {
        lunco_doc::DocumentOrigin::Untitled { name } => name.clone(),
        lunco_doc::DocumentOrigin::File { path, .. } => path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("(file)")
            .to_string(),
    };
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
    mut ws: ResMut<lunco_workbench::WorkspaceResource>,
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
    mut ws: ResMut<lunco_workbench::WorkspaceResource>,
) {
    let id = trigger.event().doc;
    let Some(host) = registry.host(id) else { return };
    let doc = host.document();
    let new_origin = doc.origin().clone();
    let new_title = match &new_origin {
        lunco_doc::DocumentOrigin::Untitled { name } => name.clone(),
        lunco_doc::DocumentOrigin::File { path, .. } => path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("(file)")
            .to_string(),
    };
    if let Some(entry) = ws.document_mut(id) {
        entry.origin = new_origin;
        entry.title = new_title;
    }
}

/// Drop the document linked to a despawned `ModelicaModel` entity, and
/// any compile-state bookkeeping attached to that document.
///
/// Behavior preserved from the entity-keyed era: when an entity is
/// despawned, its backing [`ModelicaDocument`](crate::document::ModelicaDocument)
/// is also removed. The long-term design lets documents outlive entities
/// (edit-without-running, cosim re-spawn), so this will become opt-in
/// once the tab/view layer can explicitly unload a document.
fn cleanup_removed_documents(
    mut removed: RemovedComponents<ModelicaModel>,
    registry: Option<ResMut<ModelicaDocumentRegistry>>,
    compile_states: Option<ResMut<CompileStates>>,
    canvas_state: Option<ResMut<panels::canvas_diagram::CanvasDiagramState>>,
    class_names: Option<ResMut<panels::canvas_diagram::DrilledInClassNames>>,
    signals: Option<ResMut<lunco_viz::SignalRegistry>>,
    viz_registry: Option<ResMut<lunco_viz::VisualizationRegistry>>,
) {
    let Some(mut registry) = registry else { return };
    let mut compile_states = compile_states;
    let mut canvas_state = canvas_state;
    let mut class_names = class_names;
    let mut signals = signals;
    let mut viz_registry = viz_registry;
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
            if let Some(names) = class_names.as_mut() {
                names.remove(doc);
            }
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
/// design doc ([`docs/architecture/11-workbench.md`] § 4).
pub struct AnalyzePerspective;

impl Perspective for AnalyzePerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("modelica_analyze") }
    fn title(&self) -> String { "📊 Analyze".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        // Side dock = Twin Browser only. The legacy
        // `PackageBrowserPanel` stays registered (View → Panels can
        // re-dock it) but is not docked by default — its remaining
        // unique features (MSL palette, drag-to-instantiate) will
        // migrate into the Twin Browser as a future `MslSection`.
        // Side-by-side dock would just present users with two
        // browsers solving the same job.
        layout.set_side_browser(Some(lunco_workbench::TWIN_BROWSER_PANEL_ID));
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
        layout.set_center(vec![PanelId("modelica_welcome")]);
        layout.set_active_center_tab(0);
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
        layout.set_bottom_tabs(vec![
            PanelId("modelica_graphs"),
            PanelId("modelica_diagnostics"),
            PanelId("modelica_console"),
        ]);
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
        // Twin-level change journal subscribes to the generic document
        // lifecycle events this plugin fires. One journal per App —
        // adding the plugin multiple times is a no-op on `init_resource`.
        app.add_plugins(lunco_doc_bevy::TwinJournalPlugin);

        // Shared Modelica class cache — drill-in, preload, and
        // (later) compile dep-walk all funnel through this one
        // Arc-shared store so every .mo file is read once and
        // parsed once per session.
        app.add_plugins(crate::class_cache::ClassCachePlugin);

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
        app.add_plugins(crate::api_edits::ModelicaApiEditPlugin);

        app.init_resource::<WorkbenchState>()
            .init_resource::<ModelicaDocumentRegistry>()
            .init_resource::<CompileStates>()
            .init_resource::<panels::model_view::ModelTabs>()
            .init_resource::<panels::code_editor::EditorBufferState>()
            .init_resource::<panels::console::ConsoleLog>()
            .init_resource::<panels::diagnostics::DiagnosticsLog>()
            // Forward StatusBus events to the Console panel so the
            // user has a chronological audit trail of every status
            // event from every subsystem (MSL, compile, sim, …).
            .add_systems(Update, fan_status_bus_to_console)
            .init_resource::<panels::canvas_projection::DiagramAutoLayoutSettings>()
            .init_resource::<panels::palette::PaletteState>()
            .insert_resource(panels::package_browser::PackageTreeCache::new())
            .init_resource::<browser_dispatch::PendingDrillIns>()
            .add_systems(Update, browser_dispatch::drain_browser_actions)
            .add_systems(Update, panels::package_browser::handle_package_loading_tasks)
            .add_systems(Update, cleanup_removed_documents)
            .add_systems(Update, drain_document_changes)
            // Workspace shadow-sync: keep `WorkspaceResource` populated
            // from the existing document-registry lifecycle so the new
            // session surface is ready for step 5b.2 readers.
            .add_observer(sync_workspace_on_doc_opened)
            .add_observer(sync_workspace_on_doc_closed)
            .add_observer(sync_workspace_on_doc_saved)
            .add_systems(Update, panels::diagnostics::refresh_diagnostics)
            // Debounced AST reparse — reparses any doc that has
            // stopped receiving keystrokes for AST_DEBOUNCE_MS (250 ms).
            // Keeps text-edit latency constant regardless of how busy
            // the sim worker is.
            .init_resource::<ast_refresh::PendingAstParses>()
            .init_resource::<input_activity::InputActivity>()
            .add_systems(bevy::prelude::PreUpdate, input_activity::stamp_user_input)
            .add_systems(Update, ast_refresh::refresh_stale_asts)
            .add_systems(Startup, register_settings_menu)
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
            .register_panel(panels::welcome::WelcomePanel)
            .register_panel(panels::telemetry::TelemetryPanel)
            .register_panel(panels::graphs::GraphsPanel)
            .register_panel(panels::console::ConsolePanel)
            .register_panel(panels::diagnostics::DiagnosticsPanel)
            .register_panel(panels::canvas_diagram::CanvasDiagramPanel)
            .init_resource::<panels::canvas_diagram::CanvasDiagramState>()
            .init_resource::<panels::canvas_diagram::PaletteSettings>()
            .init_resource::<panels::canvas_diagram::DiagramProjectionLimits>()
            .init_resource::<panels::canvas_diagram::DrilledInClassNames>()
            .init_resource::<panels::canvas_diagram::DrillInLoads>()
            .init_resource::<panels::canvas_diagram::CanvasSnapSettings>()
            .init_resource::<panels::canvas_diagram::DuplicateLoads>()
            .add_systems(Update, panels::canvas_diagram::drive_drill_in_loads)
            .add_systems(Update, panels::canvas_diagram::drive_duplicate_loads)
            .add_systems(bevy_egui::EguiPrimaryContextPass, alpha_banner)
            .register_panel(panels::inspector::InspectorPanel)
            .register_panel(panels::palette::ComponentPalettePanel)
            // Multi-instance: one tab per open document. Instances are
            // opened at runtime by the Package Browser.
            .register_instance_panel(panels::model_view::ModelViewPanel::default())
            .register_perspective(AnalyzePerspective);

        // Contribute the Modelica section to the Twin Browser's
        // section registry. The workbench's WorkbenchPlugin already
        // installed the registry resource and the built-in Files
        // section; we just append. ensure it exists first to avoid
        // panics during mixed-mode or deferred plugin builds.
        app.init_resource::<lunco_workbench::BrowserSectionRegistry>();
        app.world_mut()
            .resource_mut::<lunco_workbench::BrowserSectionRegistry>()
            .register(browser_section::ModelicaSection::default());
    }
}

/// Alpha-quality warning strip pinned to the top of the workbench.
/// Many MSL examples still fail to compile/simulate — surface that up
/// front so users don't assume a broken model is their fault.
fn alpha_banner(mut contexts: bevy_egui::EguiContexts) {
    use bevy_egui::egui;
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::TopBottomPanel::top("modelica_alpha_banner")
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(120, 60, 0))
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("⚠ Alpha")
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(
                        "— Modelica workbench is experimental. \
                         Many MSL examples do not yet compile or simulate; \
                         expect rough edges and missing features.",
                    )
                    .color(egui::Color32::WHITE),
                );
            });
        });
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
    ctx.add_bytes_loader(loader.clone());
    bevy::log::info!(
        "[ModelicaImageLoader] installed egui_extras loaders + modelica:// loader"
    );

    // Pre-warm the canvas's SVG-bytes cache for every MSL component
    // the right-click Add menu can spawn. The optimistic-synth path
    // drops a node into the scene immediately on Add; the canvas's
    // node visual then calls `svg_bytes_for(icon_asset)` which does
    // a synchronous `std::fs::read` on the render thread for any
    // cold-cache icon (5-50ms each, multiplied across icons painted
    // for the first time). Pre-warming reads every MSL palette icon
    // off the main thread before the user ever Adds — by the time
    // they do, the cache hit is constant-time and the icon paints in
    // the same frame as the optimistic synth.
    let asset_paths: Vec<String> = crate::visual_diagram::msl_component_library()
        .iter()
        .filter_map(|comp| comp.icon_asset.clone())
        .filter(|s| !s.is_empty())
        .collect();
    bevy::log::info!(
        "[svg_bytes] queueing prewarm for {} canvas-icon assets",
        asset_paths.len()
    );
    crate::ui::panels::canvas_diagram::prewarm_svg_bytes(asset_paths);

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
