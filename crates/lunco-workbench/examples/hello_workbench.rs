//! Minimal demo of the v0.1 `lunco-workbench` shell.
//!
//! Three stub panels — one in each dock slot — prove the layout renders
//! and the `Panel` trait integrates with a Bevy app.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p lunco-workbench --example hello_workbench
//! ```

use bevy::prelude::*;
use bevy_egui::{egui, EguiPlugin};
use lunco_workbench::{
    Panel, PanelId, PanelSlot, Workspace, WorkspaceId, WorkbenchAppExt, WorkbenchLayout,
    WorkbenchPlugin,
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "hello_workbench — lunco-workbench v0.1".into(),
                resolution: bevy::window::WindowResolution::new(1280, 800),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_plugins(WorkbenchPlugin)
        // Panels: register once, then workspaces decide who goes where.
        .register_panel(SceneTreePanel)
        .register_panel(InspectorPanel)
        .register_panel(ConsolePanel)
        // Workspaces: two presets demonstrate the switcher.
        // The first registered ("Build") also activates immediately.
        .register_workspace(BuildWorkspace)
        .register_workspace(SimulateWorkspace)
        .add_systems(Startup, set_initial_status)
        .run();
}

fn set_initial_status(mut layout: ResMut<WorkbenchLayout>) {
    layout.set_status("hello_workbench · demo · v0.1");
}

// ─── Workspace presets ──────────────────────────────────────────────────

struct BuildWorkspace;
impl Workspace for BuildWorkspace {
    fn id(&self) -> WorkspaceId { WorkspaceId("build") }
    fn title(&self) -> String { "🏗 Build".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(true);
        layout.set_side_browser(Some(PanelId("hello::scene_tree")));
        layout.set_right_inspector(Some(PanelId("hello::inspector")));
        layout.set_bottom(Some(PanelId("hello::console")));
    }
}

struct SimulateWorkspace;
impl Workspace for SimulateWorkspace {
    fn id(&self) -> WorkspaceId { WorkspaceId("simulate") }
    fn title(&self) -> String { "🎮 Simulate".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        // Simulate mode: minimise chrome, keep the inspector for live values.
        layout.set_activity_bar(false);
        layout.set_side_browser(None);
        layout.set_right_inspector(Some(PanelId("hello::inspector")));
        layout.set_bottom(Some(PanelId("hello::console")));
        layout.set_bottom_visible(false); // start collapsed
    }
}

// ─── Three stub panels, one per dock slot ───────────────────────────────

struct SceneTreePanel;
impl Panel for SceneTreePanel {
    fn id(&self) -> PanelId { PanelId("hello::scene_tree") }
    fn title(&self) -> String { "Scene Tree".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }

    fn render(&mut self, ui: &mut egui::Ui, _world: &mut World) {
        ui.label("• Colony");
        ui.indent("colony_indent", |ui| {
            ui.label("├ Rover_A");
            ui.label("├ Balloon");
            ui.label("└ Lander");
        });
    }
}

struct InspectorPanel;
impl Panel for InspectorPanel {
    fn id(&self) -> PanelId { PanelId("hello::inspector") }
    fn title(&self) -> String { "Inspector".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, _world: &mut World) {
        ui.label("Selection: (none)");
        ui.separator();
        ui.label(egui::RichText::new("Transform").strong());
        ui.label("position: 0.0, 0.0, 0.0");
        ui.label("rotation: 0.0, 0.0, 0.0, 1.0");
        ui.separator();
        ui.label(egui::RichText::new("(context-aware content goes here)").weak());
    }
}

struct ConsolePanel;
impl Panel for ConsolePanel {
    fn id(&self) -> PanelId { PanelId("hello::console") }
    fn title(&self) -> String { "Console".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Bottom }

    fn render(&mut self, ui: &mut egui::Ui, _world: &mut World) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            for line in [
                "[info] workbench initialised",
                "[info] 3 panels registered",
                "[info] viewport placeholder is the central region",
            ] {
                ui.label(line);
            }
        });
    }
}
