//! Lunica — Web entry point.
//!
//! ## Why a separate binary?
//!
//! Desktop uses `std::thread::spawn` for the simulation worker. On wasm32
//! this panics because `web_time::Instant` and `std::thread` are not
//! implemented in the browser sandbox. The web binary:
//!
//! 1. Uses `wasm_bindgen(start)` instead of `fn main()` — the browser calls
//!    this automatically when the WASM module loads.
//! 2. Embeds model files via `include_str!` at compile time (no filesystem).
//! 3. Points to the LunCoSim/rumoca `web-fix` branch which replaces
//!    `web_time::Instant` with `instant::Instant` (backed by `performance.now()`).
//! 4. Disables `std::time`-dependent Bevy systems; `spawn_modelica_requests`
//!    uses a fixed 16ms timestep instead of `Time::elapsed_secs_f64()`.
//!
//! See `../lib.rs` for the inline worker implementation.

use bevy::prelude::*;
use std::path::PathBuf;
use lunco_modelica::{
    ModelicaPlugin,
    ModelicaModel,
    models::bundled_models,
};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Desktop stub — this binary only works on wasm32.
/// Use `lunica` for desktop.
fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    panic!("lunica_web is a wasm32-only binary. Use `cargo run --bin lunica` for desktop.");
}

/// Browser entry point. Called automatically when the WASM module loads.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
pub fn run() {
    // Panic messages go to browser console instead of being silent failures.
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    // Pick the bundled model that loads on startup. Default is the
    // first entry (`bundled_models()` is sorted by filename), but the
    // page URL can override via `?example=<filename>` so we can deep-
    // link straight to a specific model — handy for tutorials, bug
    // reports, and the autonomous test loop.
    //
    //   /                                 → first bundled model
    //   /?example=AnnotatedRocketStage.mo → AnnotatedRocketStage
    //   /?example=Battery                 → Battery (.mo extension is
    //                                       auto-appended if missing)
    //
    // Unknown names log a warn and fall back to the default so a stale
    // bookmark doesn't break the page.
    let models = bundled_models();
    let fallback = models
        .first()
        .expect("at least one bundled model");
    #[cfg(target_arch = "wasm32")]
    let url_example: Option<String> = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|s| {
            // `s` is the literal query string including the leading
            // `?`. URLSearchParams handles decoding for us.
            web_sys::UrlSearchParams::new_with_str(s.trim_start_matches('?'))
                .ok()
                .and_then(|p| p.get("example"))
        });
    #[cfg(not(target_arch = "wasm32"))]
    let url_example: Option<String> = None;

    // Only auto-open a tab when the URL explicitly asks for one
    // (`?example=…`). On a bare `/` we land on the Welcome screen and
    // let `wasm_autosave`'s restore path open whatever the user had
    // open last time — opening a hard-coded library example here was
    // surprising (it appeared even when the user never asked for it
    // and obscured restored work). The first bundled model is still
    // the named fallback for *unrecognised* `?example=` values, but
    // a missing query string now means "open nothing".
    let chosen: Option<&lunco_modelica::models::BundledModel> = url_example
        .as_deref()
        .map(|name| {
            let matches = |candidate: &str| {
                let n_with_ext = if name.ends_with(".mo") {
                    name.to_string()
                } else {
                    format!("{name}.mo")
                };
                candidate == name || candidate == n_with_ext
            };
            models
                .iter()
                .find(|m| matches(m.filename))
                .unwrap_or_else(|| {
                    bevy::log::warn!(
                        "[lunica_web] ?example={name:?} not found in bundled models — \
                         falling back to {}",
                        fallback.filename
                    );
                    fallback
                })
        });
    let _ = fallback;

    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    // Match the index.html backdrop so the first wgpu clear paints
    // the same dark colour the canvas already has — eliminates the
    // gray flash between wasm init resolving and the first egui
    // frame landing. Wasm-only because there's no host HTML page on
    // native to colour-match against.
    #[cfg(target_arch = "wasm32")]
    app.insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)));
    app
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Lunica".into(),
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
        // EguiPlugin is auto-added by WorkbenchPlugin (idempotent).
        .add_plugins(lunco_workbench::WorkbenchPlugin)
        .add_plugins(ModelicaPlugin)
        // Autosave Untitled / duplicated docs to the browser's
        // localStorage so a page reload doesn't silently lose
        // anything. No-op on native (the desktop binary doesn't
        // include this plugin).
        .add_plugins(lunco_modelica::ui::wasm_autosave::WasmAutosavePlugin)
        // JS clipboard bridge — installs document-level capture-phase
        // listeners for `copy`/`cut`/`paste` and pre-empts bevy_egui's
        // broken async wasm clipboard pipeline. See
        // `ui/wasm_clipboard.rs`.
        .add_plugins(lunco_modelica::ui::wasm_clipboard::WasmClipboardPlugin)
        // Camera2d + PrimaryEguiContext now auto-spawned by
        // WorkbenchPlugin (see lunco-workbench/src/viewport.rs::
        // ensure_egui_host). The old explicit `Camera2d` spawn here was
        // missing the marker and was the latent cause of the
        // "UI vanishes" class of bugs.
        .add_systems(Update, hide_html_loader_once_painted);

    // Only register the auto-open Startup system + its config resource
    // when the URL asked for a specific example. Without it the user
    // lands on Welcome and `wasm_autosave` is free to restore tabs
    // from localStorage without an unwanted bundled tab competing.
    if let Some(model) = chosen {
        app.insert_resource(BundledModelInfo {
            default_filename: model.filename.to_string(),
            default_source: model.source.to_string(),
        })
        .add_systems(Startup, setup_web_workbench);
    }

    // Off-thread Modelica worker. The result-side sender was registered by
    // `ModelicaPlugin::build` above; this call attaches a `web_sys::Worker`
    // instance pointing at the second wasm bundle. Bevy's
    // `worker_transport::pump_commands_to_worker` system then ships every
    // ModelicaCommand to the worker, and the worker's postMessage replies
    // are decoded and pushed back into the channel that
    // `handle_modelica_responses` already drains.
    //
    // If this fails (worker bundle missing, COOP/COEP misconfigured, etc.)
    // we log + continue: the inline worker remains as a fallback so the
    // page still loads, just with the old UI-blocking compile path.
    #[cfg(target_arch = "wasm32")]
    if let Err(e) = lunco_modelica::worker_transport::install_worker("./worker/worker_bootstrap.js") {
        bevy::log::error!(
            "[lunica_web] failed to start off-thread worker; falling back to inline: {e:?}"
        );
    }

    app.run();
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
    compile_states: ResMut<lunco_modelica::ui::CompileStates>,
    mut model_tabs: ResMut<lunco_modelica::ui::panels::model_view::ModelTabs>,
    mut layout: ResMut<lunco_workbench::WorkbenchLayout>,
    model_info: Res<BundledModelInfo>,
) {
    // Camera is spawned by an unconditional Startup system in `run()`
    // so it exists even on a bare-URL boot. Don't double-spawn here.

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
    // No initial compile is dispatched (see comment further down).
    // Leave CompileStates at the default Idle — setting it to
    // Compiling here without a corresponding `Compile` send would
    // leave the toolbar stuck on the sandglass icon forever.
    let _ = compile_states;

    // Open the model tab so the user lands on the actual model view
    // instead of the Welcome placeholder.
    let tab_id = model_tabs.ensure_for(doc_id, None);
    layout.open_instance(
        lunco_modelica::ui::panels::model_view::MODEL_VIEW_KIND,
        tab_id,
    );

    // Select this entity so panels default to viewing it. Observable
    // auto-binding into the default plot happens inside
    // `handle_modelica_responses` on the `is_new_model` branch.
    workbench_state.selected_entity = Some(entity);

    // No automatic compile on boot: the user clicks Compile when
    // ready. Avoids racing the MSL fetch (which lands seconds later
    // on web) and respects the principle that compile is an explicit
    // user action. `entity`, `model_name`, `source` are bound here so
    // the toolbar/keyboard Compile path finds them populated.
    let _ = (entity, model_name, source, channels);
}

/// Hide the centred HTML loader once Bevy has actually started ticking.
/// We wait two Update frames so the first egui frame has been queued
/// and (likely) painted — hiding earlier would leave the user staring
/// at a dark canvas with no UI while plugins finish building. Runs
/// only on `wasm32` because the loader element only exists there.
#[cfg(target_arch = "wasm32")]
fn hide_html_loader_once_painted(mut frame: bevy::prelude::Local<u32>) {
    use wasm_bindgen::JsCast;
    *frame += 1;
    if *frame != 2 {
        return;
    }
    let Some(win) = web_sys::window() else { return };
    let Ok(fnval) = js_sys::Reflect::get(&win, &"__lc_app_ready".into()) else { return };
    let Ok(func) = fnval.dyn_into::<js_sys::Function>() else { return };
    let _ = func.call0(&win.into());
}

#[cfg(not(target_arch = "wasm32"))]
fn hide_html_loader_once_painted() {}
