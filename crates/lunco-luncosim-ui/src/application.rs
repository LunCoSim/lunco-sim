//! Windowed LunCoSim application composition.
//!
//! This module owns the GPU-backed application boundary: command-line render
//! choices, Bevy's window/render plugins, offscreen capture, and presentation
//! composition. The headless runtime remains in `lunco-luncosim-runtime`.

use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;
use lunco_luncosim_core::AppExit;
use lunco_luncosim_runtime::LunCoSimRuntimePlugin;
use lunco_luncosim_services::ScenePath;
use lunco_luncosim_simulation::LunCoSimSimulationPlugin;

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

fn parse_record_preset(
    args: &[String],
) -> Result<lunco_capture::screenshot::OfflineVideoPreset, String> {
    let mut preset = lunco_capture::screenshot::OfflineVideoPreset::default();
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
        preset = lunco_capture::screenshot::OfflineVideoPreset::parse(value)?;
        index += 1;
    }
    Ok(preset)
}

fn parse_recording_args(
    args: &[String],
) -> Result<
    (
        Option<lunco_capture::screenshot::OfflineRecordingRequest>,
        Option<lunco_capture::screenshot::OfflineRecordLimit>,
    ),
    String,
> {
    let mut output_dir = None;
    let mut fps = None;
    let mut frame_limit = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--record-offline" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "`--record-offline` needs a directory or video path".to_string())?;
            if value.starts_with('-') {
                return Err(
                    "`--record-offline` needs a directory or video path, not another flag"
                        .to_string(),
                );
            }
            if output_dir.replace(value.into()).is_some() {
                return Err("`--record-offline` may only be specified once".to_string());
            }
        } else if let Some(value) = args[index].strip_prefix("--record-offline=") {
            if value.is_empty() {
                return Err("`--record-offline` needs a directory or video path".to_string());
            }
            if output_dir.replace(value.into()).is_some() {
                return Err("`--record-offline` may only be specified once".to_string());
            }
        } else if args[index] == "--record-fps" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "`--record-fps` needs a positive integer".to_string())?;
            fps = Some(parse_positive_record_number("--record-fps", value)?);
        } else if let Some(value) = args[index].strip_prefix("--record-fps=") {
            fps = Some(parse_positive_record_number("--record-fps", value)?);
        } else if args[index] == "--record-frames" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "`--record-frames` needs a positive integer".to_string())?;
            frame_limit = Some(parse_positive_record_number("--record-frames", value)? as u64);
        } else if let Some(value) = args[index].strip_prefix("--record-frames=") {
            frame_limit = Some(parse_positive_record_number("--record-frames", value)? as u64);
        }
        index += 1;
    }

    if output_dir.is_none() && (fps.is_some() || frame_limit.is_some()) {
        return Err("`--record-fps` and `--record-frames` require `--record-offline`".to_string());
    }

    let request = output_dir.map(
        |output_dir| lunco_capture::screenshot::OfflineRecordingRequest {
            output_dir,
            fps: fps.unwrap_or(60),
        },
    );
    Ok((
        request,
        frame_limit.map(lunco_capture::screenshot::OfflineRecordLimit),
    ))
}

fn parse_positive_record_number(flag: &str, value: &str) -> Result<u32, String> {
    let number = value
        .parse::<u32>()
        .map_err(|_| format!("`{flag}` needs a positive integer, got `{value}`"))?;
    if number == 0 {
        return Err(format!("`{flag}` needs a positive integer, got `0`"));
    }
    Ok(number)
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

    #[test]
    fn parses_offline_recording_request_and_frame_limit() {
        let (request, limit) = parse_recording_args(&[
            "luncosim".to_string(),
            "--record-offline=/tmp/take".to_string(),
            "--record-fps".to_string(),
            "30".to_string(),
            "--record-frames=3".to_string(),
        ])
        .unwrap();
        let request = request.unwrap();
        assert_eq!(request.output_dir, std::path::PathBuf::from("/tmp/take"));
        assert_eq!(request.fps, 30);
        assert_eq!(limit.unwrap().0, 3);
    }

    #[test]
    fn recording_limits_require_an_output_and_positive_values() {
        assert!(parse_recording_args(&[
            "luncosim".to_string(),
            "--record-frames".to_string(),
            "1".to_string(),
        ])
        .is_err());
        assert!(parse_recording_args(&[
            "luncosim".to_string(),
            "--record-offline".to_string(),
            "take".to_string(),
            "--record-fps=0".to_string(),
        ])
        .is_err());
    }
}

/// Usage text for `--help`. Every flag here is one the binary ACTUALLY parses,
/// and they are spread across crates — this crate (`--no-ui`, `--api`, `--scene`,
/// `--no-vsync`, `--log-diag`), `ui::mod` (`--no-throttle`),
/// `lunco_networking::NetworkMode::from_args` (`--host`, `--connect`),
/// `lunco_networking::server::resolve_cert_paths` (`--cert`, `--key`) and
/// `lunco_workbench_window::window_placement` (`--window-pos`). Grep all of them before
/// editing this: an undocumented flag is invisible, and a documented flag that
/// nothing parses is a lie.
#[cfg(not(target_family = "wasm"))]
fn help_text() -> String {
    let api = lunco_api_contracts::DEFAULT_API_PORT;
    let net = lunco_core_session::DEFAULT_HOST_PORT;
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
        --validate PATH…   Pre-flight-check asset files (.mo/.usda/.sysml/.kerml/.wgsl/.rhai):
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

/// Composition root for the windowed shell. The first line of every GUI run
/// identifies the exact build that produced its log.
///
/// A tester's log is only useful if it names the binary that produced it. Without this,
/// five Windows runs in the 2026-07-26 report could be distinguished only by install
/// path and asset counts — so the report groups them by inference instead of by fact,
/// and two of its findings could not be attributed to a build at all.
///
/// Printed with `println!` rather than `info!` because it must survive `RUST_LOG`
/// filtering and precede `LogPlugin` — a build identity that a log level can suppress is
/// exactly as useless as none.
pub fn run_gui() -> AppExit {
    // `--offscreen`: GPU-FULL windowless recording mode. Real render stack and
    // visuals, no window/winit/egui — the scene renders into an offscreen target
    // image and the offline recorder captures that. It is only meaningful in a
    // `ui` build because it is provided by this UI package.
    let offscreen = cfg!(feature = "api-transport") && std::env::args().any(|a| a == "--offscreen");
    let args: Vec<String> = std::env::args().collect();
    let max_speed_requested = args.iter().any(|arg| arg == "--headless-max-speed");
    if max_speed_requested {
        eprintln!(
            "luncosim: --headless-max-speed requires --no-ui or the luncosim-server launcher"
        );
        return AppExit::error();
    }
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
    let record_preset = match parse_record_preset(&args) {
        Ok(preset) => preset,
        Err(error) => {
            eprintln!("luncosim: {error}");
            return AppExit::error();
        }
    };
    let (recording_request, record_limit) = match parse_recording_args(&args) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("luncosim: {error}");
            return AppExit::error();
        }
    };
    lunco_luncosim_core::log_build_identity(if offscreen { "offscreen" } else { "windowed" });
    // Answer GUI `--help` without building an app (see
    // `print_help_if_requested`). The headless server owns its own no-build
    // help path in `lunco-luncosim-core`.
    #[cfg(not(target_family = "wasm"))]
    if print_help_if_requested() {
        return AppExit::Success;
    }
    // Native deep-link single-instance gate (GUI only). Register the
    // `luncosim://` scheme handler (desktop integration, this crate), then decide
    // whether THIS process is the app or just a courier forwarding a clicked link
    // to an already-running instance. Must happen before building the app so a
    // forward exits without opening a window. The returned inbox is inserted
    // below; a Bevy system drains it into the confirm prompt.
    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    let deeplink_inbox = if !offscreen {
        use lunco_networking::single_instance::{acquire, LaunchOutcome};
        crate::url_scheme::register_best_effort();
        match acquire() {
            // This process is just a courier — it forwarded the link to the
            // running instance and has nothing to run, so exit cleanly.
            LaunchOutcome::Forwarded => return AppExit::Success,
            LaunchOutcome::Primary(inbox) => Some(inbox),
        }
    } else {
        None
    };

    let mut app = build_gui_app_with_profile(offscreen, render_profile);

    #[cfg(all(
        feature = "api-transport",
        feature = "transport-http",
        not(target_arch = "wasm32")
    ))]
    if let Some(error) = app
        .world_mut()
        .remove_resource::<lunco_api_transport::transports::HttpServerStartupError>()
    {
        eprintln!(
            "luncosim: cannot start HTTP API on {}:{}: {}",
            std::net::Ipv4Addr::LOCALHOST,
            error.port,
            error.message
        );
        return AppExit::error();
    }

    app.insert_resource(lunco_capture::screenshot::OfflineVideoSettings {
        preset: record_preset,
    });
    if let Some(request) = recording_request {
        app.insert_resource(request);
    }
    if let Some(limit) = record_limit {
        app.insert_resource(limit);
    }

    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    if let Some(inbox) = deeplink_inbox {
        app.insert_resource(inbox);
    }

    if !offscreen {
        #[cfg(feature = "package-icons")]
        app.insert_resource(crate::WindowIconBytes(crate::window_icon_bytes()));
        app.add_plugins(crate::LunCoSimUiPlugin {
            config: crate::LunCoSimUiConfig {
                product_version: lunco_luncosim_core::PRODUCT_VERSION,
                git_sha: lunco_luncosim_core::GIT_SHA,
                repository_url: lunco_luncosim_core::REPOSITORY_URL,
                initial_scene: app.world().resource::<ScenePath>().0.clone(),
            },
        });
    }

    #[cfg(feature = "api-transport")]
    if offscreen {
        app.add_plugins(crate::LunCoSimOffscreenPlugin);
    }

    apply_render_quality_override(&mut app, render_quality);

    // Return the AppExit so a non-zero exit (e.g. the startup-scene fail-loud
    // guard's `AppExit::error()`) propagates to the process exit code.
    app.run()
}

/// Queue a process-level quality choice until the authored profile policy has
/// loaded. Omitting the flag leaves persisted Graphics settings unchanged.
fn apply_render_quality_override(app: &mut App, quality: Option<lunco_render::RenderingQuality>) {
    let Some(quality) = quality else {
        return;
    };
    let Some(mut settings) = app
        .world_mut()
        .get_resource_mut::<lunco_render::RenderingQualitySettings>()
    else {
        warn!(
            "[render] --render-quality={} requested without a GPU settings resource",
            quality.label()
        );
        return;
    };

    settings.request_profile(quality);
    info!(
        "[render] explicit quality profile requested: {}",
        quality.label()
    );
}

fn build_gui_app_with_profile(offscreen: bool, render_profile: LunCoSimRenderProfile) -> App {
    let mut app = App::new();
    // Register every LunCo asset source (lunco:// and twin://) +
    // the shared `TwinRoots` resource in ONE shared place (`lunco-assets-core`), so all
    // binaries get identical schemes. MUST run before `DefaultPlugins`/`AssetPlugin`
    // snapshots the source registry.
    lunco_assets_runtime::register_lunco_asset_sources(&mut app);
    let plugins = default_plugins_with_profile(offscreen, render_profile);
    app.add_plugins(plugins);
    lunco_assets_runtime::register_lunco_asset_types(&mut app);
    // Flushes the WARN/ERROR dedup counters the `LogPlugin` filter accumulates.
    app.add_plugins(lunco_luncosim_core::log_dedup::LogDedupPlugin);
    {
        if render_profile == LunCoSimRenderProfile::Fast {
            app.insert_resource(lunco_render_bevy::RenderProfile::Fast);
            info!("[render] fast profile enabled");
        }
        app.add_plugins(lunco_render_bevy::LuncoRenderPlugin);
    }
    app.add_plugins(LunCoSimSimulationPlugin);
    app.add_plugins(LunCoSimRuntimePlugin::default());
    if !offscreen {
        crate::camera::install_interactive_camera(&mut app);
    }
    // The browser boot-screen handshake is an application-shell concern. Keep
    // it at the UI edge so headless/server compositions do not carry web glue.
    app.add_plugins(lunco_web::WebReadyPlugin);
    // Provisioning is an explicit GUI application capability. The shared core
    // installs only the lightweight registry/discovery plugin, so headless and
    // server compositions do not inherit native HTTP/archive/image workers.
    app.add_plugins(lunco_assets::datasets::DatasetProvisioningPlugin::default());
    // Scene-wide ambient fill is a presentation concern. Keep the resource on
    // the GUI edge so the shared simulation core has no dependency on Bevy's
    // light/render feature family.
    app.insert_resource(bevy::light::GlobalAmbientLight {
        brightness: 0.0,
        ..Default::default()
    });
    lunco_luncosim_presentation::register(&mut app);
    app
}

/// Construct the luncosim's primary window for the custom workbench chrome.
///
/// The workbench owns the edge hit testing for undecorated Windows/Linux
/// windows. It delegates the actual OS gesture to winit, which requires this
/// window to remain resizable.
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
        ..lunco_workbench_window::restored_window(title)
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

#[cfg(test)]
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

/// Build the Bevy plugin group for the windowed application shell.
///
/// The headless application group belongs to `lunco-luncosim-runtime`; keeping
/// this group here means the GUI shell is the only owner of renderer/window
/// configuration.
fn default_plugins_with_profile(
    offscreen: bool,
    render_profile: LunCoSimRenderProfile,
) -> bevy::app::PluginGroupBuilder {
    // Window title (advertises the `--api` port so side-by-side instances are
    // distinguishable) + present mode are windowed-only and must be known at
    // window-build time, so they're computed here rather than in the UI plugin.
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
                api_port = Some(lunco_api_contracts::DEFAULT_API_PORT);
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
            file_path: lunco_assets_core::assets_dir_abs().to_string_lossy().to_string(),
            // File watching is an interactive authoring capability. Offscreen
            // runs must be deterministic and must not allocate OS watcher
            // resources; scene tests and render capture use explicit paths.
            watch_for_changes_override: Some(!offscreen),
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
                        .with_filter(lunco_luncosim_core::log_dedup::DedupFilter),
                ))
            },
            ..default()
        });

    let vertical = std::env::args().any(|a| a == "--vertical");

    // Window/winit setup. Offscreen is windowless WITH a GPU; the default Bevy
    // render plugin renders surfaceless into the offscreen target image.
    // Host-neutral asset/task construction belongs to lunco-luncosim-core;
    // renderer-independent domain composition is installed by this host.
    let group = if offscreen {
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
    group.build().disable::<TransformPlugin>()
}
