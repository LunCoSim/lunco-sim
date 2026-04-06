//! Generic engineering workbench for testing any Modelica model.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use lunco_modelica::{
    LunCoModelicaPlugin, 
    ui::ModelicaInspectorPlugin, 
    ModelicaModel
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

fn setup_sandbox(
    mut commands: Commands,
    channels: Res<lunco_modelica::ModelicaChannels>,
    mut workbench_state: ResMut<lunco_modelica::ui::WorkbenchState>,
) {
    commands.spawn(Camera2d);

    let model_path = "assets/models/Battery.mo".to_string();
    let source = std::fs::read_to_string(&model_path).unwrap_or_default();
    let model_name = lunco_modelica::extract_model_name(&source).unwrap_or_else(|| "Battery".to_string());
    let initial_params = lunco_modelica::extract_parameters(&source);

    // Initialize UI state with the default model's source
    workbench_state.editor_buffer = source.clone();

    // Spawn a generic sandbox entity.
    let entity = commands.spawn((
        ModelicaSandbox,
        Name::new("Modelica_Sandbox"),
        ModelicaModel {
            model_path: model_path.clone(),
            model_name: model_name.clone(),
            parameters: initial_params,
            ..default()
        },
    )).id();

    // Trigger initial compilation
    let _ = channels.tx.send(lunco_modelica::ModelicaCommand::Compile {
        entity,
        model_name,
        source,
    });
}

