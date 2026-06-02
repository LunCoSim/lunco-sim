//! UI surface for USD documents — the Twin-browser entry, lifecycle
//! observers that maintain it, and (Phase 4+) the prim tree, property
//! inspector, and theme tokens.
//!
//! **Layer 4 (UI).** Per `AGENTS.md` §4.1, [`UsdUiPlugin`] is added
//! independently of [`UsdPlugins`](crate::UsdPlugins) — headless apps
//! and the sandbox bin run without it; workbench bins opt in.
//!
//! ## Lifecycle wiring
//!
//! - [`DocumentOpened`] →
//!   register a [`WorkspaceStage`] in
//!   [`LoadedUsdStages`],
//!   gated on registry membership so we don't react to Modelica /
//!   SysML opens.
//! - [`DocumentClosed`] →
//!   unregister by stage id (idempotent — Modelica closes are no-ops).
//!
//! Twin-driven `SystemUsdStage` registration is deferred; the trait
//! is in place so the loader slots in alongside Twin externals.

use bevy::prelude::*;
use lunco_doc_bevy::{DocumentClosed, DocumentOpened};
use lunco_workbench::BrowserSectionRegistry;

use crate::registry::UsdDocumentRegistry;

pub mod browser_dispatch;
pub mod browser_section;
pub mod loaded_stages;
pub mod session_codec;
pub mod viewport;

pub use browser_section::UsdSceneSection;
pub use loaded_stages::{LoadedStage, LoadedUsdStages, WorkspaceStage};
pub use viewport::{
    SetActiveUsdViewport, UsdViewportPanel, UsdViewportPlugin, UsdViewportState,
    USD_VIEWPORT_PANEL_ID,
};

/// Plugin that installs the USD Twin-browser section and the lifecycle
/// observers that keep it in sync with the document registry.
pub struct UsdUiPlugin;

impl Plugin for UsdUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadedUsdStages>();

        // Register the section with the workbench's registry.
        // `init_resource` is defensive: the workbench plugin owns the
        // canonical insertion, but registering before its build runs is
        // safe — `init_resource` is a no-op when the resource already
        // exists, so no double-init.
        app.init_resource::<BrowserSectionRegistry>();
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(UsdSceneSection);

        app.add_observer(register_workspace_stage_on_doc_opened);
        app.add_observer(drop_workspace_stage_on_doc_closed);

        // Document hot-exit: persist & restore open USD buffers via the
        // per-Twin workspace-state, mirroring Modelica. Restore replays
        // `UsdDocumentRegistry::allocate`, which fires `DocumentOpened`
        // → the stage registration above. See `session_codec`.
        use lunco_workbench::AppDocumentSessionExt;
        app.register_document_session_codec(session_codec::UsdSessionCodec);

        // Click-to-open: `.usda` / `.usdc` rows in the Twin browser
        // become USD documents. This system only *translates* the
        // browser-panel click into the domain load pipeline owned by
        // `UsdCommandsPlugin` (the file read, registry allocate, and the
        // typed `OpenFile` command observer all live there, so HTTP /
        // MCP / `Open`-URI dispatch and headless bins work too). Modelica
        // owns `.mo`; the shared `BrowserActions` outbox is partitioned
        // by extension so the two drains coexist without ordering coupling.
        app.add_systems(Update, browser_dispatch::drain_browser_actions_for_usd);
    }
}

/// Observer: when *any* document opens, check whether it lives in the
/// USD registry — if so, register a [`WorkspaceStage`] so the
/// browser surfaces it. Modelica / SysML documents miss the gate and
/// are ignored, exactly mirroring the `lunco-modelica` shape.
fn register_workspace_stage_on_doc_opened(
    trigger: On<DocumentOpened>,
    registry: Res<UsdDocumentRegistry>,
    mut loaded: ResMut<LoadedUsdStages>,
) {
    let doc = trigger.event().doc;
    if !registry.contains(doc) {
        return;
    }
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
    use lunco_doc::DocumentOrigin;

    /// Smoke-test: opening a USD document via the registry surfaces
    /// it as a `WorkspaceStage` after the events drain.
    #[test]
    fn workspace_stage_registered_on_doc_opened() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(crate::commands::UsdCommandsPlugin);
        app.add_plugins(UsdUiPlugin);
        app.update();

        let doc_id = {
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\ndef Xform \"World\" {}\n".to_string(),
                DocumentOrigin::writable_file("/tmp/scene.usda"),
            )
        };
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

    /// Closing the document drops the corresponding stage entry.
    #[test]
    fn workspace_stage_dropped_on_doc_closed() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(crate::commands::UsdCommandsPlugin);
        app.add_plugins(UsdUiPlugin);
        app.update();

        let doc_id = {
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\n".to_string(),
                DocumentOrigin::writable_file("/tmp/scene.usda"),
            )
        };
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<LoadedUsdStages>().entries.len(),
            1
        );

        // Remove from registry → drains as DocumentClosed → observer
        // drops the stage entry.
        app.world_mut()
            .resource_mut::<UsdDocumentRegistry>()
            .remove(doc_id);
        app.update();
        app.update();

        assert!(app
            .world()
            .resource::<LoadedUsdStages>()
            .entries
            .is_empty());
    }
}
