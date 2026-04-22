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

    // Force continuous frame rate even when the window is unfocused
    // so the HTTP API stays responsive under automation. Default
    // winit behaviour throttles unfocused windows, which masquerades
    // as an "app hang" when we're driving it via curl from a headless
    // test harness (the bridge drain runs on a Bevy system; no frame
    // = no drain).
    use bevy::winit::{UpdateMode, WinitSettings};
    app.insert_resource(WinitSettings {
        focused_mode: UpdateMode::Continuous,
        unfocused_mode: UpdateMode::Continuous,
    });

    // Heartbeat: emit an INFO log every 2s of wall clock from each
    // major schedule. Lets us distinguish "main loop frozen" vs
    // "API bridge frozen" vs "one schedule wedged" when diagnosing
    // apparent hangs.
    app.add_systems(bevy::prelude::Update, heartbeat_update);
    app.add_systems(bevy::prelude::FixedUpdate, heartbeat_fixed);
    app.add_systems(bevy::prelude::PostUpdate, heartbeat_post);

    app.run();
}

fn hb(tag: &'static str, last: &mut Option<std::time::Instant>) {
    let now = std::time::Instant::now();
    let fire = match *last {
        None => true,
        Some(t) => now.duration_since(t).as_secs_f32() >= 2.0,
    };
    if fire {
        *last = Some(now);
        bevy::log::info!("[hb:{tag}]");
    }
}

fn heartbeat_update(mut last: Local<Option<std::time::Instant>>) { hb("upd", &mut last); }
fn heartbeat_fixed(mut last: Local<Option<std::time::Instant>>) { hb("fix", &mut last); }
fn heartbeat_post(mut last: Local<Option<std::time::Instant>>) { hb("post", &mut last); }

fn setup_sandbox(mut commands: Commands) {
    // Start empty: the user lands on the Welcome tab, opens whatever
    // they need via Package Browser / Twin / Ctrl+N. Auto-loading
    // Battery was a debug convenience that confused new users —
    // `cargo run` would show a random model with no explanation.
    commands.spawn(Camera2d);
}
