//! UI for the sandbox editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use bevy::prelude::*;
use lunco_workbench::{
    PanelId, ViewportPanel, Perspective, PerspectiveId, WorkbenchAppExt, WorkbenchLayout,
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
            .register_perspective(ViewPerspective)
            .register_perspective(BuildPerspective);
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
pub struct ViewPerspective;

impl Perspective for ViewPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("rover_view") }
    fn title(&self) -> String { "🎬 View".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(None);
        layout.set_right_inspector(None);
        layout.set_bottom(None);
        layout.set_center(vec![]);
    }
}

/// Build mode — Spawn left, 3D centre, Inspector + Entities tabbed right.
///
/// Spawn lives on the left because it's the primary "add stuff" tool;
/// Entities + Inspector tab together on the right because the entity
/// list is rarely the main view (you mostly click in the 3D scene to
/// select). Bottom dock is empty — fewer rows of chrome.
pub struct BuildPerspective;

impl Perspective for BuildPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("rover_build") }
    fn title(&self) -> String { "🏗 Build".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser_tabs(vec![
            PanelId("spawn_palette"),
            // Optional — registered by the rover binary; filtered out
            // in other apps.
            PanelId("rover_models"),
        ]);
        layout.set_center(vec![VIEWPORT_PANEL_ID]);
        layout.set_right_inspector_tabs(vec![
            PanelId("sandbox_inspector"),
            PanelId("entity_list"),
            // Optional — only renders if the host binary registers a
            // panel with this id (the rover binary does, modelica
            // workbench doesn't). The workbench filters unknown ids.
            PanelId("rover_code"),
        ]);
        layout.set_bottom(None);
    }
}
