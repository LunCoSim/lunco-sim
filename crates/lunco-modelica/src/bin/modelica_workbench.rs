//! Generic engineering workbench for testing any Modelica model.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use lunco_assets::assets_dir;
use lunco_modelica::{
    ModelicaPlugin,
    ModelicaModel,
    ui::WorkbenchState,
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
        .add_plugins(bevy_workbench::WorkbenchPlugin {
            config: bevy_workbench::WorkbenchConfig {
                show_menu_bar: false,
                show_toolbar: false,
                enable_game_view: false,
                ..default()
            },
        })
        .add_plugins(ModelicaPlugin)
        .add_systems(Startup, setup_sandbox)
        .run();
}

#[derive(Component)]
struct ModelicaSandbox;

fn setup_sandbox(
    mut commands: Commands,
    channels: Res<lunco_modelica::ModelicaChannels>,
    mut workbench_state: ResMut<WorkbenchState>,
) {
    commands.spawn(Camera2d);

    let model_path = assets_dir().join("models/Battery.mo");
    let source = std::fs::read_to_string(&model_path).unwrap_or_default();
    let model_name = lunco_modelica::extract_model_name(&source).unwrap_or_else(|| "Battery".to_string());
    let initial_params = lunco_modelica::extract_parameters(&source);
    let initial_inputs = lunco_modelica::extract_inputs_with_defaults(&source);

    // Initialize UI state with the default model's source
    workbench_state.editor_buffer = source.clone();

    // Spawn a generic sandbox entity.
    let entity = commands.spawn((
        ModelicaSandbox,
        Name::new("Modelica_Sandbox"),
        ModelicaModel {
            model_path,
            model_name: model_name.clone(),
            parameters: initial_params,
            inputs: initial_inputs,
            ..default()
        },
    )).id();

    // Trigger initial compilation
    let _ = channels.tx.send(lunco_modelica::ModelicaCommand::Compile {
        entity,
        session_id: 0,
        model_name,
        source,
    });
}

