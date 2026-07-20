//! The LunCo sandbox application — ground mobility + physics, loaded from USD.
//!
//! [`run`] builds and runs the app. It is the single shared entry point for BOTH
//! binaries:
//!   - `sandbox` (this crate, default `ui` feature) — the windowed GUI;
//!   - `sandbox-server` (the `lunco-sandbox-server` crate, no `ui`) — headless.
//!
//! ## Architecture: composition root, not a UI host
//!
//! The app is three named plugins, composed by a tiny shell — mirroring how the
//! library crates split into core modules + a `*UiPlugin`:
//!   - [`SandboxCorePlugin`] — sim / physics / cosim / USD / networking / API.
//!     Headless-safe, added unconditionally.
//!   - [`ui::SandboxUiPlugin`] (`ui` feature) — egui workbench, picking, the
//!     in-scene editor, materials, panels, fallback camera. Added only when
//!     running windowed.
//!   - [`SandboxHeadlessPlugin`] — the `ScheduleRunner` + the Modelica/spawn
//!     cores a server needs in the UI plugin's place. Added only when headless.
//!
//! GUI = `SandboxCorePlugin + SandboxUiPlugin`; headless =
//! `SandboxCorePlugin + SandboxHeadlessPlugin`. Both bins compose the SAME
//! `SandboxCorePlugin`, so they can never drift. The only place the GUI/headless
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

use bevy::prelude::*;
use bevy::asset::{AssetMetaCheck, AssetPlugin};
use big_space::prelude::*;
use avian3d::prelude::PhysicsPlugins;

use lunco_mobility::LunCoMobilityPlugin;
use lunco_hardware::LunCoHardwarePlugin;
// USD core (scene load + collider build) is always needed; the Twin browser /
// RTT viewport UI plugins are `ui`-only (added by `SandboxUiPlugin`).
use lunco_usd::{LoadScene, UsdPlugins, UsdPrimPath, UsdStageAsset};
// The USD-reading systems read the LIVE canonical stage via `StageView`, which
// implements `UsdRead` (the COMPOSED stage — as opposed to `UsdDataExt`, a raw
// AUTHORED layer; the retired flattened reader used to blur the two). Since the
// terrain projector moved to `lunco-usd-terrain`, the only reader left in this crate
// is the `ui`-gated terrain layer-map binding.
#[cfg(feature = "ui")]
use lunco_usd_bevy::UsdRead;
use bevy::asset::AssetLoadFailedEvent;

/// Re-exported so the (bevy-free) bin crates can return it from `main` to
/// propagate the process exit code (e.g. the startup-scene fail-loud guard).
pub use bevy::app::AppExit;
use lunco_terrain_globe::TerrainPlugin;
use lunco_obstacle_field::ObstacleFieldPlugin;
use lunco_terrain_surface::TerrainSurfacePlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::LunCoAvatarPlugin;
use lunco_environment::EnvironmentPlugin;
use lunco_cosim::CoSimPlugin;
use lunco_cosim::systems::propagate::CosimSet as PropagateCosimSet;
use lunco_cosim::systems::apply_forces::CosimSet as ApplyForcesCosimSet;
// `ModelicaSet` orders the cosim pipeline (always). The egui workbench plugin is
// added by `SandboxUiPlugin`; headless adds `ModelicaCorePlugin` instead.
use lunco_modelica::ModelicaSet;

#[cfg(feature = "ui")]
mod ui;
/// Engine light-handling policy (`ShadowCastingSettings` + reactive
/// possession-driven headlight shadow projection). Client render concern, so
/// `ui`-gated. See [`light_policy`].
#[cfg(feature = "ui")]
mod light_policy;
#[cfg(feature = "ui")]
mod terrain_horizon;
/// OS `luncosim://` scheme registration (desktop integration). Native + the
/// networking feature only — there's nothing to dial without the wire.
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod url_scheme;

/// `sandbox rhai` — stdin→HTTP rhai REPL client for a running instance. Native
/// only (raw `std::net` HTTP; no window). See [`rhai_repl::run_if_requested`].
#[cfg(not(target_family = "wasm"))]
pub mod rhai_repl;

/// Run the sandbox, choosing GUI vs. headless from the build + flags: headless
/// when the `ui` feature is absent, or `--no-ui` / `LUNCO_NO_UI` is set;
/// otherwise the windowed GUI. This is the `sandbox` (GUI) bin's entry point.
pub fn run() -> AppExit {
    let headless = !cfg!(feature = "ui")
        || std::env::args().any(|a| a == "--no-ui")
        || std::env::var("LUNCO_NO_UI").is_ok_and(|v| v != "0" && !v.is_empty());
    run_with_mode(headless)
}

/// Run the sandbox HEADLESS, unconditionally — the `sandbox-server` bin's entry
/// point. Forcing the mode here (rather than inferring it from the absent `ui`
/// feature) makes the server stay windowless **even if `ui` gets unified on** by
/// a `cargo build --workspace` (which compiles the GUI `sandbox` bin alongside
/// it). So the server never tries to open a window; in a lean `-p
/// lunco-sandbox-server` build the GUI stack isn't linked at all.
pub fn run_headless() -> AppExit {
    run_with_mode(true)
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
sandbox — the LunCoSim lunar simulator.

USAGE:
    sandbox [FLAGS]
    sandbox rhai [--api PORT] [-e SNIPPET | -f FILE]

FLAGS:
    -h, --help           Print this help and exit.
        --no-ui          Run headless (no window). Also via LUNCO_NO_UI=1.
        --api [PORT]     Serve the HTTP command API (default {api}). NOT implied
                         by --no-ui: without this flag there is no API port.
                         POST /api/commands  {{\"command\":\"Name\",\"params\":{{…}}}}
        --scene PATH     Load this USD scene at startup, relative to assets/.
        --window-pos SPEC  Place the OS window, e.g. 1920x1080+0+0.
        --validate PATH…   Pre-flight-check asset files (.mo/.usda/.wgsl/.rhai):
                         parse-only, no window/GPU/app. Prints a report and
                         exits 0 (all ok) or 1 (any failed).

RECORDING:
        --record-offline <dir|out.mp4>
                         Record deterministic frames once the scene is ready:
                         a directory gets a PNG sequence; an .mp4/.mkv/.mov
                         path streams straight into ffmpeg (falls back to a
                         PNG sequence, loudly, if ffmpeg is not installed).
        --record-fps N   Recording output frame rate (default 60).
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
        --no-vsync       Uncap the frame rate (present without vsync).
        --no-throttle    Keep running at full rate while unfocused.
        --log-diag       Log FPS / frame-time / physics diagnostics.

SUBCOMMAND:
    rhai                 REPL client against a RUNNING instance's --api port.
                         Reads stdin, or -e SNIPPET / -f FILE for one-shot.

Measuring FPS? Use --no-vsync --no-throttle, else you are timing the
compositor and the unfocused power-save throttle, not the renderer.",
    )
}

/// Handle `--help`/`-h` BEFORE the app is built: print usage and exit. It has to
/// come first — building the app opens a window, spins up the GPU and loads a
/// scene, which is why `sandbox --help` used to launch the simulator instead of
/// answering the question.
#[cfg(not(target_family = "wasm"))]
fn print_help_if_requested() -> bool {
    if std::env::args()
        .skip(1)
        .any(|a| a == "--help" || a == "-h")
    {
        println!("{}", help_text());
        return true;
    }
    false
}

/// Composition root. Builds the shared core, then conditionally layers on the UI
/// or the headless runner. Nothing UI-specific lives here beyond selecting the
/// windowing backend in [`default_plugins`].
fn run_with_mode(headless: bool) -> AppExit {
    // `--offscreen`: GPU-FULL windowless recording mode. Real render stack and
    // visuals, no window/winit/egui — the scene renders into an offscreen target
    // image and the offline recorder captures that. Only meaningful in a `ui`
    // build (it needs the render stack) and mutually exclusive with headless
    // (which is the no-GPU server); headless wins if both are given.
    let offscreen = cfg!(all(feature = "ui", feature = "lunco-api"))
        && !headless
        && std::env::args().any(|a| a == "--offscreen");
    // Answer `--help` without building an app (see `print_help_if_requested`).
    // Placed in the composition root, not in one bin's `main`, so EVERY entry
    // point that runs the sandbox — GUI `sandbox`, headless `sandbox-server` —
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

    let mut app = build_sim_app(headless, offscreen);

    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    if let Some(inbox) = deeplink_inbox {
        app.insert_resource(inbox);
    }


    #[cfg(feature = "ui")]
    if !headless && !offscreen {
        app.add_plugins(ui::SandboxUiPlugin);
    }

    #[cfg(all(feature = "ui", feature = "lunco-api"))]
    if offscreen {
        app.add_plugins(SandboxOffscreenPlugin);
    }

    if headless {
        app.add_plugins(SandboxHeadlessPlugin);
    }

    // Return the AppExit so a non-zero exit (e.g. the startup-scene fail-loud
    // guard's `AppExit::error()`) propagates to the process exit code.
    app.run()
}

/// Build the base [`DefaultPlugins`] group for the chosen mode.
///
/// This is the one place the GUI/headless split touches plugin *configuration*,
/// because the render backend and the window must be decided at `PluginGroup`
/// build time — a plugin added later cannot reconfigure `RenderPlugin`/
/// `WindowPlugin`. Headless (and every `--no-ui`-feature build) uses `backends:
/// None`: the asset stores still initialise (so USD visual sync can populate the
/// meshes avian colliders read), but no GPU device is created and nothing is
/// drawn — `ScheduleRunnerPlugin` (added by [`SandboxHeadlessPlugin`]) ticks the
/// app in winit's place.
///
/// NB: with no backend, `RenderPlugin` does NOT build the render world — it skips
/// `ExtractPlugin`/`SyncWorldPlugin` entirely, while still installing the render-
/// sync component hooks that expect them. [`SandboxHeadlessPlugin`] adds
/// `SyncWorldPlugin` back to keep despawns from aborting; see the note there.
/// THE simulation app — asset sources, engine plugins, and every LunCo domain
/// system. This is the whole application minus its user interface.
///
/// Every binary that runs the simulation builds it through here: the GUI
/// ([`run`]), the headless server, and the headless scene-test runner. The UI is
/// added ON TOP by `run` alone, which is the right direction of dependency — the
/// simulation does not know the interface exists, and a test runner exercises the
/// same app the user does rather than a re-assembled lookalike.
///
/// **This exists because assembling it by hand is a trap.** The asset-source
/// registration below MUST happen before `AssetPlugin`, which snapshots the source
/// registry when it is built. Miss it and you get an app with no `lunco://` or
/// `twin://` scheme and no `TwinRoots` resource — which surfaces as
/// `Res<TwinRoots> failed validation: Resource does not exist` from an observer far
/// away, or, worse, as assets that silently never resolve. Four call sites had
/// already open-coded this prelude and a fifth (the scene-test runner) hit the panic
/// on its first run.
///
/// Do not "fix" a missing `TwinRoots` by initialising the resource on its own: that
/// manufactures the resource WITHOUT the asset sources it is meant to accompany, and
/// trades a loud panic for silent unresolved assets.
pub fn build_sim_app(headless: bool, offscreen: bool) -> App {
    let mut app = App::new();
    // Register every LunCo asset source (lunco://, twin://, cached_textures://) +
    // the shared `TwinRoots` resource in ONE shared place (`lunco-assets`), so all
    // binaries get identical schemes. MUST run before `DefaultPlugins`/`AssetPlugin`
    // snapshots the source registry.
    lunco_assets::register_lunco_asset_sources(&mut app);
    app.add_plugins(default_plugins(headless, offscreen));
    app.add_plugins(SandboxCorePlugin { headless });
    app
}

/// Engine-level plugin set, render/UI stripped when `headless`.
///
/// `pub` so [`build_sim_app`] is not the only way in for a binary that genuinely
/// needs a different plugin set — but prefer `build_sim_app`, which also does the
/// asset-source prelude this function cannot do (it returns a group, not an `App`).
pub fn default_plugins(headless: bool, offscreen: bool) -> bevy::app::PluginGroupBuilder {
    // `bevy::render` EXISTS ONLY IN A `ui` BUILD. The no-`ui` server does not link
    // bevy_render at all (that is the point of the render decoupling), so every
    // `bevy::render::*` path below must be gated — an ungated one does not merely link a
    // GPU stack, it fails to compile. It did: `cargo check -p lunco-sandbox-server` was
    // broken, and nothing caught it because `--workspace` unifies `ui` on and the CI render
    // guard only runs `cargo tree` (which resolves the graph but never builds it).
    #[cfg(feature = "ui")]
    use bevy::render::settings::WgpuSettings;
    // `headless`/`offscreen` only select render/window config in `ui` builds; a
    // no-`ui` build is always windowless, so the params are unused there.
    #[cfg(not(feature = "ui"))]
    let _ = (headless, offscreen);

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
            Some(p) => format!("sandbox — Listening on {p}"),
            None => "sandbox".to_string(),
        };
        let present = if no_vsync || networked {
            bevy::window::PresentMode::Mailbox
        } else {
            bevy::window::PresentMode::Fifo
        };
        (title, present)
    };

    #[cfg(feature = "ui")]
    let render_creation = if headless {
        WgpuSettings { backends: None, ..default() }.into()
    } else {
        lunco_workbench::preferred_wgpu_settings().into()
    };

    let group = DefaultPlugins
        .set(AssetPlugin {
            file_path: lunco_assets::assets_dir_abs().to_string_lossy().to_string(),
            // Don't probe for `.meta` sidecars: we ship none, so every asset
            // load would otherwise fire a failed `<asset>.meta` fetch.
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .set(bevy::log::LogPlugin {
            // Quieten third-party noise (rumoca JIT + diffsol per-step).
            filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
            ..default()
        });

    // Only a `ui` build has a render stack to configure. Without `ui`, `DefaultPlugins`
    // carries no `RenderPlugin` (bevy_render isn't linked) and there is nothing to set.
    #[cfg(feature = "ui")]
    let group = group.set(bevy::render::RenderPlugin { render_creation, ..default() });

    #[cfg(feature = "ui")]
    let vertical = std::env::args().any(|a| a == "--vertical");

    // Window/winit setup. With the `ui` feature the runtime `headless` flag still
    // picks the windowless variant (no primary window, WinitPlugin disabled) —
    // and so does `offscreen`, which is windowless WITH a GPU (the
    // `render_creation` above stays `preferred_wgpu_settings` because
    // `offscreen` never sets `headless`; wgpu renders surfaceless into the
    // offscreen target image).
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
            primary_window: Some({
                let mut window = Window {
                    // On wasm, attach to the `#bevy` canvas and mirror its CSS size.
                    #[cfg(target_arch = "wasm32")]
                    canvas: Some("#bevy".to_string()),
                    #[cfg(target_arch = "wasm32")]
                    fit_canvas_to_parent: true,
                    present_mode,
                    // Centralized merged-titlebar chrome + persisted geometry.
                    ..lunco_workbench::restored_window(window_title)
                };
                if vertical {
                    window.resolution = bevy::window::WindowResolution::new(540, 960);
                } else {
                    window.resolution = bevy::window::WindowResolution::new(1280, 720);
                }
                window.resizable = false;
                window
            }),
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
    // peers mount `twin://sandbox/sandbox_scene.usda`, and this load lands ~1 s
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
    let uri = lunco_networking::scenario_sync::mount_scenario_twin(
        &twins,
        &m.scenario_id,
        &m.name,
        scene,
    );
    info!("[net] scenario fully cached; loading entry scene (read-only): {scene}");
    commands.trigger(LoadScene { path: uri, root_prim: String::new() });
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
    mut registry: ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>,
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
    registry: Option<ResMut<lunco_modelica::state::ModelicaDocumentRegistry>>,
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
        lunco_modelica::experiment_journal::replay_experiment_op(&mut registry, &op);
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
/// peer's journaled obstacle-field spec onto the local `ObstacleFieldSpec` and
/// fires `RegenerateField`. This is what replaced the former bespoke host→client
/// broadcast (`sync_obstacle_field_spec`): the spec now rides the journal plane,
/// so a tweak syncs BOTH directions and persists. No single-doc limitation (the
/// spec is a singleton). No-ops when the spec resource / journal are absent.
#[cfg(feature = "networking")]
fn replay_scenario_journal_obstacle(
    role: Res<lunco_core::NetworkRole>,
    remote: Res<lunco_networking::scenario::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    spec: Option<ResMut<lunco_obstacle_field::ObstacleFieldSpec>>,
    mut regen: MessageWriter<lunco_obstacle_field::RegenerateField>,
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
    // the LAST one matters, and each `RegenerateField` is a full terrain re-stamp
    // + rock re-scatter — so install once and fire one regen.
    let mut last_spec = None;
    for (id, op) in pending {
        if let Some(new_spec) = lunco_obstacle_field::journal::replay_spec(&op) {
            last_spec = Some(new_spec);
        }
        applied.insert(id);
    }
    if let Some(new_spec) = last_spec {
        // Install the peer's spec + regenerate. Sets the resource directly
        // (NOT the `UpdateObstacleFieldSpec` command), so no re-record.
        *spec = new_spec;
        regen.write(lunco_obstacle_field::RegenerateField);
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
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let Some(journal) = journal else {
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
        lunco_twin_journal::DomainKind::ToolLibrary,
    );
    for (id, op) in pending {
        if let Some((name, source)) =
            lunco_scripting::registration_journal::replay_tool_library(&op)
        {
            lunco_scripting::tool_libs::register_tool_library(&name, &source);
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
    store: Option<ResMut<lunco_scripting::timelines::TimelineStore>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut store)) = (journal, store) else {
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
        lunco_twin_journal::DomainKind::Timeline,
    );
    for (id, op) in pending {
        if let Some((name, timeline)) =
            lunco_scripting::registration_journal::replay_timeline(&op)
        {
            store.insert(name, timeline);
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
                registry
                    .set_status(id, lunco_experiments::RunStatus::Done { wall_time_ms: wall });
                info!("[experiment] loaded result artifact for {}", id.as_artifact_stem());
            }
            Err(e) => warn!("[experiment] result artifact parse failed for {}: {e}", id.as_artifact_stem()),
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
/// hook definition — the projected form of `scripted_policy::PolicyDef`.
#[cfg(feature = "networking")]
const LUNCO_POLICY_TYPE: &str = "LunCoPolicy";

/// One authored `LunCoPolicy` prim, BEFORE its rhai source is resolved. The source is
/// authored EITHER inline (`info:sourceCode`, a `string` that rides the USD journal
/// plane — live-editable, per-op synced) OR by file reference (`info:sourceAsset`,
/// an `asset` `@…rhai@` that rides the whole-twin content plane, CID-verified). Inline
/// wins over the file — the same rule as `lunco:script`/`lunco:scriptPath` and
/// `lunco:behavior`/`lunco:behaviorPath`.
#[cfg(feature = "networking")]
struct AuthoredPolicy {
    seam: String,
    entry: String,
    deterministic: bool,
    /// Inline rhai source (`info:sourceCode`), non-empty when authored.
    inline_source: Option<String>,
    /// Asset path to a `.rhai` file (`info:sourceAsset`), when authored.
    source_path: Option<String>,
}

/// Read every composed `LunCoPolicy` prim across all live stages into the authored
/// policy set — the "policy is a projected USD prim" extractor. Reads the **composed**
/// stage, so an opinion authored at any layer (global/twin/scene) resolves to one
/// effective policy per seam. A prim missing `seam`, or carrying NEITHER an inline
/// `source` nor a `sourcePath`, is skipped (incompletely authored). Pure over the
/// stages, so it's unit-testable without a running app — the file-ref RESOLUTION (asset
/// load) happens in [`project_usd_policies`], not here.
#[cfg(feature = "networking")]
fn extract_usd_policies(canonical: &lunco_usd_bevy::CanonicalStages) -> Vec<AuthoredPolicy> {
    let mut out = Vec::new();
    for (_, cs) in canonical.iter() {
        let view = cs.view();
        for prim in view.prim_paths() {
            if view.prim_type_name(&prim).as_deref() != Some(LUNCO_POLICY_TYPE) {
                continue;
            }
            let seam = view.value::<String>(&prim, "lunco:policy:seam").unwrap_or_default();
            // The schema default for both is empty (`""` / `@@`), so filter empties: an
            // unauthored opinion reads back as the fallback, which is not a real source.
            let inline_source = view
                .value::<String>(&prim, "info:sourceCode")
                .filter(|s| !s.is_empty());
            // `asset`-typed ref — read via `UsdRead::asset` (a `Value::AssetPath`, which a
            // `String` read would miss). UFCS, so no trait import is needed here.
            let source_path =
                lunco_usd_bevy::UsdRead::asset(&view, &prim, "info:sourceAsset")
                    .filter(|s| !s.is_empty());
            if seam.is_empty() || (inline_source.is_none() && source_path.is_none()) {
                continue;
            }
            out.push(AuthoredPolicy {
                seam,
                entry: view.value::<String>(&prim, "lunco:policy:entry").unwrap_or_default(),
                deterministic: view
                    .value::<bool>(&prim, "lunco:policy:deterministic")
                    .unwrap_or(true),
                inline_source,
                source_path,
            });
        }
    }
    out
}

/// The three states of resolving a `info:sourceAsset` `.rhai` reference.
#[cfg(feature = "networking")]
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
/// dropped mid-load. Mirrors `lunco_scripting::commands::resolve_embedded_scenario_paths`
/// — and shares its `TODO(scenario-resolve)`: a `.rhai` fetched into a peer's
/// scenario cache is loaded against the DEFAULT asset source here, so a
/// twin/imported file policy syncs (whole-twin content plane) but needs the resolver's
/// `canonicalize` anchoring to load on the peer. Inline source is unaffected (rides the doc).
#[cfg(feature = "networking")]
fn resolve_policy_source_file(
    path: &str,
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
    let handle = pending.entry(path.to_string()).or_insert_with(|| {
        // The AssetServer root is already `assets/`; strip an authored prefix (mirrors
        // lunco-usd-sim + resolve_embedded_scenario_paths).
        let rel = path.strip_prefix("assets/").unwrap_or(path).to_string();
        asset_server.load(rel)
    });
    if asset_server.load_state(&*handle).is_failed() {
        warn!("[policy] failed to load sourcePath '{path}' via AssetServer");
        return PolicySource::Failed;
    }
    match sources.get(&*handle) {
        Some(src) => PolicySource::Ready(src.text.clone()),
        None => PolicySource::Loading,
    }
}

/// **Policy projection** — activation half of "policy is a USD prim". On any
/// composed-stage change, read the `LunCoPolicy` prims and project them into the
/// live hook registry via
/// [`project_policies`](lunco_networking::scripted_policy::project_policies): a new
/// prim registers its rhai hook (and, at [`MERGE_SEAM`](lunco_networking::scripted_policy::MERGE_SEAM),
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
#[cfg(feature = "networking")]
#[allow(clippy::type_complexity)]
fn project_usd_policies(
    canonical: NonSend<lunco_usd_bevy::CanonicalStages>,
    mut registry: ResMut<lunco_networking::scripted_policy::ScriptedPolicyRegistry>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    asset_server: Res<AssetServer>,
    sources: Option<Res<Assets<lunco_scripting::source_asset::RhaiSource>>>,
    mut pending: Local<
        std::collections::HashMap<String, Handle<lunco_scripting::source_asset::RhaiSource>>,
    >,
    mut last: Local<Option<(usize, u64)>>,
    mut awaiting: Local<bool>,
) {
    let signal = (canonical.len(), canonical.iter().map(|(_, cs)| cs.generation()).sum());
    // Re-run when the stage moved OR a file-backed source is still loading.
    if *last == Some(signal) && !*awaiting {
        return;
    }
    *last = Some(signal);

    let authored = extract_usd_policies(&canonical);
    // Drop cached handles for paths no longer authored, so a removed file-policy stops
    // pinning its asset.
    let live: std::collections::HashSet<&str> =
        authored.iter().filter_map(|a| a.source_path.as_deref()).collect();
    pending.retain(|p, _| live.contains(p.as_str()));

    let mut desired = Vec::with_capacity(authored.len());
    let mut unresolved = false;
    for a in &authored {
        // Inline wins over the file (the script/behavior convention).
        let source = if let Some(src) = &a.inline_source {
            src.clone()
        } else if let Some(path) = &a.source_path {
            match resolve_policy_source_file(path, &asset_server, sources.as_deref(), &mut pending) {
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
        desired.push(lunco_networking::scripted_policy::PolicyDef {
            seam: a.seam.clone(),
            entry: a.entry.clone(),
            source,
            deterministic: a.deterministic,
        });
    }
    *awaiting = unresolved;
    lunco_networking::scripted_policy::project_policies(desired, &mut registry, journal.as_deref());
}

/// **Environment-settings projection** — the read half of persisting
/// `SetEnvironmentLight` render knobs (exposure / bloom / ambient / earthshine)
/// onto the `LunCoEnvironment` settings prim (see
/// [`lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE`]). On any composed-stage
/// change, read that prim's `lunco:env:*` attrs and apply them **directly** to
/// the live render state — never by re-triggering `SetEnvironmentLight`, which
/// would re-persist and loop. So a persisted render tweak round-trips on reload
/// and syncs to peers (the prim rides the USD journal → each peer recomposes →
/// each peer's projector applies) with no bespoke broadcast. Change-gated on
/// total stage generation + count, like [`project_usd_policies`]. UI-gated: the
/// knobs are render/camera state (`Bloom` lives in `bevy_post_process`, under
/// `ui`); the headless server has no cameras to apply to.
#[cfg(feature = "ui")]
fn project_env_settings(
    canonical: NonSend<lunco_usd_bevy::CanonicalStages>,
    mut q_exposure: Query<&mut bevy::camera::Exposure>,
    mut q_bloom: Query<&mut bevy::post_process::bloom::Bloom>,
    // Ambient is NOT projected here any more — it is composed from authored
    // `DomeLight` prims by `light.rs::on_usd_light_added`. See the note below.
    _ambient: Option<ResMut<bevy::light::GlobalAmbientLight>>,
    mut q_earthshine: Query<&mut DirectionalLight, With<lunco_environment::Earthshine>>,
    mut last: Local<Option<(usize, u64)>>,
) {
    let signal = (canonical.len(), canonical.iter().map(|(_, cs)| cs.generation()).sum());
    if *last == Some(signal) {
        return;
    }
    *last = Some(signal);

    for (_, cs) in canonical.iter() {
        let view = cs.view();
        for prim in view.prim_paths() {
            if view.prim_type_name(&prim).as_deref()
                != Some(lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE)
            {
                continue;
            }
            if let Some(ev) = view.value::<f32>(&prim, "lunco:env:exposureEv100") {
                for mut e in &mut q_exposure {
                    e.ev100 = ev;
                }
            }
            if let Some(bi) = view.value::<f32>(&prim, "lunco:env:bloomIntensity") {
                for mut b in &mut q_bloom {
                    b.intensity = bi;
                }
            }
            // `lunco:env:ambientBrightness` is DELETED, not deprecated. Uniform
            // environment illumination is already standard USD — an untextured
            // `UsdLuxDomeLight` — and `light.rs::on_usd_light_added` composes the
            // scene ambient as the sum over authored domes, which is what UsdLux
            // semantics require (lights add).
            //
            // Keeping both spellings is what caused the bug. This projector
            // ASSIGNED the custom attribute's value, the dome sum ASSIGNED its own,
            // and a *textured* dome contributes nothing to that sum — so loading a
            // starfield dome zeroed the regolith bounce a scene had authored here,
            // and the memoised `last` guard meant this system never ran again to
            // put it back. Two writers, one field, order-dependent: a scene that
            // rendered correctly could go dark on an unrelated change.
            //
            // Scenes author the bounce as a `DomeLight` prim now. There is
            // deliberately no fallback read.
            if let Some(lux) = view.value::<f32>(&prim, "lunco:env:earthshineIntensity") {
                for mut l in &mut q_earthshine {
                    l.illuminance = lux;
                }
            }
            if let Some([r, g, b]) = view.value_vec3(&prim, "lunco:env:earthshineColor") {
                for mut l in &mut q_earthshine {
                    l.color = Color::linear_rgb(r as f32, g as f32, b as f32);
                }
            }
        }
    }
}

/// Convenience command: author (or hot-replace) a rhai policy as a `LunCoPolicy`
/// USD prim under `/World/Policies/<name>` in ONE call, instead of hand-issuing the
/// underlying `ApplyUsdOp`s. Because it authors USD doc ops, the policy **journals →
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
    /// Prim name under `/World/Policies` (the identity for hot-replace); defaults to
    /// a sanitized `seam` when empty.
    pub name: String,
    /// The hook seam (id): e.g. `"journal.merge.order"`, `"rbac.authorize"`, or a
    /// `lunco:driveKernel` id a rover points at.
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
    registry: Res<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>,
    mut commands: Commands,
) {
    use lunco_usd::{ApplyUsdOp, LayerId, UsdOp};
    let cmd = trigger.event();
    let docs: Vec<_> = registry.ids().collect();
    let [doc] = docs.as_slice() else {
        warn!("[policy] SetRhaiPolicy needs exactly one scene document (found {})", docs.len());
        return;
    };
    let doc = *doc;

    // USD prim names are identifier-like — sanitize the seam/name into one.
    let base = if cmd.name.is_empty() { &cmd.seam } else { &cmd.name };
    let mut name: String =
        base.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    if name.is_empty() {
        name = "policy".to_string();
    }
    let prim = format!("/World/Policies/{name}");
    let root = LayerId::root();

    // Idempotent: define_prim + attribute overwrite → re-issuing hot-replaces.
    // String values are RAW — `SetAttribute` authors them verbatim and the writer
    // escapes on serialize (see the op's string branch). No hand-escaping here: the
    // old `format!("{:?}")` produced Rust-debug quoting, not USDA delimiting, and
    // silently corrupted any multi-line rhai `source`.
    let ops = vec![
        UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: "/World".into(),
            name: "Policies".into(),
            type_name: Some("Scope".into()),
            reference: None,
        },
        UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: "/World/Policies".into(),
            name: name.clone(),
            type_name: Some("LunCoPolicy".into()),
            reference: None,
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
        commands.trigger(ApplyUsdOp { doc, op });
    }
    info!("[policy] SetRhaiPolicy authored `{prim}` (seam '{}') — journals + projects", cmd.seam);
}

/// Save a live-edited rhai scenario's current source back onto the `LunCoProgram`
/// prim it came from — the other half of scenario authoring.
///
/// The source is authored onto that prim's `info:sourceCode`, which is what
/// the loader prefers over a `sourceAsset`: text authored in place is an author saying
/// they mean it. The write goes through [`SetAttribute`](lunco_usd::UsdOp::SetAttribute)
/// (whose `string` type authors the value RAW — no hand-escaping), so the whole rhai
/// source round-trips verbatim, journals like any edit, and reaches the `.usda` on
/// `SaveDocument`.
///
/// It authors onto the PROGRAM, not onto the vessel running it
/// ([`ScenarioProgramPrim`](lunco_core::ScenarioProgramPrim) carries the path): a
/// vessel can run several programs, and a source written onto the vessel would sit on
/// a prim that runs nothing.
///
/// Only doc-backed twin scenes have an editable document; a raw-file scene is
/// **refused** (logged, not silently dropped) — matching the rule that the builder
/// must only edit doc-backed scenes or it eats work on the next reload.
#[lunco_core::Command]
pub struct SaveScenario {
    /// The scripted entity whose live scenario source to persist onto its prim.
    /// Ownership-gated (same as `RunScenario`): saving a scenario is editing it.
    #[authz_target]
    pub target: Entity,
}

impl Default for SaveScenario {
    // `#[Command]` needs a Default for Reflect; `Entity` has none. The placeholder
    // is never dispatched — a real save always carries the selected entity.
    fn default() -> Self {
        Self { target: Entity::PLACEHOLDER }
    }
}

#[lunco_core::on_command(SaveScenario)]
fn on_save_scenario(
    trigger: On<SaveScenario>,
    q_model: Query<&lunco_scripting::doc::ScriptedModel>,
    q_prim: Query<&lunco_usd::UsdPrimPath>,
    q_program: Query<&lunco_core::ScenarioProgramPrim>,
    registry: Res<lunco_scripting::ScriptRegistry>,
    backed: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let target = trigger.event().target;

    // 1. The live source the runtime is currently running for this entity.
    let Ok(model) = q_model.get(target) else {
        warn!("[save-scenario] entity {target} has no scenario attached");
        return;
    };
    let Some(doc_id) = model.document_id else {
        warn!("[save-scenario] entity {target}'s scenario has no document");
        return;
    };
    let Some(host) = registry.documents.get(&lunco_doc::DocumentId::new(doc_id)) else {
        warn!("[save-scenario] no script document {doc_id} for entity {target}");
        return;
    };
    let source = host.document().source.clone();

    // 2. The prim to author onto + the editable scene document behind it.
    let Ok(upp) = q_prim.get(target) else {
        warn!("[save-scenario] entity {target} is not a USD-backed prim — nothing to save onto");
        return;
    };
    let Some(scene_doc) =
        lunco_usd::twin_projection::scene_document_for(&backed, &asset_server, upp.stage_handle.id())
    else {
        warn!(
            "[save-scenario] the scene backing {target} is a raw-file scene (not doc-backed) — \
             open it as a Twin to save scenarios in place"
        );
        return;
    };

    // 3. Author the source onto the PROGRAM prim's `info:sourceCode` (root
    //    layer → durable in the .usda on SaveDocument). A `string` value is authored
    //    RAW — `SetAttribute` handles the escaping (writer-side), so the whole rhai
    //    source round-trips verbatim with no hand-escaping here. Through `ApplyUsdOp`
    //    so it journals like any edit.
    let Ok(program) = q_program.get(target) else {
        warn!(
            "[save-scenario] entity {target} runs a scenario that came from no program prim \
             (it was started at runtime, not authored in the scene) — nothing to save onto"
        );
        return;
    };
    commands.trigger(lunco_usd::ApplyUsdOp {
        doc: scene_doc,
        op: lunco_usd::UsdOp::SetAttribute {
            edit_target: lunco_usd::LayerId::root(),
            path: program.0.clone(),
            name: "info:sourceCode".into(),
            type_name: "string".into(),
            value: source,
        },
    });
    info!(
        "[save-scenario] {target}: scenario source written onto `{}` (doc {}) — journals; SaveDocument persists to disk",
        program.0, scene_doc.0
    );
}

lunco_core::register_commands!(on_set_rhai_policy, on_save_scenario);

#[cfg(all(test, feature = "networking", not(target_arch = "wasm32")))]
mod policy_projection_tests {
    use super::extract_usd_policies;
    use lunco_usd_bevy::{CanonicalStage, CanonicalStages, StageRecipe, UsdRead};

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
            p.inline_source.as_deref().unwrap_or_default().contains("may_take_control"),
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
        assert_eq!(extract_usd_policies(&stages)[0].inline_source.as_deref(), Some("fn drive(c){1}"));

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
        let new_src = lunco_usd_bevy::author::parse_attribute_value("string", "\"fn drive(c){2}\"")
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
/// mesh/material asset stores (never touches a GPU device), so it's safe under
/// `backends: None`.
pub struct SandboxCorePlugin {
    pub headless: bool,
}

impl Plugin for SandboxCorePlugin {
    fn build(&self, app: &mut App) {
        let args: Vec<String> = std::env::args().collect();

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
                // A video destination (`out.mp4`) needs its PARENT, not itself,
                // to exist as a directory — see `output_is_video`.
                let dir_to_create = if lunco_workbench::screenshot::output_is_video(&path) {
                    path.parent().map(std::path::Path::to_path_buf).unwrap_or_default()
                } else {
                    path.clone()
                };
                if let Err(e) = (!dir_to_create.as_os_str().is_empty())
                    .then(|| std::fs::create_dir_all(&dir_to_create))
                    .unwrap_or(Ok(()))
                {
                    error!("[offline-record] CLI failed to create output directory {}: {e}", dir_to_create.display());
                } else {
                    info!(
                        "[offline-record] CLI mode armed: recording to {} at {} FPS — \
                         starts once the scene's visuals are ready",
                        path.display(),
                        record_fps
                    );
                    // Arms through the workbench recorder's readiness gate, NOT a
                    // hand-built active state: this path once listed its own fields
                    // and drifted (skipped the KeepAwake/present-mode setup → recorded
                    // through the power-save throttle at 2-10 s per frame), and
                    // starting `active` here captured black opening frames from the
                    // not-yet-loaded scene.
                    lunco_workbench::screenshot::arm_recording_at_startup(app, path, record_fps);
                }
            }
        }

        // Convenience command: `SetRhaiPolicy` authors a `LunCoPolicy` prim as USD
        // doc ops (journals → syncs → projector activates). Authoring works with or
        // without networking; the activation projector is networking-gated for now.
        register_all_commands(app);

        // `--scene <path>` overrides the default sandbox_scene.usda load. Path is
        // relative to the asset source root (`assets/`). Used by automated joint/
        // physics tests that need an isolated minimal scene.
        let scene_path = {
            let mut s = "scenes/sandbox/sandbox_scene.usda".to_string();
            for i in 0..args.len() {
                if args[i] == "--scene" && i + 1 < args.len() {
                    s = args[i + 1].clone();
                    break;
                }
            }
            s
        };

        // Cap how much catchup `FixedUpdate` does after a slow frame. Default
        // Bevy behaviour: a 50ms frame breeds 3 catch-up fixed ticks next frame,
        // making that frame slow too — a self-feeding jitter cascade. The cap
        // lives on `Time<Virtual>`; `Time<Fixed>` reads its delta from Virtual,
        // so capping Virtual transitively caps fixed catchup. 33ms ≈ 2 ticks —
        // residual real time is dropped instead of compounded.
        let mut virtual_time = Time::<Virtual>::default();
        virtual_time.set_max_delta(std::time::Duration::from_millis(33));

        app.insert_resource(ScenePath(scene_path))
            .insert_resource(virtual_time)
            // Match the workbench theme's backdrop so the window's first-frame
            // clear lines up with egui's panel fill (no "left hairline" at panel
            // boundaries under non-integer DPRs). Harmless headless.
            .insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)))
            .insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ))
            .insert_resource(avian3d::prelude::Gravity::ZERO)
            .insert_resource(lunco_environment::Gravity::flat(9.81, bevy::math::DVec3::NEG_Y))
            // Studio lighting for the sandbox — a generic editor scene, NOT a
            // calibrated lunar surface (the canonical 128 klx / EV15 `LunarSun`
            // crushes the dark blueprint ground to black). Inserted BEFORE
            // `EnvironmentPlugin` so its `init_resource` keeps these. The sun
            // spawn AND every camera's exposure read this one resource, so lux
            // and EV stay matched. Tunable live via `SetEnvironmentLight`.
            .insert_resource(lunco_environment::LunarSun {
                illuminance_lux: 10_000.0,
                exposure_ev100: 9.7,
                ..Default::default()
            })
            // Persistent world shell: one BigSpace root + `WorldGrid` + one
            // `FloatingOrigin`. The validation plugin (debug builds only, logs
            // errors, never panics) is ENABLED: WorldRoot is Transform-free —
            // big_space-canonical — now that the Phase 5 bridge owns BOTH
            // things the root `Transform` was load-bearing for (avian's GT
            // sync and its root-anchored ColliderTransform propagation). The
            // validator is the guard that keeps new spawn paths canonical.
            .add_plugins(BigSpaceDefaultPlugins)
            // EntityCount is cheap and useful any time we look at perf.
            .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
            .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
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
            // 12 solver substeps (avian default 6): joint-based rovers buzz the
            // chassis under drive torque at 6 substeps. Quantified in the headless
            // `rover_jitter` probe. See `project_physical_rover_suspension`.
            //
            // WEB: 8 substeps — the single wasm thread runs the whole solver inline,
            // so 12 substeps is a third of the physics budget per frame. 8 keeps the
            // physical (joint) rover acceptably calm while giving the browser back
            // frame time; native/server stay at 12 for full fidelity + peer
            // determinism (networked play is native/server-authoritative).
            .insert_resource(avian3d::prelude::SubstepCount(if cfg!(target_arch = "wasm32") {
                8
            } else {
                12
            }))
            .add_plugins(CoSimPlugin)
            .add_plugins(lunco_core::LunCoCorePlugin)
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
            // Procedural crater + rock field generator (replaces the flat Cube
            // ground for rover mobility testing). Server-authoritative colliders;
            // client adds visuals. See `project_obstacle_field_generator`.
            .add_plugins(ObstacleFieldPlugin)
            // ...but in DEM-delegated mode: the real ground is the USD DEM terrain,
            // which consumes the SAME `ObstacleFieldSpec` (craters stamped into the
            // grid, rocks scattered on its surface) — so the standalone 400 m flat
            // slab must NOT build (it would float on / z-fight the DEM). The one
            // Inspector panel + the networked spec drive the DEM layers directly.
            .insert_resource(lunco_obstacle_field::ObstacleFieldMode::DemDelegated)
            // Streamed, dynamically-LOD'd terrain (DEM tiles + heightfield
            // colliders). Inert at M0 (config only); see lunco-terrain-surface
            // and docs/terrain-streaming-PLAN.md.
            .add_plugins(TerrainSurfacePlugin)
            // Celestial stack (doc 43): dormant unless the SCENE asks for it. Bodies
            // are authored in USD (`LunCoCelestialBodyAPI` — reference
            // `assets/celestial/solar_system.usda`), and every celestial subsystem
            // gates on that authored fact, so the flat sandbox arena gets no sky at
            // all. The sandbox avatar keeps the FloatingOrigin either way. The
            // generic link kernel (doc 49) is always on — it needs no hierarchy —
            // and publishes `LinkState` + `link.aos`/`link.los`, NOT `comms:*`
            // ports (there is no comms subsystem to own them).
            .insert_resource(lunco_celestial::CelestialConfig {
                spawn_observer_camera: false,
            })
            .add_plugins(lunco_celestial::CelestialPlugin)
            // Real VSOP2013/ELP body positions on ALL platforms (wasm too) —
            // replaces the NoOp provider CelestialPlugin seeds.
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
            // Autopilot = a headless AiAgent actor that possesses + drives a vessel
            // (spec 034). Placed on the control path, not the avatar — runs on the
            // `--no-ui` server identically.
            .add_plugins(lunco_autopilot::AutopilotPlugin)
            .add_plugins(LunCoAvatarPlugin)
            .add_plugins(lunco_scripting::LunCoScriptingPlugin)
            // Default scene-wide fill for scenes that author no lighting; a
            // scene-authored UsdLux light takes ambient over.
            .insert_resource(bevy::light::GlobalAmbientLight {
                brightness: 40.0,
                ..Default::default()
            })
            .add_systems(Startup, setup_sandbox)
            .add_observer(on_restore_fallback_lights)
            // Fail loud if the requested `--scene` never loads (e.g. a wrong
            // path that resolves to a missing asset). Without this the app
            // silently boots a scene-less world (only procedural terrain /
            // obstacles), which masks the real error.
            .add_systems(Update, startup_scene_failguard)
            // Cosim pipeline ordering inside FixedUpdate:
            //   HandleResponses → Propagate → ApplyForces → SpawnRequests.
            .configure_sets(FixedUpdate, (
                ModelicaSet::HandleResponses,
                PropagateCosimSet::Propagate,
                ApplyForcesCosimSet::ApplyForces,
                ModelicaSet::SpawnRequests,
            ).chain());

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


        // Durable twin history for headless (`lunco-sandbox-server` / any
        // `--no-ui` host): the SAME twin-folder-scoped persistence the GUI uses
        // (`<twin>/history/journal.json`) — load on twin open, save on
        // `DocumentSaved` + debounced periodic — so a running server's
        // collaborative edit history survives restarts, in the project folder.
        // One code path for GUI + headless (DRY); the old global
        // `~/.lunco/journal/` `lunco-doc-bevy` copy is retired.
        if self.headless {
            // `setup_sandbox`'s twin-load path (and the journal persistence) needs
            // `WorkspaceResource`, which the GUI gets from `lunco-workbench`'s
            // `WorkspacePlugin` — a crate the headless server doesn't link. Bare
            // `init_resource` + just the journal plugin (not the full
            // `WorkspacePlugin`) keeps the headless surface minimal.
            app.init_resource::<lunco_workspace::WorkspaceResource>();
            app.add_plugins(lunco_workspace::journal_persistence::WorkspaceJournalPlugin);
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
            // here, in `SandboxCorePlugin`, so BOTH the GUI and the headless server
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
            // Policy projection: activate `LunCoPolicy` prims from the composed
            // stage into the hook registry. Because policies are USD prims they
            // sync via the journal above — no bespoke policy broadcast.
            app.add_systems(Update, project_usd_policies);
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
        app.add_systems(Update, track_ground_collider_pending);
        // Bind authored terrain layer maps (albedo/mineral/surface/normal) onto
        // the terrain's `ShaderMaterial`. GUI-only (materials are an `ui`-feature
        // concern; the headless server has no render materials and needs only the
        // collider).
        #[cfg(feature = "ui")]
        app.add_systems(Update, bind_terrain_layers);

        // Terrain-streaming progress → status bar: while the wanted tile set is
        // still baking in, show "streaming terrain N/M" (with the bus's progress
        // bar) instead of leaving the viewport an unexplained black void on
        // scene open. Pure derived read of `TerrainStreamStatus`.
        // `StatusBus` is initialized by the UI plugin stack, which `--no-ui` skips
        // at RUNTIME while the `ui` cargo feature stays compiled in — so gate on the
        // resource, not the feature, or a headless host panics on param validation.
        #[cfg(feature = "ui")]
        app.add_systems(
            Update,
            report_terrain_stream_status
                .run_if(resource_exists::<lunco_workbench::status_bus::StatusBus>),
        );

        // Scene-spawn progress → status bar, on the same terms and for the same
        // reason as the terrain mirror above. This one is also load-bearing for
        // OFFLINE RECORDING: `lunco-workbench`'s readiness gate
        // (`screenshot.rs::scene_visuals_ready`) treats an active `"scene"` entry as
        // "not presentable yet", which is how a shot avoids opening on half-spawned
        // prims. The workbench cannot read `SceneLoadInFlight` itself — it is a
        // UI-shell crate with no USD dependency — so the sandbox mirrors it here,
        // exactly as it mirrors `TerrainStreamStatus`.
        #[cfg(feature = "ui")]
        app.add_systems(
            Update,
            report_scene_spawn_status
                .run_if(resource_exists::<lunco_workbench::status_bus::StatusBus>),
        );

        // Hold each camera path at its first frame until the RECORDER rolls, so the
        // captured shot starts at the path's own frame 0.
        //
        // `ui`-gated, and gated on the resource, for the SAME two reasons as the two
        // status mirrors above — the previous unconditional registration was wrong on
        // both counts:
        //
        // * COMPILE time: `OfflineRecordingState` lives in `lunco-workbench`, which is
        //   `optional` + `ui`-only (it is the crate that owns the screenshot backend).
        //   Naming it unconditionally broke `cargo check -p lunco-sandbox-server`
        //   outright — the render-free server has no `lunco_workbench` in scope.
        // * RUN time: `--no-ui` is a runtime choice on a binary that still has the `ui`
        //   feature compiled in, and there the workbench plugin is never added, so a
        //   bare `Res<OfflineRecordingState>` fails param validation and panics the app.
        //
        // There is no "headless capture" this locks out: the offline recorder IS the
        // workbench's screenshot backend, so a build without the workbench has nothing
        // to start a shot from. Interactive (non-recording) playback is the `CameraPath`
        // transport command instead — see `camera_path_transport`.
        #[cfg(feature = "ui")]
        app.add_systems(
            Update,
            start_camera_paths_when_recording_starts
                .run_if(resource_exists::<lunco_workbench::screenshot::OfflineRecordingState>),
        );

        // Recorder → terrain streaming: put tile streaming in lockstep with the frame
        // for the duration of a capture. The REVERSE direction of the two status
        // mirrors above, and here for the same reason — `lunco-workbench` owns the
        // recorder but cannot name terrain, `lunco-terrain-surface` must not know what
        // a recorder is, and this crate is the assembly point that sees both.
        //
        // Ordered BEFORE `update_lod_tiles` so the flag a frame streams under is the
        // one that frame's recording state implies, not the previous frame's — an
        // off-by-one here would leave exactly the first captured frame streaming on
        // the wall clock, which is the frame everything else was made deterministic
        // for.
        #[cfg(feature = "ui")]
        app.add_systems(
            Update,
            mirror_recording_to_terrain_lockstep
                .before(lunco_terrain_surface::stream_viz::update_lod_tiles)
                .run_if(resource_exists::<lunco_workbench::screenshot::OfflineRecordingState>),
        );

        // Far-field self-shadow for STREAMED terrains: bake the horizon
        // heightfield from the surface oracle (no static mesh to rasterize) and
        // mirror the environment's R8 sun-visibility cache onto the LOD tile
        // materials. See `terrain_horizon`.
        #[cfg(feature = "ui")]
        terrain_horizon::register(app);

        // Engine light-handling policy: the locally-possessed vessel's headlights
        // cast shadows, the parked rovers stay cheap (reactive on possession
        // events — see `light_policy`). Render concern → `ui`-gated.
        #[cfg(feature = "ui")]
        app.add_plugins(light_policy::LightPolicyPlugin);
        // Environment-settings projection: apply a persisted `LunCoEnvironment`
        // prim's render knobs (exposure/bloom/ambient/earthshine) to the live
        // render state on stage change. UI-gated (render/camera state); core
        // persistence (authoring the prim) happens in `lunco-scene-commands`.
        #[cfg(feature = "ui")]
        app.add_systems(Update, project_env_settings);

        // LogDiagnosticsPlugin is loud (a multi-line summary every second) — gate
        // it on `--log-diag`.
        if args.iter().any(|a| a == "--log-diag") {
            app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
        }
    }
}

/// Hold dynamic-body activation while ground might still be on its way:
/// - a DEM terrain build is in flight (`DemTerrainRequest` — removed in the same
///   command batch that inserts the finished collider / oracle), OR
/// - the DEM bridge hasn't examined every USD prim yet (`Without<DemBridged>`) —
///   a terrain prim may be about to request a build, and rovers activate within
///   frames of spawn, so gating only on the request loses the startup race.
///
/// Without this gate a rover spawned over not-yet-collidable terrain free-falls
/// through the surface during the multi-second off-thread collider bake and is
/// lost below the map. Bounded by a timeout so a permanently-unbridgeable prim
/// (stage asset without a recipe) degrades to a loud warning instead of freezing
/// every dynamic body forever. This crate is the assembly point that sees both
/// `lunco-terrain-surface` (the request) and `lunco-usd-sim` (the activation
/// gate resource, via the `lunco-usd` facade).
fn track_ground_collider_pending(
    time: Res<Time>,
    building: Query<(), With<lunco_terrain_surface::DemTerrainRequest>>,
    unexamined: Query<(), (With<lunco_usd::UsdPrimPath>, Without<lunco_usd_terrain::DemBridged>)>,
    mut held_secs: Local<f32>,
    mut pending: ResMut<lunco_usd::GroundColliderPending>,
) {
    const MAX_HOLD_SECS: f32 = 30.0;
    let want = !building.is_empty() || !unexamined.is_empty();
    if want {
        *held_secs += time.delta_secs();
    } else {
        *held_secs = 0.0;
    }
    let now = want && *held_secs < MAX_HOLD_SECS;
    if want && !now && pending.0 {
        warn!(
            "[terrain] ground-collider gate held {MAX_HOLD_SECS}s — releasing dynamic bodies \
             (unbridgeable USD prim or stuck terrain build?)"
        );
    }
    if pending.0 != now {
        pending.0 = now;
    }
}

/// One-shot marker: the terrain's layer maps have been bound (or the prim authors
/// none), so [`bind_terrain_layers`] stops re-scanning it.
#[cfg(feature = "ui")]
#[derive(Component)]
struct TerrainLayersBound;

/// A bindable terrain layer role: the Shader input pair it reads
/// (`inputs:<name>_map` / `inputs:weight_<name>`), the `ShaderMaterial` texture
/// slot it fills, and the reflected blend-weight param(s) it raises.
#[cfg(feature = "ui")]
struct LayerRole {
    /// Shader-input stem + log label, e.g. `"albedo"`.
    name: &'static str,
    /// Sets the matching `Option<Handle<Image>>` slot on the material.
    set_slot: fn(&mut lunco_render_bevy::ShaderMaterial, Handle<Image>),
    /// Reflected `weight_*` params raised to the authored weight (surface has two).
    weights: &'static [&'static str],
}

/// GUI-only: read the terrain's **layer maps** from its bound **UsdShade
/// Material network** (doc 18 §3) — the only authoring path:
/// `rel material:binding` → Material → `outputs:surface.connect` → Shader,
/// whose `asset inputs:<role>_map` name the rasters and
/// `float inputs:weight_<role>` their blend weights (default 1.0).
///
/// Map paths are **relative to the scene's own root** (e.g.
/// `terrain/apollo15/materials/textures/ortho.png`) and load through the
/// `twin://` asset source, so they travel with the Twin — no engine-global
/// `lunco://` link. [`bind_terrain_layers`] sets the matching `ShaderMaterial`
/// slot and raises the role's blend weight(s).
///
/// Roles: `albedo` (real colour), `mineral` (unlit classification drape),
/// `surface` (packed rough/AO/hazard — overrides the P3b derived bake),
/// `normal` (meso normal — overrides the derived bake). Maps only render when
/// the prim's `shaderPath` is `terrain_layered.wgsl` (which declares the
/// bindings); with `regolith.wgsl` the slots are simply ignored.
///
/// Reads via [`UsdRead`], i.e. the live `StageView` over the canonical stage.
///
/// CONNECTED map inputs are skipped — a connected port is fed by a producer
/// node (doc 18 Tier B, bake nodes), not by an authored file.
#[cfg(feature = "ui")]
fn read_material_network_layer_maps(
    reader: &lunco_usd_bevy::StageView<'_>,
    sdf: &openusd::sdf::Path,
    roles: &'static [LayerRole],
) -> Vec<(&'static LayerRole, String, f32)> {
    let Some(shader) = lunco_usd_bevy::resolve_bound_shader(reader, sdf) else {
        return Vec::new();
    };
    roles
        .iter()
        .filter_map(|role| {
            let map_attr = format!("inputs:{}_map", role.name);
            if !reader.connections(&shader, &map_attr).is_empty() {
                return None;
            }
            let rel = reader.asset(&shader, &map_attr)?;
            // One authored weight per role, mirroring the flat-attr contract
            // (surface's two shader weights both receive it).
            let weight = role
                .weights
                .first()
                .and_then(|w| reader.real_f32(&shader, &format!("inputs:{w}")))
                .unwrap_or(1.0);
            Some((role, rel, weight))
        })
        .collect()
}

/// **Start each camera path when the RECORDER starts.** A shot begins when the
/// camera rolls.
///
/// This replaced `start_camera_paths_when_terrain_ready`, which released on
/// "terrain resident" — an asset event on the wall clock. That was wrong twice
/// over, and the second one was expensive:
///
/// 1. **It made offline recording irreproducible.** MEASURED: two runs of
///    `episode_02_rover.usda` differed at EVERY frame of EVERY shot, starting at
///    frame 0 (viewport-crop RMSE 0.019-0.61, far above the perf-HUD text burnt
///    into each frame). A path released on a wall-clock event has already advanced
///    by an unknown amount of real time when capture begins, so the domain clock's
///    value at frame 0 was accumulated real time, not a constant. `camera_path.rs`
///    samples the curve as a pure function of that clock — pure of a floating
///    origin is still floating. Pinning the per-frame delta downstream (which the
///    recorder does) cannot fix an origin that moves.
/// 2. **It mis-framed shots.** One measured run opened on the camera pitched up at
///    empty starfield: the path had begun *before* the recorder and the opening
///    beat was simply gone. The hold existed to prevent exactly that.
///
/// Releasing from the recorder's start edge makes the gate's own time 0 at frame 0
/// by construction, after which it advances `1/fps` per captured frame — so the
/// pose at frame N is `f(N/fps)`, identical across runs and machines.
///
/// **Continuation across shots is the idempotence.** `release_camera_path_gate` is
/// a no-op on an already-running gate, and `Playback::head` keeps advancing between
/// shots, so the campaign's single 58 s curve spanning six shots stays ONE
/// continuous move — the release fires for real only on the first shot. It is not
/// rewound per shot, which would give six identical stutters instead.
///
/// **Now possible: per-shot camera paths.** Every gate used to release
/// simultaneously on one global terrain event, which is why the campaign is
/// authored as a single continuous curve. With release owned by the recorder, a
/// path could instead be bound to a specific shot and released only when that shot
/// starts. Nothing here does that yet — noted so whoever authors shots next knows
/// the constraint has lifted.
///
/// **Consequence — live preview.** Terrain-ready was also what started paths in an
/// ordinary interactive session, where no recorder ever runs; those paths would
/// otherwise stay held forever. That is now served by an EXPLICIT transport verb,
/// the `CameraPath` command
/// ([`camera_path_transport`](lunco_usd_bevy::camera_path::camera_path_transport)),
/// addressed by the path prim's USD path. Still deliberately not a second
/// *automatic* release: two things racing to start the same shot is the bug this
/// replaced. One automatic start (the recorder, for capture) and one manual verb
/// (the command, for preview and scrubbing), never a fallback chain between them.
#[cfg(feature = "ui")]
fn start_camera_paths_when_recording_starts(
    recording: Res<lunco_workbench::screenshot::OfflineRecordingState>,
    resolved: Res<lunco_time::ResolvedDomains>,
    mut gates: Query<(&lunco_usd_bevy::camera_path::CameraPathGate, &mut lunco_time::TimeDomain)>,
    // Edge, not level: release exactly on the false→true transition so the gate's
    // origin is a single well-defined instant rather than "every frame we happen to
    // be recording".
    mut was_active: Local<bool>,
) {
    let started = recording.active && !*was_active;
    *was_active = recording.active;
    if !started || gates.is_empty() {
        return;
    }
    for (gate, mut domain) in &mut gates {
        let Some(parent_t) = resolved.get(gate.parent) else {
            continue; // parent clock not resolved yet — try next frame
        };
        // Change-detection: `Query::iter_mut` hands out `Mut`, so touching an
        // already-running gate would mark it changed every frame. The release is
        // idempotent, but do not pay for it on a running shot.
        if domain.scale == 0.0 {
            lunco_usd_bevy::camera_path::release_camera_path_gate(&mut domain, parent_t);
            info!("[camera-path] recording started — rolling shot from its first frame");
        }
    }
}

/// Mirror the recorder's `active` bit onto
/// [`TerrainStreamLockstep`](lunco_terrain_surface::TerrainStreamLockstep), so terrain
/// tile streaming runs in lockstep with the captured frame instead of against the
/// wall clock for exactly as long as a recording is capturing.
///
/// The problem it closes: the readiness gate makes the scene presentable at frame 0,
/// and recorder-owned camera-path release makes frame 0 bit-identical across runs —
/// but neither holds streaming steady THROUGH a shot. As the camera moves the LOD
/// selection changes, bakes are queued, and they land a scheduling-dependent number
/// of frames later. MEASURED before this: two runs of `episode_02_rover.usda`
/// differed on the frozen shots (01, 02, 03, 06) in 25-38 separate blocks of frames
/// each, with the final frame matching every time — a transient, not accumulation,
/// which is the signature of streaming catching up at a different rate.
///
/// See [`TerrainStreamLockstep`](lunco_terrain_surface::TerrainStreamLockstep) for
/// what the flag changes and why it is a flag rather than the default.
///
/// Level-triggered, not edge-triggered (unlike
/// [`start_camera_paths_when_recording_starts`], which needs an instant): the flag
/// must be true for the whole capture and false after, including after a recording
/// that ended by timing out. Writes only on an actual change so the resource's
/// change-detection tick stays meaningful.
#[cfg(feature = "ui")]
fn mirror_recording_to_terrain_lockstep(
    recording: Res<lunco_workbench::screenshot::OfflineRecordingState>,
    mut lockstep: ResMut<lunco_terrain_surface::TerrainStreamLockstep>,
) {
    if lockstep.0 != recording.active {
        lockstep.0 = recording.active;
        info!(
            "[terrain] streaming lockstep {} (offline recording {})",
            if recording.active { "ON" } else { "OFF" },
            if recording.active { "started" } else { "ended" },
        );
    }
}

/// Mirror [`lunco_terrain_surface::TerrainStreamStatus`] into the workbench
/// [`StatusBus`](lunco_workbench::status_bus::StatusBus) so scene-open tile
/// baking is visible ("streaming terrain N/M" + progress bar) instead of an
/// unexplained black viewport. Progress entries auto-clear once the wanted set
/// is fully resident.
#[cfg(feature = "ui")]
fn report_terrain_stream_status(
    status: Res<lunco_terrain_surface::TerrainStreamStatus>,
    // `Option`: the `ui` FEATURE is compile-time, but `--no-ui` headless is a
    // RUNTIME choice on the same binary — the workbench (and its `StatusBus`)
    // is simply not added there, and a bare `ResMut` panics the whole app.
    bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
) {
    let Some(mut bus) = bus else { return };
    // Shared with the screenshot readiness gate's `VISUAL_BUSY_SOURCES` — see the
    // const's docs for why this must not be a local literal.
    const SOURCE: &str = lunco_workbench::status_bus::TERRAIN_SOURCE;
    if status.wanted > 0 && status.resident < status.wanted {
        bus.push_progress(
            SOURCE,
            format!("streaming terrain tiles {}/{}", status.resident, status.wanted),
            status.resident as u64,
            status.wanted as u64,
        );
    } else {
        bus.clear_progress(SOURCE);
    }
}

/// Mirror USD scene-spawn progress into the workbench
/// [`StatusBus`](lunco_workbench::status_bus::StatusBus) under
/// [`SCENE_SOURCE`](lunco_workbench::status_bus::SCENE_SOURCE), the twin of
/// [`report_terrain_stream_status`].
///
/// Two signals, because they cover different windows and neither subsumes the
/// other:
///
/// * [`SceneLoadInFlight`](lunco_usd_sim::cosim::SceneLoadInFlight) — present from
///   `LoadScene` until every `UsdAwaitingStage` prim for that stage has been
///   drained by `sync_usd_visuals`. This covers the gap BEFORE any prim entity
///   exists, which an entity count alone reads as "nothing to wait for".
/// * `UsdAwaitingStage` entities — prims queued on a stage that has not resolved.
///   This covers spawns with no `LoadScene` guard behind them (deferred instance
///   and reference spawns), which the resource alone would miss.
///
/// Consumed by the offline recorder's readiness gate as well as the status bar;
/// see the registration site for why the mirror lives here rather than in
/// `lunco-workbench`.
#[cfg(feature = "ui")]
fn report_scene_spawn_status(
    in_flight: Option<Res<lunco_usd_sim::cosim::SceneLoadInFlight>>,
    awaiting: Query<(), With<lunco_usd_bevy::UsdAwaitingStage>>,
    // `Option` for the same reason as the terrain mirror: `--no-ui` is a RUNTIME
    // choice on a binary that still has the `ui` feature compiled in.
    bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
) {
    let Some(mut bus) = bus else { return };
    const SOURCE: &str = lunco_workbench::status_bus::SCENE_SOURCE;
    let pending = awaiting.iter().count();
    if let Some(g) = in_flight {
        // `total = 0` is the bus's "indeterminate" encoding — the number of prims
        // a scene will spawn is not known until it has spawned them.
        bus.push_progress(SOURCE, format!("spawning scene {}", g.path), 0, 0);
    } else if pending > 0 {
        bus.push_progress(SOURCE, format!("spawning {pending} prims"), 0, 0);
    } else {
        bus.clear_progress(SOURCE);
    }
}

#[cfg(feature = "ui")]
fn bind_terrain_layers(
    q: Query<
        (Entity, &lunco_usd::UsdPrimPath, &MeshMaterial3d<lunco_render_bevy::ShaderMaterial>),
        (With<lunco_terrain_surface::DemTerrainSurface>, Without<TerrainLayersBound>),
    >,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    asset_server: Res<AssetServer>,
    // OPTIONAL, because `Assets<ShaderMaterial>` only exists where a renderer does:
    // `LuncoRenderPlugin` registers the store, and `--no-ui` never adds it. A plain
    // `ResMut` here made this system a hard panic ("Resource does not exist") on every
    // headless start of this binary. Binding maps onto a material is meaningless without
    // a material, so the honest headless behaviour is to skip.
    mats: Option<ResMut<Assets<lunco_render_bevy::ShaderMaterial>>>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
) {
    use lunco_materials::ParamValue;

    let Some(mut mats) = mats else { return };

    const ROLES: &[LayerRole] = &[
        LayerRole { name: "albedo", set_slot: |m, h| m.albedo_map = Some(h), weights: &["weight_albedo"] },
        LayerRole { name: "mineral", set_slot: |m, h| m.mineral_map = Some(h), weights: &["weight_mineral"] },
        LayerRole { name: "surface", set_slot: |m, h| m.surface_map = Some(h), weights: &["weight_rough", "weight_ao"] },
        LayerRole { name: "normal", set_slot: |m, h| m.normal_map = Some(h), weights: &["weight_normal"] },
    ];

    for (entity, prim_path, mat3d) in &q {
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else {
            commands.entity(entity).try_insert(TerrainLayersBound);
            continue;
        };

        // Read the LIVE canonical stage (built on demand from the asset's recipe)
        // — the source of truth — through the `UsdRead` read body
        // (`read_material_network_layer_maps`).
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else {
            // No live stage (asset carries no recipe / build failed) — retry next frame.
            continue;
        };
        // Collect the authored (role, rel-path, weight) before touching the
        // material, so we can wait for the Twin + material without half-binding.
        // The bound UsdShade Material network is the ONLY source (doc 18 §3).
        let authored: Vec<(&LayerRole, String, f32)> =
            read_material_network_layer_maps(&cs.view(), &sdf, ROLES);

        if authored.is_empty() {
            // No layer authored — stop re-scanning this terrain.
            commands.entity(entity).try_insert(TerrainLayersBound);
            continue;
        }
        // Layer paths are root-relative (`{base_uri}/{rel}` below), and the scene's
        // OWN asset path is the only authority on which root that is: every scene
        // — an open Twin or a downloaded scenario — is addressed
        // `twin://<name>/<rel>`, so its source + first segment give the root.
        //
        // There is deliberately NO fallback. Guessing the "primary" open Twin for a
        // scene we cannot identify is what bound a downloaded twin's layers under
        // the local demo twin and silently loaded the wrong textures. A scene from
        // a source with no root (a bare default-source asset) has no twin root to
        // resolve against, so binding is skipped and said out loud.
        let Some(asset_path) = asset_server.get_path(id) else { continue };
        let source = match asset_path.source() {
            bevy::asset::io::AssetSourceId::Name(n) => n.to_string(),
            bevy::asset::io::AssetSourceId::Default => {
                warn!(
                    "[usd-dem] terrain layers on `{}` are root-relative, but the scene \
                     carries no source root to resolve them against — skipping",
                    asset_path.path().display()
                );
                commands.entity(entity).try_insert(TerrainLayersBound);
                continue;
            }
        };
        let Some(root) = asset_path
            .path()
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
        else {
            continue;
        };
        let base_uri = format!("{source}://{root}");
        // Wait for the material to exist before binding (created async by the USD
        // shader system); retry next frame until it does.
        let Some(mut material) = mats.get_mut(&mat3d.0) else { continue };

        // PUBLISH alongside binding, because the static mesh is only half the
        // audience: a `lodViz = true` site draws streamed geomorph tiles, whose
        // materials this system never touches. Handing the same handles to
        // `TerrainAuthoredMaps` is what lets those tiles show the authored
        // orthophoto instead of pure procedural regolith (doc 18 step 4).
        let mut published = lunco_terrain_surface::TerrainAuthoredMaps::default();

        for (role, rel, weight) in authored {
            let uri = format!("{base_uri}/{rel}");
            let handle: Handle<Image> = asset_server.load(&uri);
            (role.set_slot)(&mut material, handle.clone());
            for w in role.weights {
                material.set(w, ParamValue::F32(weight));
            }
            match role.name {
                "albedo" => {
                    published.albedo = Some(handle);
                    published.weight_albedo = weight;
                }
                "mineral" => {
                    published.mineral = Some(handle);
                    published.weight_mineral = weight;
                }
                // `surface`/`normal` are not forwarded: the streamed path bakes
                // its own from the DEM at tile resolution (`TerrainDerivedMaps`)
                // and its per-depth weights are a LOD decision, not the author's.
                _ => {}
            }
            info!("[usd-dem] bound terrain {} layer '{rel}' (weight {weight}) → {uri}", role.name);
        }
        commands.entity(entity).try_insert(published);
        commands.entity(entity).try_insert(TerrainLayersBound);
    }
}

/// The headless runner: the Modelica/spawn cores a windowed build gets
/// transitively from its UI plugins, plus the `ScheduleRunnerPlugin` that ticks
/// the app in winit's place. Added only when running headless.
/// GPU-full WINDOWLESS recording mode (`--offscreen`): the render stack is real
/// (wgpu device, render world, `LuncoRenderPlugin` visuals) but no window ever
/// opens — the scene renders into an offscreen target image sized by
/// `--record-size WxH` (default 1280x720, the same resolution the windowed
/// sandbox opens at) and the offline recorder captures that image. Combined
/// with `--record-offline out.mp4 --record-frames N` this is the one-command
/// take: the process exits by itself once the recording drains.
///
/// Contrast with [`SandboxHeadlessPlugin`] (the `--no-ui` SERVER: no GPU at
/// all, `backends: None`): offscreen must NOT insert `NoRenderVisuals` (meshes
/// really load) and must NOT re-add `SyncWorldPlugin` (a real backend builds
/// the render world itself — adding it again would double-register).
#[cfg(all(feature = "ui", feature = "lunco-api"))]
pub struct SandboxOffscreenPlugin;

#[cfg(all(feature = "ui", feature = "lunco-api"))]
impl Plugin for SandboxOffscreenPlugin {
    fn build(&self, app: &mut App) {
        // Same non-UI cores the headless server needs (see the twin comments in
        // `SandboxHeadlessPlugin`): the Modelica compile channels and the
        // spawn-command registry both normally arrive via UI plugins.
        app.add_plugins(lunco_modelica::ModelicaCorePlugin);
        app.add_plugins(lunco_scene_commands::commands::SpawnCommandPlugin);

        // The workspace session (WorkspaceResource + journal persistence) —
        // the GUI gets this from `WorkbenchPlugin`, which this mode skips.
        // `setup_sandbox`'s twin-load path panics without it.
        app.add_plugins(lunco_workspace::WorkspacePlugin);

        // The offline recorder itself — normally added by `WorkbenchPlugin`,
        // which this mode skips (egui needs a window).
        app.add_plugins(lunco_workbench::screenshot::ScreenshotPlugin);

        // No winit event loop, so tick the app ourselves — flat out, zero wait:
        // while recording, `drive_offline_clock` paces the sim (one 1/fps step
        // per frame, back-pressure holds the clock), so a faster tick rate means
        // faster-than-realtime capture, never a wrong-speed video.
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::ZERO,
        ));

        // One-shot contract: when the recording fully drains (frames delivered,
        // saves done, video trailer written), exit the process.
        app.insert_resource(lunco_workbench::screenshot::ExitAfterRecording);

        app.add_systems(Startup, setup_offscreen_target);
        app.add_systems(
            Update,
            (retarget_cameras_to_offscreen, activate_offscreen_camera),
        );

        info!("[offscreen] GPU-full windowless recording mode: no window, scene renders to an offscreen target");
    }
}

/// Create the offscreen render-target image and expose it to the recorder as
/// [`lunco_workbench::screenshot::OfflineCaptureTarget`].
#[cfg(all(feature = "ui", feature = "lunco-api"))]
fn setup_offscreen_target(
    mut images: ResMut<Assets<bevy::image::Image>>,
    mut commands: Commands,
) {
    let (width, height) = parse_record_size();
    let mut image = bevy::image::Image::new_target_texture(
        width,
        height,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    // `new_target_texture` sets RENDER_ATTACHMENT|TEXTURE_BINDING|COPY_DST;
    // the screenshot readback additionally copies OUT of the texture.
    image.texture_descriptor.usage |= bevy::render::render_resource::TextureUsages::COPY_SRC;
    let handle = images.add(image);
    info!("[offscreen] render target {width}x{height} (override with --record-size WxH)");
    commands.insert_resource(lunco_workbench::screenshot::OfflineCaptureTarget(handle));
}

/// Point every camera that targets a window at the offscreen image instead.
/// Runs every frame because cameras spawn throughout a session (scene loads,
/// camera paths, possession) and each spawns targeting the — nonexistent —
/// primary window; the rewrite is a cheap match on a handful of entities.
#[cfg(all(feature = "ui", feature = "lunco-api"))]
fn retarget_cameras_to_offscreen(
    target: Option<Res<lunco_workbench::screenshot::OfflineCaptureTarget>>,
    mut cameras: Query<(
        &mut bevy::camera::RenderTarget,
        Option<&mut bevy::camera::Projection>,
    )>,
) {
    let Some(target) = target else { return };
    for (mut rt, projection) in &mut cameras {
        if matches!(*rt, bevy::camera::RenderTarget::Window(_)) {
            *rt = bevy::camera::RenderTarget::Image(target.0.clone().into());
            // BEVY QUIRK (0.19): `camera_system` recomputes a camera's target
            // info on window/image EVENTS, `is_added`, or PROJECTION changes —
            // NOT on `RenderTarget` component changes. A camera whose
            // projection bound while its target was still the nonexistent
            // primary window resolves to nothing, and pointing it at the image
            // afterwards leaves `computed_size = None` FOREVER — the render
            // world silently skips it (black take, no log). Touching the
            // projection's change tick forces the recompute.
            if let Some(mut projection) = projection {
                projection.set_changed();
            }
        }
    }
}

/// Windowed mode always has an active camera — the workbench VIEWPORT camera,
/// which this mode skips along with the rest of the workbench. Every camera a
/// scene brings spawns `is_active: false` by design (see the camera-ambiguity
/// fix), so without this nothing renders and the recording is black frames.
/// When NO camera is active, activate the first authored [`lunco_render::SceneCamera`]
/// — the scene's own framing intent (e.g. the sandbox `WideShot`), and the same
/// camera its cinematic paths drive. Scenes without an authored camera get a
/// loud once-per-run warning rather than a silent black take.
#[cfg(all(feature = "ui", feature = "lunco-api"))]
fn activate_offscreen_camera(
    mut cameras: Query<(
        Entity,
        &mut Camera,
        bevy::ecs::query::Has<Camera3d>,
    )>,
    scene_cams: Query<(), With<lunco_render::SceneCamera>>,
    mut warned: Local<bool>,
) {
    // "Active" must mean RENDERING: a bare `Camera` with no `Camera3d` has no
    // render graph and draws nothing (the avatar rig spawns one such), so it
    // neither counts as coverage nor may keep the take black by squatting on
    // the active slot.
    if cameras.iter().any(|(_, c, has_pipeline)| c.is_active && has_pipeline) {
        return;
    }
    let target = cameras
        .iter()
        .find(|(e, _, has_pipeline)| *has_pipeline && scene_cams.contains(*e))
        .map(|(e, ..)| e);
    if let Some(target) = target {
        for (e, mut cam, _) in &mut cameras {
            if e == target {
                info!("[offscreen] activating authored scene camera {e}");
                cam.is_active = true;
            } else if cam.is_active {
                info!("[offscreen] deactivating non-rendering active camera {e}");
                cam.is_active = false;
            }
        }
    } else if !*warned && !cameras.is_empty() {
        *warned = true;
        warn!(
            "[offscreen] no renderable camera yet (SceneCamera binding pending or the \
             scene authors none) — the recording stays black until one exists"
        );
    }
}

/// Parse `--record-size WxH`; default 1280x720 — the resolution the windowed
/// sandbox authors for its window (see `default_plugins`), so offscreen
/// recordings match windowed ones by default.
#[cfg(all(feature = "ui", feature = "lunco-api"))]
fn parse_record_size() -> (u32, u32) {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--record-size" {
            if let Some(spec) = args.get(i + 1) {
                if let Some((w, h)) = spec.split_once('x') {
                    if let (Ok(w), Ok(h)) = (w.trim().parse(), h.trim().parse()) {
                        return (w, h);
                    }
                }
                warn!("--record-size expects WxH (e.g. 1920x1080), got {spec:?} — using 1280x720");
            }
        }
    }
    (1280, 720)
}

pub struct SandboxHeadlessPlugin;

impl Plugin for SandboxHeadlessPlugin {
    fn build(&self, app: &mut App) {
        // Modelica COMPILE CORE only (channels + worker thread + `.mo` asset
        // loader + compile-dispatch systems) — NO egui/viz/workbench. Windowed
        // builds get this transitively via `ModelicaWorkbenchPlugin`; headless
        // must add it directly or the cosim `on_load_scene` observer panics on a
        // missing `Res<ModelicaChannels>`. The server runs Modelica cosim models
        // authoritatively, so it needs the real compile path, not a stub.
        app.add_plugins(lunco_modelica::ModelicaCorePlugin);

        // Spawn-command CORE (runtime spawn/move/property commands + the
        // `apply_net_replication` system that tags dynamic scene bodies with
        // `NetReplicate`). Windowed builds get this transitively via
        // `SandboxEditPlugin`; without it the headless host replicates NOTHING
        // (the connect baseline is empty) because nothing marks the rovers. The
        // gizmo/selection/physics-viz halves of `SandboxEditPlugin` stay UI-only.
        app.add_plugins(lunco_scene_commands::commands::SpawnCommandPlugin);

        // No GPU renderer here, so the render-side systems that produce visual
        // components (`Mesh3d`, and the shader-pipeline `ShaderMaterial`) never
        // run. Tell the USD sim loader NOT to wait for them before building wheel
        // physics — otherwise raycast rovers defer their drivetrain forever and
        // the authoritative server can't simulate or replicate a drivable rover.
        app.insert_resource(lunco_usd::NoRenderVisuals);

        // Restore the render-world SYNC BOOKKEEPING that `backends: None` skips.
        //
        // Without this, `LoadScene` ABORTS the process headless. `RenderPlugin`
        // only adds `ExtractPlugin` — and hence `SyncWorldPlugin`, the owner of
        // the `PendingSyncEntity` resource — when a GPU backend actually comes up
        // (bevy_render/src/lib.rs: "We only create the render world ... if we have
        // a rendering backend"). But the per-component plugins it adds
        // UNCONDITIONALLY (`SyncComponentPlugin`, via `CameraPlugin` and friends)
        // still install component hooks that do a bare
        // `world.resource_mut::<PendingSyncEntity>()`. So on a backend-less app the
        // hook fires with no resource → `Res` validation failure → non-unwinding
        // panic in a Drop → `abort`.
        //
        // It only bites on REMOVAL, which is why boot survived and only scene
        // swaps died: the *add* observer lives in the absent `SyncWorldPlugin`, but
        // the *remove* hook lives in the always-present `SyncComponentPlugin`.
        // `NoRenderVisuals` keeps `Mesh3d` off a headless entity, but `Camera` and
        // the lights are render-synced too — and `LoadScene` despawns them.
        //
        // `SyncWorldPlugin` alone is the minimal repair: it inits the resource and
        // adds the matching add/remove observers, with NO render sub-app (adding
        // `ExtractPlugin` would build one and then run render schedules against a
        // device that doesn't exist). Nothing drains `PendingSyncEntity` here, so
        // it accumulates — but only one small record per synced-entity spawn/despawn,
        // never per frame, so a server that loads a scene now and then grows by a
        // few hundred KB, not without bound in steady state.
        //
        // Upstream shape: bevy registers hooks that assume a plugin it may not have
        // added. Re-test on the next bevy bump and drop this if it starts adding
        // `SyncWorldPlugin` unconditionally.
        // ONLY in a `ui` build. The hooks this works around are installed by
        // `RenderPlugin` itself — with no bevy_render linked (the `--no-ui` server) there
        // are no render-sync hooks, so there is nothing to repair and no plugin to add.
        #[cfg(feature = "ui")]
        app.add_plugins(bevy::render::sync_world::SyncWorldPlugin);

        // No winit event loop drives updates headless, so install a runner that
        // ticks the app at the sim's fixed rate. (Windowed builds are paced by
        // winit / vsync.)
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / lunco_core::FIXED_HZ as f64),
        ));

        info!("[net] sandbox running HEADLESS (--no-ui): no window/GPU/egui; sim + networking host only");
    }
}

/// Resource that holds the asset-source-relative path of the scene to load on
/// Startup. Initialised from the `--scene` CLI arg by [`SandboxCorePlugin`].
#[derive(Resource)]
pub struct ScenePath(pub String);

// `set_parent_in_place` is `disallowed_methods`-banned for its atomicity
// hazard (a `GridAnchor`/`RigidBody` parented after spawn can be mis-tagged
// `RigidBody::Static`). The two uses here parent the big_space root → Grid
// and a `DirectionalLight` → Grid — neither is a rigid body / GridAnchor, so
// that hazard doesn't apply. Locally allowed.
#[allow(clippy::disallowed_methods)]
fn setup_sandbox(world: &mut World) {
    let scene_path: String = world.resource::<ScenePath>().0.clone();

    // The persistent world shell (BigSpace root + `WorldGrid` + the single
    // `FloatingOrigin`) is owned by `WorldShellPlugin`. `ensure_world_root` is
    // create-or-get, so the Sun hangs off the canonical grid regardless of which
    // Startup system ran first.
    let grid = lunco_core::ensure_world_root(world);

    // --- Sun (directional light) on the world grid ---
    //
    // Real lunar shadows: hard-edged, jet-black, and long. Canonical lunar-sun
    // cascade split + 4096² atlas from the single source of truth
    // (`lunco_render::LunarSunShadow`), shared with the celestial and USD paths.
    // The biases are overridden for this binary's hard-shadow look: with
    // `Hardware2x2` filtering (see `force_hard_shadow_filtering`) the normal bias
    // must stay small or it detaches/softens the contact edge — unlike the
    // terrain-acne-tuned default (0.06/2.5) used under PCF.
    let sun = lunco_render::LunarSunShadow {
        depth_bias: 0.02,
        normal_bias: 0.8,
        ..Default::default()
    };
    // Illuminance + angular size from the active-scene `LunarSun` resource (every
    // camera's exposure reads the same resource, so sun lux and camera EV can't
    // drift apart).
    let ls = *world.resource::<lunco_environment::LunarSun>();
    world.insert_resource(sun.shadow_map());
    world.spawn((
        sun.directional_light(Color::WHITE, ls.illuminance_lux),
        sun.cascade_config(),
        lunco_core::SunAngularDiameter(ls.angular_diameter_deg),
        // Low sun (~11° above horizon, yaw 0.5 rad) for long raking lunar
        // shadows — same YXZ convention as `SetEnvironmentLight` and the
        // Inspector → Environment controls.
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.5, -0.2, 0.0)),
        GlobalTransform::default(),
        CellCoord::default(),
        Name::new("Sun"),
        // Default sun for scenes that author no lighting. A scene that authors a
        // UsdLux `DistantLight` (e.g. the moonbase Twin) replaces it: the loader
        // despawns every `FallbackSceneLight` and takes over ambient too.
        lunco_usd::FallbackSceneLight,
        ChildOf(grid),
    ));

    // ── Boot-entry policy (GUI only) ─────────────────────────────────────────
    // Before loading the default scene, consult the shared boot policy
    // (`boot.rhai`, via `lunco_tutorial::consult_boot`). On a first interactive
    // run it TAKES OVER — onboards with a tutorial that `load_scene`s its own
    // environment — so we skip the default load and there's no load-then-replace
    // race. Explicit `--scene` / `--api` → the policy stands down and we load
    // normally. Headless (no `ui`, no tutorial engine) never onboards → loads.
    // The world shell (grid + Sun) above is set up regardless, so a taking-over
    // tutorial scene still has it.
    #[cfg(feature = "ui")]
    {
        let has_scene_arg = std::env::args().any(|a| a == "--scene");
        let automated = std::env::args().any(|a| a == "--api" || a == "--no-ui");
        if lunco_tutorial::consult_boot(world, has_scene_arg, automated) {
            return;
        }
    }

    // WEB: do NOT load a startup scene here. The generated page's autoload hook
    // (index.html → a `LoadScene` command) loads the deployment's default twin
    // (moonbase) directly. A second built-in `sandbox_scene` load here raced that
    // autoload: the twin reload's cleanup despawned the sandbox_scene entity while
    // `sync_usd_visuals` still had a deferred `insert::<UsdPrimPath>` queued for it
    // → "Entity despawned" panic → aborted wasm → dark viewport. The filesystem
    // twin-resolve below is meaningless in the browser anyway (no `twin.toml` FS).
    #[cfg(not(target_arch = "wasm32"))]
    load_startup_scene(world, scene_path);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (world, scene_path);
        info!("[sandbox] web startup: no built-in scene load — the page autoload hook loads the default twin directly");
    }
}

/// Native/headless startup-scene load: resolve the enclosing Twin folder for
/// `scene_path` (walk up to a `twin.toml`), register it as a workspace Twin so it
/// mounts doc-first, or fall back to a direct [`LoadScene`]. Web skips this — its
/// autoload hook loads the deployment twin directly (see [`setup_sandbox`]).
#[cfg(not(target_arch = "wasm32"))]
fn load_startup_scene(world: &mut World, scene_path: String) {
    // --- Load scene from USD ---
    // Resolve the absolute path to find the enclosing Twin folder.
    let pb = std::path::PathBuf::from(&scene_path);
    let abs_path = if pb.is_absolute() {
        pb
    } else {
        lunco_assets::assets_dir_abs().join(pb)
    };

    // The root that owns this scene — nearest `twin.toml` ancestor, else the
    // containing folder. Shared with the runtime open path (`OpenFile` →
    // `spawn_twin_from_scene`) so boot and commands cannot disagree about what
    // "the root" is for a given file.
    let twin_root = lunco_twin::root_for_file(&abs_path);

    let scene_file = abs_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    world.insert_resource(StartupSceneGuard { file: scene_file.clone() });

    // `--scene` is user-supplied, so `twin_root` may not be openable. There is
    // deliberately NO direct-`LoadScene` fallback here: a raw load mounts a
    // base-only stage and silently drops the doc overlay (placed waypoints,
    // runtime spawns, moved transforms). A fallback that discards user edits is
    // worse than a loud failure, so a bad path reports and stops — the
    // `StartupSceneGuard` failguard turns it into a visible error rather than an
    // empty viewport.
    match lunco_twin::TwinMode::open(&twin_root) {
        Ok(lunco_twin::TwinMode::Twin(mut twin)) | Ok(lunco_twin::TwinMode::Folder(mut twin)) => {
            let rel_scene_path = abs_path
                .strip_prefix(&twin_root)
                .map(lunco_assets::asset_path::slashed)
                .unwrap_or_else(|_| scene_file.clone());
            twin.set_default_scene(rel_scene_path);

            let twin_id = world
                .resource_mut::<lunco_workspace::WorkspaceResource>()
                .add_twin(twin);
            world.trigger(lunco_workspace::TwinAdded { twin: twin_id });
        }
        Ok(lunco_twin::TwinMode::Orphan(path)) => {
            error!(
                "[sandbox] `{}` resolved to an orphan (`{}`) — cannot open a root for `{scene_path}`",
                twin_root.display(),
                path.display()
            );
        }
        Err(err) => {
            error!(
                "[sandbox] could not open `{}` as a root for `{scene_path}`: {err}",
                twin_root.display()
            );
        }
    }
    // `--scene` is doc-backed through the same path as any workspace Twin: the
    // `TwinAdded` above runs the doc-first mount (`open_usd_docs_on_twin_added` →
    // `drain_pending_twin_docs`), and terrain edits stay on the incremental
    // re-bake — `LiveRebuildExempt` + `edit_confined_to_exempt_subtree` keep a
    // terrain-confined USD edit from ever reloading the scene.
}

/// Tracks the requested startup scene so [`startup_scene_failguard`] can turn a
/// silent asset-load failure into a loud, fatal error. Removed once the scene
/// has loaded (or failed), so later runtime `LoadScene`s (API / UI) — which
/// must NOT crash the app on a bad request — are never affected.
#[derive(Resource)]
struct StartupSceneGuard {
    /// File name of the requested startup scene, e.g. `sandbox_scene.usda`.
    file: String,
}

/// Fail loud if the `--scene` (or default) USD scene fails to load at startup.
///
/// The bug this guards: `--scene` paths are relative to the `assets/` source
/// root; prefixing `assets/` doubles it (`assets/assets/…`), the asset is not
/// found, and the app *silently* boots a scene-less world. Here a matching
/// `AssetLoadFailedEvent<UsdStageAsset>` → clear error + non-zero exit. Disarms
/// on success (scene produced `UsdPrimPath` entities) so runtime loads are safe.
fn startup_scene_failguard(
    guard: Option<Res<StartupSceneGuard>>,
    mut failures: MessageReader<AssetLoadFailedEvent<UsdStageAsset>>,
    scene: Query<(), With<UsdPrimPath>>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
) {
    let Some(guard) = guard else { return };

    for failed in failures.read() {
        let is_startup_scene = failed
            .path
            .path()
            .file_name()
            .and_then(|s| s.to_str())
            == Some(guard.file.as_str());
        if is_startup_scene {
            error!(
                "Startup scene `{}` failed to load: {}. \
                 NOTE: `--scene` is relative to the `assets/` source root — do NOT prefix \
                 `assets/` (use `scenes/sandbox/sandbox_scene.usda`, not `assets/scenes/...`).",
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

// (The hardcoded G-to-detach system was removed: dock release is now a vessel-class
//  actuator on the normal intent→port machinery — the `Release` intent (KeyG) → the
//  `release` port → `lunco_scene_commands::commands::ReleaseActuator` → DetachJoint.
//  See the joint-as-actuator refactor. Works for any possessed vessel + dock joint.)

fn on_restore_fallback_lights(
    _trigger: On<lunco_core::RestoreFallbackLights>,
    mut commands: Commands,
    fallbacks: Query<Entity, With<lunco_core::FallbackSceneLight>>,
    grid_q: Query<Entity, With<lunco_core::WorldGrid>>,
    ls: Res<lunco_environment::LunarSun>,
) {
    if !fallbacks.is_empty() {
        return;
    }
    let Some(grid) = grid_q.iter().next() else {
        warn!("[restore-fallback-lights] No WorldGrid found to parent sandbox fallback light");
        return;
    };

    let sun = lunco_render::LunarSunShadow {
        depth_bias: 0.02,
        normal_bias: 0.8,
        ..Default::default()
    };
    commands.insert_resource(sun.shadow_map());
    commands.spawn((
        sun.directional_light(Color::WHITE, ls.illuminance_lux),
        sun.cascade_config(),
        lunco_core::SunAngularDiameter(ls.angular_diameter_deg),
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.5, -0.2, 0.0)),
        GlobalTransform::default(),
        CellCoord::default(),
        Name::new("Sun"),
        lunco_core::FallbackSceneLight,
        ChildOf(grid),
    ));
    info!("[restore-fallback-lights] restored sandbox fallback light");
}

