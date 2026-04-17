//! Generic engineering workbench for testing any Modelica model.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use lunco_modelica::ModelicaPlugin;

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "LunCo Modelica Workbench".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_plugins(lunco_workbench::WorkbenchPlugin)
        // LuncoVizPlugin must come before any plugin that publishes
        // signals (ModelicaPlugin below) so the `SignalRegistry`
        // resource is present when the worker starts mirroring
        // samples into it.
        .add_plugins(lunco_viz::LuncoVizPlugin)
        .add_plugins(ModelicaPlugin)
        .add_systems(Startup, setup_sandbox);

    #[cfg(feature = "lunco-api")]
    app.add_plugins(lunco_api::LunCoApiPlugin::default());

    app.run();
}

fn setup_sandbox(mut commands: Commands) {
    // Start empty: the user lands on the Welcome tab, opens whatever
    // they need via Package Browser / Twin / Ctrl+N. Auto-loading
    // Battery was a debug convenience that confused new users —
    // `cargo run` would show a random model with no explanation.
    commands.spawn(Camera2d);
}
