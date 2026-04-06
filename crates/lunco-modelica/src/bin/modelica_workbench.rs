//! Generic engineering workbench for testing any Modelica model.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use lunco_modelica::{
    LunCoModelicaPlugin, 
    ui::ModelicaInspectorPlugin, 
    ModelicaModel, 
    ModelicaInput
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "LunCo Modelica Workbench".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_plugins(LunCoModelicaPlugin)
        .add_plugins(ModelicaInspectorPlugin)
        .add_systems(Startup, setup_sandbox)
        .run();
}

#[derive(Component)]
struct ModelicaSandbox;

fn setup_sandbox(mut commands: Commands) {
    commands.spawn(Camera2d);

    // Spawn a generic sandbox entity
    // The workbench UI will populate parameters and variables automatically on first compile
    commands.spawn((
        ModelicaSandbox,
        Name::new("Modelica_Sandbox"),
        ModelicaModel {
            model_path: "assets/models/Battery.mo".to_string(),
            model_name: "Battery".to_string(),
            ..default()
        },
        ModelicaInput {
            variable_name: "current_in".to_string(),
            value: 0.0,
        },
    ));
}
