//! Generic engineering workbench for testing any Modelica model.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use lunco_assets::assets_dir;
use lunco_modelica::{
    ModelicaPlugin,
    ModelicaModel,
    ui::{
        panels::model_view::{ModelTabs, MODEL_VIEW_KIND},
        CompileState, CompileStates, ModelicaDocumentRegistry, ModelLibrary, WorkbenchState,
    },
};

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
        .add_plugins(ModelicaPlugin)
        .add_systems(Startup, setup_sandbox);

    #[cfg(feature = "lunco-api")]
    app.add_plugins(lunco_api::LunCoApiPlugin::default());

    app.run();
}

#[derive(Component)]
struct ModelicaSandbox;

fn setup_sandbox(
    mut commands: Commands,
    channels: Res<lunco_modelica::ModelicaChannels>,
    mut workbench_state: ResMut<WorkbenchState>,
    mut doc_registry: ResMut<ModelicaDocumentRegistry>,
    mut compile_states: ResMut<CompileStates>,
    mut model_tabs: ResMut<ModelTabs>,
    mut layout: ResMut<lunco_workbench::WorkbenchLayout>,
) {
    commands.spawn(Camera2d);

    let model_path = assets_dir().join("models/Battery.mo");
    let source = std::fs::read_to_string(&model_path).unwrap_or_default();
    let model_name = lunco_modelica::extract_model_name(&source).unwrap_or_else(|| "Battery".to_string());
    let initial_params = lunco_modelica::extract_parameters(&source);
    let initial_inputs = lunco_modelica::extract_inputs_with_defaults(&source);

    // Initialize UI state with the default model's source
    workbench_state.editor_buffer = source.clone();
    workbench_state.loaded_file_path = Some(model_path.clone());

    // Allocate the Document first so the entity is spawned with a valid
    // `document` id from the start — the Document is the single source of
    // truth for this model's text. Record its origin so `SaveDocument` and
    // read-only classification work.
    let doc_id = doc_registry.allocate_with_origin(
        source.clone(),
        Some(model_path.clone()),
        ModelLibrary::Bundled,
    );

    let entity = commands.spawn((
        ModelicaSandbox,
        Name::new("Modelica_Sandbox"),
        ModelicaModel {
            model_path,
            model_name: model_name.clone(),
            parameters: initial_params,
            inputs: initial_inputs,
            document: doc_id,
            ..default()
        },
    )).id();

    doc_registry.link(entity, doc_id);
    compile_states.set(doc_id, CompileState::Compiling);

    // Open a model tab for the bundled default so the user lands on
    // the Battery view instead of the Welcome placeholder.
    model_tabs.ensure(doc_id);
    layout.open_instance(MODEL_VIEW_KIND, doc_id.raw());

    // Trigger initial compilation
    let _ = channels.tx.send(lunco_modelica::ModelicaCommand::Compile {
        entity,
        session_id: 0,
        model_name,
        source,
    });
}
