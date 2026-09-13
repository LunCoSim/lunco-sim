//! UI surface for USD documents — the Twin-browser entry, lifecycle
//! observers that maintain it, and (Phase 4+) the prim tree, property
//! inspector, and theme tokens.
//!
//! **Layer 4 (UI).** Per `AGENTS.md` §4.1, [`UsdUiPlugin`] is added
//! independently of the complete USD runtime bundle — headless apps
//! and the sandbox bin run without it; workbench bins opt in.
//!
//! ## Lifecycle wiring
//!
//! - [`DocumentOpened`] → register a [`WorkspaceStage`] in
//!   [`LoadedUsdStages`] only for user-owned USD documents. Twin default-scene
//!   documents remain internal scene leases until the user opens or edits one.
//! - [`DocumentClosed`] →
//!   unregister by stage id (idempotent — Modelica closes are no-ops).
//!
//! Twin-driven `SystemUsdStage` registration is deferred; the trait
//! is in place so the loader slots in alongside Twin externals.

use bevy::prelude::*;
use lunco_doc::{DocumentId, OpenOutcome};
use lunco_doc_bevy::{
    DocumentClosed, DocumentOpened, DocumentRegistry, DocumentSaved, SaveAsDocument,
};
use lunco_status_core::status_bus::{StatusBus, StatusLevel};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_workbench::{BrowserSectionRegistry, ViewportPlaceholder};
use lunco_workbench_core::PanelId;

use lunco_usd_bevy_twin::UsdDocumentUserOwned;
use lunco_usd_core::document::UsdDocument;
use lunco_workspace::WorkspaceResource;

pub mod browser_dispatch;
pub mod browser_section;
pub mod loaded_stages;
pub mod scene_files;
pub mod session_codec;
pub mod viewport;

/// Stable singleton panel id for the USD wiring graph. The panel renderer is
/// supplied by the simulator editor, while navigation belongs to the USD
/// domain's Twin Browser contribution.
pub const USD_CONNECTION_CANVAS_PANEL_ID: PanelId = PanelId("usd_connection_canvas");

pub use browser_section::{ConnectionsSection, UsdSceneSection};
pub use loaded_stages::{
    produce_usd_browser_view, LoadedStage, LoadedUsdStages, UsdBrowserView, WorkspaceStage,
};
pub use scene_files::{
    produce_scene_file_view, SceneFileKind, SceneFileRescan, SceneFileRow, SceneFileView,
    SceneFilesSection,
};
pub use viewport::{
    ApplyUsdInspectionPreset, CloseUsdPreview, CloseUsdPreviewView, DeleteUsdInspectionPreset,
    ExplodeUsdPreview, FocusUsdPreview, FocusUsdPreviewView, FrameUsdPreviewSelection,
    OpenUsdPreview, OpenUsdPreviewView, SaveUsdInspectionPreset, SetUsdPreviewTextLayer,
    SetUsdPreviewViewMode, UsdPreviewExplodeAction, UsdPreviewExplodeAxis, UsdPreviewId,
    UsdPreviewRenderBudget, UsdPreviewSession, UsdPreviewTextLayer, UsdPreviewView,
    UsdPreviewViewId, UsdPreviewViewMode, UsdPreviewViewPanel, UsdViewportClick, UsdViewportPanel,
    UsdViewportPlugin, UsdViewportState, EDITOR_PREVIEW_ID, USD_PREVIEW_VIEW_PANEL_ID,
    USD_VIEWPORT_PANEL_ID,
};

/// Plugin that installs the USD Twin-browser section and the lifecycle
/// observers that keep it in sync with the document registry.
pub struct UsdUiPlugin;

impl Plugin for UsdUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadedUsdStages>();
        // Change-gated view-model the `UsdSceneSection` reads each frame.
        // The producer refreshes parse caches + flattens stages into it
        // only when the (id, generation, dirty state, review revision)
        // signature changes.
        app.init_resource::<UsdBrowserView>();
        app.add_systems(Update, produce_usd_browser_view);

        // Register the section with the workbench's registry.
        // `init_resource` is defensive: the workbench plugin owns the
        // canonical insertion, but registering before its build runs is
        // safe — `init_resource` is a no-op when the resource already
        // exists, so no double-init.
        app.init_resource::<BrowserSectionRegistry>();
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(UsdSceneSection);
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(ConnectionsSection);

        // "What files is this scene made of" — the resolved reference closure,
        // in the Files scope beside the raw folder tree. Its producer is gated on
        // the scene-root set (the walk parses layers off disk), so it costs
        // nothing per frame; see `scene_files.rs`.
        app.init_resource::<SceneFileView>();
        app.init_resource::<SceneFileRescan>();
        app.add_systems(Update, produce_scene_file_view);
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(SceneFilesSection);

        app.add_observer(register_workspace_stage_on_doc_opened);
        app.add_observer(register_workspace_stage_on_doc_user_owned);
        app.add_observer(drop_workspace_stage_on_doc_closed);
        app.add_observer(sync_workspace_on_doc_opened);
        app.add_observer(sync_workspace_on_doc_saved);
        app.add_observer(sync_workspace_on_doc_closed);
        app.add_observer(on_usd_document_ready_status);
        app.add_observer(on_save_as_document_ui);

        // Document hot-exit: persist & restore open USD buffers via the
        // per-Twin workspace-state, mirroring Modelica. Restore replays
        // `DocumentRegistry::<UsdDocument>::allocate`, which fires `DocumentOpened`
        // → the stage registration above. See `session_codec`.
        use lunco_workbench::AppDocumentSessionExt;
        app.register_document_session_codec(session_codec::UsdSessionCodec);

        // Click-to-open: `.usda` / `.usd` / `.usdc` rows in the Twin browser
        // become USD documents. This system only *translates* the
        // browser-panel click into the domain load pipeline owned by
        // `UsdCommandsPlugin` (the file read, registry allocate, and the
        // typed `OpenFile` command observer all live there, so HTTP /
        // MCP / `Open`-URI dispatch and headless bins work too). Modelica
        // owns `.mo`; the shared `BrowserActions` outbox is partitioned
        // by extension so the two drains coexist without ordering coupling.
        app.add_systems(Update, browser_dispatch::drain_browser_actions_for_usd);

        // Surface external on-disk edits (git pull, another editor) to the user.
        app.add_systems(Update, badge_externally_changed_usd_docs);
        app.add_systems(Update, update_viewport_placeholder);
    }
}

/// The generic hint shown when the viewport is empty and no specific cause
/// was recorded by the USD scene transition.
pub const GENERIC_EMPTY_HINT: &str = "No visual scene is loaded.";

/// Keep the workbench placeholder synchronized with USD's mounted scene and
/// the domain-owned empty-scene diagnostic. The placeholder is presentation;
/// [`lunco_usd::commands::EmptyViewportReason`] remains the only reason owner.
fn update_viewport_placeholder(
    scene: Query<(), With<UsdPrimPath>>,
    empty_reason: Res<lunco_usd::commands::EmptyViewportReason>,
    placeholder: Option<ResMut<ViewportPlaceholder>>,
) {
    let Some(mut placeholder) = placeholder else {
        return;
    };
    if !scene.is_empty() {
        placeholder.message = None;
        return;
    }
    let message = Some(
        empty_reason
            .0
            .as_deref()
            .unwrap_or(GENERIC_EMPTY_HINT)
            .to_string(),
    );
    if placeholder.message != message {
        placeholder.message = message;
    }
}

/// Present non-fatal document-load outcomes on the UI status bus. The USD
/// command layer emits the typed event even in headless applications.
fn on_usd_document_ready_status(
    trigger: On<lunco_usd::commands::UsdDocumentReady>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    mut bus: Option<ResMut<StatusBus>>,
) {
    let Some(message) = (match trigger.event().outcome {
        OpenOutcome::KeptDirty => Some("has unsaved edits — kept them, did not reload from disk"),
        OpenOutcome::KeptUnparsable => Some("does not parse as USDA — kept the open document"),
        OpenOutcome::Allocated | OpenOutcome::Refreshed => None,
    }) else {
        return;
    };
    let Some(bus) = bus.as_mut() else {
        return;
    };
    let doc = trigger.event().doc;
    let name = registry
        .host(doc)
        .map(|host| host.document().origin().display_name())
        .unwrap_or_else(|| doc.to_string());
    bus.push("usd", StatusLevel::Warn, format!("{name} {message}"));
}

fn suggested_usd_name(origin: &str) -> String {
    if origin.to_ascii_lowercase().ends_with(".usda")
        || origin.to_ascii_lowercase().ends_with(".usdc")
        || origin.to_ascii_lowercase().ends_with(".usd")
    {
        origin.to_string()
    } else {
        format!("{origin}.usda")
    }
}

/// Own the native Save-As picker. A supplied path is handled by the USD
/// command layer; an empty path opens the shared workbench picker.
#[cfg(not(target_arch = "wasm32"))]
fn on_save_as_document_ui(
    trigger: On<SaveAsDocument>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<WorkspaceResource>>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc_id;
    if !trigger.event().path.is_empty() {
        return;
    }
    let Some(host) = registry.host(doc) else {
        return;
    };
    let suggested_name = suggested_usd_name(&host.document().origin().display_name());
    let start_dir = workspace
        .as_deref()
        .and_then(|ws| ws.active_twin)
        .and_then(|id| workspace.as_deref()?.twin(id))
        .map(|twin| lunco_storage::StorageHandle::File(twin.root.clone()));
    commands.trigger(lunco_workbench::picker::PickHandle {
        mode: lunco_workbench::picker::PickMode::SaveFile(lunco_workbench::picker::SaveHint {
            suggested_name: Some(suggested_name),
            start_dir,
            filters: vec![lunco_workbench::picker::OpenFilter::new(
                "USD stages",
                &["usda", "usdc", "usd"],
            )],
        }),
        on_resolved: lunco_workbench::picker::PickFollowUp::SaveAs(doc),
    });
}

/// Own browser Save-As. A supplied path is a download name; an empty path
/// still uses the shared picker so the browser can resolve the file name.
#[cfg(target_arch = "wasm32")]
fn on_save_as_document_ui(
    trigger: On<SaveAsDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc_id;
    let target_path = trigger.event().path.clone();
    let Some(host) = registry.host(doc) else {
        return;
    };
    if target_path.is_empty() {
        let suggested_name = suggested_usd_name(&host.document().origin().display_name());
        commands.trigger(lunco_workbench::picker::PickHandle {
            mode: lunco_workbench::picker::PickMode::SaveFile(lunco_workbench::picker::SaveHint {
                suggested_name: Some(suggested_name),
                start_dir: None,
                filters: vec![lunco_workbench::picker::OpenFilter::new(
                    "USD stages",
                    &["usda", "usdc", "usd"],
                )],
            }),
            on_resolved: lunco_workbench::picker::PickFollowUp::SaveAs(doc),
        });
        return;
    }
    let source = host.document().source().to_string();
    lunco_workbench::picker::download_file(&target_path, &source);
    if let Some(host) = registry.host_mut(doc) {
        host.document_mut().mark_saved();
    }
    commands.trigger(DocumentSaved::local(doc));
}

/// Keep the generic Workspace document list in step with the USD registry.
///
/// USD stages are documents just like Modelica models. The previous viewport
/// registration made a newly-created stage visible to USD panels but left the
/// shared File menu without an active document, so Save/Save-As could never
/// complete the first-use workflow.
fn sync_workspace_on_doc_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    if workspace.document(doc).is_some() {
        workspace.active_document = Some(doc);
        return;
    }
    let origin = host.document().origin().clone();
    let context_twin = if origin.is_untitled() {
        workspace.active_twin
    } else {
        None
    };
    workspace.add_document(lunco_workspace::DocumentEntry {
        id: doc,
        kind: lunco_workspace::DocumentKindId::new(lunco_usd::commands::USD_DOCUMENT_KIND),
        title: origin.display_name(),
        origin,
        context_twin,
        dirty: host.document().is_dirty(),
    });
    workspace.active_document = Some(doc);
}

/// Reflect USD Save and Save-As origin changes into the generic Workspace.
fn sync_workspace_on_doc_saved(
    trigger: On<DocumentSaved>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    let origin = host.document().origin().clone();
    if let Some(path) = origin.canonical_path() {
        workspace.recents.push_loose(path.to_path_buf());
    }
    if let Some(entry) = workspace.document_mut(doc) {
        entry.title = origin.display_name();
        entry.origin = origin;
    }
}

/// Remove the Workspace shadow entry when a USD registry document closes.
fn sync_workspace_on_doc_closed(
    trigger: On<DocumentClosed>,
    workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    workspace.close_document(trigger.event().doc);
}

/// Poll the registry for USD documents whose file changed on disk behind the
/// app, and raise one status badge per episode.
///
/// **Badge, never auto-reload.** A silent reload while a sim is running would
/// restart the world (collaboration doc §UX), so this only notifies — the user
/// re-opens to take the disk copy. Throttled to ~2 s because the check stats
/// each open file; deduped via a `Local` set so a persistently-stale file
/// nags once, not every tick, and drops from the set once it re-syncs (reload
/// or save re-baselines the watermark), rearming for the next real change.
fn badge_externally_changed_usd_docs(
    time: Res<Time>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    mut bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
    mut timer: Local<f32>,
    mut badged: Local<std::collections::HashSet<lunco_doc::DocumentId>>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;

    let Some(bus) = bus.as_mut() else { return };
    let stale: std::collections::HashSet<_> = registry.stale_docs().into_iter().collect();
    // Re-arm docs that are no longer stale (re-opened, saved, or closed).
    badged.retain(|d| stale.contains(d));
    for doc in &stale {
        if badged.insert(*doc) {
            bus.push(
                "usd",
                lunco_status_core::status_bus::StatusLevel::Warn,
                format!("{doc} changed on disk — re-open to load the new version"),
            );
        }
    }
}

/// Observer: when *any* document opens, check whether it lives in the
/// USD registry — if so, register a [`WorkspaceStage`] so the
/// browser surfaces it. Modelica / SysML documents miss the gate and
/// are ignored, exactly mirroring the `lunco-modelica-core` shape.
fn register_workspace_stage_on_doc_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    backed: Res<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    mut loaded: ResMut<LoadedUsdStages>,
) {
    let doc = trigger.event().doc;
    if !registry.contains(doc) || !backed.is_user_owned(doc) {
        return;
    }
    register_workspace_stage(doc, &mut loaded);
}

/// Expose a Twin document only after it becomes user-owned. This covers a
/// scene the user edits while it is running: it started as an internal Twin
/// projection, so its initial `DocumentOpened` event was intentionally hidden.
fn register_workspace_stage_on_doc_user_owned(
    trigger: On<UsdDocumentUserOwned>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    mut loaded: ResMut<LoadedUsdStages>,
) {
    let doc = trigger.event().doc;
    if !registry.contains(doc) {
        return;
    }
    register_workspace_stage(doc, &mut loaded);
}

fn register_workspace_stage(doc: DocumentId, loaded: &mut LoadedUsdStages) {
    let stage = WorkspaceStage::new(doc);
    // Guard against duplicate registration if the same DocumentOpened
    // somehow fires twice (replay, observer ordering quirks).
    if loaded
        .entries
        .iter()
        .any(|s| s.id() == format!("workspace-usd:{}", doc.raw()))
    {
        return;
    }
    loaded.register(Box::new(stage));
}

/// Observer: when *any* document closes, drop the matching
/// `WorkspaceStage` if we have one. Idempotent — Modelica /
/// foreign-id closes find no entry and quietly return.
fn drop_workspace_stage_on_doc_closed(
    trigger: On<DocumentClosed>,
    mut loaded: ResMut<LoadedUsdStages>,
) {
    let doc = trigger.event().doc;
    let id = format!("workspace-usd:{}", doc.raw());
    loaded.unregister(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke-test: opening a USD document via the registry surfaces
    /// it as a `WorkspaceStage` after the events drain.
    #[test]
    fn workspace_stage_registered_on_doc_opened() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.add_plugins(UsdUiPlugin);
        app.update();

        let doc_id = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.open_file(
                "/tmp/scene.usda",
                "#usda 1.0\ndef Xform \"World\" {}\n".to_string(),
            )
            .0
        };
        app.world_mut()
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .claim_user(doc_id);
        // Drain pending events → DocumentOpened trigger → observer
        // registers the WorkspaceStage. Two updates so the trigger
        // queue flushes before we assert.
        app.update();
        app.update();

        let loaded = app.world().resource::<LoadedUsdStages>();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(
            loaded.entries[0].id(),
            format!("workspace-usd:{}", doc_id.raw())
        );
    }

    #[test]
    fn twin_scene_document_is_hidden_until_user_owned() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.add_plugins(UsdUiPlugin);
        app.update();

        let doc_id = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.open_file(
                "/tmp/twin-scene.usda",
                "#usda 1.0\ndef Xform \"World\" {}\n".to_string(),
            )
            .0
        };
        app.world_mut()
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .track(
                doc_id,
                "/tmp/twin".into(),
                "twin".into(),
                "twin-scene.usda".into(),
            );

        app.update();
        app.update();

        assert!(app.world().resource::<LoadedUsdStages>().entries.is_empty());
    }

    /// Closing the document drops the corresponding stage entry.
    #[test]
    fn workspace_stage_dropped_on_doc_closed() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.add_plugins(UsdUiPlugin);
        app.update();

        let doc_id = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.open_file("/tmp/scene.usda", "#usda 1.0\n".to_string())
                .0
        };
        app.world_mut()
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .claim_user(doc_id);
        app.update();
        app.update();
        assert_eq!(app.world().resource::<LoadedUsdStages>().entries.len(), 1);

        // Remove from registry → drains as DocumentClosed → observer
        // drops the stage entry.
        app.world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .remove(doc_id);
        app.update();
        app.update();

        assert!(app.world().resource::<LoadedUsdStages>().entries.is_empty());
    }
}
