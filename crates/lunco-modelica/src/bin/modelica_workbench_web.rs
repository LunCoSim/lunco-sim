//! LunCo Modelica Workbench - Web version.
//!
//! Compiled to WebAssembly for browser deployment with WebGPU rendering.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use lunco_modelica::{
    ModelicaPlugin,
    ModelicaModel,
    models::BUNDLED_MODELS,
};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub fn main() {}

/// Initialize Bevy app with bundled models
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
pub fn run() {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    let default_model = BUNDLED_MODELS.first().unwrap_or(&("", ""));
    let (default_filename, default_source) = default_model;

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "LunCo Modelica Workbench".into(),
                resolution: bevy::window::WindowResolution::new(1280, 720),
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                prevent_default_event_handling: true,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_plugins(ModelicaPlugin)
        .insert_resource(BundledModelInfo {
            default_filename: default_filename.to_string(),
            default_source: default_source.to_string(),
        })
        .add_systems(Startup, setup_web_workbench)
        .run();
}

#[derive(Resource)]
struct BundledModelInfo {
    default_filename: String,
    default_source: String,
}

#[derive(Component)]
struct WebWorkbench;

fn setup_web_workbench(
    mut commands: Commands,
    channels: Res<lunco_modelica::ModelicaChannels>,
    mut workbench_state: ResMut<lunco_modelica::ui::WorkbenchState>,
    model_info: Res<BundledModelInfo>,
) {
    commands.spawn(Camera2d);

    let model_path = model_info.default_filename.clone();
    let source = model_info.default_source.clone();
    let model_name = lunco_modelica::extract_model_name(&source).unwrap_or_else(|| "Model".to_string());
    let initial_params = lunco_modelica::extract_parameters(&source);
    let initial_inputs = lunco_modelica::extract_inputs_with_defaults(&source);

    workbench_state.editor_buffer = source.clone();

    let entity = commands.spawn((
        WebWorkbench,
        Name::new("Modelica_Sandbox"),
        ModelicaModel {
            model_path,
            model_name: model_name.clone(),
            parameters: initial_params,
            inputs: initial_inputs,
            ..default()
        },
    )).id();

    let _ = channels.tx.send(lunco_modelica::ModelicaCommand::Compile {
        entity,
        session_id: 0,
        model_name,
        source,
    });
}
