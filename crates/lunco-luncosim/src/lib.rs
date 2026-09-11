//! The LunCo luncosim application — ground mobility + physics, loaded from USD.
//!
//! [`run`] builds and runs the app. It is the single shared entry point for BOTH
//! binaries:
//!   - `luncosim` (this crate, default `ui` feature) — the windowed GUI;
//!   - `luncosim-server` (the `lunco-luncosim-server` crate, no `ui`) — headless.
//!
//! ## Architecture: composition root, not a UI host
//!
//! The app is three named plugins, composed by a tiny shell — mirroring how the
//! library crates split into core modules + a `*UiPlugin`:
//!   - [`LunCoSimCorePlugin`] — sim / physics / cosim / USD / networking / API.
//!     Headless-safe, added unconditionally.
//!   - [`lunco_luncosim_ui::LunCoSimUiPlugin`] (`ui` feature) — egui workbench, picking, the
//!     in-scene editor, materials, panels, and explicit camera controls. Added only when
//!     running windowed.
//!   - [`LunCoSimHeadlessPlugin`] — the `ScheduleRunner` + the Modelica/spawn
//!     cores a server needs in the UI plugin's place. Added only when headless.
//!
//! GUI = `LunCoSimCorePlugin + LunCoSimUiPlugin`; headless =
//! `LunCoSimCorePlugin + LunCoSimHeadlessPlugin`. Both bins compose the SAME
//! `LunCoSimCorePlugin`, so they can never drift. The only place the GUI/headless
//! decision touches plugin *configuration* is [`default_plugins`] (the window /
//! render / winit backend must be chosen at `PluginGroup` build time) — that is
//! inherently a shell concern.

// glibc's allocator serialises cross-thread allocations through a
// shared arena lock; with avian's contact graph allocating heavily on
// a parallel task pool every fixed tick, the main render thread paid
// a tail-latency penalty on every alloc. mimalloc uses per-thread
// heaps and a lock-free fast path, removing the contention. Native
// only — wasm has its own allocator pipeline.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use avian3d::prelude::PhysicsPlugins;
use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;
use big_space::prelude::*;

use lunco_hardware::LunCoHardwarePlugin;
use lunco_mobility::LunCoMobilityPlugin;
// USD core (scene load + collider build) is always needed; the Twin browser /
// RTT viewport UI plugins are `ui`-only (added by `LunCoSimUiPlugin`).
#[cfg(feature = "networking")]
use lunco_usd::LoadScene;
use lunco_usd::UsdPlugins;
use lunco_usd_bevy_scene::UsdPrimPath;
// USD policy and terrain presentation read the composed reader selected by the
// shared USD projection boundary. Initial scene loads use the worker-produced
// plan; authored generations use the live canonical stage. `UsdDataExt` remains
// the separate authored-layer surface for document questions.
use bevy::asset::AssetLoadFailedEvent;
use lunco_usd_bevy_core::read::UsdReadObject;
use lunco_usd_bevy_core::UsdStageAsset;

/// Re-exported so the (bevy-free) bin crates can return it from `main` to
/// propagate the process exit code (e.g. the startup-scene fail-loud guard).
pub use bevy::app::AppExit;
/// SemVer2 product version stamped into this build. Release builds may carry a
/// CI-derived nightly version while Cargo.toml keeps the stable package base.
pub const PRODUCT_VERSION: &str = env!("LUNCO_RELEASE_VERSION");
/// Short source revision stamped into this build for diagnostics.
pub const GIT_SHA: &str = env!("LUNCO_GIT_SHA");
/// Public GitHub repository containing the stamped source revision.
pub const REPOSITORY_URL: &str = env!("LUNCO_REPOSITORY_URL");

use lunco_avatar::LunCoAvatarPlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_cosim::systems::apply_forces::CosimSet as ApplyForcesCosimSet;
use lunco_cosim::systems::propagate::CosimSet as PropagateCosimSet;
use lunco_cosim::CoSimPlugin;
use lunco_environment::EnvironmentPlugin;
use lunco_obstacle_field::ObstacleFieldPlugin;
use lunco_terrain_globe::TerrainPlugin;
use lunco_terrain_surface::TerrainSurfacePlugin;
// `ModelicaSet` orders the cosim pipeline (always). The egui workbench plugin is
// added by `LunCoSimUiPlugin`; headless adds `ModelicaCorePlugin` instead.
use lunco_modelica_core::ModelicaSet;

/// Chassis smoothness census (`LUNCO_JITTER_CSV`) — compares solver `Position`
/// against the rendered `Transform`, so it only means anything in a `ui` build.
#[cfg(feature = "ui")]
mod jitter_probe;
/// Collapse repeated WARN/ERROR log lines into one line + a count (§6.4).
mod log_dedup;
/// OS `luncosim://` scheme registration (desktop integration). Native + the
/// networking feature only — there's nothing to dial without the wire.
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod url_scheme;

/// Headless authored-scene regression runner, also exposed by
/// `luncosim test`. Keeping the implementation in the luncosim crate
/// lets the standalone test binary and the production CLI use one runner.
#[cfg(not(target_family = "wasm"))]
#[path = "debug_scene.rs"]
pub mod debug_scene;

/// Run the luncosim, choosing GUI vs. headless from the build + flags: headless
/// when the `ui` feature is absent, or `--no-ui` / `LUNCO_NO_UI` is set;
/// otherwise the windowed GUI. This is the `luncosim` GUI bin's entry point.
pub fn run() -> AppExit {
    let headless = !cfg!(feature = "ui")
        || std::env::args().any(|a| a == "--no-ui")
        || std::env::var("LUNCO_NO_UI").is_ok_and(|v| v != "0" && !v.is_empty());
    run_with_mode(headless)
}

/// Run LunCoSim HEADLESS, unconditionally — the `luncosim-server` bin's entry
/// point. Forcing the mode here (rather than inferring it from the absent `ui`
/// feature) makes the server stay windowless **even if `ui` gets unified on** by
/// a `cargo build --workspace` (which compiles the GUI `luncosim` bin alongside
/// it). So the server never tries to open a window; in a lean `-p
/// lunco-luncosim-server` build the GUI stack isn't linked at all.
pub fn run_headless() -> AppExit {
    run_with_mode(true)
}

/// The luncosim's process-start render choice. The binary selects it while
/// `lunco-render-bevy` owns how the policy is rendered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LunCoSimRenderProfile {
    #[default]
    Standard,
    Fast,
}

fn parse_render_profile(args: &[String]) -> Result<LunCoSimRenderProfile, String> {
    let mut profile = LunCoSimRenderProfile::Standard;
    let mut index = 0;
    while index < args.len() {
        let value = if args[index] == "--render-profile" {
            index += 1;
            args.get(index)
                .map(String::as_str)
                .ok_or_else(|| "`--render-profile` needs one of: standard, fast".to_string())?
        } else if let Some(value) = args[index].strip_prefix("--render-profile=") {
            value
        } else {
            index += 1;
            continue;
        };
        profile = match value {
            "standard" => LunCoSimRenderProfile::Standard,
            "fast" => LunCoSimRenderProfile::Fast,
            _ => {
                return Err(format!(
                    "invalid render profile `{value}`; expected `standard` or `fast`"
                ));
            }
        };
        index += 1;
    }
    Ok(profile)
}

/// Parse the explicit Graphics quality override used by GPU-backed runs.
///
/// The persisted Graphics section remains the default when this flag is absent.
/// A command-line override is useful for render tests and recordings because it
/// makes their visual budget part of the invocation instead of depending on the
/// developer's settings file.
fn parse_render_quality(args: &[String]) -> Result<Option<lunco_render::RenderingQuality>, String> {
    let mut quality = None;
    let mut index = 0;
    while index < args.len() {
        let value = if args[index] == "--render-quality" {
            index += 1;
            args.get(index)
                .map(String::as_str)
                .ok_or_else(|| "`--render-quality` needs low, balanced, or high".to_string())?
        } else if let Some(value) = args[index].strip_prefix("--render-quality=") {
            value
        } else {
            index += 1;
            continue;
        };
        quality = Some(match value {
            "low" => lunco_render::RenderingQuality::Low,
            "balanced" => lunco_render::RenderingQuality::Balanced,
            "high" => lunco_render::RenderingQuality::High,
            _ => {
                return Err(format!(
                    "invalid render quality `{value}`; expected `low`, `balanced`, or `high`"
                ));
            }
        });
        index += 1;
    }
    Ok(quality)
}

#[cfg(feature = "ui")]
fn parse_record_preset(
    args: &[String],
) -> Result<lunco_workbench::screenshot::OfflineVideoPreset, String> {
    let mut preset = lunco_workbench::screenshot::OfflineVideoPreset::default();
    let mut index = 0;
    while index < args.len() {
        let value = if args[index] == "--record-preset" {
            index += 1;
            args.get(index).map(String::as_str).ok_or_else(|| {
                "`--record-preset` needs ultrafast, veryfast, or medium".to_string()
            })?
        } else if let Some(value) = args[index].strip_prefix("--record-preset=") {
            value
        } else {
            index += 1;
            continue;
        };
        preset = lunco_workbench::screenshot::OfflineVideoPreset::parse(value)?;
        index += 1;
    }
    Ok(preset)
}

/// Read the one explicit startup-scene argument, if present.
///
/// Startup has no scene default. Keeping this parser pure makes the empty-shell
/// policy testable without constructing the renderer or a Bevy app.
fn startup_scene_arg(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == "--scene")
        .map(|pair| pair[1].clone())
}

#[cfg(test)]
mod startup_scene_tests {
    use super::startup_scene_arg;

    #[test]
    fn no_scene_argument_keeps_startup_empty() {
        let args = [
            "luncosim".to_string(),
            "--api".to_string(),
            "5544".to_string(),
        ];
        assert_eq!(startup_scene_arg(&args), None);
    }

    #[test]
    fn explicit_scene_argument_is_preserved_verbatim() {
        let args = [
            "luncosim".to_string(),
            "--scene".to_string(),
            "/tmp/mission.usda".to_string(),
        ];
        assert_eq!(
            startup_scene_arg(&args),
            Some("/tmp/mission.usda".to_string())
        );
    }

    #[test]
    fn missing_scene_value_does_not_create_a_default() {
        let args = ["luncosim".to_string(), "--scene".to_string()];
        assert_eq!(startup_scene_arg(&args), None);
    }
}

#[cfg(test)]
mod fixed_step_budget_tests {
    use std::time::Duration;

    #[test]
    fn low_rates_keep_the_normal_virtual_delta_cap() {
        assert_eq!(
            lunco_time::fixed_step_raw_delta_limit(1.0, Duration::from_secs_f64(1.0 / 60.0)),
            lunco_time::BASE_VIRTUAL_MAX_DELTA
        );
        assert_eq!(
            lunco_time::fixed_step_raw_delta_limit(4.0, Duration::from_secs_f64(1.0 / 60.0)),
            lunco_time::BASE_VIRTUAL_MAX_DELTA
        );
    }

    #[test]
    fn eight_x_is_bounded_to_the_fixed_step_budget_per_raw_frame() {
        let fixed = Duration::from_secs_f64(1.0 / 60.0);
        let limit = lunco_time::fixed_step_raw_delta_limit(8.0, fixed);
        let requested_fixed = limit.as_secs_f64() * 8.0 / fixed.as_secs_f64();
        assert!(
            requested_fixed <= lunco_time::MAX_FIXED_STEPS_PER_FRAME as f64 + 1e-9,
            "raw cap requests {requested_fixed} fixed ticks"
        );
    }

    #[test]
    fn sixteen_x_is_bounded_to_the_fixed_step_budget_per_raw_frame() {
        let fixed = Duration::from_secs_f64(1.0 / 60.0);
        let limit = lunco_time::fixed_step_raw_delta_limit(16.0, fixed);
        let requested_fixed = limit.as_secs_f64() * 16.0 / fixed.as_secs_f64();
        assert!(
            requested_fixed <= lunco_time::MAX_FIXED_STEPS_PER_FRAME as f64 + 1e-9,
            "raw cap requests {requested_fixed} fixed ticks"
        );
    }

    #[test]
    fn sixty_four_x_stays_within_the_fixed_step_budget() {
        assert_eq!(
            lunco_time::fixed_step_raw_delta_limit(64.0, Duration::from_secs_f64(1.0 / 60.0)),
            Duration::from_secs_f64(1.0 / 60.0)
        );
    }
}

#[cfg(test)]
mod render_profile_tests {
    use super::*;

    #[test]
    fn parses_fast_profile_in_both_cli_forms() {
        for args in [
            vec![
                "luncosim".to_string(),
                "--render-profile".to_string(),
                "fast".to_string(),
            ],
            vec!["luncosim".to_string(), "--render-profile=fast".to_string()],
        ] {
            assert_eq!(parse_render_profile(&args), Ok(LunCoSimRenderProfile::Fast));
        }
    }

    #[test]
    fn rejects_an_unknown_render_profile() {
        assert!(parse_render_profile(&[
            "luncosim".to_string(),
            "--render-profile=turbo".to_string()
        ])
        .is_err());
    }

    #[test]
    fn parses_render_quality_in_both_cli_forms() {
        assert_eq!(
            parse_render_quality(&[
                "luncosim".to_string(),
                "--render-quality".to_string(),
                "high".to_string(),
            ]),
            Ok(Some(lunco_render::RenderingQuality::High))
        );
        assert_eq!(
            parse_render_quality(&[
                "luncosim".to_string(),
                "--render-quality=balanced".to_string(),
            ]),
            Ok(Some(lunco_render::RenderingQuality::Balanced))
        );
    }

    #[test]
    fn absent_render_quality_keeps_the_persisted_setting() {
        assert_eq!(parse_render_quality(&["luncosim".to_string()]), Ok(None));
    }

    #[test]
    fn rejects_an_unknown_render_quality() {
        assert!(parse_render_quality(&[
            "luncosim".to_string(),
            "--render-quality=ultra".to_string(),
        ])
        .is_err());
    }

    #[cfg(feature = "ui")]
    #[test]
    fn explicit_render_quality_replaces_the_existing_settings_resource() {
        let mut app = App::new();
        app.insert_resource(lunco_render::RenderingQualitySettings::default());

        apply_render_quality_override(&mut app, Some(lunco_render::RenderingQuality::High));

        assert_eq!(
            app.world()
                .resource::<lunco_render::RenderingQualitySettings>()
                .profile(),
            lunco_render::RenderingQuality::High.profile()
        );
    }
}

/// Usage text for `--help`. Every flag here is one the binary ACTUALLY parses,
/// and they are spread across crates — this crate (`--no-ui`, `--api`, `--scene`,
/// `--no-vsync`, `--log-diag`), `ui::mod` (`--no-throttle`),
/// `lunco_networking::NetworkMode::from_args` (`--host`, `--connect`),
/// `lunco_networking::server::resolve_cert_paths` (`--cert`, `--key`) and
/// `lunco_workbench::window_placement` (`--window-pos`). Grep all of them before
/// editing this: an undocumented flag is invisible, and a documented flag that
/// nothing parses is a lie.
#[cfg(not(target_family = "wasm"))]
fn help_text() -> String {
    let api = lunco_core::session::DEFAULT_API_PORT;
    let net = lunco_core::session::DEFAULT_HOST_PORT;
    format!(
        "\
luncosim — the LunCoSim lunar simulator.

USAGE:
    luncosim [FLAGS]
    luncosim rhai [--api PORT] [-e SNIPPET | -f FILE]

FLAGS:
    -h, --help           Print this help and exit.
        --no-ui          Run headless (no window). Also via LUNCO_NO_UI=1.
        --api [PORT]     Serve the HTTP command API (default {api}). NOT implied
                         by --no-ui: without this flag there is no API port.
                         POST /api/commands  {{\"type\":\"ExecuteCommand\",\"command\":\"Name\",\"params\":{{…}}}}
        --scene PATH     Load this USD scene at startup. PATH may be relative to
                         assets/, relative to the current directory, or absolute.
                         Without --scene, start with an empty persistent world
                         shell; the sandbox is an explicit scene/test fixture.
        --window-pos SPEC  Place the OS window, e.g. 1920x1080+0+0.
        --validate PATH…   Pre-flight-check asset files (.mo/.usda/.wgsl/.rhai/.xml):
                         parse-only, no window/GPU/app. Prints a report and
                         exits 0 (all ok) or 1 (any failed).

RECORDING:
        --record-offline <dir|out.mp4>
                         Record deterministic frames once the scene is ready:
                         a directory gets a PNG sequence; an .mp4/.mkv/.mov
                         path streams straight into ffmpeg (falls back to a
                         PNG sequence, loudly, if ffmpeg is not installed).
        --record-fps N   Recording output frame rate (default 60).
        --record-preset MODE
                         Direct-video H.264 preset (default ultrafast; use
                         veryfast or medium for archival capture files).
        --record-frames N
                         Stop the recording automatically after N frames.
        --offscreen      GPU-full windowless recording: no window opens, the
                         scene renders into an offscreen target and the process
                         exits when the recording drains. Use with
                         --record-offline [--record-frames] for a one-command
                         take.
        --record-size WxH
                         Offscreen render-target resolution (default 1280x720,
                         the windowed default).

NETWORKING:
        --host [PORT]    Host a session over WebTransport (default {net}).
        --connect ADDR   Join a hosted session (ADDR without a port ⇒ :{net}).
                         A bare IP skips TLS validation (LAN/dev).
        --cert PATH      TLS cert for --host: a certbot live dir, or a file
                         (then --key, else the sibling privkey.pem). Omit both
                         for a dev self-signed cert.
        --key PATH       TLS private key, when --cert names a file.

PERFORMANCE:
        --render-profile MODE
                         `standard` (default) preserves authored PBR rendering;
                         `fast` uses unlit, texture-free materials and disables
                         HDR, bloom and MSAA. Startup-only; restart to change it.
        --render-quality MODE
                         Graphics quality override: `low`, `balanced`, or `high`.
                         `high` is the highest shipped quality: it raises the
                         shadow, sky cubemap, lunar terrain, LOD and tessellation
                         budgets. It applies to GPU-backed runs only and does not
                         replace the shaders authored by the USD scene.
        --no-vsync       Uncap the frame rate (present without vsync).
        --no-throttle    Keep running at full rate while unfocused.
        --headless-max-speed
                         With --no-ui, run the fixed simulation lattice as fast
                         as CPU and causal participants permit. This changes
                         wall-clock execution only; it does not fake a transport
                         rate or bypass the co-simulation barrier.
        --log-diag       Log FPS / frame-time / physics diagnostics.

SUBCOMMAND:
    rhai                 REPL client against a RUNNING instance's --api port.
                         Reads stdin, or -e SNIPPET / -f FILE for one-shot.
    test --scene PATH    Run an authored scene's Rhai regression test headless.
                         Deterministic; exits 0=PASS, 1=FAIL, 2=no verdict.

Measuring FPS? Use --no-vsync --no-throttle, else you are timing the
compositor and the unfocused power-save throttle, not the renderer.",
    )
}

/// Handle `--help`/`-h` BEFORE the app is built: print usage and exit. It has to
/// come first — building the app opens a window, spins up the GPU and loads a
/// scene, which is why `luncosim --help` used to launch the simulator instead of
/// answering the question.
#[cfg(not(target_family = "wasm"))]
fn print_help_if_requested() -> bool {
    if std::env::args().skip(1).any(|a| a == "--help" || a == "-h") {
        println!("{}", help_text());
        return true;
    }
    false
}

/// Composition root. Builds the shared core, then conditionally layers on the UI
/// or the headless runner. Nothing UI-specific lives here beyond selecting the
/// windowing backend in [`default_plugins`].
/// The first line of every run: which build is this?
///
/// A tester's log is only useful if it names the binary that produced it. Without this,
/// five Windows runs in the 2026-07-26 report could be distinguished only by install
/// path and asset counts — so the report groups them by inference instead of by fact,
/// and two of its findings could not be attributed to a build at all.
///
/// Printed with `println!` rather than `info!` because it must survive `RUST_LOG`
/// filtering and precede `LogPlugin` — a build identity that a log level can suppress is
/// exactly as useless as none.
fn log_build_identity(
    headless: bool,
    offscreen: bool,
    execution_mode: lunco_core::SimulationExecutionMode,
) {
    let mode = if headless {
        match execution_mode {
            lunco_core::SimulationExecutionMode::Realtime => "headless",
            lunco_core::SimulationExecutionMode::MaxSpeed => "headless-max-speed",
        }
    } else if offscreen {
        "offscreen"
    } else {
        "windowed"
    };
    println!(
        "[lunco] luncosim {ver} ({sha}) {profile} {mode} {os}/{arch}",
        ver = PRODUCT_VERSION,
        sha = GIT_SHA,
        profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    );
}

fn run_with_mode(headless: bool) -> AppExit {
    // `--offscreen`: GPU-FULL windowless recording mode. Real render stack and
    // visuals, no window/winit/egui — the scene renders into an offscreen target
    // image and the offline recorder captures that. Only meaningful in a `ui`
    // build (it needs the render stack) and mutually exclusive with headless
    // (which is the no-GPU server); headless wins if both are given.
    let offscreen = cfg!(all(feature = "ui", feature = "lunco-api"))
        && !headless
        && std::env::args().any(|a| a == "--offscreen");
    let args: Vec<String> = std::env::args().collect();
    let max_speed_requested = args.iter().any(|arg| arg == "--headless-max-speed");
    if max_speed_requested && !headless {
        eprintln!(
            "luncosim: --headless-max-speed requires --no-ui or the luncosim-server launcher"
        );
        return AppExit::error();
    }
    let execution_mode = if max_speed_requested {
        lunco_core::SimulationExecutionMode::MaxSpeed
    } else {
        lunco_core::SimulationExecutionMode::Realtime
    };
    let render_profile = match parse_render_profile(&args) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("luncosim: {error}");
            return AppExit::error();
        }
    };
    let render_quality = match parse_render_quality(&args) {
        Ok(quality) => quality,
        Err(error) => {
            eprintln!("luncosim: {error}");
            return AppExit::error();
        }
    };
    #[cfg(feature = "ui")]
    let record_preset = match parse_record_preset(&args) {
        Ok(preset) => preset,
        Err(error) => {
            eprintln!("luncosim: {error}");
            return AppExit::error();
        }
    };
    log_build_identity(headless, offscreen, execution_mode);
    // Answer `--help` without building an app (see `print_help_if_requested`).
    // Placed in the composition root, not in one bin's `main`, so EVERY entry
    // point that runs LunCoSim — GUI `luncosim`, headless `luncosim-server` —
    // gets the same usage for free and they cannot drift apart.
    #[cfg(not(target_family = "wasm"))]
    if print_help_if_requested() {
        return AppExit::Success;
    }
    // Native deep-link single-instance gate (GUI only). Register the
    // `luncosim://` scheme handler (desktop integration, this crate), then decide
    // whether THIS process is the app or just a courier forwarding a clicked link
    // to an already-running instance. Must happen before building the app so a
    // forward exits without opening a window. The returned inbox is inserted
    // below; a Bevy system drains it into the confirm prompt. Headless skips it.
    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    let deeplink_inbox = if !headless && !offscreen {
        use lunco_networking::single_instance::{acquire, LaunchOutcome};
        url_scheme::register_best_effort();
        match acquire() {
            // This process is just a courier — it forwarded the link to the
            // running instance and has nothing to run, so exit cleanly.
            LaunchOutcome::Forwarded => return AppExit::Success,
            LaunchOutcome::Primary(inbox) => Some(inbox),
        }
    } else {
        None
    };

    let mut app = build_sim_app_with_profile(headless, offscreen, None, render_profile);

    #[cfg(all(
        feature = "lunco-api",
        feature = "transport-http",
        not(target_arch = "wasm32")
    ))]
    if let Some(error) = app
        .world_mut()
        .remove_resource::<lunco_api::transports::HttpServerStartupError>()
    {
        eprintln!(
            "luncosim: cannot start HTTP API on 127.0.0.1:{}: {}",
            error.port, error.message
        );
        return AppExit::error();
    }

    #[cfg(feature = "ui")]
    app.insert_resource(lunco_workbench::screenshot::OfflineVideoSettings {
        preset: record_preset,
    });

    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    if let Some(inbox) = deeplink_inbox {
        app.insert_resource(inbox);
    }

    #[cfg(feature = "ui")]
    if !headless && !offscreen {
        app.insert_resource(lunco_luncosim_ui::WindowIconBytes(
            lunco_luncosim_ui::window_icon_bytes(),
        ));
        app.add_plugins(lunco_luncosim_ui::LunCoSimUiPlugin {
            config: lunco_luncosim_ui::LunCoSimUiConfig {
                product_version: PRODUCT_VERSION,
                git_sha: GIT_SHA,
                repository_url: REPOSITORY_URL,
                initial_scene: app.world().resource::<ScenePath>().0.clone(),
            },
        });
    }

    #[cfg(all(feature = "ui", feature = "lunco-api"))]
    if offscreen {
        app.add_plugins(lunco_luncosim_ui::LunCoSimOffscreenPlugin);
    }

    if headless {
        app.add_plugins(LunCoSimHeadlessPlugin { execution_mode });
    }

    apply_render_quality_override(&mut app, render_quality);

    // Return the AppExit so a non-zero exit (e.g. the startup-scene fail-loud
    // guard's `AppExit::error()`) propagates to the process exit code.
    app.run()
}

/// Apply a process-level quality choice after all render plugins have initialized
/// their settings resource. Omitting the flag leaves the persisted Graphics choice
/// (or the renderer's documented default) unchanged; supplying it is an explicit
/// test/recording contract.
#[cfg(feature = "ui")]
fn apply_render_quality_override(app: &mut App, quality: Option<lunco_render::RenderingQuality>) {
    let Some(quality) = quality else {
        return;
    };
    let Some(mut settings) = app
        .world_mut()
        .get_resource_mut::<lunco_render::RenderingQualitySettings>()
    else {
        warn!(
            "[render] --render-quality={} requested for a headless run; no GPU settings resource exists",
            quality.label()
        );
        return;
    };

    settings.apply_preset(quality);
    info!(
        "[render] explicit quality override enabled: {}",
        quality.label()
    );
}

#[cfg(not(feature = "ui"))]
fn apply_render_quality_override(_app: &mut App, _quality: Option<lunco_render::RenderingQuality>) {
}

/// Build the base [`DefaultPlugins`] group for the chosen mode.
///
/// This is the one place the GUI/headless split touches plugin *configuration*.
/// The render backend and the window must be decided at `PluginGroup`
/// build time — a plugin added later cannot reconfigure `RenderPlugin`/
/// `WindowPlugin`. Headless builds retain the simulation's asset/type plugins,
/// but omit the renderer and its render-world consumers entirely. The
/// [`ScheduleRunnerPlugin`] added by [`LunCoSimHeadlessPlugin`] ticks the app in
/// winit's place.
/// The luncosim's gravity before any scene is loaded, and the value scene
/// teardown restores when one unloads.
///
/// Lunar: every vehicle the luncosim ships is sized for 1.62 — the rovers'
/// drivetrains, the lander's struts and its propellant budget. A scene states
/// its own through `UsdPhysicsScene`; this is what an empty viewport uses.
pub const SANDBOX_GRAVITY: lunco_environment::Gravity = lunco_environment::Gravity::flat(
    lunco_environment::MOON_SURFACE_GRAVITY,
    bevy::math::DVec3::NEG_Y,
);

/// Build the production simulation app: asset sources, engine plugins, and every
/// LunCo domain system. Every binary that runs the simulation uses this path, so
/// the GUI, headless server, and scene-test runner share one composition. The
/// interactive UI is layered on by `run`; render-capable builds register the UI
/// package's presentation bridges at this assembly boundary, while headless builds
/// omit them.
///
/// **This exists because assembling it by hand is a trap.** Asset-source
/// registration must happen before `AssetPlugin`, which snapshots the source
/// registry when it is built. Missing that ordering leaves `lunco://`/`twin://`
/// unresolved or produces a distant `TwinRoots` resource validation failure.
pub fn build_sim_app(headless: bool, offscreen: bool) -> App {
    build_sim_app_with_profile(headless, offscreen, None, LunCoSimRenderProfile::Standard)
}

/// Build the production simulation app with an optional fixed compute-pool size.
/// Scene tests use this to pin physics deterministically while retaining the
/// exact asset-source, logging, and plugin composition used by the luncosim.
pub fn build_sim_app_with_threads(
    headless: bool,
    offscreen: bool,
    compute_threads: Option<usize>,
) -> App {
    build_sim_app_with_profile(
        headless,
        offscreen,
        compute_threads,
        LunCoSimRenderProfile::Standard,
    )
}

/// Open the high-precision propagation set only when its output can change.
///
/// BigSpace's propagation system already prunes clean subtrees, but its
/// channeled implementation still creates a compute scope and walks every grid
/// on each PostUpdate before it can discover that all subtrees are clean. The
/// application owns the schedule boundary, while BigSpace remains the sole
/// owner of propagation and its exact per-entity rules. This condition mirrors
/// only the authoritative inputs that can invalidate a high-precision global
/// transform; a false positive costs one ordinary propagation pass, while a
/// false negative would leave rendered GlobalTransforms stale.
fn high_precision_propagation_due(
    grids: Query<&Grid>,
    changed_spatial: Query<
        (),
        (
            With<CellCoord>,
            Without<Stationary>,
            Or<(Changed<Transform>, Changed<CellCoord>, Changed<ChildOf>)>,
        ),
    >,
    changed_grid_children: Query<(), (With<Grid>, Changed<Children>)>,
    uninitialized_stationary: Query<(), (With<Stationary>, Without<StationaryInitialized>)>,
) -> bool {
    grids
        .iter()
        .any(|grid| !grid.local_floating_origin().is_local_origin_unchanged())
        || !changed_spatial.is_empty()
        || !changed_grid_children.is_empty()
        || !uninitialized_stationary.is_empty()
}

/// Open BigSpace's local-floating-origin walk only when its reference-frame
/// inputs can change.
fn local_origin_propagation_due(
    changed_origin_cell: Query<(), (With<FloatingOrigin>, Changed<CellCoord>)>,
    changed_hierarchy: Query<(), Or<(Changed<ChildOf>, Changed<Children>)>>,
    added_origin: Query<(), Added<FloatingOrigin>>,
    added_grid: Query<(), Added<Grid>>,
    added_big_space: Query<(), Added<BigSpace>>,
) -> bool {
    !changed_origin_cell.is_empty()
        || !changed_hierarchy.is_empty()
        || !added_origin.is_empty()
        || !added_grid.is_empty()
        || !added_big_space.is_empty()
}

/// Open BigSpace's low-precision walk only when a local transform hierarchy or
/// an upstream high-precision root changed.
fn low_precision_propagation_due(
    changed_transforms: Query<(), Or<(Changed<Transform>, Added<Transform>)>>,
    changed_hierarchy: Query<(), Or<(Changed<ChildOf>, Changed<Children>)>>,
    changed_global_transforms: Query<(), Or<(Changed<GlobalTransform>, Added<GlobalTransform>)>>,
    mut removed_transforms: RemovedComponents<Transform>,
    mut removed_hierarchy: RemovedComponents<ChildOf>,
    mut removed_global_transforms: RemovedComponents<GlobalTransform>,
) -> bool {
    !changed_transforms.is_empty()
        || !changed_hierarchy.is_empty()
        || !changed_global_transforms.is_empty()
        || removed_transforms.read().next().is_some()
        || removed_hierarchy.read().next().is_some()
        || removed_global_transforms.read().next().is_some()
}

/// Keep BigSpace's debug hierarchy validator authoritative while avoiding a
/// full-tree walk on every settled frame. Hierarchy validity can change only
/// when the spatial component set or parent/child topology changes; transform
/// value changes do not alter which node kind an entity is.
#[cfg(debug_assertions)]
fn gate_big_space_hierarchy_validation(app: &mut App) {
    let removed = app
        .remove_systems_in_set(
            PostUpdate,
            big_space::validation::validate_hierarchy::<big_space::validation::SpatialHierarchyRoot>,
            bevy::ecs::schedule::ScheduleCleanupPolicy::RemoveSystemsOnly,
        )
        .expect("BigSpace hierarchy validator must be installed by BigSpaceDefaultPlugins");
    assert_eq!(
        removed, 1,
        "BigSpace hierarchy validator must be installed once"
    );
    app.add_systems(
        PostUpdate,
        big_space::validation::validate_hierarchy::<big_space::validation::SpatialHierarchyRoot>
            .run_if(big_space_hierarchy_validation_due)
            .after(TransformSystems::Propagate),
    );
}

#[cfg(debug_assertions)]
fn big_space_hierarchy_validation_due(
    changed_components: Query<
        (),
        Or<(
            Added<CellCoord>,
            Added<Transform>,
            Added<GlobalTransform>,
            Added<BigSpace>,
            Added<Grid>,
            Added<FloatingOrigin>,
            Added<ChildOf>,
            Changed<ChildOf>,
            Changed<Children>,
        )>,
    >,
    mut removed_cell: RemovedComponents<CellCoord>,
    mut removed_transform: RemovedComponents<Transform>,
    mut removed_global_transform: RemovedComponents<GlobalTransform>,
    mut removed_big_space: RemovedComponents<BigSpace>,
    mut removed_grid: RemovedComponents<Grid>,
    mut removed_floating_origin: RemovedComponents<FloatingOrigin>,
    mut removed_child: RemovedComponents<ChildOf>,
) -> bool {
    !changed_components.is_empty()
        || removed_cell.read().next().is_some()
        || removed_transform.read().next().is_some()
        || removed_global_transform.read().next().is_some()
        || removed_big_space.read().next().is_some()
        || removed_grid.read().next().is_some()
        || removed_floating_origin.read().next().is_some()
        || removed_child.read().next().is_some()
}

fn build_sim_app_with_profile(
    headless: bool,
    offscreen: bool,
    compute_threads: Option<usize>,
    render_profile: LunCoSimRenderProfile,
) -> App {
    let mut app = App::new();
    // Register every LunCo asset source (lunco:// and twin://) +
    // the shared `TwinRoots` resource in ONE shared place (`lunco-assets`), so all
    // binaries get identical schemes. MUST run before `DefaultPlugins`/`AssetPlugin`
    // snapshots the source registry.
    lunco_assets::register_lunco_asset_sources(&mut app);
    let mut plugins = default_plugins_with_profile(headless, offscreen, render_profile);
    let compute_policy = if let Some(threads) = compute_threads {
        assert!(threads > 0, "compute_threads must be positive");
        bevy::app::TaskPoolThreadAssignmentPolicy {
            min_threads: threads,
            max_threads: threads,
            percent: 1.0,
            on_thread_spawn: None,
            on_thread_destroy: None,
        }
    } else {
        // BigSpace's high-precision propagation creates a worker/channel scope
        // on every frame. The default Bevy policy leaves all remaining logical
        // cores to that scope (24 workers on this 32-thread host), which makes
        // a settled world pay the task fan-out cost continuously and competes
        // with the render and async terrain pools. Keep one shared production
        // envelope for GUI and headless hosts; scene tests still pass their
        // explicit deterministic thread count above.
        bevy::app::TaskPoolThreadAssignmentPolicy {
            min_threads: 1,
            max_threads: 8,
            percent: 1.0,
            on_thread_spawn: None,
            on_thread_destroy: None,
        }
    };
    plugins = plugins.set(bevy::app::TaskPoolPlugin {
        task_pool_options: bevy::app::TaskPoolOptions {
            compute: compute_policy,
            ..default()
        },
    });
    app.add_plugins(plugins);
    // Flushes the WARN/ERROR dedup counters the `LogPlugin` filter accumulates.
    app.add_plugins(log_dedup::LogDedupPlugin);
    app.add_plugins(LunCoSimCorePlugin {
        headless,
        #[cfg(feature = "ui")]
        render_profile,
    });
    #[cfg(feature = "ui")]
    if !headless {
        lunco_luncosim_ui::register_presentation_bridges(&mut app);
    }
    app
}

/// Construct the luncosim's primary window for the custom workbench chrome.
///
/// The workbench owns the edge hit testing for undecorated Windows/Linux
/// windows. It delegates the actual OS gesture to winit, which requires this
/// window to remain resizable.
#[cfg(feature = "ui")]
fn luncosim_window(
    title: String,
    present_mode: bevy::window::PresentMode,
    vertical: bool,
    render_profile: LunCoSimRenderProfile,
) -> Window {
    let mut window = Window {
        // On wasm, attach to the `#bevy` canvas and mirror its CSS size.
        #[cfg(target_arch = "wasm32")]
        canvas: Some("#bevy".to_string()),
        #[cfg(target_arch = "wasm32")]
        fit_canvas_to_parent: true,
        present_mode,
        // Centralized merged-titlebar chrome + persisted geometry.
        ..lunco_workbench::restored_window(title)
    };
    if render_profile == LunCoSimRenderProfile::Fast {
        // A smaller default framebuffer is the largest predictable saving on
        // integrated GPUs. The user can still resize the window; the profile does
        // not alter authored scene units or simulation precision.
        window.resolution = bevy::window::WindowResolution::new(960, 540);
    } else if vertical {
        window.resolution = bevy::window::WindowResolution::new(540, 960);
    } else {
        window.resolution = bevy::window::WindowResolution::new(1280, 720);
    }
    // Keep the OS application identity aligned with the generated desktop
    // entry and icon name.
    window.name = Some("luncosim".to_string());
    // `merged_titlebar_window` removes Windows' native frame, so the workbench
    // supplies edge-resize hit testing and forwards it to winit with
    // `start_drag_resize`. That API only accepts a resizable window; disabling
    // it makes the custom resize path invalid and can leave DX12's swap chain in
    // use during a ResizeBuffers reconfiguration.
    window.resizable = true;
    window
}

#[cfg(all(test, feature = "ui"))]
mod window_tests {
    use super::{luncosim_window, LunCoSimRenderProfile};

    #[test]
    fn custom_chrome_window_remains_resizable() {
        let window = luncosim_window(
            "luncosim test".to_string(),
            bevy::window::PresentMode::Fifo,
            false,
            LunCoSimRenderProfile::Standard,
        );

        assert!(
            window.resizable,
            "the workbench delegates border drags to winit, which requires a resizable window"
        );
    }
}

/// Engine-level plugin set, render/UI stripped when `headless`.
///
/// `pub` so [`build_sim_app`] is not the only way in for a binary that genuinely
/// needs a different plugin set — but prefer `build_sim_app`, which also does the
/// asset-source prelude this function cannot do (it returns a group, not an `App`).
pub fn default_plugins(headless: bool, offscreen: bool) -> bevy::app::PluginGroupBuilder {
    default_plugins_with_profile(headless, offscreen, LunCoSimRenderProfile::Standard)
}

fn default_plugins_with_profile(
    headless: bool,
    offscreen: bool,
    render_profile: LunCoSimRenderProfile,
) -> bevy::app::PluginGroupBuilder {
    // `bevy::render` EXISTS ONLY IN A `ui` BUILD. The no-`ui` server does not link
    // bevy_render at all (that is the point of the render decoupling), so every
    // `bevy::render::*` path below must be gated — an ungated one does not merely link a
    // GPU stack, it fails to compile. It did: `cargo check -p lunco-luncosim-server` was
    // broken, and nothing caught it because `--workspace` unifies `ui` on and the CI render
    // guard only runs `cargo tree` (which resolves the graph but never builds it).
    // `headless`/`offscreen` only select render/window config in `ui` builds; a
    // no-`ui` build is always windowless, so the params are unused there.
    #[cfg(not(feature = "ui"))]
    let _ = (headless, offscreen, render_profile);

    // Window title (advertises the `--api` port so side-by-side instances are
    // distinguishable) + present mode are windowed-only and must be known at
    // window-build time, so they're computed here rather than in the UI plugin.
    #[cfg(feature = "ui")]
    let (window_title, present_mode) = {
        let args: Vec<String> = std::env::args().collect();
        let no_vsync = args.iter().any(|a| a == "--no-vsync");
        // Networked side-by-side windows: one is ALWAYS unfocused, and an
        // unfocused window under `Fifo` (vsync) can block on present when the
        // compositor stops servicing it — which stalls the WHOLE update loop
        // (sim + netcode + the 20 Hz snapshot send), not just rendering. Use
        // non-blocking `Mailbox` while networked so the background window keeps
        // ticking at full rate.
        let networked = args.iter().any(|a| a == "--host" || a == "--connect");
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
        let title = match api_port {
            Some(p) => format!("LunCoSim — Listening on {p}"),
            None => "LunCoSim".to_string(),
        };
        // Let the backend select the fastest available non-vsync mode. Wayland
        // commonly rejects `Immediate`, while `AutoNoVsync` can still select
        // Mailbox; hard-coding Immediate silently falls back to FIFO and caps
        // the production window at the compositor refresh rate.
        let present = if no_vsync {
            bevy::window::PresentMode::AutoNoVsync
        } else if networked {
            bevy::window::PresentMode::Mailbox
        } else {
            bevy::window::PresentMode::Fifo
        };
        (title, present)
    };

    let group = DefaultPlugins
        .set(AssetPlugin {
            file_path: lunco_assets::assets_dir_abs().to_string_lossy().to_string(),
            // File watching is an interactive authoring capability. Headless
            // and offscreen runs must be deterministic and must not allocate
            // OS watcher resources; scene tests and render capture use the
            // explicit API/recording paths instead.
            watch_for_changes_override: Some(!headless && !offscreen),
            // Don't probe for `.meta` sidecars: we ship none, so every asset
            // load would otherwise fire a failed `<asset>.meta` fetch.
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .set(bevy::log::LogPlugin {
            // Quieten third-party noise (rumoca JIT + diffsol per-step).
            filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
            // Bevy's default fmt layer always emits ANSI colour codes to stderr,
            // even when stderr is redirected to a file or pipe — which peppers
            // captured logs (agent/CI/`> log 2>&1`) with `\x1b[..m` escapes. Emit
            // colour only for a real terminal, and honour `NO_COLOR` either way.
            fmt_layer: |_app| {
                use bevy::log::tracing_subscriber::Layer;
                use std::io::IsTerminal;
                let ansi =
                    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
                // Collapse repeated WARN/ERROR lines (§6.4): the filter suppresses
                // and counts, `LogDedupPlugin` flushes the counts as summaries.
                Some(Box::new(
                    bevy::log::tracing_subscriber::fmt::Layer::default()
                        .with_ansi(ansi)
                        .with_writer(std::io::stderr)
                        .with_filter(crate::log_dedup::DedupFilter),
                ))
            },
            ..default()
        });

    // Only a windowed `ui` build owns the renderer. A headless `ui` build keeps
    // the asset/type plugins above but removes every render-world consumer from
    // the default group, so no plugin can assume a RenderApp exists.
    #[cfg(feature = "ui")]
    let group = if headless {
        group
            .disable::<bevy::render::RenderPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
            .disable::<bevy::core_pipeline::CorePipelinePlugin>()
            .disable::<bevy::post_process::PostProcessPlugin>()
            .disable::<bevy::anti_alias::AntiAliasPlugin>()
            .disable::<bevy::sprite_render::SpriteRenderPlugin>()
            .disable::<bevy::ui_render::UiRenderPlugin>()
            .disable::<bevy::gltf::GltfPlugin>()
            .disable::<bevy::pbr::PbrPlugin>()
            .disable::<bevy::gizmos_render::GizmoRenderPlugin>()
    } else {
        group
    };

    #[cfg(feature = "ui")]
    let vertical = std::env::args().any(|a| a == "--vertical");

    // Window/winit setup. With the `ui` feature the runtime `headless` flag still
    // picks the windowless variant (no primary window, WinitPlugin disabled) —
    // and so does `offscreen`, which is windowless WITH a GPU; the default
    // Bevy render plugin renders surfaceless into the offscreen target image.
    // Without `ui` there's no winit crate to disable, so just declare a
    // windowless `WindowPlugin`.
    #[cfg(feature = "ui")]
    let group = if headless || offscreen {
        group
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                close_when_requested: false,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
    } else {
        group.set(WindowPlugin {
            primary_window: Some(luncosim_window(
                window_title,
                present_mode,
                vertical,
                render_profile,
            )),
            ..default()
        })
    };
    #[cfg(not(feature = "ui"))]
    let group = group.set(WindowPlugin {
        primary_window: None,
        exit_condition: bevy::window::ExitCondition::DontExit,
        close_when_requested: false,
        ..default()
    });

    group.build().disable::<TransformPlugin>()
}

/// Scenario distribution Phase 4 (client consume): when the connected client has
/// downloaded + verified **every** asset of the host's advertised scenario, load
/// its entry scene (`default_scene`) from the cache, mounted as a Twin root.
/// Loaded once per
/// scenario **revision** (a mid-session swap bumps the revision → reload).
///
/// This is a transient, read-only consume: the scene is mounted via `LoadScene`
/// as a bare stage, NOT added to the workspace as an editable Twin (no `twin.toml`,
/// no journal). Turning it into an editable on-disk Twin is a separate "promote"
/// step; write-gating enforcement rides that later work. Host peers early-out
/// (they already hold the scene). Headless-safe (`LoadScene` needs no GPU).
#[cfg(feature = "networking")]
fn load_ready_scenario(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    downloads: Res<lunco_networking::scenario_sync::AssetDownloads>,
    // Twin roots: a downloaded scenario is mounted here as a root over its cache
    // dir, so it loads under the SAME `twin://<name>/<rel>` the host uses.
    twins: Res<lunco_assets::twin_source::TwinRoots>,
    // Last scenario revision we triggered a load for — reload only on change.
    mut last_loaded: Local<Option<[u8; 32]>>,
    mut commands: Commands,
) {
    if role.is_host() {
        return;
    }
    let Some(m) = remote.manifest.as_ref() else {
        return;
    };
    let Some(scene) = m.default_scene.as_deref() else {
        return; // scenario advertises no entry scene → nothing to auto-load
    };
    if *last_loaded == Some(m.revision) || !downloads.all_cached(m) {
        return;
    }
    // Mounting registers the scenario's cache dir as this twin's root (unless the
    // twin is already open locally, which keeps its own). Either way the URI is
    // the host's, so a client that already booted this scene re-triggers the SAME
    // asset path and `LoadScene` no-ops instead of remounting.
    //
    // Verified on a native host/client pair (`scripts/run_host_client.sh`): both
    // peers mount `twin://luncosim/sandbox_scene.usda`, and this load lands ~1 s
    // after the client's own boot load — INSIDE the spawn window, so the no-op
    // depends on `LoadScene`'s `SceneLoadInFlight` arm, not on its
    // already-spawned-prims arm.
    //
    // TODO(verify-web-client): the case this addressing exists for — a peer with
    // NO local checkout, resolving through the mounted cache dir — is still
    // unverified. A native pair takes the "twin already open locally" branch, so
    // it exercises URI agreement but never the cache-root mount. It fails
    // silently: a wrong root gives that peer its own `GlobalEntityId`s, so
    // possession and client prediction never bind while the scene still renders.
    let uri = match lunco_networking::scenario_sync::mount_scenario_twin(
        &twins,
        &m.scenario_id,
        &m.name,
        scene,
    ) {
        Ok(uri) => uri,
        Err(error) => {
            lunco_core::trigger_error(
                &mut commands,
                "scenario-twin-mount-failed",
                format!("could not mount downloaded scenario Twin: {error}"),
            );
            return;
        }
    };
    info!("[net] scenario fully cached; loading entry scene (read-only): {scene}");
    commands.trigger(LoadScene {
        path: uri,
        root_prim: String::new(),
    });
    *last_loaded = Some(m.revision);
}

/// Scenario distribution Layer B: replay peers' live authored edits onto the
/// local scene. The journal plane converges every peer's journal
/// (`append_remote` + merge, bidirectional); this projects the merged Op entries
/// onto the local USD scene so *other* peers' edits become visible. Runs on
/// **both** roles now (full bidirectional collaboration):
///
/// - **Client** — projects entries AFTER the manifest's `journal_head` (the
///   downloaded snapshot's base — so history baked into the files isn't
///   double-applied), authored by another peer.
/// - **Host** — projects entries AFTER the head its own scenario manifest
///   advertises (see below); the `author != me` filter then selects only
///   client-authored edits (its own are already applied at author time), so the
///   host *sees* clients' edits.
///
/// Both roles therefore share ONE invariant: *the files on disk already reflect
/// history up to `journal_head`; replay only what came after.* The host used to
/// pass `base = None` (replay the whole log, trusting `author != me` to drop its
/// own edits). That holds only while the host's local author id equals the id
/// that wrote the journal. A twin authored anywhere else — another machine, an
/// earlier session, a downloaded twin, or merely a different `LUNCO_PEER_ID`
/// (which `scripts/run_host_client.sh` sets) — looks entirely foreign, so the
/// host re-applied its whole saved history on top of files that already contained
/// it: prims re-added, rovers churned. (Historically this also double-despawned a
/// wheel joint whose bodies were already gone and tripped avian's
/// `assert!(island.joint_count > 0)`; that is now structurally impossible — every
/// synthesized joint is owned by its chassis via `ChildOf`, so it dies exactly
/// once with the rover subtree. See `setup_physical_wheel`.)
///
/// The head is sampled ONCE, not read every frame: a mid-session manifest rebuild
/// advances `journal_head`, which would move the base past client entries this
/// frame has not projected yet.
///
/// Each entry applies once, without re-recording (`replay_op`). The
/// assembly-crate bridge: the only place that sees the wire state
/// (`RemoteScenarioManifest`), the journal, AND the USD registry.
///
/// Single active scene doc for now — multi-doc needs stable cross-peer
/// `DocumentId` mapping (a follow-up); `scene_ops_after` selects by author, not
/// by the entry's peer-local `doc` id, which single-scene makes irrelevant.
#[cfg(feature = "networking")]
fn replay_scenario_journal(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    // Host-side only (inserted by `setup_host`) — the manifest this host serves.
    local_scenario: Option<Res<lunco_networking::scenario::ScenarioManifestResource>>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    mut registry: ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd_core::document::UsdDocument>>,
    // Entry ids already projected onto the scene (once-per-entry guard).
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
    // The host's replay base, latched the first frame its manifest exists.
    mut host_base: Local<Option<Option<lunco_twin_journal::EntryId>>>,
) {
    let Some(journal) = journal else {
        return;
    };
    // Base head: the state the on-disk files already reflect. The host reads it
    // off the manifest it built (deferring until that build lands); a client
    // bases on the downloaded snapshot's head, or waits if no scenario is loaded.
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        if host_base.is_none() {
            let Some(scenario) = local_scenario.as_ref() else {
                return; // no host manifest resource → nothing to base on yet
            };
            let Some(manifest) = scenario.manifest.as_ref() else {
                return; // manifest build still in flight → defer, don't replay history
            };
            *host_base = Some(manifest.journal_head.clone());
        }
        host_base.as_ref().and_then(|h| h.as_ref())
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    // Single active scene doc (scenario consume is single-scene for now).
    let docs: Vec<_> = registry.ids().collect();
    let [doc] = docs.as_slice() else {
        return;
    };
    let doc = *doc;
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::scene_ops_after(&journal, base, &me, &applied);
    for (id, op) in pending {
        registry.replay_op(doc, &op);
        applied.insert(id);
    }
}

/// Scenario distribution Layer B for **Modelica** — the parallel of
/// [`replay_scenario_journal`] for the model domain. The journal plane, its merge,
/// and the strategy-honoring op selector are all domain-generic; only this consume
/// leg is per-domain. Selects the merged, not-yet-applied `Modelica` op entries via
/// [`domain_ops_after`](lunco_networking::journal_plane::domain_ops_after)
/// (`DomainKind::Modelica`) — so a scripted merge policy reorders Modelica replay
/// identically to USD — and applies each through `ModelicaDocumentRegistry::replay_op`
/// (no re-recording).
///
/// Resources are `Option`: the Modelica registry / journal aren't present in every
/// app configuration (a pure-USD headless build), so this no-ops when either is
/// absent. Single active model for now — the same cross-peer `DocumentId` limitation
/// the USD leg documents (selection is by author, which one open model makes
/// sufficient); with more than one open model it defers rather than misroute.
#[cfg(feature = "networking")]
fn replay_scenario_journal_modelica(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<ResMut<lunco_modelica_core::state::ModelicaDocumentRegistry>>,
    // Modelica-domain entry ids already projected (its own once-per-entry guard,
    // independent of the USD driver's applied-set).
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry)) = (journal, registry) else {
        return;
    };
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    // Single active Modelica model (see doc note); >1 open model → defer.
    let docs: Vec<_> = registry.iter().map(|(id, _)| id).collect();
    let [doc] = docs.as_slice() else {
        return;
    };
    let doc = *doc;
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::domain_ops_after(
        &journal,
        base,
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Modelica,
    );
    for (id, op) in pending {
        registry.replay_op(doc, &op);
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::Script` — the script twin of
/// [`replay_scenario_journal_modelica`]. Selects the merged, not-yet-applied
/// `Script` op entries via [`domain_ops_after`](lunco_networking::journal_plane::domain_ops_after)
/// (so a scripted merge policy reorders script replay identically to USD/Modelica)
/// and applies each through `ScriptRegistry::replay_op` (no re-recording), so a
/// live rover-behaviour edit (`ScriptOp::SetSource`) recorded on one peer projects
/// onto another's `ScriptDocument`.
///
/// Same single-active-doc limitation as the Modelica leg: `ScriptOp` carries no
/// `DocumentId`, and scenario doc ids are minted locally (not stable cross-peer),
/// so this routes only when exactly one script doc is live; otherwise it defers
/// rather than misroute. Full multi-doc cross-peer replay lands with stable
/// cross-peer document identity. No-ops when the registry / journal is absent.
#[cfg(feature = "networking")]
fn replay_scenario_journal_script(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<ResMut<lunco_scripting::ScriptRegistry>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry)) = (journal, registry) else {
        return;
    };
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    // Single active script doc (see doc note); 0 or >1 → defer.
    let docs: Vec<_> = registry.documents.keys().copied().collect();
    let [doc] = docs.as_slice() else {
        return;
    };
    let doc = *doc;
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::domain_ops_after(
        &journal,
        base,
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Script,
    );
    for (id, op) in pending {
        registry.replay_op(doc, &op);
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::Experiment` — projects a
/// peer's journaled experiment *definitions* (create / rename / bounds / params
/// / delete) onto the local `ExperimentRegistry`. Unlike the script/modelica
/// legs there is **no single-doc limitation**: every `ExperimentOp` carries its
/// own cross-peer-stable id (the authored UUID, replayed via `insert_with_id`),
/// so any number of experiments route correctly. Run results/status are NOT here
/// — they ride the content/presence planes. No-ops when registry/journal absent.
#[cfg(feature = "networking")]
fn replay_scenario_journal_experiment(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<ResMut<lunco_experiments::ExperimentRegistry>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry)) = (journal, registry) else {
        return;
    };
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::domain_ops_after(
        &journal,
        base,
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Experiment,
    );
    for (id, op) in pending {
        lunco_modelica_core::experiment_journal::replay_experiment_op(&mut registry, &op);
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::Shader` — projects a peer's
/// journaled WGSL edits (`ShaderOp::SetSource`) onto the local `ShaderRegistry`
/// and **hot-reloads** the live `Assets<Shader>`, so a shader tweak on one machine
/// recompiles on every peer. No single-doc limitation: the op carries the shader
/// `path` (cross-peer-stable), so `apply_replayed` routes by path. `Assets<Shader>`
/// / `ShaderRegistry` are `Option` — a headless (no-render) relay host has neither
/// and simply no-ops (it still forwards the journal entry to GUI peers).
#[cfg(feature = "networking")]
fn replay_scenario_journal_shader(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<ResMut<lunco_scene_commands::shader_doc::ShaderRegistry>>,
    asset_server: Option<Res<AssetServer>>,
    shaders: Option<ResMut<Assets<bevy::shader::Shader>>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry), Some(asset_server), Some(mut shaders)) =
        (journal, registry, asset_server, shaders)
    else {
        return;
    };
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::domain_ops_after(
        &journal,
        base,
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Shader,
    );
    for (id, op) in pending {
        if let Ok(shader_op) =
            serde_json::from_value::<lunco_scene_commands::shader_doc::ShaderOp>(op)
        {
            if let Some((path, source)) = registry.apply_replayed(&shader_op) {
                // Same hot-reload hook as the local edit: overwrite the asset id
                // every material holds so the recompile propagates.
                let handle = asset_server.load::<bevy::shader::Shader>(path.clone());
                let _ = shaders.insert(handle.id(), bevy::shader::Shader::from_wgsl(source, path));
            }
        }
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::ObstacleField` — installs a
/// peer's journaled obstacle-field spec onto the local `ObstacleFieldSpec`.
/// This is what replaced the former bespoke host→client
/// broadcast (`sync_obstacle_field_spec`): the spec now rides the journal plane,
/// so a tweak syncs BOTH directions and persists. No single-doc limitation (the
/// spec is a singleton). No-ops when the spec resource / journal are absent.
#[cfg(feature = "networking")]
fn replay_scenario_journal_obstacle(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    spec: Option<ResMut<lunco_obstacle_field::ObstacleFieldSpec>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut spec)) = (journal, spec) else {
        return;
    };
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::domain_ops_after(
        &journal,
        base,
        &me,
        &applied,
        lunco_twin_journal::DomainKind::ObstacleField,
    );
    // Coalesce: a batch may carry several SetSpec ops (rapid slider drags); only
    // the LAST one matters — so install once.
    let mut last_spec = None;
    for (id, op) in pending {
        if let Some(new_spec) = lunco_obstacle_field::journal::replay_spec(&op) {
            last_spec = Some(new_spec);
        }
        applied.insert(id);
    }
    if let Some(new_spec) = last_spec {
        // Install the peer's spec. Sets the resource directly (NOT the
        // `UpdateObstacleFieldSpec` command), so no re-record.
        *spec = new_spec;
    }
}

/// Per-domain journal consume leg for `DomainKind::ToolLibrary` — re-registers a
/// peer's journaled rhai tool library into the process-global tool registry
/// (hot-replacing any prior one; the runtime picks it up on its next refresh).
/// Tool libraries are process-global (reachable from the rhai engine outside the
/// ECS), so this needs no ECS resource beyond the journal. No-ops when absent.
#[cfg(feature = "networking")]
fn replay_scenario_journal_tools(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scoped: Option<ResMut<lunco_scripting::tool_libs::TwinToolLibraries>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let Some(journal) = journal else {
        return;
    };
    let Some(workspace) = workspace.as_deref() else {
        return;
    };
    let Some(active) = workspace.active_twin else {
        return;
    };
    let Some(mut scoped) = scoped else {
        return;
    };
    scoped.ensure_active(active);
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::domain_ops_after(
        &journal,
        base,
        &me,
        &applied,
        lunco_twin_journal::DomainKind::ToolLibrary,
    );
    for (id, op) in pending {
        if let Some((name, source)) =
            lunco_scripting::registration_journal::replay_tool_library(&op)
        {
            if let Err(error) = scoped.register(active, &name, &source) {
                warn!("[tool_libs] ignored journal replay outside its active scope: {error}");
            }
        }
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::Timeline` — stores a peer's
/// journaled mission timeline in the local `TimelineStore` (hot-replacing any
/// prior one), so `RunStoredTimeline`/`ListTimelines` see it. No-ops when the
/// store / journal are absent.
#[cfg(feature = "networking")]
fn replay_scenario_journal_timeline(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    store: Option<ResMut<lunco_scripting::timelines::TimelineStore>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut store)) = (journal, store) else {
        return;
    };
    let Ok(owner) = lunco_scripting::timelines::active_owner(workspace.as_deref()) else {
        return;
    };
    if !matches!(owner, lunco_scripting::timelines::TimelineOwner::Twin(_)) {
        return;
    }
    store.ensure_scope(owner);
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        manifest.journal_head.as_ref()
    };
    let me = journal.local_author();
    let pending = lunco_networking::journal_plane::domain_ops_after(
        &journal,
        base,
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Timeline,
    );
    for (id, op) in pending {
        if let Some((name, timeline)) = lunco_scripting::registration_journal::replay_timeline(&op)
        {
            if let Err(error) = store.insert_for(owner, name, timeline) {
                warn!("[timeline] ignored journal replay outside its active scope: {error:?}");
            }
        }
        applied.insert(id);
    }
}

/// Result-artifact writer: on `RunCompleted`, the host serializes the finished
/// `RunResult` to `<twin>/results/<experiment-id>.json` so it rides the **content
/// plane** — the twin file-walk CID's `results/` (a non-dot dir) and the manifest
/// sync ships it to peers. Host-authoritative: a Client never ran the sim, so it
/// writes nothing (it *receives* the artifact). The result is recovered from the
/// registry (core writes it there before `RunCompleted` fires — same pattern as
/// `project_run_results_to_ui`). JSON today; parquet is a deferred format swap
/// pending a wasm-reader spike (see `NETWORKING_STATE_SYNC_TAXONOMY_DESIGN.md`).
/// `RunResult` to `<twin>/results/<experiment-id>.json` through the cross-platform
/// [`lunco_storage`] layer (native file / wasm WebStorage). This is **core
/// persistence, not a networking concern** — a single-player run's results
/// survive a restart, and when networking is on the same file rides the content
/// plane to peers. Host/standalone only: a networked Client never ran the sim, so
/// it writes nothing (it *receives* the artifact). Recovered from the registry
/// (core writes it there before `RunCompleted` fires — same pattern as
/// `project_run_results_to_ui`). JSON today; parquet is a deferred format swap.
fn write_run_result_artifact(
    mut completed: MessageReader<lunco_experiments::RunCompleted>,
    registry: Res<lunco_experiments::ExperimentRegistry>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    role: Option<Res<lunco_core::NetworkRole>>,
) {
    if matches!(role.as_deref(), Some(lunco_core::NetworkRole::Client)) {
        return;
    }
    for msg in completed.read() {
        let id = msg.experiment_id;
        let Some(result) = registry.get(id).and_then(|e| e.result.as_ref()) else {
            continue;
        };
        let Some(active) = workspace.active_twin else {
            continue;
        };
        let Some(twin) = workspace.twin(active) else {
            continue;
        };
        // The storage layer creates parent dirs on write (FileStorage tmp+rename;
        // WebStorage is key-based), so no explicit mkdir — all I/O goes through it.
        let dest = twin
            .root
            .join("results")
            .join(format!("{}.json", id.as_artifact_stem()));
        match serde_json::to_vec_pretty(result) {
            Ok(bytes) => match lunco_storage::write_file_sync(&dest, &bytes) {
                Ok(()) => info!("[experiment] wrote result artifact {dest:?}"),
                Err(e) => warn!("[experiment] result artifact write failed: {e}"),
            },
            Err(e) => warn!("[experiment] result serialize failed: {e}"),
        }
    }
}

/// Result-artifact loader — the consume half of persistence/ship-artifact. For
/// each known experiment that lacks a trajectory, reads
/// `<twin>/results/<id>.json` through [`lunco_storage`] (cross-platform, no
/// directory listing — bounded by the registry cap) and loads it. This restores a
/// single-player run's results after a restart AND makes a networked peer *see*
/// the host's results once their file syncs.
///
/// Change-driven on [`ExperimentRegistry`] mutation (a definition synced, a run
/// completed, a status update) — so a just-synced result file is picked up on the
/// next registry change (e.g. the presence status flip) rather than by polling.
fn load_run_result_artifacts(
    mut registry: ResMut<lunco_experiments::ExperimentRegistry>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
) {
    let Some(active) = workspace.active_twin else {
        return;
    };
    let Some(root) = workspace.twin(active).map(|t| t.root.join("results")) else {
        return;
    };
    // Ids known but resultless — the only candidates worth a storage read.
    let want: Vec<lunco_experiments::ExperimentId> = registry
        .iter_all()
        .filter(|e| e.result.is_none())
        .map(|e| e.id)
        .collect();
    for id in want {
        let path = root.join(format!("{}.json", id.as_artifact_stem()));
        let Ok(bytes) = lunco_storage::read_file_sync(&path) else {
            continue; // not present (yet)
        };
        match serde_json::from_slice::<lunco_experiments::RunResult>(&bytes) {
            Ok(result) => {
                let wall = result.meta.wall_time_ms;
                registry.set_result(id, result);
                registry.set_status(
                    id,
                    lunco_experiments::RunStatus::Done { wall_time_ms: wall },
                );
                info!(
                    "[experiment] loaded result artifact for {}",
                    id.as_artifact_stem()
                );
            }
            Err(e) => warn!(
                "[experiment] result artifact parse failed for {}: {e}",
                id.as_artifact_stem()
            ),
        }
    }
}

/// Networking distribution trigger: when a run finishes on the host, ask for an
/// immediate scenario-manifest rebuild so already-connected peers pull the
/// just-written result artifact now (serviced by `service_manifest_rebuild_request`
/// in lunco-networking). The write itself is the core persistence system's job;
/// this only nudges distribution. Host-only.
#[cfg(feature = "networking")]
fn request_rebuild_after_result(
    mut completed: MessageReader<lunco_experiments::RunCompleted>,
    role: Option<Res<lunco_core::NetworkRole>>,
    mut rebuild: ResMut<lunco_networking::sync::RequestManifestRebuild>,
) {
    if !matches!(role.as_deref(), Some(lunco_core::NetworkRole::Host)) {
        return;
    }
    if completed.read().count() > 0 {
        rebuild.0 = true;
    }
}

/// Presence broadcast: the host relays experiment run-status transitions
/// (Running progress → Done/Failed/Cancelled) to clients over the wire, so a
/// peer watches a run advance live. Ephemeral — progress rides the lossy
/// `ControlStream`, terminal states the reliable `CommandBus` (so the final
/// flip is never dropped). Host-only; the assembly crate maps `RunStatus` to the
/// primitive `RunStatusMsg` here (keeping networking free of an experiments dep).
#[cfg(feature = "networking")]
fn broadcast_run_status(
    role: Option<Res<lunco_core::NetworkRole>>,
    mut outbox: ResMut<lunco_networking::sync::SyncOutbox>,
    mut progress: MessageReader<lunco_experiments::RunProgress>,
    mut completed: MessageReader<lunco_experiments::RunCompleted>,
    mut failed: MessageReader<lunco_experiments::RunFailed>,
    mut cancelled: MessageReader<lunco_experiments::RunCancelled>,
    registry: Res<lunco_experiments::ExperimentRegistry>,
) {
    if !matches!(role.as_deref(), Some(lunco_core::NetworkRole::Host)) {
        return;
    }
    use lunco_core::SyncChannel;
    use lunco_networking::sync::{RunStatusMsg, SyncEnvelope};
    let msg = |id: lunco_experiments::ExperimentId,
               phase: u8,
               t_current: f64,
               wall_time_ms: u64,
               error: String| {
        SyncEnvelope::RunStatus(RunStatusMsg {
            experiment_id: id.uuid_bytes(),
            phase,
            t_current,
            wall_time_ms,
            error,
        })
    };
    for m in progress.read() {
        outbox.0.push((
            SyncChannel::ControlStream,
            msg(m.experiment_id, 2, m.t_current, 0, String::new()),
        ));
    }
    for m in completed.read() {
        let wall = registry
            .get(m.experiment_id)
            .and_then(|e| e.result.as_ref())
            .map(|r| r.meta.wall_time_ms)
            .unwrap_or(0);
        outbox.0.push((
            SyncChannel::CommandBus,
            msg(m.experiment_id, 3, 0.0, wall, String::new()),
        ));
    }
    for m in failed.read() {
        outbox.0.push((
            SyncChannel::CommandBus,
            msg(m.experiment_id, 4, 0.0, 0, m.error.clone()),
        ));
    }
    for m in cancelled.read() {
        outbox.0.push((
            SyncChannel::CommandBus,
            msg(m.experiment_id, 5, 0.0, 0, String::new()),
        ));
    }
}

/// Presence apply (client): drain host-sent run-status updates into the local
/// `ExperimentRegistry` so a synced experiment's row advances Running → Done.
/// Won't clobber a `Done` already loaded from the result artifact (the artifact
/// carries the trajectory; a late progress packet must not downgrade it).
#[cfg(feature = "networking")]
fn apply_run_status(
    mut pending: ResMut<lunco_networking::sync::PendingRunStatus>,
    mut registry: ResMut<lunco_experiments::ExperimentRegistry>,
) {
    if pending.0.is_empty() {
        return;
    }
    for m in std::mem::take(&mut pending.0) {
        let id = lunco_experiments::ExperimentId::from_uuid_bytes(m.experiment_id);
        let already_done = matches!(
            registry.get(id).map(|e| &e.status),
            Some(lunco_experiments::RunStatus::Done { .. })
        );
        if already_done && m.phase != 3 {
            continue;
        }
        let status = match m.phase {
            1 => lunco_experiments::RunStatus::Queued,
            2 => lunco_experiments::RunStatus::Running {
                t_current: m.t_current,
            },
            3 => lunco_experiments::RunStatus::Done {
                wall_time_ms: m.wall_time_ms,
            },
            4 => lunco_experiments::RunStatus::Failed {
                error: m.error,
                partial: false,
            },
            5 => lunco_experiments::RunStatus::Cancelled,
            _ => lunco_experiments::RunStatus::Pending,
        };
        registry.set_status(id, status);
    }
}

/// The USD type name of a policy prim, and the attribute names carrying its rhai
/// hook definition — the projected form of `lunco_scripting::policy::PolicyDef`.
const LUNCO_POLICY_TYPE: &str = "LunCoPolicy";

/// One authored `LunCoPolicy` prim, BEFORE its rhai source is resolved. The source is
/// authored EITHER inline (`info:sourceCode`, a `string` that rides the USD journal
/// plane — live-editable, per-op synced) OR by file reference (`info:sourceAsset`,
/// an `asset` `@…rhai@` that rides the whole-twin content plane, CID-verified).
/// The policy reader has its own inline-over-file rule; it is not a
/// `LunCoProgramAPI` and is therefore outside the program source resolver.
struct AuthoredPolicy {
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    seam: String,
    entry: String,
    deterministic: bool,
    /// Inline rhai source (`info:sourceCode`), non-empty when authored.
    inline_source: Option<String>,
    /// Asset path to a `.rhai` file (`info:sourceAsset`), when authored.
    source_path: Option<String>,
}

/// Append every composed `LunCoPolicy` prim from one reader to the authored policy
/// set. Reads the composed stage, so an opinion authored at any layer
/// (global/twin/scene) resolves to one effective policy per seam. A prim missing
/// `seam`, or carrying neither an inline source nor a source path, is skipped as
/// incompletely authored. File-reference resolution happens in
/// [`project_usd_policies`], not here.
fn append_usd_policies(
    reader: &dyn UsdReadObject,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    out: &mut Vec<AuthoredPolicy>,
) {
    for prim in reader.prim_paths() {
        if reader.type_name(&prim).as_deref() != Some(LUNCO_POLICY_TYPE) {
            continue;
        }
        let seam = reader.text(&prim, "lunco:policy:seam").unwrap_or_default();
        let inline_source = reader
            .text(&prim, "info:sourceCode")
            .filter(|source| !source.is_empty());
        let source_path = reader
            .asset(&prim, "info:sourceAsset")
            .filter(|source| !source.is_empty());
        if seam.is_empty() || (inline_source.is_none() && source_path.is_none()) {
            continue;
        }
        out.push(AuthoredPolicy {
            stage_id,
            seam,
            entry: reader.text(&prim, "lunco:policy:entry").unwrap_or_default(),
            deterministic: reader
                .boolean(&prim, "lunco:policy:deterministic")
                .unwrap_or(true),
            inline_source,
            source_path,
        });
    }
}

/// Read policies from the active scene's prepared/live composed source. The
/// active scene root is the ownership boundary; unrelated loaded asset plans
/// must not register policy hooks in the running simulation.
fn extract_active_usd_policies(
    stages: &Assets<UsdStageAsset>,
    canonical: &lunco_usd_bevy_core::canonical::CanonicalStages,
    roots: impl IntoIterator<Item = AssetId<UsdStageAsset>>,
) -> Vec<AuthoredPolicy> {
    let mut out = Vec::new();
    for stage_id in roots {
        let Some(stage_asset) = stages.get(stage_id) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(stage_id, stage_asset);
        append_usd_policies(&reader, stage_id, &mut out);
    }
    out
}

/// The three states of resolving a `info:sourceAsset` `.rhai` reference.
enum PolicySource {
    /// Loaded — the file's text.
    Ready(String),
    /// The asset server is still fetching it — re-run next frame.
    Loading,
    /// Load failed, or no loader present — drop this policy (do not spin).
    Failed,
}

/// Resolve a `info:sourceAsset` `.rhai` reference to its text via the
/// `AssetServer` (wasm-safe — no `std::fs`), caching the handle so the asset isn't
/// dropped mid-load. The stage owns the reference's anchor, and the shared USD
/// path resolver produces the exact same identity used by the Rhai dependency
/// loader. Inline source is unaffected (rides the doc).
fn resolve_policy_source_file(
    path: &str,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_server: &AssetServer,
    sources: Option<&Assets<lunco_scripting::source_asset::RhaiSource>>,
    pending: &mut std::collections::HashMap<
        String,
        Handle<lunco_scripting::source_asset::RhaiSource>,
    >,
) -> PolicySource {
    let Some(sources) = sources else {
        warn!("[policy] sourcePath '{path}' authored but the RhaiSource asset loader is absent");
        return PolicySource::Failed;
    };
    let asset_id = lunco_usd_bevy::resolve_stage_asset_path(asset_server, stage_id, path);
    let handle = pending.entry(asset_id.clone()).or_insert_with(|| {
        asset_server.load(bevy::asset::AssetPath::parse(&asset_id).into_owned())
    });
    let root_failed = asset_server.load_state(&*handle).is_failed();
    let dependencies_failed = asset_server
        .recursive_dependency_load_state(&*handle)
        .is_failed();
    if root_failed || dependencies_failed {
        warn!(
            "[policy] failed to load sourcePath '{path}' as '{asset_id}' via AssetServer \
             (root_failed={root_failed}, dependencies_failed={dependencies_failed})"
        );
        return PolicySource::Failed;
    }
    if !asset_server.is_loaded_with_dependencies(&*handle) {
        return PolicySource::Loading;
    }
    match sources.get(&*handle) {
        Some(src) => PolicySource::Ready(src.text.clone()),
        None => PolicySource::Loading,
    }
}

/// **Policy projection** — activation half of "policy is a USD prim". On any
/// composed-stage change, read the `LunCoPolicy` prims and project them into the
/// live hook registry via
/// [`lunco_scripting::policy::project_policies`]: a new prim registers its rhai
/// hook (and, at [`lunco_scripting::policy::MERGE_SEAM`],
/// flips the journal merge strategy); a removed prim retracts it. Because a policy
/// prim rides the USD doc-op journal, cross-peer propagation is (journal sync →
/// each peer recomposes → each peer's projector re-registers) — no bespoke policy
/// broadcast.
///
/// A policy's rhai source may be authored inline (`info:sourceCode`, journal
/// plane) or by an `@…rhai@` file reference (`info:sourceAsset`, content plane),
/// inline winning — so this also drives the async asset load, keeping the file's text
/// resolved. Change-gated on total stage generation + stage count, PLUS a re-run while
/// any file-backed source is still loading.
#[allow(clippy::type_complexity)]
fn project_usd_policies(
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy_core::canonical::CanonicalStages>,
    roots: Query<&lunco_usd_bevy_scene::UsdPrimPath, With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    mut registry: ResMut<lunco_scripting::policy::ScriptedPolicyRegistry>,
    mut synthesizers: ResMut<lunco_usd_sim::domain_projection::SynthesizerRegistry>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    asset_server: Res<AssetServer>,
    sources: Option<Res<Assets<lunco_scripting::source_asset::RhaiSource>>>,
    mut pending: Local<
        std::collections::HashMap<String, Handle<lunco_scripting::source_asset::RhaiSource>>,
    >,
    mut source_events: MessageReader<AssetEvent<lunco_scripting::source_asset::RhaiSource>>,
    mut last: Local<Option<(usize, usize, u64)>>,
    mut awaiting: Local<bool>,
) {
    let source_changed = source_events.read().any(|event| {
        matches!(
            event,
            AssetEvent::Added { .. }
                | AssetEvent::Modified { .. }
                | AssetEvent::Removed { .. }
                | AssetEvent::Unused { .. }
                | AssetEvent::LoadedWithDependencies { .. }
        )
    });
    let root_ids: Vec<_> = roots.iter().map(|prim| prim.stage_handle.id()).collect();
    let signal = (
        root_ids.len(),
        root_ids.iter().filter_map(|id| stages.get(*id)).count(),
        root_ids
            .iter()
            .filter_map(|id| stages.get(*id).map(|_| canonical.generation_for(*id)))
            .sum::<u64>(),
    );
    // Re-run when the stage moved OR a file-backed source is still loading.
    if *last == Some(signal) && !*awaiting && !source_changed {
        return;
    }
    *last = Some(signal);

    let authored = extract_active_usd_policies(&stages, &canonical, root_ids);
    // Drop cached handles for paths no longer authored, so a removed file-policy stops
    // pinning its asset.
    let live: std::collections::HashSet<String> = authored
        .iter()
        .filter_map(|a| {
            a.source_path.as_deref().map(|path| {
                lunco_usd_bevy::resolve_stage_asset_path(&asset_server, a.stage_id, path)
            })
        })
        .collect();
    pending.retain(|p, _| live.contains(p.as_str()));

    let mut desired = Vec::with_capacity(authored.len());
    let mut unresolved = false;
    for a in &authored {
        // This is the policy projection's inline-over-file rule; it is separate
        // from the strict `LunCoProgramAPI` source selector.
        let source = if let Some(src) = &a.inline_source {
            src.clone()
        } else if let Some(path) = &a.source_path {
            match resolve_policy_source_file(
                path,
                a.stage_id,
                &asset_server,
                sources.as_deref(),
                &mut pending,
            ) {
                PolicySource::Ready(text) => text,
                PolicySource::Loading => {
                    unresolved = true;
                    continue;
                }
                PolicySource::Failed => continue,
            }
        } else {
            continue;
        };
        desired.push(lunco_scripting::policy::PolicyDef {
            seam: a.seam.clone(),
            entry: a.entry.clone(),
            source,
            deterministic: a.deterministic,
        });
    }
    *awaiting = unresolved;
    let previous_synthesizers: std::collections::HashSet<String> = registry
        .policies
        .iter()
        .filter_map(|policy| policy.seam.strip_prefix("synth.").map(str::to_string))
        .collect();
    let desired_synthesizers: std::collections::HashSet<String> = desired
        .iter()
        .filter_map(|policy| policy.seam.strip_prefix("synth.").map(str::to_string))
        .collect();
    lunco_scripting::policy::project_policies(desired, &mut registry, journal.as_deref());
    for name in previous_synthesizers.difference(&desired_synthesizers) {
        lunco_usd_sim::domain_projection::unregister_hook_synthesizer(&mut synthesizers, name);
    }
    for name in desired_synthesizers {
        lunco_usd_sim::domain_projection::register_hook_synthesizer(&mut synthesizers, name);
    }
}

/// Convenience command: author (or hot-replace) a rhai policy as a `LunCoPolicy`
/// USD prim under `<mounted-root>/Policies/<name>` in ONE call, instead of
/// hand-issuing the underlying `ApplyUsdOp`s. Because it authors USD doc ops, the policy **journals →
/// syncs to every peer → the projector activates it** (registers the rhai hook; at
/// `MERGE_SEAM` flips the merge strategy). Re-issuing with the same `name` (or later
/// editing `info:sourceCode`) **hot-replaces the hook live** — dynamic rhai
/// editing with no file system, converging across the network.
///
/// This command authors the INLINE source (`info:sourceCode`, journal plane) —
/// the live-edit form. A file-backed policy is authored instead by pointing
/// `info:sourceAsset` at an `@…rhai@` file (content plane, CID-synced); the
/// projector resolves it via the asset server, and inline wins when both are set.
///
/// This is the ergonomic surface over the canonical form (a `LunCoPolicy` prim); the
/// raw `ApplyUsdOp` path still works. Single active scene doc for now (mirrors the
/// journal drivers).
#[lunco_core::Command(default)]
pub struct SetRhaiPolicy {
    /// Prim name under the mounted scene's `Policies` scope (the identity for
    /// hot-replace); defaults to a sanitized `seam` when empty.
    pub name: String,
    /// The hook seam (id): e.g. `"journal.merge.order"`, `"rbac.authorize"`, or
    /// `"synth.<name>"` for a generated Modelica source/unit/layout policy.
    pub seam: String,
    /// The rhai entry function name.
    pub entry: String,
    /// The rhai source defining `entry` (+ helpers).
    pub source: String,
    /// Deterministic (fresh rhai scope per invoke). Convergent seams (merge, drive)
    /// must be `true`; the host-only authorize gate may be `false`.
    pub deterministic: bool,
}

#[lunco_core::on_command(SetRhaiPolicy)]
fn on_set_rhai_policy(
    trigger: On<SetRhaiPolicy>,
    backed: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
    roots: Query<&lunco_usd_bevy_scene::UsdPrimPath, With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    use lunco_usd::ApplyUsdOp;
    use lunco_usd_core::{LayerId, UsdOp};
    let cmd = trigger.event();
    let roots: Vec<_> = roots.iter().collect();
    let [root] = roots.as_slice() else {
        warn!(
            "[policy] SetRhaiPolicy needs exactly one mounted USD scene (found {})",
            roots.len()
        );
        return;
    };
    let Some(doc) = lunco_usd::twin_projection::scene_document_for(
        &backed,
        &asset_server,
        root.stage_handle.id(),
    ) else {
        warn!(
            "[policy] the mounted scene is not Twin document-backed; open it through a Twin to author a policy"
        );
        return;
    };
    let mounted_root = root.path.trim_end_matches('/');
    let mounted_root = if mounted_root.is_empty() {
        "/"
    } else {
        mounted_root
    };

    // USD prim names are identifier-like — sanitize the seam/name into one.
    let base = if cmd.name.is_empty() {
        &cmd.seam
    } else {
        &cmd.name
    };
    let mut name: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if name.is_empty() {
        name = "policy".to_string();
    }
    let policies_path = if mounted_root == "/" {
        "/Policies".to_string()
    } else {
        format!("{mounted_root}/Policies")
    };
    let prim = format!("{policies_path}/{name}");
    let root = LayerId::root();

    // Idempotent: define_prim + attribute overwrite → re-issuing hot-replaces.
    // String values are RAW — `SetAttribute` authors them verbatim and the writer
    // escapes on serialize (see the op's string branch). No hand-escaping here: the
    // old `format!("{:?}")` produced Rust-debug quoting, not USDA delimiting, and
    // silently corrupted any multi-line rhai `source`.
    let ops = vec![
        UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: mounted_root.into(),
            name: "Policies".into(),
            type_name: Some("Scope".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: policies_path,
            name,
            type_name: Some("LunCoPolicy".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::SetAttribute {
            edit_target: root.clone(),
            path: prim.clone(),
            name: "lunco:policy:seam".into(),
            type_name: "string".into(),
            value: cmd.seam.clone(),
        },
        UsdOp::SetAttribute {
            edit_target: root.clone(),
            path: prim.clone(),
            name: "lunco:policy:entry".into(),
            type_name: "string".into(),
            value: cmd.entry.clone(),
        },
        UsdOp::SetAttribute {
            edit_target: root.clone(),
            path: prim.clone(),
            name: "info:sourceCode".into(),
            type_name: "string".into(),
            value: cmd.source.clone(),
        },
        UsdOp::SetAttribute {
            edit_target: root,
            path: prim.clone(),
            name: "lunco:policy:deterministic".into(),
            type_name: "bool".into(),
            value: cmd.deterministic.to_string(),
        },
    ];
    for op in ops {
        commands.trigger(ApplyUsdOp {
            doc_id: doc,
            parent_gen: None,
            op,
        });
    }
    info!(
        "[policy] SetRhaiPolicy authored `{prim}` (seam '{}') — journals + projects",
        cmd.seam
    );
}

lunco_core::register_commands!(on_set_rhai_policy);

#[cfg(all(test, feature = "networking", not(target_arch = "wasm32")))]
mod policy_projection_tests {
    use super::{append_usd_policies, AuthoredPolicy};
    use lunco_usd_bevy_core::canonical::{CanonicalStage, CanonicalStages};
    use lunco_usd_core::StageRecipe;

    fn extract_usd_policies(canonical: &CanonicalStages) -> Vec<AuthoredPolicy> {
        let mut out = Vec::new();
        for (stage_id, stage) in canonical.iter() {
            let view = stage.view();
            append_usd_policies(&view, stage_id, &mut out);
        }
        out
    }

    /// A `LunCoPolicy` prim authored in the scene USD is read into a `PolicyDef` —
    /// the "settable in USD" half of proper policies. The projector then hands this
    /// to `project_policies`, so a scene-authored (or journal-synced) policy
    /// activates its rhai hook with no bespoke broadcast.
    #[test]
    fn extracts_lunco_policy_prims_from_composed_stage() {
        const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n\
            \x20   def LunCoPolicy \"takeover\"\n    {\n\
            \x20       string lunco:policy:seam = \"control.authority.take\"\n\
            \x20       string lunco:policy:entry = \"may_take_control\"\n\
            \x20       string info:sourceCode = \"fn may_take_control(ctx){true}\"\n\
            \x20       bool lunco:policy:deterministic = false\n    }\n}\n";

        let mut stages = CanonicalStages::default();
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("build stage");
        stages.insert(bevy::asset::AssetId::invalid(), cs);

        let policies = extract_usd_policies(&stages);
        assert_eq!(policies.len(), 1, "one LunCoPolicy prim → one PolicyDef");
        let p = &policies[0];
        assert_eq!(p.seam, "control.authority.take");
        assert_eq!(p.entry, "may_take_control");
        assert!(
            p.inline_source
                .as_deref()
                .unwrap_or_default()
                .contains("may_take_control"),
            "inline source carried verbatim"
        );
        assert!(p.source_path.is_none(), "no file ref authored");
        assert!(!p.deterministic, "authored deterministic=false is read");
    }

    /// A **file-backed** policy authors an `asset`-typed `info:sourceAsset`
    /// (`@…rhai@`) instead of inline source — the content-plane form. The extractor
    /// reads the authored path (via `UsdRead::asset`, which a plain `String` read would
    /// miss); the projector resolves it to text through the asset server. No inline
    /// source is present, so the file wins here.
    #[test]
    fn extracts_file_backed_policy_source_path() {
        const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n\
            \x20   def LunCoPolicy \"drive\"\n    {\n\
            \x20       string lunco:policy:seam = \"rover.drive\"\n\
            \x20       string lunco:policy:entry = \"drive\"\n\
            \x20       asset info:sourceAsset = @scripting/policy/control_authority.rhai@\n\
            \x20   }\n}\n";

        let mut stages = CanonicalStages::default();
        stages.insert(
            bevy::asset::AssetId::invalid(),
            CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
                .expect("build stage"),
        );
        let policies = extract_usd_policies(&stages);
        assert_eq!(policies.len(), 1, "one file-backed LunCoPolicy prim");
        let p = &policies[0];
        assert_eq!(p.seam, "rover.drive");
        assert!(p.inline_source.is_none(), "no inline source authored");
        assert_eq!(
            p.source_path.as_deref(),
            Some("scripting/policy/control_authority.rhai"),
            "the asset-typed sourcePath is read as its authored path"
        );
    }

    /// **Live rhai editing (no file system).** Editing a `LunCoPolicy`'s `source`
    /// attribute at runtime is a `SetAttribute` on the composed stage; the projector
    /// re-reads the NEW source (not a cached initial value). Wired end to end this is
    /// "dynamically edit a rover's rhai behaviour → the projector re-runs (change-
    /// gated on stage generation) → `project_policies` hot-replaces the hook", and —
    /// because the edit is a USD doc op — it journals so every peer re-projects the
    /// same new source. This proves the read half against a live edit.
    #[test]
    fn projector_reads_live_edited_source() {
        const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n\
            \x20   def LunCoPolicy \"drive\"\n    {\n\
            \x20       string lunco:policy:seam = \"rover.drive\"\n\
            \x20       string lunco:policy:entry = \"drive\"\n\
            \x20       string info:sourceCode = \"fn drive(c){1}\"\n\
            \x20       bool lunco:policy:deterministic = true\n    }\n}\n";

        let id = bevy::asset::AssetId::invalid();
        let mut stages = CanonicalStages::default();
        stages.insert(
            id,
            CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
                .expect("build stage"),
        );
        assert_eq!(
            extract_usd_policies(&stages)[0].inline_source.as_deref(),
            Some("fn drive(c){1}")
        );

        // Dynamically edit the rhai source on the LIVE stage — a `SetAttribute`, no
        // file touched. (Prim path taken from the live stage API, so no openusd import.)
        let prim = stages
            .get(id)
            .unwrap()
            .view()
            .prim_paths()
            .into_iter()
            .find(|p| p.to_string() == "/World/drive")
            .expect("policy prim present");
        let new_src = lunco_usd_core::author::parse_attribute_value("string", "\"fn drive(c){2}\"")
            .expect("parse");
        stages
            .get(id)
            .unwrap()
            .author_attribute(&prim, "info:sourceCode", "string", new_src)
            .expect("author live edit");

        assert_eq!(
            extract_usd_policies(&stages)[0].inline_source.as_deref(),
            Some("fn drive(c){2}"),
            "the projector reads the live-edited rhai source, not the initial value"
        );
    }
}

/// The shared, headless-safe core: the persistent world shell, physics, cosim,
/// USD scene load, mobility/hardware/controller/avatar, environment, the HTTP
/// API, and networking. Added unconditionally by both the GUI and the server, so
/// the two binaries can never drift.
///
/// The render plugins are configured in [`default_plugins`] (added before this);
/// here every plugin is pure-CPU sim/state. USD visual sync only writes the
/// mesh/material asset stores (never touches a GPU device), so it is safe in
/// headless mode.
pub struct LunCoSimCorePlugin {
    pub headless: bool,
    #[cfg(feature = "ui")]
    render_profile: LunCoSimRenderProfile,
}

/// The luncosim's one physics configuration.
///
/// A rigid body's `Position` is the collision pose and the bridge owns the
/// `Position` ↔ USD `Transform` transfer. Avian's render interpolation runs
/// after the fixed bridge writeback, while its FixedFirst completion restores
/// the authoritative stepped pose before the next bridge READ. The eased value
/// is presentation-only between physics steps, and camera/billboard paths
/// consume that same rendered pose.
fn luncosim_physics_plugins() -> impl PluginGroup {
    PhysicsPlugins::default()
        .with_collision_hooks::<lunco_usd::UsdCollisionFilter>()
        .set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all())
}

#[cfg(test)]
mod physics_configuration_tests {
    use super::*;
    use avian3d::prelude::{
        NoRotationEasing, NoTranslationEasing, RigidBody, RotationInterpolation,
        TranslationInterpolation,
    };

    #[test]
    fn physical_bodies_receive_render_interpolation() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(luncosim_physics_plugins());

        let body = app.world_mut().spawn(RigidBody::Dynamic).id();

        assert!(
            app.world().get::<TranslationInterpolation>(body).is_some(),
            "a physical body's rendered translation must be eased between solved poses"
        );
        assert!(
            app.world().get::<RotationInterpolation>(body).is_some(),
            "a physical body's rendered rotation must be eased between solved poses"
        );
        assert!(
            app.world().get::<NoTranslationEasing>(body).is_none(),
            "the bridge must not disable rendered translation easing"
        );
        assert!(
            app.world().get::<NoRotationEasing>(body).is_none(),
            "the bridge must not disable rendered rotation easing"
        );
    }
}

impl Plugin for LunCoSimCorePlugin {
    fn build(&self, app: &mut App) {
        let args: Vec<String> = std::env::args().collect();

        // Asset and loaded-stage validation is a shared headless/UI service;
        // install it once with the simulator core rather than coupling it to
        // the scene mutation command crate.
        app.add_plugins(lunco_scene_validation::SceneValidationPlugin);

        // THE RENDER GATE — and the whole of it.
        //
        // Domain crates state appearance as `lunco_render` INTENT (`PbrLook`) next
        // to their `Mesh3d` and never name a material. `LuncoRenderPlugin` — the one
        // `bevy_pbr` consumer in the graph — is what turns intent into a real
        // `MeshMaterial3d`. Headless simply does not add it.
        //
        // That is why there is no `#[cfg(feature = "render")]` anywhere in the
        // simulation crates: the gate is *which plugins you add*, not conditional
        // compilation threaded through the domain. A scene therefore keeps its full
        // appearance data on the server — inspectable, journalable, replicable — it
        // just isn't given a GPU material.
        //
        // The `#[cfg(feature = "ui")]` here is the ONE place conditional compilation
        // appears, and it has to: `lunco-render-bevy` is an OPTIONAL dependency under
        // `ui`, which is what stops the `--no-ui` server from LINKING bevy_pbr (→
        // bevy_render → wgpu + naga), not merely from running it. The runtime
        // `!headless` check remains for a `ui`-built binary launched headless.
        // See docs/architecture/render-decoupling.md.
        // Run-condition effectiveness reporting. In `LunCoSimCorePlugin` rather
        // than the UI plugin because the gates it watches (celestial cadence,
        // view-model producers) exist headless too, and a gate that stops gating
        // costs the same on a server as it does in the GUI.
        app.add_plugins(lunco_core::gate::GatePlugin);
        // TEMPORARY: chassis smoothness census, off unless `LUNCO_JITTER_CSV` is set.
        #[cfg(feature = "ui")]
        app.add_plugins(jitter_probe::JitterProbePlugin);
        // Render profile is installed before the render plugin below.
        #[cfg(feature = "ui")]
        if self.render_profile == LunCoSimRenderProfile::Fast {
            app.insert_resource(lunco_render_bevy::RenderProfile::Fast);
            info!("[render] fast profile enabled");
        }

        #[cfg(feature = "ui")]
        if !self.headless {
            app.add_plugins(lunco_render_bevy::LuncoRenderPlugin);
        }

        #[cfg(all(feature = "ui", feature = "lunco-api"))]
        if !self.headless {
            let mut record_dir = None;
            let mut record_fps = 60;
            let mut record_frames: Option<u64> = None;
            for i in 0..args.len() {
                if args[i] == "--record-offline" && i + 1 < args.len() {
                    record_dir = Some(args[i + 1].clone());
                }
                if args[i] == "--record-fps" && i + 1 < args.len() {
                    if let Ok(fps) = args[i + 1].parse::<u32>() {
                        record_fps = fps;
                    }
                }
                if args[i] == "--record-frames" && i + 1 < args.len() {
                    match args[i + 1].parse::<u64>() {
                        Ok(n) if n > 0 => record_frames = Some(n),
                        _ => warn!(
                            "--record-frames expects a positive frame count, got {:?} — ignoring",
                            args[i + 1]
                        ),
                    }
                }
            }
            if let Some(n) = record_frames {
                app.insert_resource(lunco_workbench::screenshot::OfflineRecordLimit(n));
            }
            if let Some(dir) = record_dir {
                let path = std::path::PathBuf::from(dir);
                // Route CLI recording through the same command boundary as API
                // and Rhai requests. Inserting OfflineRecordingState directly
                // would skip screenshot.rs's visual readiness gate and could
                // capture a half-loaded scene (notably before an HDRI cubemap
                // projection completes).
                app.insert_resource(lunco_workbench::screenshot::OfflineRecordingRequest {
                    output_dir: path,
                    fps: record_fps.max(1),
                });
            }
        }

        // Convenience command: `SetRhaiPolicy` authors a `LunCoPolicy` prim as USD
        // doc ops (journals → syncs → projector activates). Authoring works with or
        // without networking; the activation projector is networking-gated for now.
        register_all_commands(app);

        // `--scene <path>` is an explicit startup request. With no argument
        // the process owns only the persistent world shell; it must not
        // silently mount the safety-test sandbox and fault the session before
        // an API client has selected its scene. The resolver accepts the
        // shipped asset-root spelling, a workspace/cwd relative path, or an
        // absolute filesystem path. The latter two are required for running a
        // custom Twin without copying it into assets/.
        let scene_path = startup_scene_arg(&args);

        app.insert_resource(ScenePath(scene_path))
            // Match the workbench theme's backdrop so the window's first-frame
            // clear lines up with egui's panel fill (no "left hairline" at panel
            // boundaries under non-integer DPRs). Harmless headless.
            .insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)))
            .insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ))
            .insert_resource(avian3d::prelude::Gravity::ZERO)
            // The luncosim's gravity BEFORE any scene loads. Lunar, because every
            // vehicle in it is: the rovers' drivetrains, the lander's struts and
            // its propellant budget are all sized for 1.62. A scene overrides this
            // through its own `UsdPhysicsScene`; this is only what an empty
            // viewport uses, and the value teardown restores when a scene unloads.
            .insert_resource(SANDBOX_GRAVITY)
            // A scene SHOULD override gravity — that is what its `UsdPhysicsScene`
            // is for. What it must not do is leave that override behind: unloading
            // a lunar scene restores the luncosim's own value, so whatever loads
            // next starts from the app's baseline rather than the last scene's.
            .add_systems(lunco_core::SceneTeardown, |mut commands: Commands| {
                commands.insert_resource(SANDBOX_GRAVITY)
            })
            // Studio lighting for the luncosim — a generic editor scene, NOT a
            // calibrated lunar surface (the canonical `LunarSun` defaults
            // crush the dark blueprint ground to black). Inserted BEFORE
            // `EnvironmentPlugin` so its `init_resource` keeps this one
            // authoritative resource. The sun spawn AND every camera's
            // exposure read it, so lux and EV stay matched. Tunable live via
            // `SetEnvironmentLight`.
            .insert_resource(lunco_environment::LunarSun::default())
            // Persistent world shell: one BigSpace root + `WorldGrid` + the
            // persistent `OriginAnchor`/`FloatingOrigin`. The validation plugin (debug builds only, logs
            // errors, never panics) is ENABLED: WorldRoot is Transform-free —
            // big_space-canonical — now that the Phase 5 bridge owns BOTH
            // things the root `Transform` was load-bearing for (avian's GT
            // sync and its root-anchored ColliderTransform propagation). The
            // validator is the guard that keeps new spawn paths canonical.
            .add_plugins(BigSpaceDefaultPlugins);
        #[cfg(debug_assertions)]
        gate_big_space_hierarchy_validation(app);
        app
            // EntityCount is cheap and useful any time we look at perf.
            .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
            // `with_collision_hooks` installs the ONE pair filter avian allows per
            // app: authored `PhysicsFilteredPairsAPI` pairs (`lunco-usd-avian`'s
            // `UsdCollisionFilter`). Anything else that must veto a contact belongs
            // in that hook rather than in a second one — there is no second slot.
            .add_plugins(luncosim_physics_plugins())
            // Whoever installs physics installs its readiness gate: terrain/obstacle
            // subsystems suspend *integration* (avian's `Time<Physics>`) while their
            // colliders bake, instead of pausing the world clock. See `lunco-physics`.
            .add_plugins(lunco_physics::PhysicsGatePlugin)
            // Phase 5: physics stops sharing GlobalTransform with the render
            // world. Disables ALL of avian's f32 transform sync — including
            // `propagate_before_physics`, the third plain-GT whole-tree writer
            // (the measured 1-in-5–9 render strobe; doc 45 addendum) — and owns
            // the Position ↔ (cell, Transform) sync in the f64 cell chain. The
            // 2026-07-09 narrow_phase island panic was the old bridge dirtying
            // every static's Position every tick (whole-world contact churn);
            // this bridge is shadow-gated: a body syncs only when an external
            // writer actually moved it. Must be added AFTER PhysicsPlugins
            // (it overrides PhysicsTransformConfig).
            .add_plugins(lunco_usd::BigSpacePhysicsBridgePlugin)
            // `lunco_physics::PhysicsGatePlugin` owns the single solver-resolution
            // contract and installs eight Avian substeps for every host. Keeping
            // this choice at the physics owner prevents the GUI, server, and web
            // application paths from silently simulating different mechanics.
            .add_plugins(CoSimPlugin)
            .add_plugins(lunco_core::LunCoCorePlugin)
            // Renderer-independent exposure aggregation is kept in its own
            // production crate. It remains in the shared core path so GUI and
            // headless hosts publish identical facts, while exposure edits no
            // longer recompile this application composition root.
            .add_plugins(lunco_luncosim_exposures::RuntimeExposuresPlugin)
            .add_plugins(lunco_core::WorldShellPlugin)
            // Parameter telemetry — the PRODUCER of `SampledParameter`. Its consumer
            // side (`lunco_api`'s `sampled_param_observer`, i.e. `SubscribeTelemetry`,
            // plus `TelemetryResponse::from_sampled` and core's logger) was already
            // shipped and wired; this plugin was the one missing link, so the API
            // advertised parameter telemetry that could never arrive. Costs nothing
            // until someone authors a `Parameter` (the sampler is `run_if`-gated on
            // one existing), and it samples on the FIXED clock, so headless runs get a
            // stable telemetry rate instead of one that tracks the frame rate.
            .add_plugins(lunco_telemetry::LunCoTelemetryPlugin)
            // Canonical Twin change-journal (op log). CORE substrate, not UI:
            // it must exist on the headless server + every client so authored
            // edits are recorded (the domain registries' `wire_*_journal_handle`
            // systems fire on `resource_added::<JournalResource>` and attach a
            // recorder to each DocumentHost). Previously added only by the
            // workbench UI plugin, so a headless networked host journaled
            // nothing — the blocker for journal-on-wire sync. Pure lifecycle
            // observers + resources; no GPU/egui. The workbench add is now
            // guarded to avoid a double-add (double observers).
            .add_plugins(lunco_doc_bevy::TwinJournalPlugin)
            // GravityPlugin now rides in via CelestialPlugin below (guarded).
            .add_plugins(EnvironmentPlugin)
            .add_plugins(TerrainPlugin)
            // Procedural crater + rock field generator. Owns the shared
            // `ObstacleFieldSpec` + `UpdateObstacleFieldSpec` only; the real ground
            // is the USD DEM terrain, which observes that command and stamps
            // craters / scatters rocks into its own grid.
            .add_plugins(ObstacleFieldPlugin)
            // Streamed, dynamically-LOD'd terrain (DEM tiles + heightfield
            // colliders). Inert at M0 (config only); see lunco-terrain-surface
            // and docs/architecture/terrain-substrate.md.
            .add_plugins(TerrainSurfacePlugin)
            // Celestial stack (doc 43): dormant unless the SCENE asks for it. Bodies
            // are authored in USD (`LunCoCelestialBodyAPI` — reference
            // `assets/celestial/solar_system.usda`), and every celestial subsystem
            // gates on that authored fact, so the flat luncosim arena gets no sky at
            // all. The luncosim avatar remains independent of BigSpace origin
            // ownership. The
            // generic link kernel (doc 49) is always on — it needs no hierarchy —
            // and publishes `LinkState` + `link.aos`/`link.los`, NOT `comms:*`
            // ports (there is no comms subsystem to own them).
            .insert_resource(lunco_celestial::CelestialConfig {
                spawn_observer_camera: false,
            })
            .add_plugins(lunco_celestial::CelestialPlugin)
            // Real VSOP2013/ELP body positions on ALL platforms (wasm too) —
            // this is the explicit provider required by orbital scenes.
            .add_plugins(lunco_celestial_ephemeris::EphemerisPlugin)
            // Connectivity rides on the generic link kernel the CelestialPlugin
            // registers (doc 49): geometry in Rust, verdict via the `link.connected`
            // hook, routing authored in rhai over `query("Links")`. There is no comms
            // Rust plugin. Scene-local endpoints opt into pose tracking via
            // `lunco:solarTracked`.
            .add_plugins(LunCoHardwarePlugin)
            .add_plugins(LunCoMobilityPlugin)
            // USD scene load + avian collider build + cosim wiring —
            // server-authoritative, headless-safe.
            .add_plugins(UsdPlugins)
            // Vessel input + possession command observers. Headless-safe:
            // leafwing's InputManager rides on bevy_input (no winit), so a server
            // just produces no input while the Drive/Brake/Possess command
            // observers + wire-type registrations the host needs stay live.
            .add_plugins(LunCoControllerPlugin)
            .add_plugins(LunCoAvatarPlugin)
            .add_plugins(lunco_scripting::LunCoScriptingPlugin)
            // Default scene-wide fill for scenes that author no lighting; a
            // scene-authored UsdLux light takes ambient over.
            .insert_resource(bevy::light::GlobalAmbientLight {
                brightness: 0.0,
                ..Default::default()
            })
            .add_systems(Startup, setup_luncosim)
            .add_systems(Startup, load_startup_scene_on_boot.after(setup_luncosim))
            // Fail loud if the requested `--scene` never loads (e.g. a wrong
            // path that resolves to a missing asset). Without this the app
            // silently boots a scene-less world (only procedural terrain /
            // obstacles), which masks the real error.
            .add_systems(Update, startup_scene_failguard)
            .add_observer(startup_twin_scan_failguard)
            // BigSpace's internal stationary pruning still pays its channeled
            // worker-scope setup on a fully clean frame. Gate the whole
            // high-precision set at the application boundary using the same
            // spatial invalidation inputs, so the maintained dependency stays
            // the only propagation owner and clean frames do no fan-out.
            .configure_sets(
                PostStartup,
                BigSpaceSystems::LocalFloatingOrigins.run_if(local_origin_propagation_due),
            )
            .configure_sets(
                PostUpdate,
                BigSpaceSystems::LocalFloatingOrigins.run_if(local_origin_propagation_due),
            )
            .configure_sets(
                PostStartup,
                BigSpaceSystems::PropagateHighPrecision.run_if(high_precision_propagation_due),
            )
            .configure_sets(
                PostUpdate,
                BigSpaceSystems::PropagateHighPrecision.run_if(high_precision_propagation_due),
            )
            .configure_sets(
                PostStartup,
                BigSpaceSystems::PropagateLowPrecision.run_if(low_precision_propagation_due),
            )
            .configure_sets(
                PostUpdate,
                BigSpaceSystems::PropagateLowPrecision.run_if(low_precision_propagation_due),
            )
            // Cosim pipeline ordering: worker responses land in Update; the
            // fixed loop then propagates, applies, and dispatches the next
            // Modelica communication point.
            .configure_sets(
                FixedUpdate,
                (
                    PropagateCosimSet::Propagate,
                    ApplyForcesCosimSet::ApplyForces,
                    ModelicaSet::SpawnRequests,
                )
                    .chain(),
            );
        // Dynamic USD bodies are first promoted in `ActivateDynamicBodies`.
        // The terrain support projection must observe that promotion before it
        // decides whether physics may resume; plugin insertion order is not a
        // valid synchronization contract for a streamed physics world.
        // Experiment result-artifact persistence — CORE (not networking): a run's
        // trajectory is written to `<twin>/results/<id>.json` through the
        // cross-platform storage layer and restored on demand, so single-player run
        // history survives a restart. Networking additionally distributes the same
        // files via the content plane. Guarded so a config without experiments /
        // workspace simply skips.
        app.add_systems(
            Update,
            write_run_result_artifact.run_if(
                resource_exists::<lunco_experiments::ExperimentRegistry>
                    .and_then(resource_exists::<lunco_workspace::WorkspaceResource>),
            ),
        );
        // Load half is change-driven on the registry (a definition synced, a run
        // completed, a status flip) — so a just-arrived result file is picked up on
        // the next registry change instead of by polling.
        app.add_systems(
            Update,
            load_run_result_artifacts.run_if(
                resource_changed::<lunco_experiments::ExperimentRegistry>
                    .and_then(resource_exists::<lunco_workspace::WorkspaceResource>),
            ),
        );

        // Dismiss the HTML loading screen once the first frame paints (wasm-only;
        // no-op on native). Pairs with `web/index.html` → `lunco-boot.js`.
        app.add_plugins(lunco_web::WebReadyPlugin);

        // HTTP automation bridge — native `--api` server / wasm JS bridge. Linked
        // in the GUI and the headless compile server alike.
        #[cfg(feature = "lunco-api")]
        app.add_plugins(lunco_api::LunCoApiPlugin::default());

        // Twin history for headless (`lunco-luncosim-server` / any `--no-ui`
        // host): the SAME twin-folder-scoped persistence the GUI uses — load on
        // twin open, save on `DocumentSaved` + debounced periodic — so a running
        // server's collaborative edit history survives restarts, in the project
        // folder. One code path for GUI + headless (DRY). Writing is opt-in per
        // twin (`[journal] persist = true`); without it the journal is
        // session-only and nothing touches disk.
        if self.headless {
            // The workspace session — `setup_luncosim`'s twin-load path and the
            // journal persistence both need it. The SAME plugin the GUI gets:
            // `WorkspacePlugin` lives in `lunco-workspace`, which this binary
            // already links, and it is headless by construction (bevy substrate
            // only, no render/winit/egui).
            //
            // The WHOLE plugin, not a hand-picked subset: it also registers
            // `OpenTwin` and the folder-scan pipeline, without which a server
            // could run a twin's scenarios but never mount the twin they belong
            // to.
            app.add_plugins(lunco_workspace::WorkspacePlugin);
        }

        // Multiplayer. Native: `--host [port]` / `--connect <addr>`; browser:
        // `?connect=host`. With no address the plugin still loads client-capable
        // but idle (single-player) so the in-sim *Connect* button / `JoinServer`
        // command can dial a server at runtime.
        #[cfg(feature = "networking")]
        {
            let mode = lunco_networking::NetworkMode::resolve(self.headless);
            info!("[net] networking mode: {mode:?}");
            app.add_plugins(lunco_networking::LunCoNetworkingPlugin { mode });
            // Client-side netcode over avian bodies: snapshot interpolation,
            // prediction, rollback, reconciliation, correction smoothing. Used to
            // ride along inside `lunco_scene_commands::commands::SpawnCommandPlugin`
            // (which still registers `apply_replicated_spawns`, the spawn half — see
            // `lunco_core::NetcodeSet` for how the two halves stay ordered). Added
            // here, in `LunCoSimCorePlugin`, so BOTH the GUI and the headless server
            // get it exactly once; gated on `networking` like every other
            // `lunco_networking` use in this crate.
            app.add_plugins(lunco_networking::prediction::NetcodePredictionPlugin);
            // Scenario distribution Phase 4: once a connected client has fully
            // downloaded the host's advertised scenario, load its entry scene from
            // the cache mounted as a Twin root (read-only consume). The bridge lives here —
            // the assembly crate that owns both the wire (`lunco-networking`) and
            // the scene loader (`lunco_usd::LoadScene`) — keeping each of those
            // crates free of the other.
            app.add_systems(Update, load_ready_scenario);
            // Layer B: project peers' live journal edits onto the local scene
            // (bidirectional — clients see the host's edits, the host sees
            // clients'; no-op when no scenario/journal is present).
            app.add_systems(Update, replay_scenario_journal);
            // Same Layer B for Modelica models — the journal plane is domain-generic;
            // this is the parallel per-domain consume leg for `DomainKind::Modelica`.
            app.add_systems(Update, replay_scenario_journal_modelica);
            // Same Layer B for scripts — a recorded `ScriptOp::SetSource` (live
            // rover-behaviour edit) projects onto a peer's `ScriptDocument`.
            app.add_systems(Update, replay_scenario_journal_script);
            // Same Layer B for experiment *definitions* (`DomainKind::Experiment`):
            // a peer's sweep setup projects onto the local ExperimentRegistry.
            app.add_systems(Update, replay_scenario_journal_experiment);
            // Same Layer B for shaders (`DomainKind::Shader`): a peer's WGSL edit
            // projects onto the local ShaderRegistry + hot-reloads Assets<Shader>.
            app.add_systems(Update, replay_scenario_journal_shader);
            // Same Layer B for config/registration domains: obstacle-field spec
            // (replaces the old bespoke broadcast — now bidirectional), rhai tool
            // libraries, and mission timelines. Each installs a peer's journaled
            // op onto the local resource/registry/store.
            app.add_systems(
                Update,
                (
                    replay_scenario_journal_obstacle,
                    replay_scenario_journal_tools,
                    replay_scenario_journal_timeline,
                ),
            );
            // Presence/rebuild resources are consumed by the systems below for any
            // role; init here (idempotent with the host-side init) so a standalone
            // or client app never hits a missing resource.
            app.init_resource::<lunco_networking::sync::PendingRunStatus>();
            app.init_resource::<lunco_networking::sync::RequestManifestRebuild>();
            // Result artifacts themselves are written/loaded by the CORE persistence
            // systems (registered unconditionally below — storage-backed, all
            // platforms). Networking only adds the *distribution* trigger: when a
            // run finishes on the host, ask for an immediate manifest rebuild so
            // already-connected peers pull the just-written result now.
            app.add_systems(
                Update,
                request_rebuild_after_result
                    .run_if(resource_exists::<lunco_experiments::ExperimentRegistry>),
            );
            // Presence plane: host broadcasts run-status transitions; client
            // applies them so a synced experiment's row advances live. Guarded on
            // the registry existing so a config without experiments just skips
            // (the MessageReaders would otherwise have no registered messages).
            app.add_systems(
                Update,
                (broadcast_run_status, apply_run_status)
                    .run_if(resource_exists::<lunco_experiments::ExperimentRegistry>),
            );
            // Connect-menu bridge adapter + egui presence/tutorial overlays. Pulls
            // bevy_egui, so it's GUI-only and gated on `ui` (CQ-601) — the headless
            // server omits it. The host still answers runtime JoinServer/LeaveServer
            // via the networking plugin's typed command path (not this bridge).
            #[cfg(feature = "ui")]
            app.add_plugins(lunco_networking::ui::LunCoNetworkingUiPlugin);
        }

        // USD→terrain projection (`lunco-usd-terrain`): an authored terrain prim with
        // `lunco:assetMode="dem"` gets a DEM heightfield built onto it from its child
        // layer prims, and hand edits author back onto the document's runtime layer.
        // Core (not GUI-gated): the headless server needs the collider for
        // deterministic physics, and the crate links no render code.
        app.add_plugins(lunco_usd_terrain::UsdTerrainPlugin);
        // The activation gate stays here — it is the assembly point that sees both the
        // terrain request and `lunco-usd`'s `GroundColliderPending`.
        app.add_systems(
            Update,
            track_ground_collider_pending.after(lunco_usd_terrain::UsdTerrainSet::Bridge),
        );
        // Policy projection is a core USD→Rhai path, not a networking feature.
        // Network peers receive the same `LunCoPolicy` prim through the journal,
        // while a standalone app can author and hot-replace the same policy locally.
        app.add_systems(
            Update,
            project_usd_policies.after(lunco_scripting::source_asset::RhaiSourceAssetSet),
        );
        // LogDiagnosticsPlugin is loud (a multi-line summary every second) — gate
        // it on `--log-diag`.
        if args.iter().any(|a| a == "--log-diag") {
            app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
        }
    }
}

/// Hold dynamic-body activation while an actual DEM terrain build is in flight.
///
/// The USD terrain bridge is ordered before this system and its deferred commands
/// are flushed at that boundary, so the query sees the authoritative
/// [`DemTerrainRequest`] for a newly composed terrain in the same update. A query
/// over every [`UsdPrimPath`] is incorrect: most USD prims are not terrain and
/// would keep the entire simulation kinematic until an arbitrary timeout.
///
/// The request is removed together with the finished collider/oracle by the
/// terrain-surface owner. A declared-but-uninstalled Twin DEM carries
/// [`lunco_usd_terrain::DemDatasetPending`] instead of a build request; that
/// state is equally not ready for dynamic admission. This crate only mirrors
/// those domain-owned readiness states into the USD-simulation activation
/// resource.
fn track_ground_collider_pending(
    building: Query<
        (),
        Or<(
            With<lunco_terrain_surface::DemTerrainRequest>,
            With<lunco_usd_terrain::DemDatasetPending>,
        )>,
    >,
    mut pending: ResMut<lunco_usd::GroundColliderPending>,
) {
    pending.0 = !building.is_empty();
}

#[cfg(test)]
mod ground_collider_gate_tests {
    use super::*;

    #[test]
    fn only_an_active_dem_request_holds_dynamic_activation() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<lunco_usd::GroundColliderPending>()
            .add_systems(Update, track_ground_collider_pending);

        // A loaded USD stage contains many prims that are not terrain. They do
        // not participate in this gate.
        let stage = Handle::<UsdStageAsset>::default();
        app.world_mut().spawn(UsdPrimPath {
            stage_handle: stage,
            path: "/Rover/Chassis".into(),
        });
        app.update();
        assert!(!app.world().resource::<lunco_usd::GroundColliderPending>().0);

        let terrain = app
            .world_mut()
            .spawn(lunco_terrain_surface::DemTerrainRequest {
                uri: "terrain/site".into(),
                half_window: 1.0,
                target_res: 0,
                lod_viz: false,
                collider_ring: false,
                collider: lunco_terrain_surface::TerrainColliderSettings::default(),
            })
            .id();
        app.update();
        assert!(app.world().resource::<lunco_usd::GroundColliderPending>().0);

        app.world_mut()
            .entity_mut(terrain)
            .remove::<lunco_terrain_surface::DemTerrainRequest>();
        app.update();
        assert!(!app.world().resource::<lunco_usd::GroundColliderPending>().0);
    }

    #[test]
    fn an_uninstalled_twin_dem_keeps_dynamic_activation_held() {
        let mut app = App::new();
        app.init_resource::<lunco_usd::GroundColliderPending>()
            .add_systems(Update, track_ground_collider_pending);

        let pending = app
            .world_mut()
            .spawn(lunco_usd_terrain::DemDatasetPending::new(
                "summer-space-school/apollo15",
            ))
            .id();
        app.update();
        assert!(app.world().resource::<lunco_usd::GroundColliderPending>().0);

        app.world_mut().entity_mut(pending).despawn();
        app.update();
        assert!(!app.world().resource::<lunco_usd::GroundColliderPending>().0);
    }
}

pub struct LunCoSimHeadlessPlugin {
    /// Host execution policy. Max-speed mode uses an explicit fixed duration
    /// and a zero-wait runner; realtime mode remains wall-clock paced.
    pub execution_mode: lunco_core::SimulationExecutionMode,
}

impl Default for LunCoSimHeadlessPlugin {
    fn default() -> Self {
        Self {
            execution_mode: lunco_core::SimulationExecutionMode::Realtime,
        }
    }
}

impl Plugin for LunCoSimHeadlessPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.execution_mode);
        // A scenario's presentation intents remain valid in a headless run, but
        // there is deliberately no workbench/HUD to receive them. Acknowledge
        // the explicit presentation surface as no-ops so one scenario works in
        // interactive and acceptance modes; every other unknown command still
        // fails loudly through the normal reflection dispatcher.
        app.insert_resource(lunco_scripting::bridge_core::IgnoredScenarioCommands::new(
            [
                "SetHint",
                "SetObjectives",
                "Spotlight",
                "ClearSpotlight",
                "FocusPanel",
                "SetTourStep",
                "ClearTour",
            ],
        ));

        // Modelica COMPILE CORE only (channels + worker thread + `.mo` asset
        // loader + compile-dispatch systems) — NO egui/viz/workbench. Windowed
        // builds get this transitively via `ModelicaWorkbenchPlugin`; headless
        // must add it directly or the cosim `on_load_scene` observer panics on a
        // missing `Res<ModelicaChannels>`. The server runs Modelica cosim models
        // authoritatively, so it needs the real compile path, not a stub.
        app.add_plugins(lunco_modelica_core::ModelicaCorePlugin);

        // Spawn-command CORE (runtime spawn/move/property commands + the
        // `apply_net_replication` system that tags dynamic scene bodies with
        // `NetReplicate`). Windowed builds get this transitively via
        // `SceneEditPlugin`; without it the headless host replicates NOTHING
        // (the connect baseline is empty) because nothing marks the rovers. The
        // gizmo/selection/physics-viz halves of `SceneEditPlugin` stay UI-only.
        app.add_plugins(lunco_scene_commands::commands::SpawnCommandPlugin);

        // No winit event loop drives updates headless. Realtime mode uses the
        // fixed cadence as the server's wall-clock pacing; max-speed mode feeds
        // one fixed duration per update and removes the wait entirely. Both
        // modes still execute the same schedules and the same causal barrier.
        let wait = match self.execution_mode {
            lunco_core::SimulationExecutionMode::Realtime => {
                std::time::Duration::from_secs_f64(1.0 / lunco_core::FIXED_HZ)
            }
            lunco_core::SimulationExecutionMode::MaxSpeed => {
                app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                    std::time::Duration::from_secs_f64(lunco_core::SECS_PER_TICK),
                ));
                std::time::Duration::ZERO
            }
        };
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(wait));

        info!(
            "[luncosim] running HEADLESS (--no-ui), execution={:?}: no window/GPU/egui; local simulation only",
            self.execution_mode
        );
    }
}

/// Resource that holds the optional asset-source-relative path of the scene to
/// load on Startup. `None` means an intentionally empty world shell. It is
/// initialised from the `--scene` CLI arg by [`LunCoSimCorePlugin`].
#[derive(Resource)]
pub struct ScenePath(pub Option<String>);

// `set_parent_in_place` is `disallowed_methods`-banned for its atomicity
// hazard (a `GridAnchor`/`RigidBody` parented after spawn can be mis-tagged
// `RigidBody::Static`). `ensure_world_root` may parent the big_space root →
// Grid internally; it is not a rigid body / GridAnchor, so that hazard doesn't
// apply. Locally allowed.
#[allow(clippy::disallowed_methods)]
fn setup_luncosim(world: &mut World) {
    // The persistent world shell (BigSpace root + `WorldGrid` + the single
    // `FloatingOrigin`) is owned by `WorldShellPlugin`. `ensure_world_root` is a
    // defensive create-or-get so the shell exists before any scene loads.
    //
    // The scene's SUN is no longer spawned here. It is a UsdLux `DistantLight`
    // authored in the scene itself (`sandbox_scene.usda`), instantiated by the
    // same USD loader as every other light — so a scene clear despawns it and a
    // scene load recreates it as ordinary scene content. There is no
    // Rust-spawned sun and no restore-on-switch machinery; see
    // `lunco_usd_bevy::light` for the single light path and the
    // post-load light-existence check that errors if a scene ships without one.
    let grid = lunco_core::ensure_world_root(world);
    // The shell owns topology; the application owns which grid Avian uses.
    // Bind the canonical WorldGrid explicitly for the empty/sandbox state.
    // Scene mounts replace this binding with their authored site frame when
    // celestial placement completes.
    world.insert_resource(lunco_core::ActivePhysicsFrame(grid));
}

/// Load the explicitly requested startup scene.
fn load_startup_scene_on_boot(world: &mut World) {
    let scene_path = world.resource::<ScenePath>().0.clone();

    // WEB: do NOT load a startup scene here. The generated page's autoload hook
    // (index.html → a `LoadScene` command) loads the deployment's default twin
    // (moonbase) directly. A second built-in `sandbox_scene` load here raced that
    // autoload: the twin reload's cleanup despawned the sandbox_scene entity while
    // `sync_usd_visuals` still had a deferred `insert::<UsdPrimPath>` queued for it
    // → "Entity despawned" panic → aborted wasm → dark viewport. The filesystem
    // twin-resolve below is meaningless in the browser anyway (no `twin.toml` FS).
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(scene_path) = scene_path {
        load_startup_scene(world, scene_path);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (world, scene_path);
        info!(
            "[luncosim] web startup: no built-in scene load — the page autoload hook loads the default twin directly"
        );
    }
}

/// Native/headless startup-scene load: resolve the enclosing Twin folder for
/// `scene_path` (walk up to a `twin.toml`) and enqueue its workspace scan. The
/// scan and indexing stay off the UI thread; its completion registers the Twin
/// and mounts the selected scene through the normal doc-first path. Invalid or
/// orphaned roots report an error and do not load a base-only scene. Web skips
/// this — its autoload hook loads the deployment twin directly (see
/// [`setup_luncosim`]).
#[cfg(not(target_arch = "wasm32"))]
fn load_startup_scene(world: &mut World, scene_path: String) {
    // Resolve the absolute path to find the enclosing Twin folder. This is
    // deliberately shared by shipped scenes and external Twin roots: a CLI
    // spelling must never change which document root gets mounted.
    let abs_path = resolve_scene_cli_path(&scene_path);

    // The root that owns this scene — nearest `twin.toml` ancestor, else the
    // containing folder. Shared with the runtime open path (`OpenFile` →
    // `spawn_twin_from_scene`) so boot and commands cannot disagree about what
    // "the root" is for a given file.
    let twin_root = lunco_twin::root_for_file(&abs_path);

    let scene_file = abs_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    world.insert_resource(StartupSceneGuard {
        file: scene_file.clone(),
    });

    let rel_scene_path = abs_path
        .strip_prefix(&twin_root)
        .map(lunco_assets::asset_path::slashed)
        .unwrap_or_else(|_| scene_file.clone());
    let Some(mut pending) = world.get_resource_mut::<lunco_workspace::open::PendingTwinOpens>()
    else {
        error!(
            "[luncosim] startup scene `{scene_path}` cannot begin: workspace open pipeline is not installed"
        );
        return;
    };

    // `TwinMode::open` walks and indexes the entire root synchronously, so it
    // must use the same asynchronous scan owner as an interactive OpenFile.
    // The completion path registers the asset authority before its
    // TwinAssetMounted event and therefore preserves the doc-first overlay.
    lunco_workspace::open::spawn_twin_scan(
        &twin_root,
        &mut pending,
        "StartupScene",
        Some(rel_scene_path),
        lunco_workspace::open::TwinOpenMode::Replace,
    );
    info!(
        "[luncosim] queued startup Twin scan for `{}` (scene `{scene_file}`)",
        twin_root.display()
    );
    // `--scene` is doc-backed through the same path as any workspace Twin: the
    // asset-mounted event emitted after `TwinAdded` runs the doc-first mount
    // (`open_usd_docs_on_twin_asset_mounted` → `drain_pending_twin_docs`), and
    // terrain edits stay on the incremental re-bake — `LiveRebuildExempt` +
    // `edit_confined_to_exempt_subtree` keep a terrain-confined USD edit from
    // ever reloading the scene.
}

/// Resolve a `--scene` argument without forcing every Twin into the engine's
/// shipped `assets/` tree.
///
/// Resolution order is intentionally deterministic:
///
/// 1. absolute filesystem path;
/// 2. existing path relative to the process working directory;
/// 3. an explicit `assets/...` spelling relative to the process working
///    directory (useful when invoking the binary from the repository root);
/// 4. the normal asset-root-relative spelling used by packaged launches.
///
/// We do not canonicalize here. The Twin opener should receive the user's
/// path, including a custom Twin's symlink/layout, and report a precise error
/// if it does not exist.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_scene_cli_path(input: &str) -> std::path::PathBuf {
    use std::path::{Path, PathBuf};

    let path = Path::new(input);
    if path.is_absolute() {
        return path.to_path_buf();
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cwd_relative = cwd.join(path);
    if cwd_relative.exists() {
        return cwd_relative;
    }

    if let Ok(without_assets) = path.strip_prefix("assets") {
        let asset_spelling = lunco_assets::assets_dir_abs().join(without_assets);
        if asset_spelling.exists() {
            return asset_spelling;
        }
    }

    lunco_assets::assets_dir_abs().join(path)
}

/// Tracks an explicitly requested startup scene so the two startup failguards
/// can turn a silent scan or asset-load failure into a loud, fatal error. It is
/// removed once the scene has loaded (or failed), so later runtime `LoadScene`s
/// (API / UI) — which must NOT crash the app on a bad request — are unaffected.
#[derive(Resource)]
struct StartupSceneGuard {
    /// File name of the explicitly requested startup scene.
    file: String,
}

/// Fail loud if the explicit `--scene` Twin scan or USD scene fails at startup.
///
/// The bug this guards: `--scene` paths are relative to the `assets/` source
/// root; prefixing `assets/` doubles it (`assets/assets/…`), the asset is not
/// found, and the app *silently* boots a scene-less world. Here a matching
/// `TWIN_OPEN_FAILED` / `AssetLoadFailedEvent<UsdStageAsset>` → clear error +
/// non-zero exit. Disarms on success (scene produced `UsdPrimPath` entities) so
/// runtime loads are safe.
fn startup_twin_scan_failguard(
    trigger: On<lunco_core::TelemetryEvent>,
    guard: Option<Res<StartupSceneGuard>>,
    mut commands: Commands,
) {
    let Some(guard) = guard else { return };
    if trigger.event().name != lunco_workspace::open::TWIN_OPEN_FAILED {
        return;
    }
    let lunco_core::TelemetryValue::String(detail) = &trigger.event().data else {
        return;
    };
    if !detail.starts_with("StartupScene failed:") {
        return;
    }
    error!("Startup scene `{}` failed to scan: {detail}", guard.file);
    commands.write_message(AppExit::error());
    commands.remove_resource::<StartupSceneGuard>();
}

fn startup_scene_failguard(
    guard: Option<Res<StartupSceneGuard>>,
    mut failures: MessageReader<AssetLoadFailedEvent<UsdStageAsset>>,
    scene: Query<(), With<UsdPrimPath>>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
) {
    let Some(guard) = guard else { return };

    for failed in failures.read() {
        let is_startup_scene =
            failed.path.path().file_name().and_then(|s| s.to_str()) == Some(guard.file.as_str());
        if is_startup_scene {
            error!(
                "Startup scene `{}` failed to load: {}. \
                 NOTE: `--scene` is relative to the `assets/` source root — do NOT prefix \
                 `assets/` (use `scenes/luncosim/sandbox_scene.usda`, not `assets/scenes/...`).",
                guard.file, failed.error,
            );
            exit.write(AppExit::error());
            commands.remove_resource::<StartupSceneGuard>();
            return;
        }
    }

    // Scene loaded (entities exist) → disarm so a later runtime LoadScene
    // failure (API/UI) never trips this fatal guard.
    if !scene.is_empty() {
        commands.remove_resource::<StartupSceneGuard>();
    }
}
