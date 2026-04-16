//! UI for the sandbox editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use bevy::prelude::*;
use lunco_workbench::{
    PanelId, ViewportPanel, Workspace, WorkspaceId, WorkbenchAppExt, WorkbenchLayout,
    VIEWPORT_PANEL_ID,
};

pub mod spawn_palette;
pub mod inspector;
pub mod entity_list;

/// Plugin that registers all sandbox editing UI panels, the workbench
/// 3D viewport placeholder, and two workspace presets:
///
/// - **View** (default) — just the 3D scene, no panels.
/// - **Build** — 3D + Entities, Inspector, Spawn palette around the edges.
///
/// The user switches via the workspace tabs in the transport bar.
/// `ViewportPanel` is a transparent centre tab in both — Bevy's
/// full-window 3D scene shows through it.
pub struct SandboxEditUiPlugin;

impl Plugin for SandboxEditUiPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(spawn_palette::SpawnPalette)
            .register_panel(inspector::Inspector)
            .register_panel(entity_list::EntityList)
            .register_panel(ViewportPanel)
            // Order matters for auto-activation — View first so it's
            // the default when the rover binary boots.
            .register_workspace(ViewWorkspace)
            .register_workspace(BuildWorkspace);
    }
}

/// Rover sandbox's default workspace — full-screen 3D, no panels.
///
/// All slots empty — the workbench renders **nothing** in the
/// centre, so Bevy's 3D scene gets the pointer events directly. This
/// is the only way to keep gizmos draggable without render-to-texture:
/// any egui surface in the central area (even a transparent
/// `ViewportPanel` tab) marks the rect as egui-interactive and
/// blocks Bevy input.
pub struct ViewWorkspace;

impl Workspace for ViewWorkspace {
    fn id(&self) -> WorkspaceId { WorkspaceId("rover_view") }
    fn title(&self) -> String { "🎬 View".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(None);
        layout.set_right_inspector(None);
        layout.set_bottom(None);
        layout.set_center(vec![]);
    }
}

/// Build mode — 3D + Entities, Inspector, Spawn around the edges.
pub struct BuildWorkspace;

impl Workspace for BuildWorkspace {
    fn id(&self) -> WorkspaceId { WorkspaceId("rover_build") }
    fn title(&self) -> String { "🏗 Build".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(Some(PanelId("entity_list")));
        layout.set_center(vec![VIEWPORT_PANEL_ID]);
        layout.set_right_inspector(Some(PanelId("sandbox_inspector")));
        layout.set_bottom(Some(PanelId("spawn_palette")));
    }
}
