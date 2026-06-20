//! UI for the sandbox editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use bevy::prelude::*;
use lunco_workbench::{
    HelpMouse, HelpShortcut, PanelId, ViewportPanel, Perspective, PerspectiveId, WorkbenchAppExt,
    WorkbenchLayout, VIEWPORT_PANEL_ID,
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
/// `ViewportPanel` reserves the centre slot in both perspectives; the
/// 3D camera (tagged `WorkbenchViewportCamera`) is confined to that
/// rect each frame by `lunco_workbench::apply_workbench_viewport`, and
/// the panel paints its theme backdrop around it.
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
            .register_perspective_help(
                PerspectiveId("sandbox_view"),
                lunco_workbench::PerspectiveHelp {
                    title: "🎬 View",
                    description: "Full-screen 3D observation & control mode. Fly the \
                                  camera around the scene, take control of a vessel \
                                  and drive it, or follow one as it moves.",
                    shortcuts: vec![
                        HelpShortcut { keys: "W / A / S / D", description: "Drive the controlled vessel · fly the camera" },
                        HelpShortcut { keys: "Q / E", description: "Move camera down / up" },
                        HelpShortcut { keys: "Shift", description: "Camera speed boost" },
                        HelpShortcut { keys: "Space", description: "Brake the controlled vessel" },
                        HelpShortcut { keys: "Backspace", description: "Release control — back to free-flight camera" },
                        HelpShortcut { keys: "Esc", description: "Drop the transform gizmo / deselect" },
                        HelpShortcut { keys: "+ / −", description: "Zoom in / out" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click vessel", description: "Take control (possess) and drive it" },
                        HelpMouse { interaction: "Left-Click object", description: "Follow it with the camera" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Grab the transform gizmo to move an object" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                        HelpMouse { interaction: "Scroll", description: "Zoom in / out" },
                    ],
                    has_tour: false,
                },
            )
            .register_perspective(BuildPerspective)
            .register_perspective_help(
                PerspectiveId("rover_build"),
                lunco_workbench::PerspectiveHelp {
                    title: "🏗 Build",
                    description: "3D scene editor. Spawn objects from the palette, \
                                  select and transform them, and assemble the scene.",
                    shortcuts: vec![
                        HelpShortcut { keys: "W / A / S / D", description: "Move camera" },
                        HelpShortcut { keys: "Q / E", description: "Move camera down / up" },
                        HelpShortcut { keys: "Shift", description: "Hold to place multiple (sticky spawn)" },
                        HelpShortcut { keys: "Delete", description: "Delete the selected object" },
                        HelpShortcut { keys: "Ctrl+Z", description: "Undo" },
                        HelpShortcut { keys: "Esc", description: "Cancel placement · clear selection / gizmo" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click", description: "Select object · confirm placement" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Select + transform gizmo (drag to move)" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                        HelpMouse { interaction: "Scroll", description: "Zoom in / out" },
                    ],
                    has_tour: false,
                },
            );
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
    fn id(&self) -> PerspectiveId { PerspectiveId("sandbox_view") }
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
    // ⚒ (U+2692) instead of 🏗 (U+1F3D7) — the latter tofus in the
    // bundled DejaVu fallback; ⚒ renders everywhere (see welcome.rs).
    fn title(&self) -> String { "⚒ Build".into() }
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
