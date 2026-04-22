//! LunCo Modelica Workbench — Web entry point.
//!
//! ## Why a separate binary?
//!
//! Desktop uses `std::thread::spawn` for the simulation worker. On wasm32
//! this panics because `std::time::Instant` and `std::thread` are not
//! implemented in the browser sandbox. The web binary:
//!
//! 1. Uses `wasm_bindgen(start)` instead of `fn main()` — the browser calls
//!    this automatically when the WASM module loads.
//! 2. Embeds model files via `include_str!` at compile time (no filesystem).
//! 3. Points to the LunCoSim/rumoca `web-fix` branch which replaces
//!    `std::time::Instant` with `instant::Instant` (backed by `performance.now()`).
//! 4. Disables `std::time`-dependent Bevy systems; `spawn_modelica_requests`
//!    uses a fixed 16ms timestep instead of `Time::elapsed_secs_f64()`.
//!
//! See `../lib.rs` for the inline worker implementation.

use bevy::prelude::*;
use std::path::PathBuf;
use bevy_egui::EguiPlugin;
use lunco_modelica::{
    ModelicaPlugin,
    ModelicaModel,
    models::bundled_models,
};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Desktop stub — this binary only works on wasm32.
/// Use `modelica_workbench` for desktop.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    panic!("modelica_workbench_web is a wasm32-only binary. Use `cargo run --bin modelica_workbench` for desktop.");
}

/// Browser entry point. Called automatically when the WASM module loads.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
pub fn run() {
    // Panic messages go to browser console instead of being silent failures.
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    // Load the first bundled model (Battery.mo) as the default.
    // Models are embedded at compile time via include_str! — no filesystem access.
    // Bevy requires some default; RocketEngine is first in the list.
    let models = bundled_models();
    let default_model = models
        .first()
        .expect("at least one bundled model");
    let default_filename = default_model.filename;
    let default_source = default_model.source;

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "LunCo Modelica Workbench".into(),
                resolution: bevy::window::WindowResolution::new(1280, 720),
                // Render into the <canvas id="bevy"> element from index.html.
                canvas: Some("#bevy".into()),
                // Auto-resize to fill the browser window.
                fit_canvas_to_parent: true,
                // Prevent Bevy from swallowing keyboard events (egui needs them too).
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

/// Resource holding the default model info passed to the startup system.
#[derive(Resource)]
struct BundledModelInfo {
    default_filename: String,
    default_source: String,
}

/// Marker component for the initial workbench entity.
#[derive(Component)]
struct WebWorkbench;

/// Spawns the initial Modelica sandbox with the bundled default model.
fn setup_web_workbench(
    mut commands: Commands,
    channels: Res<lunco_modelica::ModelicaChannels>,
    mut workbench_state: ResMut<lunco_modelica::ui::WorkbenchState>,
    mut doc_registry: ResMut<lunco_modelica::ui::ModelicaDocumentRegistry>,
    mut compile_states: ResMut<lunco_modelica::ui::CompileStates>,
    mut model_tabs: ResMut<lunco_modelica::ui::panels::model_view::ModelTabs>,
    mut layout: ResMut<lunco_workbench::WorkbenchLayout>,
    model_info: Res<BundledModelInfo>,
) {
    commands.spawn(Camera2d);

    let model_path = PathBuf::from(&model_info.default_filename);
    let source = model_info.default_source.clone();
    let model_name = lunco_modelica::extract_model_name(&source).unwrap_or_else(|| "Model".to_string());
    let initial_params = lunco_modelica::extract_parameters(&source);
    let initial_inputs = lunco_modelica::extract_inputs_with_defaults(&source);

    workbench_state.editor_buffer = source.clone();

    // Allocate the Document up-front so the entity is spawned with a
    // valid `document` id pointing at its source. Record the bundled-asset
    // origin for read-only classification.
    let doc_id = doc_registry.allocate_with_origin(
        source.clone(),
        lunco_doc::DocumentOrigin::readonly_file(model_path.clone()),
    );

    let entity = commands.spawn((
        WebWorkbench,
        Name::new("Modelica_Sandbox"),
        ModelicaModel {
            model_path,
            model_name: model_name.clone(),
            parameters: initial_params,
            inputs: initial_inputs,
            paused: true, // Start paused; compile result will unpause
            document: doc_id,
            ..default()
        },
    )).id();

    doc_registry.link(entity, doc_id);
    compile_states.set(doc_id, lunco_modelica::ui::CompileState::Compiling);

    // Open the model tab so the user lands on the actual model view
    // instead of the Welcome placeholder.
    model_tabs.ensure(doc_id);
    layout.open_instance(
        lunco_modelica::ui::panels::model_view::MODEL_VIEW_KIND,
        doc_id.raw(),
    );

    // Select this entity so panels default to viewing it. Observable
    // auto-binding into the default plot happens inside
    // `handle_modelica_responses` on the `is_new_model` branch.
    workbench_state.selected_entity = Some(entity);

    // Kick off initial compilation of the bundled model.
    let _ = channels.tx.send(lunco_modelica::ModelicaCommand::Compile {
        entity,
        session_id: 0,
        model_name,
        source,
    });
}
