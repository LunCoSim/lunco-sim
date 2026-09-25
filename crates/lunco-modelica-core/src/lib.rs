//! High-performance Modelica integration for Bevy.
//!
//! This crate provides a bridge between Bevy's ECS and Modelica simulation models.
//! It features:
//! - document lifecycle and engine synchronization
//! - Document, AST, diagram, and engine synchronization primitives
//! - UI-agnostic Modelica runtime state and command contracts
//!
//! ## Architecture
//!
//! The [`ModelicaCorePlugin`] owns the document/runtime half of the Modelica
//! integration.
//! Solver workers, experiment execution, and browser transport are supplied by
//! `lunco_modelica_execution::ModelicaExecutionPlugin`. Keeping that package
//! above this one means compiler-only consumers do not rebuild the simulation
//! stack when worker code changes.
//!
//! Source parsing, lossless AST edits, source-fragment rendering, and the
//! reusable diagram graph are provided by the Modelica AST/index packages.
//! The compiler session and source admission host live in
//! `lunco-modelica-compiler`; this crate owns the document lifecycle and
//! Modelica-specific runtime integration around those source contracts.
//! Solver orchestration, simulation-result caching, and worker panic isolation
//! live in `lunco-modelica-execution`. Headless document and source-editing
//! mechanics live in `lunco-modelica-document`, so compiler consumers do not
//! compile this crate's document implementation merely to use that lower-level
//! contract. The execution-side Modelica telemetry projection is isolated in
//! `lunco-modelica-telemetry`.
use bevy::prelude::*;
#[cfg(feature = "api")]
use lunco_api::executor::DeferredCommandAppExt;
use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::ModelicaDocument;

/// SemVer2 product version stamped into this build.
pub const PRODUCT_VERSION: &str = env!("LUNCO_RELEASE_VERSION");
/// Short source revision stamped into this build for diagnostics.
pub const GIT_SHA: &str = env!("LUNCO_GIT_SHA");
/// Public GitHub repository containing the stamped source revision.
pub const REPOSITORY_URL: &str = env!("LUNCO_REPOSITORY_URL");

/// Typed identity for a Modelica class across the workbench.
///
/// Replaces the former string ID schemes (`library_path:`, `bundled://…#`,
/// raw file paths, `mem://`) with a single `ClassRef { library, path }`
/// value that flows through opening, drill-in, tab dedup, projection
/// target lookup, and documentation lookup. See module docs for the
/// migration map.
pub mod class_ref;

/// Unified read-side metadata for Modelica classes — folds the
/// pre-baked palette index and
/// the live per-document [`index::ClassEntry`] into one
/// [`class_metadata::ClassMetadata`] shape so docs view, badges,
/// and inspector title all read through one path.
pub mod class_metadata;

/// Shared parse + I/O cache for Modelica classes. Drill-in, AddComponent
/// preload, and compile dep-walk all funnel through here so every class file is
/// read once, parsed once, and shared as an `Arc` across tabs and compile jobs.
///
/// The load is **synchronous** (`peek_or_load_class_blocking`), under a
/// two-phase lock: probe → parse *outside* the lock → install. That is why
/// [`ClassLookupMode::Cached`] exists — off-thread callers must never block on a
/// cold parse, so they take the peek-only path and miss rather than stall.
pub mod class_cache;
pub mod library_documents;
pub mod library_fs;

/// Modelica-to-diagram graph builder — converts AST into DiagramGraph.
pub mod diagram;

// ── Shared headless domain modules. Presentation adapters live in
// `lunco-modelica-ui`; these modules are also used by API, worker, and scene
// hosts that do not load the workbench. ──────────────────────────────────────
/// Core data for API-driven canvas focus/connection pulses (UI drains them).
pub mod canvas_feedback;
/// Egui-free Modelica document ops application
/// (was `ui::panels::canvas_diagram::ops::apply_one_op_as` & helpers).
pub mod doc_ops;
/// Default-simulation-class resolution + run-target overrides
/// (was `ui::panels::model_view::context::default_simulation_class` & friends).
/// `ModelTabs` registry (was `ui::panels::model_view::tabs`).
pub mod model_tabs;
/// Modelica tab registry data types (was `ui::panels::model_view::types`).
pub mod model_tabs_types;
/// Package-tree backend: egui-free scanning and cache logic for the library /
/// package browser. Value types live in `lunco-modelica-index`.
pub mod package_tree;

pub mod sim_default;
/// Pure simulation-target & run-configuration resolution (which class to
/// run, what bounds to run it with). No `World`/UI deps — the `ui/` layer
/// gathers inputs and calls down. See [`sim_target`].
pub mod sim_target;

/// Core (UI-free) Modelica command helpers — `SetModelInput` application + sim-
/// bounds resolution — shared by the egui workbench and the headless API server.
pub mod model_commands;

/// Per-Twin Modelica domain engine: long-lived `rumoca_compile::Session`
/// + per-doc URI mapping. Provides cross-file inheritance-merged queries.
pub mod engine;
pub mod engine_resource;

/// Experiment-*definition* journaling (`DomainKind::Experiment`) — records
/// create/rename/bounds/params/delete into the canonical twin journal so
/// experiment setups sync + persist. Run results ride the content plane; run
/// status rides presence.
pub mod experiment_journal;
/// Minimal byte-range diff helper. Used by the code-editor commit path
/// to convert a debounced full-buffer snapshot into a single
/// `ModelicaOp::EditText` splice — finer undo granularity and
/// CRDT-friendly text edits.
pub mod text_diff;

/// Pre-warmer: walks each opened doc's AST collecting cross-package
/// type references, then primes the engine's icon cache via a single
/// off-thread task. Drill-in projection sees a populated cache.
pub mod icon_warmer;
/// Bundled Modelica models for web deployment.
/// Available on all targets, but primarily used for wasm builds.
pub mod models;

/// Shareable model links (encode model source into a URL fragment).
pub mod model_share;

/// Headless Modelica document and runtime plugin.
///
/// UI panels and editor state live in `lunco-modelica-ui` and are not part of
/// this package.
pub struct ModelicaCorePlugin;

impl Plugin for ModelicaCorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<lunco_modelica_library::worker_bridge::ModelicaWorkerBridge>();
        build_modelica_core(app);
        if !app.is_plugin_added::<lunco_modelica_source_roots::ModelicaSourceRootsPlugin>() {
            app.add_plugins(lunco_modelica_source_roots::ModelicaSourceRootsPlugin);
        }
        // Runtime model-input control is a core command, not a UI command.
        // Register it here so headless and workbench hosts expose the same
        // reflected command contract.
        model_commands::register_all_commands(app);
        #[cfg(feature = "api")]
        app.register_deferred_command::<model_commands::SetModelInput>();
        app.init_resource::<lunco_core_session::CommandPolicyRegistry>();
        app.world_mut()
            .resource_mut::<lunco_core_session::CommandPolicyRegistry>()
            .register(
                "SetModelInput",
                lunco_core_session::CommandPolicy {
                    min_role: lunco_core_session::AuthorityRole::Operator,
                    ownership_gated: false,
                },
            );
    }
}

/// Keep Modelica documents in the shared Workspace from every host mode.
/// Generated documents projected from USD must be discoverable by the same
/// session query as file-backed editor documents in headless and offscreen runs.
fn sync_workspace_on_doc_opened(
    trigger: On<lunco_doc_bevy::DocumentOpened>,
    registry: Res<DocumentRegistry<ModelicaDocument>>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let id = trigger.event().doc;
    let Some(host) = registry.host(id) else {
        return;
    };
    let document = host.document();
    let origin = document.origin().clone();
    if workspace.document(id).is_some() {
        return;
    }
    let context_twin = if origin.is_untitled() {
        workspace.active_twin
    } else {
        None
    };
    workspace.add_document(lunco_workspace::DocumentEntry {
        id,
        kind: lunco_workspace::DocumentKindId::new("modelica"),
        origin: origin.clone(),
        context_twin,
        title: origin.display_name(),
        dirty: document.is_dirty(),
    });
}

fn sync_workspace_on_doc_closed(
    trigger: On<lunco_doc_bevy::DocumentClosed>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    if let Some(mut workspace) = workspace {
        workspace.close_document(trigger.event().doc);
    }
}

fn sync_workspace_on_doc_changed(
    trigger: On<lunco_doc_bevy::DocumentChanged>,
    registry: Res<DocumentRegistry<ModelicaDocument>>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let id = trigger.event().doc;
    let Some(host) = registry.host(id) else {
        return;
    };
    if let Some(entry) = workspace.document_mut(id) {
        entry.dirty = host.document().is_dirty();
    }
}

fn sync_workspace_on_doc_saved(
    trigger: On<lunco_doc_bevy::DocumentSaved>,
    registry: Res<DocumentRegistry<ModelicaDocument>>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let id = trigger.event().doc;
    let Some(host) = registry.host(id) else {
        return;
    };
    let origin = host.document().origin().clone();
    if let Some(path) = origin.canonical_path() {
        workspace.recents.push_loose(path.to_path_buf());
    }
    if let Some(entry) = workspace.document_mut(id) {
        entry.title = origin.display_name();
        entry.origin = origin;
        entry.dirty = host.document().is_dirty();
    }
}
fn build_modelica_core(app: &mut App) {
    // Parsing and keeping each document's Rumoca session in sync are core
    // document/runtime services, not editor presentation. Headless hosts need
    // this too, especially for generated scratch models that receive edits
    // before their first syntax cache has been installed.
    if !app.is_plugin_added::<crate::engine_resource::ModelicaEnginePlugin>() {
        app.add_plugins(crate::engine_resource::ModelicaEnginePlugin);
    }

    // Ensure source-library admission is present. The compiler host consumes
    // this generic capability without owning its browser transport details.
    if !app.is_plugin_added::<lunco_modelica_library::SourceLibraryPlugin>() {
        app.add_plugins(lunco_modelica_library::SourceLibraryPlugin);
    }

    // Register the `.mo` asset loader so domain code can fetch source
    // through `AssetServer::load(...)` instead of `std::fs::read_to_string`.
    // See `docs/architecture/40-asset-io.md`.
    if !app.is_plugin_added::<lunco_modelica_runtime::ModelicaSourceAssetPlugin>() {
        app.add_plugins(lunco_modelica_runtime::ModelicaSourceAssetPlugin);
    }

    // ── Document foundation (moved out of the UI plugin so a headless server
    // journals + replicates Modelica edits, not just the GUI) ──────────────
    // The registry (source-of-truth for open `.mo` docs), the A3 journal-wire
    // auto-bridge (`wire_modelica_journal_handle`, reactive/once), and the
    // lifecycle-event drain. `ModelicaUiPlugin` no longer registers these; it
    // adds core first, so the GUI still gets them. The transport-free edit
    // command plugin is owned by `lunco-modelica-api` and installed by API
    // hosts. Guarded/idempotent so hosts can compose the packages independently.
    app.init_resource::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>();
    // Structural source edits can arrive before the asynchronous syntax cache
    // catches up. The queue and its drain are document mechanics, so headless
    // hosts need the same lifecycle as the editor UI.
    app.init_resource::<crate::doc_ops::PendingStructuralOps>();
    app.add_systems(Update, crate::doc_ops::drain_document_changes);
    app.add_systems(Update, crate::doc_ops::drain_pending_structural_ops);
    app.add_systems(
        Update,
        crate::doc_ops::wire_modelica_journal_handle
            .run_if(resource_added::<lunco_doc_bevy::JournalResource>),
    );
    app.add_observer(sync_workspace_on_doc_opened);
    app.add_observer(sync_workspace_on_doc_closed);
    app.add_observer(sync_workspace_on_doc_changed);
    app.add_observer(sync_workspace_on_doc_saved);
}

// ---------------------------------------------------------------------------
// Re-export diagram types for public API
// ---------------------------------------------------------------------------
pub use diagram::{DiagramType, ModelicaComponentBuilder, list_class_names};

#[derive(Component, Reflect, Default)]
pub struct ModelicaInput {
    pub variable_name: String,
    pub value: f64,
}

#[derive(Component, Reflect, Default)]
pub struct ModelicaOutput {
    pub variable_name: String,
    pub value: f64,
}
