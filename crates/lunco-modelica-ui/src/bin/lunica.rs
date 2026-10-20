//! Lunica — the generic Modelica engineering workbench.
//!
//! **One binary, two targets.** This single `fn main()` builds for both:
//!
//!   - **native** (`cargo run --bin lunica [-- --api <port>]`) — a winit
//!     desktop window with the merged-titlebar chrome, the optional HTTP
//!     `--api` bridge, the OS clipboard, and a real background worker
//!     thread for rumoca compiles.
//!   - **wasm** (`scripts/build_web.sh build lunica`) — renders into
//!     the `<canvas id="bevy">` from `web/index.html`, embeds the bundled
//!     models from the external asset library, with `?example=<file>` deep-linking,
//!     and (via [`ModelicaWorkbenchPlugin`]) wires the JS clipboard
//!     bridge, localStorage autosave, and the off-thread compile worker.
//!
//! Desktop and browser hosts share one entry point. wasm-bindgen with
//! `--target web` invokes `main` when the module loads.
//!
//! Everything the workbench *is* (panels, clipboard, autosave, worker,
//! source library) lives in [`ModelicaWorkbenchPlugin`]; this file is just the thin
//! app shell (window + frame pacing + native `--api`).

use bevy::prelude::*;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-allocator"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// The egui workbench shell — UI only. A headless `--no-ui` (or `--no-default-
// features`) lunica adds the Modelica compiler and execution plugins instead.
#[cfg(feature = "ui")]
use lunco_modelica_ui::ModelicaWorkbenchPlugin;

#[cfg(all(target_arch = "wasm32", feature = "ui"))]
use lunco_modelica_runtime::{ModelicaModel, ModelicaSource};
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
use std::path::{Path, PathBuf};

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    install_panic_hook();

    // ── Native-only: cap rumoca's compiler parallelism ──────────────
    //
    // Rumoca builds its own parser pool with bounded concurrency and 16 MB
    // stacks. Reserve capacity for Bevy; cap parsing at two threads on small
    // machines. No Rayon pool is configured on wasm.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let n_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let compile_threads = if n_cpus <= 4 {
            2
        } else {
            n_cpus.saturating_sub(2)
        };
        rumoca_compile::parallelism::set_compiler_parallelism(compile_threads);
        eprintln!(
            "[lunica] rumoca compiler parallelism set to {compile_threads} threads (of {n_cpus} CPUs)"
        );
    }

    // ── wasm-only: route panics to the browser console ──────────────
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    // ── Native-only: parse `--api <port>` so the window title can ────
    // advertise the listening port (automation drives the workbench via
    // it; visible in the title bar avoids confusion when several
    // instances run side-by-side — e.g. user on 4101 + a test on 3001).
    // Headless when built without the `ui` feature, or asked at runtime via
    // `--no-ui` / `LUNCO_NO_UI`. A headless lunica is a Modelica compile/run
    // server (HTTP API), no window or egui. `cfg!` folds to `true` when `ui`
    // is off, stripping the windowed paths below.
    // Headless when built without the `ui` feature, or asked at runtime via
    // `--no-ui` / `LUNCO_NO_UI`. A headless lunica is a Modelica compile/run
    // server (HTTP API), no window or egui. wasm is never headless (the web
    // build is always the GUI IDE). The window title is computed inside
    // [`default_plugins`].
    #[cfg(not(target_arch = "wasm32"))]
    let headless = !cfg!(feature = "ui")
        || std::env::args().any(|a| a == "--no-ui")
        || std::env::var("LUNCO_NO_UI").is_ok_and(|v| v != "0" && !v.is_empty());
    #[cfg(target_arch = "wasm32")]
    let headless = false;

    let mut app = App::new();
    if let Err(error) = lunco_assets_runtime::register_lunco_asset_sources(&mut app) {
        eprintln!("lunica startup failed: {error}");
        std::process::exit(1);
    }

    // Physics fixed timestep (lunco_core_runtime::FIXED_HZ). Modelica stepping runs in
    // FixedUpdate so the worker receives a predictable per-tick dt.
    app.insert_resource(Time::<Fixed>::from_hz(lunco_core_runtime::FIXED_HZ));

    // wasm: match the index.html backdrop so the first wgpu clear paints
    // the same dark colour the canvas already has — kills the gray flash
    // between wasm init resolving and the first egui frame. Native has no
    // host HTML page to colour-match against, so this is wasm-only.
    #[cfg(target_arch = "wasm32")]
    app.insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)));

    // ── Composition root ──────────────────────────────────────────────────
    // Both hosts use one compiler/execution composition. Windowed mode adds
    // the workbench presentation; headless mode adds a native schedule runner.
    app.add_plugins(default_plugins(headless));
    lunco_assets_runtime::register_lunco_asset_types(&mut app);
    app.add_plugins((
        lunco_modelica_core::ModelicaCorePlugin,
        lunco_modelica_execution::ModelicaExecutionPlugin,
    ));

    // GUI (native windowed, or wasm — always windowed). The whole workbench:
    // WorkbenchPlugin + ModelicaPlugin + clipboard and autosave. The same
    // presentation bundle is embedded by LunCoSim.
    #[cfg(feature = "ui")]
    if !headless {
        if app
            .world()
            .get_resource::<lunco_core::BuildIdentity>()
            .is_none()
        {
            app.insert_resource(lunco_core::BuildIdentity::new(
                lunco_modelica_core::PRODUCT_VERSION,
                lunco_modelica_core::GIT_SHA,
                lunco_modelica_core::REPOSITORY_URL,
            ));
        }
        app.add_plugins(ModelicaWorkbenchPlugin::default());
        // Frame pacing: Continuous focused (vsync paces Update); low-power
        // unfocused. (The 5 Hz "UI vanishes on zoom" spike-train fix.) Native
        // only — wasm is paced by requestAnimationFrame.
        #[cfg(not(target_arch = "wasm32"))]
        {
            use bevy::winit::{UpdateMode, WinitSettings};
            app.insert_resource(WinitSettings {
                focused_mode: UpdateMode::Continuous,
                unfocused_mode: UpdateMode::reactive_low_power(std::time::Duration::from_secs(1)),
            });
            app.add_systems(Update, open_latest_twin_on_startup);
        }
    }

    // HEADLESS: Modelica compile/run server. ScheduleRunnerPlugin ticks the app
    // in winit's place; the HTTP API serves compile/run requests. (wasm is never
    // headless.)
    #[cfg(not(target_arch = "wasm32"))]
    if headless {
        #[cfg(feature = "api")]
        {
            app.add_plugins(lunco_workspace_api::WorkspaceApiQueriesPlugin);
            app.add_plugins(lunco_modelica_api::ModelicaApiQueriesPlugin);
        }
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / lunco_core_runtime::FIXED_HZ),
        ));
        info!("[lunica] running HEADLESS: Modelica compile core + API, no window/egui");
    }

    // Dismiss the HTML loading screen once the first frame paints (wasm-only;
    // no-op native). Pairs with `web/index.html` → `lunco-boot.js`.
    app.add_plugins(lunco_web::WebReadyPlugin);

    // HTTP automation bridge — native `--api` server / wasm JS bridge. Linked in
    // the GUI and the headless compile server alike (the latter's reason to exist).
    #[cfg(feature = "api")]
    app.add_plugins(lunco_api_transport::LunCoApiPlugin::default());

    // wasm-only boot wiring: request the explicitly selected external model, and hide
    // the HTML loader once the first egui frame has painted. The whole
    // web-workbench wiring depends on the `ui` feature (it touches
    // `lunco_modelica_ui::ui` / `lunco_workbench`), so it's gated on it too —
    // otherwise a `--no-default-features` wasm build (the check_wasm gate)
    // fails to compile.
    #[cfg(all(target_arch = "wasm32", feature = "ui"))]
    {
        app.init_resource::<WebWorkbenchRequest>()
            .add_systems(Update, request_web_workbench)
            .add_systems(Update, setup_web_workbench);
    }

    app.run();
}

/// Base [`DefaultPlugins`] for the chosen mode. The window / render / winit
/// backend must be decided at `PluginGroup` build time, so this is the one place
/// the GUI/headless split touches plugin configuration — mirrors
/// `lunco_luncosim::default_plugins`. The egui workbench vs. the compile core is
/// layered on top by `main` (the composition root).
fn default_plugins(headless: bool) -> bevy::app::PluginGroupBuilder {
    // A no-`ui` build is always headless, so the param is unused there.
    #[cfg(not(feature = "ui"))]
    let _ = headless;

    // Windowed title — native advertises the `--api` port (so side-by-side
    // instances are distinguishable); wasm is plain. Only used by the windowed
    // branch below.
    #[cfg(feature = "ui")]
    let window_title: String = {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let args: Vec<String> = std::env::args().collect();
            let mut api_port: Option<u16> = None;
            for i in 0..args.len() {
                if args[i] == "--api" {
                    api_port = Some(lunco_api_contracts::DEFAULT_API_PORT);
                    if i + 1 < args.len() {
                        if let Ok(p) = args[i + 1].parse::<u16>() {
                            api_port = Some(p);
                        }
                    }
                    break;
                }
            }
            match api_port {
                Some(p) => format!("Lunica — Listening on {p}"),
                None => "Lunica".to_string(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            "Lunica".to_string()
        }
    };

    #[cfg(feature = "ui")]
    let group = if headless {
        // `--no-ui` on a `ui` build: render + winit still link, so disable them
        // (backends:None, WinitPlugin off).
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: bevy::render::settings::WgpuSettings {
                    backends: None,
                    ..default()
                }
                .into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                close_when_requested: false,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .set(headless_log())
    } else {
        // Windowed: merged-titlebar chrome (native) / `#bevy` canvas (wasm).
        // Route the OS X-button through the in-app save-prompt flow
        // (`close_when_requested: false`) so dirty-doc dialogs aren't skipped.
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                #[cfg(target_arch = "wasm32")]
                canvas: Some("#bevy".to_string()),
                #[cfg(target_arch = "wasm32")]
                fit_canvas_to_parent: true,
                #[cfg(target_arch = "wasm32")]
                prevent_default_event_handling: true,
                ..lunco_workbench_window::restored_window(window_title)
            }),
            close_when_requested: false,
            ..default()
        })
    };

    // A `--no-default-features` build has neither render nor winit, so plain
    // DefaultPlugins degrades to no-render/no-window automatically.
    #[cfg(not(feature = "ui"))]
    let group = DefaultPlugins.set(headless_log());

    group.build()
}

/// Quietened log filter for the headless compile server (rumoca JIT + diffsol
/// per-step noise downgraded to warn). The windowed GUI keeps Bevy's default
/// `LogPlugin`.
fn headless_log() -> bevy::log::LogPlugin {
    bevy::log::LogPlugin {
        filter: "cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
        ..default()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn install_panic_hook() {
    let log_path = std::env::var_os("LUNICA_PANIC_LOG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| lunco_assets_core::temp_dir().join("lunica_panic.log"));
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let msg = format!(
            "\n===== panic @ {:?} =====\n{info}\nthread: {:?}\n{backtrace}\n",
            std::time::SystemTime::now(),
            std::thread::current().name().unwrap_or("<unnamed>"),
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
            let _ = f.flush();
        }
        eprint!("{msg}");
        default(info);
    }));
}

// ─────────────────────────────────────────────────────────────────────
// wasm-only boot helpers
// ─────────────────────────────────────────────────────────────────────

/// External model requested by the `?example=` URL parameter.
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
#[derive(Resource)]
struct BundledModelInfo {
    asset_path: String,
    source: Handle<ModelicaSource>,
}

#[cfg(all(target_arch = "wasm32", feature = "ui"))]
#[derive(Resource, Default)]
struct WebWorkbenchRequest {
    requested_name: Option<String>,
    resolved: bool,
}

#[cfg(all(target_arch = "wasm32", feature = "ui"))]
#[derive(Resource)]
struct WebWorkbenchStarted;

/// Resolve the explicitly named Modelica deep link against the runtime asset
/// manifest. The URL names a model, not a storage directory: the manifest is
/// the authority for which `.mo` assets exist in this bundle.
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
fn request_web_workbench(
    mut commands: Commands,
    server: Res<AssetServer>,
    manifest: Res<lunco_assets_runtime::discovery::AssetManifest>,
    mut request: ResMut<WebWorkbenchRequest>,
    started: Option<Res<WebWorkbenchStarted>>,
) {
    if started.is_some() || request.resolved {
        return;
    }
    if request.requested_name.is_none() {
        request.requested_name = web_sys::window()
            .and_then(|window| window.location().search().ok())
            .and_then(|search| {
                web_sys::UrlSearchParams::new_with_str(search.trim_start_matches('?'))
                    .ok()
                    .and_then(|params| params.get("example"))
            });
    }
    let Some(requested_name) = request.requested_name.clone() else {
        return;
    };
    if !manifest.ready() {
        return;
    }

    let requested_file = if requested_name.ends_with(".mo") {
        requested_name.to_owned()
    } else {
        format!("{requested_name}.mo")
    };
    let candidates: Vec<&String> = manifest
        .rels()
        .iter()
        .filter(|path| {
            Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mo"))
                && Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == requested_file)
        })
        .collect();
    request.resolved = true;

    let asset_path = match candidates.as_slice() {
        [asset_path] => (*asset_path).clone(),
        [] => {
            bevy::log::error!(
                "[lunica] Modelica deep link `{requested_name}` is not present in the runtime asset manifest"
            );
            commands.insert_resource(WebWorkbenchStarted);
            return;
        }
        _ => {
            bevy::log::error!(
                "[lunica] Modelica deep link `{requested_name}` is ambiguous in the runtime asset manifest"
            );
            commands.insert_resource(WebWorkbenchStarted);
            return;
        }
    };
    let path = match lunco_assets_core::asset_path::load_asset_path(&asset_path, None, None, None) {
        Ok(path) => path,
        Err(error) => {
            bevy::log::error!("[lunica] invalid Modelica deep link `{asset_path}`: {error}");
            commands.insert_resource(WebWorkbenchStarted);
            return;
        }
    };
    commands.insert_resource(BundledModelInfo {
        source: server.load(path),
        asset_path,
    });
}

/// Spawn the initial Modelica document after its external asset is loaded.
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
fn setup_web_workbench(
    mut commands: Commands,
    mut workbench_state: ResMut<lunco_modelica_ui::ui::workbench_state::WorkbenchState>,
    mut doc_registry: ResMut<
        lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>,
    >,
    mut model_tabs: ResMut<lunco_modelica_ui::model_tabs::ModelTabs>,
    model_info: Option<Res<BundledModelInfo>>,
    sources: Res<Assets<ModelicaSource>>,
    server: Res<AssetServer>,
    started: Option<Res<WebWorkbenchStarted>>,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
) {
    if started.is_some() {
        return;
    }
    let Some(model_info) = model_info else {
        return;
    };
    let Some(asset) = sources.get(&model_info.source) else {
        if server.load_state(&model_info.source).is_failed() {
            bevy::log::error!(
                "[lunica] external Modelica asset failed to load: {}",
                model_info.asset_path
            );
            commands.insert_resource(WebWorkbenchStarted);
        }
        return;
    };
    commands.insert_resource(WebWorkbenchStarted);
    let model_path = PathBuf::from(&model_info.asset_path);
    let source = asset.text.clone();
    let model_name = asset.interface.model_name.clone();
    let initial_params = asset.interface.parameters.clone();

    workbench_state.editor_buffer = source.clone();

    // Open the initial source through the generic file identity boundary so
    // the entity spawns with a valid document id and read-only origin.
    let (doc_id, _) = doc_registry.open_file_with_writable(model_path, source, false);
    if let Some(workspace) = workspace.as_deref_mut() {
        if let Some(host) = doc_registry.host(doc_id) {
            lunco_modelica_core::doc_ops::register_document_context(
                workspace,
                doc_id,
                host.document().origin().clone(),
                lunco_workspace::DocumentRuntimeOwner::Application,
                false,
            );
        }
    }

    let entity = model_name.map(|model_name| {
        let entity = commands
            .spawn((
                Name::new(model_name.clone()),
                ModelicaModel {
                    model_name,
                    parameters: initial_params,
                    paused: true,
                    document: doc_id,
                    ..default()
                },
            ))
            .id();
        doc_registry
            .link(entity, doc_id)
            .expect("initial Modelica document link");
        entity
    });
    // Open the model tab so the user lands on the model view.
    let tab_id = model_tabs.ensure_for(doc_id, None);
    commands.trigger(lunco_workbench_core::commands::OpenTab {
        kind: lunco_modelica_ui::ui::MODEL_VIEW_KIND.as_str().to_owned(),
        instance: tab_id,
    });

    // Select this entity so panels default to viewing it.
    workbench_state.selected_entity = entity;

    // No automatic compile on boot: the user clicks Compile when ready.
    // Avoids racing the source library fetch (lands seconds later on web).
}

/// Auto-opens the most recently opened Twin on native GUI startup.
#[cfg(all(feature = "ui", not(target_arch = "wasm32")))]
fn open_latest_twin_on_startup(
    workspace: Res<lunco_workspace::WorkspaceResource>,
    mut commands: Commands,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    *done = true;
    if let Some(latest) = workspace.recents.twin_paths.first() {
        let path = latest.to_string_lossy().into_owned();
        bevy::log::info!("[lunica] Auto-opening latest twin from recents: {}", path);
        commands.trigger(lunco_workspace::open::OpenFolder { path });
    }
}
