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
//!     models via `include_str!`, supports `?example=<file>` deep-linking,
//!     and (via [`ModelicaWorkbenchPlugin`]) wires the JS clipboard
//!     bridge, localStorage autosave, and the off-thread compile worker.
//!
//! ## Why this used to be two files
//!
//! Desktop and web lived in separate sources (`lunica.rs` + `lunica_web.rs`)
//! and **drifted**: the web build silently lacked the `WinitSettings`
//! frame-pacing fix that cured the "UI vanishes on zoom" bug on native,
//! and the embedded-in-sandbox copy lacked clipboard + autosave. They are
//! now one `fn main()` with `#[cfg(target_arch = "wasm32")]` branches —
//! the same unification `crates/lunco-sandbox/src/bin/sandbox.rs` already
//! uses for its desktop+web entry. wasm-bindgen (`--target web`) runs
//! `main` automatically when the module loads, so there is no separate
//! `#[wasm_bindgen(start)]` entry point.
//!
//! Everything the workbench *is* (panels, clipboard, autosave, worker,
//! MSL) lives in [`ModelicaWorkbenchPlugin`]; this file is just the thin
//! app shell (window + frame pacing + native `--api`).

use bevy::prelude::*;
// The egui workbench shell — UI only. A headless `--no-ui` (or `--no-default-
// features`) lunica adds `ModelicaCorePlugin` (compile core) instead.
#[cfg(feature = "ui")]
use lunco_modelica::ModelicaWorkbenchPlugin;

#[cfg(all(target_arch = "wasm32", feature = "ui"))]
use lunco_modelica::{models::bundled_models, ModelicaModel};
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
use std::path::PathBuf;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    install_panic_hook();

    // ── Native-only: cap rumoca's compiler parallelism ──────────────
    //
    // History: when projection + ast_refresh still ran on rayon, the
    // unconfigured pool grabbed `num_cpus - 1` threads and starved the
    // renderer's pipelined extract — every Add/Move edit froze the UI
    // for 1.5–2.5 s. After the SyntaxCache refactor those run on Bevy's
    // `AsyncComputeTaskPool`; the only remaining global-pool caller is
    // rumoca (`parse_files_parallel` / `Session`, short bursts at MSL
    // preload / file load). Our indexer runs on its own bounded pool and
    // is unaffected either way.
    //
    // We USED to win the pool by racing rumoca to `build_global()`. That
    // was fragile (lose the race and the cap was silently lost) and it
    // handed the parse threads rayon's default 2 MB stacks — rumoca sizes
    // them at 16 MB for deep MSL class hierarchies. Since 0.9.20 rumoca
    // takes the thread count directly and builds the pool itself, so we
    // just state the policy: leave 2 cores for Bevy (renderer + main),
    // give the rest to rumoca. On ≤4-core machines cap at 2 — the original
    // starvation problem dominates there. (rumoca's own default reserves 4
    // cores but hands ALL cores to a ≤4-core box, which is the case we most
    // need to protect.) No rayon on wasm.
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

    // ── wasm-only: pick the bundled model to auto-open ──────────────
    //
    //   /                                 → open nothing (Welcome / restore)
    //   /?example=AnnotatedRocketStage.mo → AnnotatedRocketStage
    //   /?example=Battery                 → Battery (.mo auto-appended)
    //
    // Only auto-open a tab when the URL explicitly asks (`?example=`). On
    // a bare `/` we land on Welcome and let `wasm_autosave`'s restore
    // path reopen whatever the user had last. Unknown names warn and fall
    // back to the first bundled model so a stale bookmark doesn't break.
    #[cfg(all(target_arch = "wasm32", feature = "ui"))]
    let chosen: Option<lunco_modelica::models::BundledModel> = {
        let models = bundled_models();
        let fallback = models.first().expect("at least one bundled model");
        let url_example: Option<String> = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| {
                web_sys::UrlSearchParams::new_with_str(s.trim_start_matches('?'))
                    .ok()
                    .and_then(|p| p.get("example"))
            });
        url_example.as_deref().map(|name| {
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
                .copied()
                .unwrap_or_else(|| {
                    bevy::log::warn!(
                        "[lunica] ?example={name:?} not found in bundled models — \
                         falling back to {}",
                        fallback.filename
                    );
                    *fallback
                })
        })
    };

    let mut app = App::new();

    // Physics fixed timestep (lunco_core::FIXED_HZ). Modelica stepping runs in
    // FixedUpdate so the worker receives a predictable per-tick dt.
    app.insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ));

    // wasm: match the index.html backdrop so the first wgpu clear paints
    // the same dark colour the canvas already has — kills the gray flash
    // between wasm init resolving and the first egui frame. Native has no
    // host HTML page to colour-match against, so this is wasm-only.
    #[cfg(target_arch = "wasm32")]
    app.insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)));

    // ── Composition root ──────────────────────────────────────────────────
    // Base plugins (window / render / winit backend) are chosen by mode in
    // `default_plugins`; then we layer the egui workbench OR the headless
    // compile core on top — lunica's "UI plugin" is the modelica crate's
    // `ModelicaWorkbenchPlugin`, its "core plugin" the crate's
    // `ModelicaCorePlugin`. Mirrors `lunco_sandbox`'s Core/Ui/Headless split.
    app.add_plugins(default_plugins(headless));

    // GUI (native windowed, or wasm — always windowed). The whole workbench:
    // WorkbenchPlugin + ModelicaPlugin + clipboard, autosave, worker. Same
    // bundle the sandbox embeds as its Design tab.
    #[cfg(feature = "ui")]
    if !headless {
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
        app.add_plugins(lunco_modelica::ModelicaCorePlugin);
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / lunco_core::FIXED_HZ as f64),
        ));
        info!("[lunica] running HEADLESS: Modelica compile core + API, no window/egui");
    }

    // Dismiss the HTML loading screen once the first frame paints (wasm-only;
    // no-op native). Pairs with `web/index.html` → `lunco-boot.js`.
    app.add_plugins(lunco_web::WebReadyPlugin);

    // HTTP automation bridge — native `--api` server / wasm JS bridge. Linked in
    // the GUI and the headless compile server alike (the latter's reason to exist).
    #[cfg(feature = "lunco-api")]
    app.add_plugins(lunco_api::LunCoApiPlugin::default());

    // Cap FixedUpdate catchup after a slow frame. Bevy default: a 250ms
    // hitch breeds ~15 fixed ticks next frame, which makes that frame slow
    // too — a self-feeding cascade (the 5 Hz spike train). Capping
    // `Time<Virtual>` to 33ms ≈ 2 fixed ticks drops residual real time
    // instead of compounding it. `Time<Fixed>` reads its delta from
    // Virtual, so this transitively caps the catchup loop.
    {
        let mut virtual_time = Time::<Virtual>::default();
        virtual_time.set_max_delta(std::time::Duration::from_millis(33));
        app.insert_resource(virtual_time);
    }

    // wasm-only boot wiring: auto-open the chosen bundled model, and hide
    // the HTML loader once the first egui frame has painted. The whole
    // web-workbench wiring depends on the `ui` feature (it touches
    // `lunco_modelica::ui` / `lunco_workbench`), so it's gated on it too —
    // otherwise a `--no-default-features` wasm build (the check_wasm gate)
    // fails to compile.
    #[cfg(all(target_arch = "wasm32", feature = "ui"))]
    {
        if let Some(model) = chosen {
            app.insert_resource(BundledModelInfo {
                default_filename: model.filename.to_string(),
                default_source: model.source.to_string(),
            })
            .add_systems(Startup, setup_web_workbench);
        }
    }

    app.run();
}

/// Base [`DefaultPlugins`] for the chosen mode. The window / render / winit
/// backend must be decided at `PluginGroup` build time, so this is the one place
/// the GUI/headless split touches plugin configuration — mirrors
/// `lunco_sandbox::default_plugins`. The egui workbench vs. the compile core is
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
                    api_port = Some(lunco_core::session::DEFAULT_API_PORT);
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
                ..lunco_workbench::restored_window(window_title)
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
        .unwrap_or_else(|| lunco_assets::temp_dir().join("lunica_panic.log"));
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
    std::env::set_var("RUST_BACKTRACE", "1");
}

// ─────────────────────────────────────────────────────────────────────
// wasm-only boot helpers
// ─────────────────────────────────────────────────────────────────────

/// Resource holding the default model info passed to the startup system.
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
#[derive(Resource)]
struct BundledModelInfo {
    default_filename: String,
    default_source: String,
}

/// Marker component for the initial workbench entity.
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
#[derive(Component)]
struct WebWorkbench;

/// Spawns the initial Modelica document tab with the bundled model named
/// by `?example=`. Only registered when such a model was chosen.
#[cfg(all(target_arch = "wasm32", feature = "ui"))]
fn setup_web_workbench(
    mut commands: Commands,
    channels: Res<lunco_modelica::ModelicaChannels>,
    mut workbench_state: ResMut<lunco_modelica::state::WorkbenchState>,
    mut doc_registry: ResMut<lunco_modelica::state::ModelicaDocumentRegistry>,
    compile_states: ResMut<lunco_doc_bevy::DocumentDiagnostics>,
    mut model_tabs: ResMut<lunco_modelica::model_tabs::ModelTabs>,
    mut layout: ResMut<lunco_workbench::WorkbenchLayout>,
    model_info: Res<BundledModelInfo>,
) {
    let model_path = PathBuf::from(&model_info.default_filename);
    let source = model_info.default_source.clone();
    let model_name =
        lunco_modelica::extract_model_name(&source).unwrap_or_else(|| "Model".to_string());
    let initial_params = lunco_modelica::extract_parameters(&source);
    let initial_inputs = lunco_modelica::extract_inputs_with_defaults(&source);

    workbench_state.editor_buffer = source.clone();

    // Allocate the Document up-front so the entity spawns with a valid
    // `document` id. Record the bundled-asset origin for read-only
    // classification.
    let doc_id = doc_registry.allocate_with_origin(
        source.clone(),
        lunco_doc::DocumentOrigin::readonly_file(model_path.clone()),
    );

    let entity = commands
        .spawn((
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
        ))
        .id();

    doc_registry.link(entity, doc_id);
    // Leave CompileStates at Idle — setting Compiling here without a
    // matching `Compile` send would stick the toolbar on the sandglass.
    let _ = compile_states;

    // Open the model tab so the user lands on the model view.
    let tab_id = model_tabs.ensure_for(doc_id, None);
    layout.open_instance(lunco_modelica::ui::MODEL_VIEW_KIND, tab_id);

    // Select this entity so panels default to viewing it.
    workbench_state.selected_entity = Some(entity);

    // No automatic compile on boot: the user clicks Compile when ready.
    // Avoids racing the MSL fetch (lands seconds later on web).
    let _ = (entity, model_name, source, channels);
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
        commands.trigger(lunco_workbench::file_ops::OpenFolder { path });
    }
}
