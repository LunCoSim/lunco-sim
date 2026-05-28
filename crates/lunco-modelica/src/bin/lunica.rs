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
//! the same unification `crates/lunco-client/src/bin/sandbox.rs` already
//! uses for its desktop+web entry. wasm-bindgen (`--target web`) runs
//! `main` automatically when the module loads, so there is no separate
//! `#[wasm_bindgen(start)]` entry point.
//!
//! Everything the workbench *is* (panels, clipboard, autosave, worker,
//! MSL) lives in [`ModelicaWorkbenchPlugin`]; this file is just the thin
//! app shell (window + frame pacing + native `--api`).

use bevy::prelude::*;
use lunco_modelica::ModelicaWorkbenchPlugin;

#[cfg(target_arch = "wasm32")]
use std::path::PathBuf;
#[cfg(target_arch = "wasm32")]
use lunco_modelica::{models::bundled_models, ModelicaModel};

fn main() {
    // ── Native-only: cap rayon's global pool ────────────────────────
    //
    // History: when projection + ast_refresh still ran on rayon, the
    // unconfigured pool grabbed `num_cpus - 1` threads and starved the
    // renderer's pipelined extract — every Add/Move edit froze the UI
    // for 1.5–2.5 s. After the SyntaxCache refactor those run on Bevy's
    // `AsyncComputeTaskPool`; the only remaining rayon caller is rumoca's
    // `parse_files_parallel` (short bursts at MSL preload / file load).
    // Policy: leave 2 cores for Bevy (renderer + main), give the rest to
    // rumoca. On ≤4-core machines still cap at 2 — the original
    // starvation problem dominates there. (No rayon on wasm.)
    #[cfg(not(target_arch = "wasm32"))]
    {
        let n_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let rayon_threads = if n_cpus <= 4 { 2 } else { n_cpus.saturating_sub(2) };
        match rayon::ThreadPoolBuilder::new()
            .num_threads(rayon_threads)
            .build_global()
        {
            Ok(()) => eprintln!(
                "[lunica] rayon global pool capped at {rayon_threads} threads (of {n_cpus} CPUs)"
            ),
            Err(e) => eprintln!("[lunica] WARN: rayon already initialised, our cap LOST: {e}"),
        }
    }

    // ── wasm-only: route panics to the browser console ──────────────
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    // ── Native-only: parse `--api <port>` so the window title can ────
    // advertise the listening port (automation drives the workbench via
    // it; visible in the title bar avoids confusion when several
    // instances run side-by-side — e.g. user on 3000 + a test on 3001).
    #[cfg(not(target_arch = "wasm32"))]
    let window_title: String = {
        let args: Vec<String> = std::env::args().collect();
        let mut api_port: Option<u16> = None;
        for i in 0..args.len() {
            if args[i] == "--api" {
                api_port = Some(3000);
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
    };

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
    #[cfg(target_arch = "wasm32")]
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

    // Physics fixed timestep: 60 Hz. Modelica stepping runs in
    // FixedUpdate so the worker receives a predictable per-tick dt.
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    // wasm: match the index.html backdrop so the first wgpu clear paints
    // the same dark colour the canvas already has — kills the gray flash
    // between wasm init resolving and the first egui frame. Native has no
    // host HTML page to colour-match against, so this is wasm-only.
    #[cfg(target_arch = "wasm32")]
    app.insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)));

    // Window. Native: centered desktop window with the merged-titlebar
    // chrome; route the OS X-button through the in-app save-prompt flow
    // (`close_when_requested: false`) so dirty-doc dialogs aren't skipped.
    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            resolution: bevy::window::WindowResolution::new(1600, 1000),
            position: WindowPosition::Centered(MonitorSelection::Primary),
            ..lunco_workbench::merged_titlebar_window(window_title)
        }),
        close_when_requested: false,
        ..default()
    }));

    // Window. Wasm: render into `<canvas id="bevy">` and mirror its
    // parent's CSS size; let egui see keyboard events too.
    #[cfg(target_arch = "wasm32")]
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Lunica".into(),
            resolution: bevy::window::WindowResolution::new(1280, 720),
            canvas: Some("#bevy".into()),
            fit_canvas_to_parent: true,
            prevent_default_event_handling: true,
            ..default()
        }),
        ..default()
    }));

    // The whole workbench: WorkbenchPlugin + ModelicaPlugin + (on wasm)
    // clipboard bridge, autosave, and off-thread worker. Same bundle the
    // sandbox embeds as its Design tab, so the two can't drift.
    app.add_plugins(ModelicaWorkbenchPlugin::default());

    // Dismiss the HTML loading screen once the first frame paints
    // (wasm-only; no-op on native). Pairs with `web/index.html` →
    // `lunco-boot.js`. See `lunco_web`.
    app.add_plugins(lunco_web::WebReadyPlugin);

    // Native-only HTTP automation bridge. The feature is off on wasm
    // (`--no-default-features` drops the axum/tokio stack), so the cfg is
    // false there and `lunco_api` isn't even linked.
    #[cfg(feature = "lunco-api")]
    app.add_plugins(lunco_api::LunCoApiPlugin::default());

    // Frame pacing — applies to BOTH targets now. (Web previously lacked
    // this, which is exactly why the "UI vanishes on zoom" bug surfaced
    // only on the web build.)
    //
    // Focused: Continuous lets vsync / requestAnimationFrame act as the
    // pacer so each Update lands on a real vblank. An independent
    // Reactive(1/60s) timer drifts against the real refresh and stalls
    // present every ~13 frames (the 5 Hz spike train). Unfocused:
    // ReactiveLowPower(1s) keeps fans quiet in the background.
    {
        use bevy::winit::{UpdateMode, WinitSettings};
        app.insert_resource(WinitSettings {
            focused_mode: UpdateMode::Continuous,
            unfocused_mode: UpdateMode::reactive_low_power(std::time::Duration::from_secs(1)),
        });
    }

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
    // the HTML loader once the first egui frame has painted.
    #[cfg(target_arch = "wasm32")]
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

// ─────────────────────────────────────────────────────────────────────
// wasm-only boot helpers
// ─────────────────────────────────────────────────────────────────────

/// Resource holding the default model info passed to the startup system.
#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct BundledModelInfo {
    default_filename: String,
    default_source: String,
}

/// Marker component for the initial workbench entity.
#[cfg(target_arch = "wasm32")]
#[derive(Component)]
struct WebWorkbench;

/// Spawns the initial Modelica document tab with the bundled model named
/// by `?example=`. Only registered when such a model was chosen.
#[cfg(target_arch = "wasm32")]
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
    layout.open_instance(
        lunco_modelica::ui::panels::model_view::MODEL_VIEW_KIND,
        tab_id,
    );

    // Select this entity so panels default to viewing it.
    workbench_state.selected_entity = Some(entity);

    // No automatic compile on boot: the user clicks Compile when ready.
    // Avoids racing the MSL fetch (lands seconds later on web).
    let _ = (entity, model_name, source, channels);
}
