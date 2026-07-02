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
use lunco_usd::{LoadScene, UsdDataExt, UsdPlugins, UsdPrimPath, UsdStageAsset};
use bevy::asset::AssetLoadFailedEvent;

/// Re-exported so the (bevy-free) bin crates can return it from `main` to
/// propagate the process exit code (e.g. the startup-scene fail-loud guard).
pub use bevy::app::AppExit;
use lunco_terrain_globe::TerrainPlugin;
use lunco_obstacle_field::ObstacleFieldPlugin;
use lunco_terrain_surface::TerrainSurfacePlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::LunCoAvatarPlugin;
use lunco_celestial::GravityPlugin;
use lunco_environment::EnvironmentPlugin;
use lunco_cosim::CoSimPlugin;
use lunco_cosim::systems::propagate::CosimSet as PropagateCosimSet;
use lunco_cosim::systems::apply_forces::CosimSet as ApplyForcesCosimSet;
// `ModelicaSet` orders the cosim pipeline (always). The egui workbench plugin is
// added by `SandboxUiPlugin`; headless adds `ModelicaCorePlugin` instead.
use lunco_modelica::ModelicaSet;

#[cfg(feature = "ui")]
mod ui;
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

/// Composition root. Builds the shared core, then conditionally layers on the UI
/// or the headless runner. Nothing UI-specific lives here beyond selecting the
/// windowing backend in [`default_plugins`].
fn run_with_mode(headless: bool) -> AppExit {
    // Native deep-link single-instance gate (GUI only). Register the
    // `luncosim://` scheme handler (desktop integration, this crate), then decide
    // whether THIS process is the app or just a courier forwarding a clicked link
    // to an already-running instance. Must happen before building the app so a
    // forward exits without opening a window. The returned inbox is inserted
    // below; a Bevy system drains it into the confirm prompt. Headless skips it.
    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    let deeplink_inbox = if !headless {
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

    let mut app = App::new();

    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    if let Some(inbox) = deeplink_inbox {
        app.insert_resource(inbox);
    }

    // Register every LunCo asset source (lunco://, lunco-lib://, twin://,
    // cached_textures://) + the shared `TwinRoots` resource in ONE shared place
    // (`lunco-assets`), so all binaries get identical schemes. MUST run before
    // `DefaultPlugins`/`AssetPlugin` snapshots the source registry.
    lunco_assets::register_lunco_asset_sources(&mut app);

    app.add_plugins(default_plugins(headless));
    app.add_plugins(SandboxCorePlugin { headless });

    #[cfg(feature = "ui")]
    if !headless {
        app.add_plugins(ui::SandboxUiPlugin);
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
/// None`: the render world + asset stores initialise (so USD visual sync can
/// populate the meshes avian colliders read), but no GPU device is created and
/// nothing is drawn — `ScheduleRunnerPlugin` (added by [`SandboxHeadlessPlugin`])
/// ticks the app in winit's place.
fn default_plugins(headless: bool) -> bevy::app::PluginGroupBuilder {
    use bevy::render::settings::WgpuSettings;
    // `headless` only selects render/window config in `ui` builds; a no-`ui`
    // build is always windowless, so the param is unused there.
    #[cfg(not(feature = "ui"))]
    let _ = headless;

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
    #[cfg(not(feature = "ui"))]
    let render_creation = WgpuSettings { backends: None, ..default() }.into();

    let group = DefaultPlugins
        .set(AssetPlugin {
            file_path: std::env::current_dir().unwrap_or_default().join("assets").to_string_lossy().to_string(),
            // Don't probe for `.meta` sidecars: we ship none, so every asset
            // load would otherwise fire a failed `<asset>.meta` fetch.
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .set(bevy::log::LogPlugin {
            // Quieten third-party noise (rumoca JIT + diffsol per-step).
            filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
            ..default()
        })
        .set(bevy::render::RenderPlugin { render_creation, ..default() });

    // Window/winit setup. With the `ui` feature the runtime `headless` flag still
    // picks the windowless variant (no primary window, WinitPlugin disabled).
    // Without `ui` there's no winit crate to disable, so just declare a
    // windowless `WindowPlugin`.
    #[cfg(feature = "ui")]
    let group = if headless {
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
            primary_window: Some(Window {
                // On wasm, attach to the `#bevy` canvas and mirror its CSS size.
                #[cfg(target_arch = "wasm32")]
                canvas: Some("#bevy".to_string()),
                #[cfg(target_arch = "wasm32")]
                fit_canvas_to_parent: true,
                present_mode,
                // Centralized merged-titlebar chrome + persisted geometry.
                ..lunco_workbench::restored_window(window_title)
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
/// its entry scene (`default_scene`) from the `scenario://` cache. Loaded once per
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
    let uri = lunco_networking::scenario_sync::scenario_asset_uri(&m.scenario_id, scene);
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
/// - **Host** — projects over its full native history (base `None`); the
///   `author != me` filter selects only client-authored edits (its own are
///   already applied at author time), so the host *sees* clients' edits.
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
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    mut registry: ResMut<lunco_usd::UsdDocumentRegistry>,
    // Entry ids already projected onto the scene (once-per-entry guard).
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let Some(journal) = journal else {
        return;
    };
    // Base head: the host holds full history natively (None); a client bases on
    // the downloaded snapshot's head, or waits if no scenario is loaded yet.
    let base: Option<&lunco_twin_journal::EntryId> = if role.is_host() {
        None
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
            // `FloatingOrigin`. big_space only registers its validation plugin
            // under `debug_assertions`, so the `.disable()` is gated the same way
            // (calling it in release would panic — the plugin isn't in the group).
            .add_plugins({
                let group = BigSpaceDefaultPlugins.build();
                #[cfg(debug_assertions)]
                let group = group.disable::<big_space::validation::BigSpaceValidationPlugin>();
                group
            })
            // EntityCount is cheap and useful any time we look at perf.
            .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
            .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
            // 12 solver substeps (avian default 6): joint-based rovers buzz the
            // chassis under drive torque at 6 substeps. Quantified in the headless
            // `rover_jitter` probe. See `project_physical_rover_suspension`.
            .insert_resource(avian3d::prelude::SubstepCount(12))
            .add_plugins(CoSimPlugin)
            .add_plugins(lunco_core::LunCoCorePlugin)
            .add_plugins(lunco_core::WorldShellPlugin)
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
            .add_plugins(GravityPlugin)
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
            .add_systems(Update, (startup_scene_failguard, lander_rover_joint_detach_key))
            // Cosim pipeline ordering inside FixedUpdate:
            //   HandleResponses → Propagate → ApplyForces → SpawnRequests.
            .configure_sets(FixedUpdate, (
                ModelicaSet::HandleResponses,
                PropagateCosimSet::Propagate,
                ApplyForcesCosimSet::ApplyForces,
                ModelicaSet::SpawnRequests,
            ).chain());

        // Dismiss the HTML loading screen once the first frame paints (wasm-only;
        // no-op on native). Pairs with `web/index.html` → `lunco-boot.js`.
        app.add_plugins(lunco_web::WebReadyPlugin);

        // HTTP automation bridge — native `--api` server / wasm JS bridge. Linked
        // in the GUI and the headless compile server alike.
        #[cfg(feature = "lunco-api")]
        app.add_plugins(lunco_api::LunCoApiPlugin::default());

        // Durable journal history for headless (`lunco-sandbox-server` / any
        // `--no-ui` host): load-on-startup + debounced-save of the canonical
        // journal, so collaborative edit history survives restarts. The GUI keeps
        // its own Twin-scoped `lunco-workspace` persistence, so this is
        // headless-only to avoid two mechanisms writing the same history.
        if self.headless {
            app.add_plugins(lunco_doc_bevy::JournalPersistencePlugin);
            // `setup_sandbox`'s twin-load path needs `WorkspaceResource`, which the
            // GUI gets from `lunco-workbench`'s `WorkspacePlugin` — a crate the
            // headless server doesn't link. Without this, a headless boot panics in
            // `setup_sandbox`. Bare `init_resource` (not the full `WorkspacePlugin`)
            // so we don't also pull in that plugin's Twin-scoped journal observers,
            // which would double up on the `JournalPersistencePlugin` above.
            app.init_resource::<lunco_workspace::WorkspaceResource>();
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
            // Scenario distribution Phase 4: once a connected client has fully
            // downloaded the host's advertised scenario, load its entry scene from
            // the `scenario://` cache (read-only consume). The bridge lives here —
            // the assembly crate that owns both the wire (`lunco-networking`) and
            // the scene loader (`lunco_usd::LoadScene`) — keeping each of those
            // crates free of the other.
            app.add_systems(Update, load_ready_scenario);
            // Layer B: project peers' live journal edits onto the local scene
            // (bidirectional — clients see the host's edits, the host sees
            // clients'; no-op when no scenario/journal is present).
            app.add_systems(Update, replay_scenario_journal);
            // Connect-menu bridge adapter + egui presence/tutorial overlays. Pulls
            // bevy_egui, so it's GUI-only and gated on `ui` (CQ-601) — the headless
            // server omits it. The host still answers runtime JoinServer/LeaveServer
            // via the networking plugin's typed command path (not this bridge).
            #[cfg(feature = "ui")]
            app.add_plugins(lunco_networking::ui::LunCoNetworkingUiPlugin);
        }

        // USD→DEM bridge: an authored terrain prim with `lunco:assetMode="dem"`
        // gets a DEM heightfield built onto it; its `materialType` authors the
        // material via the universal ShaderMaterial path. Core (not GUI-gated):
        // the headless server needs the collider for deterministic physics.
        app.add_systems(Update, (bridge_usd_dem_terrain, refresh_layered_terrain_layers));
        // Bind authored terrain layer maps (albedo/mineral/surface/normal) onto
        // the terrain's `ShaderMaterial`. GUI-only (materials are an `ui`-feature
        // concern; the headless server has no render materials and needs only the
        // collider).
        #[cfg(feature = "ui")]
        app.add_systems(Update, bind_terrain_layers);

        // LogDiagnosticsPlugin is loud (a multi-line summary every second) — gate
        // it on `--log-diag`.
        if args.iter().any(|a| a == "--log-diag") {
            app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
        }
    }
}

/// Marks a USD prim already examined by the DEM bridge (one-shot per prim).
#[derive(Component)]
struct DemBridged;

/// One-shot marker: the terrain's layer maps have been bound (or the prim authors
/// none), so [`bind_terrain_layers`] stops re-scanning it.
#[cfg(feature = "ui")]
#[derive(Component)]
struct TerrainLayersBound;

/// A bindable terrain layer role: the USD `lunco:terrain:layer:<name>:*`
/// namespace, the `ShaderMaterial` texture slot it fills, and the reflected
/// blend-weight param(s) it raises.
#[cfg(feature = "ui")]
struct LayerRole {
    /// USD namespace segment + log label, e.g. `"albedo"`.
    name: &'static str,
    /// Sets the matching `Option<Handle<Image>>` slot on the material.
    set_slot: fn(&mut lunco_materials::ShaderMaterial, Handle<Image>),
    /// Reflected `weight_*` params raised to the authored weight (surface has two).
    weights: &'static [&'static str],
}

/// GUI-only: bind authored terrain **layer maps** onto the terrain's
/// `ShaderMaterial`. For each role below it reads
/// `lunco:terrain:layer:<role>:map` (a path **relative to the open Twin**, e.g.
/// `terrain/connecting_ridge/color.png`) + optional `:weight` (default `1.0`) off
/// the terrain prim, loads the map through the `twin://` asset source (so it
/// travels with the Twin — no engine-global `lunco-lib://` link), sets the
/// matching slot, and raises the role's blend weight(s).
///
/// Roles: `albedo` (real colour), `mineral` (classification tint), `surface`
/// (packed rough/AO/hazard — overrides the P3b derived bake), `normal`
/// (meso normal — overrides the derived bake). Maps only render when the prim's
/// `shaderPath` is `terrain_layered.wgsl` (which declares the bindings); with
/// `regolith.wgsl` the slots are simply ignored. One-shot per terrain.
#[cfg(feature = "ui")]
fn bind_terrain_layers(
    q: Query<
        (Entity, &lunco_usd::UsdPrimPath, &MeshMaterial3d<lunco_materials::ShaderMaterial>),
        (With<lunco_terrain_surface::DemTerrainSurface>, Without<TerrainLayersBound>),
    >,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    twins: Res<lunco_assets::twin_source::TwinRoots>,
    asset_server: Res<AssetServer>,
    mut mats: ResMut<Assets<lunco_materials::ShaderMaterial>>,
    mut commands: Commands,
) {
    use lunco_materials::ParamValue;

    const ROLES: &[LayerRole] = &[
        LayerRole { name: "albedo", set_slot: |m, h| m.albedo_map = Some(h), weights: &["weight_albedo"] },
        LayerRole { name: "mineral", set_slot: |m, h| m.mineral_map = Some(h), weights: &["weight_mineral"] },
        LayerRole { name: "surface", set_slot: |m, h| m.surface_map = Some(h), weights: &["weight_rough", "weight_ao"] },
        LayerRole { name: "normal", set_slot: |m, h| m.normal_map = Some(h), weights: &["weight_normal"] },
    ];

    for (entity, prim_path, mat3d) in &q {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue };
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else {
            commands.entity(entity).insert(TerrainLayersBound);
            continue;
        };
        let reader = &*stage.reader;

        // Collect the authored (role, rel-path, weight) before touching the
        // material, so we can wait for the Twin + material without half-binding.
        let authored: Vec<(&LayerRole, String, f32)> = ROLES
            .iter()
            .filter_map(|role| {
                let map_attr = format!("lunco:terrain:layer:{}:map", role.name);
                let rel = reader.prim_attribute_value::<String>(&sdf, &map_attr)?;
                let weight = reader
                    .prim_attribute_value::<f32>(&sdf, &format!("lunco:terrain:layer:{}:weight", role.name))
                    .unwrap_or(1.0);
                Some((role, rel, weight))
            })
            .collect();

        if authored.is_empty() {
            // No layer authored — stop re-scanning this terrain.
            commands.entity(entity).insert(TerrainLayersBound);
            continue;
        }
        // Resolve relative to the open Twin via the `twin://<name>/<rel>` source.
        let Some((twin_name, _)) = twins.primary() else { continue };
        // Wait for the material to exist before binding (created async by the USD
        // shader system); retry next frame until it does.
        let Some(material) = mats.get_mut(&mat3d.0) else { continue };

        for (role, rel, weight) in authored {
            let uri = format!("twin://{twin_name}/{rel}");
            (role.set_slot)(material, asset_server.load(&uri));
            for w in role.weights {
                material.set(w, ParamValue::F32(weight));
            }
            info!("[usd-dem] bound terrain {} layer '{rel}' (weight {weight}) → {uri}", role.name);
        }
        commands.entity(entity).insert(TerrainLayersBound);
    }
}

/// USD→DEM bridge. For each USD prim authoring `lunco:assetMode = "dem"` +
/// `lunco:terrain:demSource = "<rel path>"`, resolve the path against the open
/// Twin root and put a `DemTerrainRequest` on the prim entity. `lunco-terrain-
/// streaming` then builds the heightfield mesh + collider onto it, and the prim's
/// `materialType` authors the material — so the whole path rides the universal
/// USD material/settings system with no bespoke material code.
/// USD-backed [`LayerAttrSource`](lunco_terrain_surface::LayerAttrSource): reads a
/// child layer prim's attributes through the stage reader, so terrain-surface's layer
/// parsers stay USD-free. Generic over the reader so we needn't name its concrete type.
struct UsdLayerAttrs<'a, R: lunco_usd::UsdDataExt> {
    reader: &'a R,
    sdf: openusd::sdf::Path,
}

impl<R: lunco_usd::UsdDataExt> lunco_terrain_surface::LayerAttrSource for UsdLayerAttrs<'_, R> {
    fn get_f32(&self, name: &str) -> Option<f32> {
        self.reader.prim_attribute_value::<f32>(&self.sdf, name)
    }
    fn get_i64(&self, name: &str) -> Option<i64> {
        self.reader.prim_attribute_value::<i32>(&self.sdf, name).map(|v| v as i64)
    }
    fn get_string(&self, name: &str) -> Option<String> {
        self.reader.prim_attribute_value::<String>(&self.sdf, name)
    }
    fn get_bool(&self, name: &str) -> Option<bool> {
        self.reader.prim_attribute_value::<bool>(&self.sdf, name)
    }
}

/// The `dem` (ground) child layer prim of a layered terrain, if authored.
fn find_dem_layer<R: lunco_usd::UsdDataExt>(
    reader: &R,
    terrain: &openusd::sdf::Path,
) -> Option<openusd::sdf::Path> {
    reader
        .prim_children(terrain)
        .into_iter()
        .find(|c| reader.prim_attribute_value::<String>(c, "lunco:layer").as_deref() == Some("dem"))
}

/// Parse the non-ground child layer prims (`craters`/`rocks`/`shader`/…) into the
/// composable [`TerrainLayerStack`](lunco_terrain_surface::TerrainLayerStack) via the
/// registry. Shared by the bridge (initial build) and the live-edit refresh.
fn parse_terrain_layer_stack<R: lunco_usd::UsdDataExt>(
    reader: &R,
    terrain: &openusd::sdf::Path,
    registry: &lunco_terrain_surface::TerrainLayerParserRegistry,
) -> lunco_terrain_surface::TerrainLayerStack {
    let mut stack = lunco_terrain_surface::TerrainLayerStack::default();
    for child in reader.prim_children(terrain) {
        let Some(layer_type) = reader.prim_attribute_value::<String>(&child, "lunco:layer") else {
            continue;
        };
        if layer_type == "dem" {
            continue;
        }
        if !registry.knows(&layer_type) {
            warn!("[usd-dem] child layer '{layer_type}' has no registered terrain layer parser");
            continue;
        }
        let attrs = UsdLayerAttrs { reader, sdf: child.clone() };
        if let Some(layer) = registry.parse(&layer_type, &attrs) {
            stack.0.push(layer);
        }
    }
    stack
}

/// Seed the shared [`ObstacleFieldSpec`] from the USD-authored `craters`/`rocks` child
/// layer prims so the Inspector's "Craters & Rocks" panel opens showing the scene's
/// ACTUAL values (density, size, ratios) instead of the resource defaults. Mirrors the
/// `SizeDist` the layer parsers build (`craters` → `new(8, mode, 40, 0.7)`, `rocks` →
/// `new(0.2, mode, mode*4, 0.6)`) so a subsequent panel edit starts from the authored
/// look rather than jumping. Writes the resource only (no `UpdateObstacleFieldSpec`,
/// no re-stamp — the terrain already built from the same USD stack).
fn sync_obstacle_spec_from_usd<R: lunco_usd::UsdDataExt>(
    reader: &R,
    terrain: &openusd::sdf::Path,
    spec: &mut lunco_obstacle_field::spec::ObstacleFieldSpec,
) {
    use lunco_obstacle_field::spec::SizeDist;
    for child in reader.prim_children(terrain) {
        match reader.prim_attribute_value::<String>(&child, "lunco:layer").as_deref() {
            Some("craters") => {
                let density = reader.prim_attribute_value::<f32>(&child, "density").unwrap_or(0.0);
                let mode = reader.prim_attribute_value::<f32>(&child, "sizeMode").unwrap_or(22.0);
                spec.craters.enabled = density > 0.0;
                spec.craters.density = density;
                spec.craters.depth_ratio =
                    reader.prim_attribute_value::<f32>(&child, "depthRatio").unwrap_or(0.3);
                spec.craters.rim_height_ratio =
                    reader.prim_attribute_value::<f32>(&child, "rimRatio").unwrap_or(0.5);
                spec.craters.size = SizeDist::new(8.0, mode, 40.0, 0.7);
                if let Some(seed) = reader.prim_attribute_value::<i32>(&child, "seed") {
                    spec.seed = seed as u64;
                }
            }
            Some("rocks") => {
                let density = reader.prim_attribute_value::<f32>(&child, "density").unwrap_or(0.0);
                let mode = reader.prim_attribute_value::<f32>(&child, "sizeMode").unwrap_or(0.6);
                spec.rocks.enabled = density > 0.0;
                spec.rocks.density = density;
                spec.rocks.size = SizeDist::new(0.2, mode, (mode * 4.0).max(2.5), 0.6);
            }
            _ => {}
        }
    }
}

/// Live-edit: when a stage is modified (a terrain layer prim was edited in the
/// Inspector / via `SetObjectProperty`), re-parse the composable stack of every
/// layered terrain on that stage and re-insert it. The change is picked up by
/// `regenerate_dem_layers` (it re-stamps off the retained base grid + re-scatters —
/// no GeoTIFF re-read), so crater/rock/shader tuning applies live.
fn refresh_layered_terrain_layers(
    mut ev: MessageReader<AssetEvent<lunco_usd::UsdStageAsset>>,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    registry: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    q: Query<(Entity, &lunco_usd::UsdPrimPath), With<lunco_terrain_surface::DemTerrainSurface>>,
    mut commands: Commands,
) {
    let mut modified = std::collections::HashSet::new();
    for e in ev.read() {
        if let AssetEvent::Modified { id } = e {
            modified.insert(*id);
        }
    }
    if modified.is_empty() {
        return;
    }
    for (entity, prim_path) in &q {
        if !modified.contains(&prim_path.stage_handle.id()) {
            continue;
        }
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue };
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        let stack = parse_terrain_layer_stack(&*stage.reader, &sdf, &registry);
        commands.entity(entity).insert(stack);
    }
}

fn bridge_usd_dem_terrain(
    q: Query<(Entity, &lunco_usd::UsdPrimPath), Without<DemBridged>>,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    twins: Res<lunco_assets::twin_source::TwinRoots>,
    registry: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    mut obstacle_spec: ResMut<lunco_obstacle_field::ObstacleFieldSpec>,
    mut commands: Commands,
) {
    for (entity, prim_path) in &q {
        // Wait until the prim's stage asset is loaded (read attrs from it).
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue };
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else {
            commands.entity(entity).insert(DemBridged);
            continue;
        };
        let reader = &*stage.reader;
        commands.entity(entity).insert(DemBridged); // examined — don't re-scan

        // A DEM-backed terrain: `lunco:assetMode = "dem"` (or "layered"). Its surface
        // is COMPOSED from child LAYER prims (`lunco:layer = "dem" | "craters" |
        // "rocks" | "shader" | …`) — add a layer by adding a prim. The `dem` (ground)
        // layer supplies the heightmap source + window; the rest stamp/scatter/shade.
        let asset_mode = reader.prim_attribute_value::<String>(&sdf, "lunco:assetMode");
        if !matches!(asset_mode.as_deref(), Some("dem") | Some("layered")) {
            continue;
        }

        // The ground (`dem`) layer + the composable stack (craters/rocks/shader/…),
        // parsed from the child layer prims (helpers shared with the live-edit refresh).
        let dem_layer_sdf = find_dem_layer(reader, &sdf);
        let stack = parse_terrain_layer_stack(reader, &sdf, &registry);
        // Seed the Inspector's shared spec from the authored values so the panel opens
        // showing THIS scene's craters/rocks, not the resource defaults. `bypass_change_
        // detection` so it doesn't look like a runtime edit (no networked re-broadcast).
        sync_obstacle_spec_from_usd(reader, &sdf, obstacle_spec.bypass_change_detection());

        // DEM/ground parameters: prefer a `dem` child layer prim (plain attr names);
        // fall back to the Terrain prim's own `lunco:terrain:*` attrs (back-compat).
        let dem = dem_layer_sdf.clone();
        let attr_f32 = |name: &str, legacy: &str| -> Option<f32> {
            dem.as_ref()
                .and_then(|d| reader.prim_attribute_value::<f32>(d, name))
                .or_else(|| reader.prim_attribute_value::<f32>(&sdf, legacy))
        };
        let attr_i32 = |name: &str, legacy: &str| -> Option<i32> {
            dem.as_ref()
                .and_then(|d| reader.prim_attribute_value::<i32>(d, name))
                .or_else(|| reader.prim_attribute_value::<i32>(&sdf, legacy))
        };
        let attr_bool = |name: &str, legacy: &str| -> Option<bool> {
            dem.as_ref()
                .and_then(|d| reader.prim_attribute_value::<bool>(d, name))
                .or_else(|| reader.prim_attribute_value::<bool>(&sdf, legacy))
        };

        let rel = dem
            .as_ref()
            .and_then(|d| reader.prim_attribute_value::<String>(d, "demSource"))
            .or_else(|| reader.prim_attribute_value::<String>(&sdf, "lunco:terrain:demSource"));
        let Some(rel) = rel else {
            warn!("[usd-dem] prim {} is a DEM terrain but has no dem-layer demSource", prim_path.path);
            continue;
        };
        let Some((_, root)) = twins.primary() else {
            warn!("[usd-dem] no open Twin to resolve DEM source '{rel}'");
            continue;
        };
        let uri = root.join(&rel).to_string_lossy().to_string();
        // `windowM` = side length (m) realized at native res. 0 = whole map; >0 = side;
        // absent/negative = a safe 4 km window (avoid an accidental full-map build).
        let half_window = match attr_f32("windowM", "lunco:terrain:windowM") {
            Some(w) if w == 0.0 => f64::INFINITY,
            Some(w) if w > 0.0 => (w * 0.5) as f64,
            _ => 2048.0,
        };
        // `targetRes` = visual-quality downsample target (samples/side). ≤ 0 = native.
        let target_res = attr_i32("targetRes", "lunco:terrain:targetRes")
            .filter(|&r| r > 0)
            .map(|r| r as usize)
            .unwrap_or(0);
        // `lodViz` = stream CDLOD tiles (default ON) vs one static mesh.
        let lod_viz = attr_bool("lodViz", "lunco:terrain:lodViz").unwrap_or(true);
        // `colliderRing` = stream a per-rover collider ring vs one static collider.
        let collider_ring = attr_bool("colliderRing", "lunco:terrain:colliderRing").unwrap_or(false);
        // `detailUpsample` = INTELLIGENT UPSCALING factor: bilinearly upscale the coarse
        // DEM ground before stamping craters, so generated craters get high fidelity
        // (sub-DEM-res rims) decoupled from the ~5 m source. ≤ 1 = native.
        let detail_upsample = attr_i32("detailUpsample", "lunco:terrain:detailUpsample")
            .filter(|&u| u > 1)
            .map(|u| u as usize)
            .unwrap_or(1);

        let layer_count = stack.0.len();
        commands.entity(entity).insert((
            lunco_terrain_surface::DemTerrainRequest {
                uri,
                half_window,
                target_res,
                lod_viz,
                collider_ring,
                with_default_material: false,
                detail_upsample,
            },
            stack,
            lunco_terrain_surface::DemTerrainSurface,
        ));
        // Georeference (#5): the `lunco:anchor:*` lat/lon/height anchor + the stage
        // `metersPerUnit`. The terrain math is metres, so a non-1 `metersPerUnit`
        // is recorded but flagged loudly (we don't rescale the DEM). Attach a
        // `TerrainGeoref` whenever any of these are authored.
        let anchor_lat = reader.prim_attribute_value::<f64>(&sdf, "lunco:anchor:lat");
        let anchor_lon = reader.prim_attribute_value::<f64>(&sdf, "lunco:anchor:lon");
        let anchor_height = reader.prim_attribute_value::<f64>(&sdf, "lunco:anchor:height");
        let meters_per_unit = reader.prim_attribute_value::<f64>(&sdf, "metersPerUnit");
        if let Some(mpu) = meters_per_unit {
            if (mpu - 1.0).abs() >= 1e-6 {
                warn!(
                    "[usd-dem] prim {} authors metersPerUnit={mpu}; terrain assumes 1 m/unit — \
                     heights/colliders are NOT rescaled",
                    prim_path.path
                );
            }
        }
        if anchor_lat.is_some() || anchor_lon.is_some() || anchor_height.is_some() {
            let georef = lunco_terrain_surface::TerrainGeoref {
                center_lat_deg: anchor_lat.unwrap_or(0.0),
                center_lon_deg: anchor_lon.unwrap_or(0.0),
                anchor_height_m: anchor_height.unwrap_or(0.0),
                meters_per_unit: meters_per_unit.unwrap_or(1.0),
            };
            commands.entity(entity).insert(georef);
            info!(
                "[usd-dem] georef: lat {:.4} lon {:.4} height {:.1} m (mpu {})",
                georef.center_lat_deg, georef.center_lon_deg, georef.anchor_height_m, georef.meters_per_unit
            );
        }
        info!(
            "[usd-dem] bridged layered terrain prim {} → DEM '{rel}' (target_res {target_res}, \
             lod_viz {lod_viz}, collider_ring {collider_ring}, detail_upsample {detail_upsample}, \
             {layer_count} composed layer(s))",
            prim_path.path
        );
    }
}

/// The headless runner: the Modelica/spawn cores a windowed build gets
/// transitively from its UI plugins, plus the `ScheduleRunnerPlugin` that ticks
/// the app in winit's place. Added only when running headless.
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
        app.add_plugins(lunco_sandbox_edit::commands::SpawnCommandPlugin);

        // No GPU renderer here, so the render-side systems that produce visual
        // components (`Mesh3d`, and the shader-pipeline `ShaderMaterial`) never
        // run. Tell the USD sim loader NOT to wait for them before building wheel
        // physics — otherwise raycast rovers defer their drivetrain forever and
        // the authoritative server can't simulate or replicate a drivable rover.
        app.insert_resource(lunco_usd::NoRenderVisuals);

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

    // --- Load scene from USD ---
    // Resolve the absolute path to find the enclosing Twin folder.
    let pb = std::path::PathBuf::from(&scene_path);
    let abs_path = if pb.is_absolute() {
        pb
    } else {
        std::env::current_dir().unwrap_or_default().join("assets").join(pb)
    };

    // Find the enclosing Twin root folder (walk up to find twin.toml or use the parent dir)
    let mut current = abs_path.parent().map(|p| p.to_path_buf());
    let mut twin_root = None;
    while let Some(dir) = current {
        if dir.join(lunco_twin::MANIFEST_FILENAME).is_file() {
            twin_root = Some(dir);
            break;
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }
    let twin_root = twin_root.unwrap_or_else(|| {
        abs_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    });

    let scene_file = abs_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    world.insert_resource(StartupSceneGuard { file: scene_file.clone() });

    let mut twin_loaded = false;
    if let Ok(twin_mode) = lunco_twin::TwinMode::open(&twin_root) {
        let mut twin = match twin_mode {
            lunco_twin::TwinMode::Twin(t) | lunco_twin::TwinMode::Folder(t) => t,
            lunco_twin::TwinMode::Orphan(_) => panic!("expected folder or twin"),
        };

        // Override or insert default_scene in the manifest
        let rel_scene_path = abs_path.strip_prefix(&twin_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(scene_file.clone());

        if let Some(manifest) = &mut twin.manifest {
            if let Some(usd) = &mut manifest.usd {
                usd.default_scene = Some(rel_scene_path);
            } else {
                manifest.usd = Some(lunco_twin::UsdManifest {
                    default_scene: Some(rel_scene_path),
                });
            }
        } else {
            // It was loaded as Folder, create a default manifest.
            // `uuid` is left `None`: this is an in-memory synthetic
            // manifest for a folder with no `twin.toml`. The networking
            // scenario-sync layer derives a stable id (path digest)
            // when `uuid` is absent, so the server's scenario stays
            // stable across restarts without writing a `twin.toml`.
            twin.manifest = Some(lunco_twin::TwinManifest {
                name: twin_root.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                description: None,
                version: "0.1.0".into(),
                uuid: None,
                default_perspective: None,
                children: Vec::new(),
                usd: Some(lunco_twin::UsdManifest {
                    default_scene: Some(rel_scene_path),
                }),
            });
        }

        let twin_id = world.resource_mut::<lunco_workspace::WorkspaceResource>().add_twin(twin);
        world.trigger(lunco_workspace::TwinAdded { twin: twin_id });
        twin_loaded = true;
    }

    if !twin_loaded {
        let load_path = {
            let pb = std::path::PathBuf::from(&scene_path);
            match (
                pb.is_absolute(),
                pb.parent(),
                pb.parent().and_then(|p| p.file_name()),
                pb.file_name(),
            ) {
                (true, Some(parent), Some(key), Some(file)) => {
                    let key = key.to_string_lossy().into_owned();
                    world
                        .resource::<lunco_assets::twin_source::TwinRoots>()
                        .register(&key, parent);
                    format!("twin://{}/{}", key, file.to_string_lossy())
                }
                _ => scene_path.clone(),
            }
        };
        info!("Failed to load Twin; falling back to direct load of sandbox scene `{}` via LoadScene", load_path);
        world.trigger(LoadScene {
            path: load_path,
            root_prim: String::new(),
        });
    }
    // NOTE: full USD doc-backing of the `--scene` (Step 0 / E1b `sync_twin_overlays`)
    // is deliberately NOT enabled here. It works, but its edit path RELOADS +
    // re-instantiates the WHOLE scene on every USD edit, which fights the terrain's
    // own lightweight incremental re-bake (`regenerate_dem_layers` on
    // `Changed<TerrainLayerStack>`) → two re-bake paths → a duplicated scene.
    // Live terrain tuning instead goes through the terrain's incremental path (the
    // Inspector → `TerrainLayerStack` edit → `regenerate_dem_layers`). A
    // lightweight doc-reparse (USD edit → re-parse just the layer stack, no scene
    // reload) is the future unification — see the design doc.
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

fn lander_rover_joint_detach_key(
    keys: Res<ButtonInput<KeyCode>>,
    q_names: Query<(Entity, &Name)>,
    mut commands: Commands,
) {
    // Tutorial-scene UX affordance: press G to detach the joint holding the
    // docked rover against the lander in `assets/scenes/sandbox/lander_test.usda`.
    // The keyboard input that DRIVES the lander (WASD + QE + Space) no longer
    // lives here — it flows through the typed-command path
    // (`lunco-controller::drive_from_bindings` → `lunco_cosim::SetPorts` →
    // `PortRegistry` writes to the `SimComponent` `manual_*` inputs), keyed off the
    // vessel's `SimComponent` topology (the possess-time `ControlBinding`) so any
    // lander in any scene is drivable without per-scene name matching.
    //
    // This G-to-detach shortcut is a different concern: it isn't a vessel-class
    // behaviour, it's a click-equivalent for ONE specific named joint in ONE
    // specific scene (the tutorial's `/LanderTest/LanderRoverJoint`). Tightly
    // bound to that scene's USD path, hence the literal name match below. The
    // principled generalization (find the joint connected to the currently
    // possessed vessel and dispatch `DetachJoint`) is a TODO for when the
    // scene authoring convention for dock joints stabilizes; for now this
    // preserves the existing one-key tutorial flow.
    if !keys.just_pressed(KeyCode::KeyG) { return; }
    for (entity, name) in &q_names {
        if name.as_str() == "/LanderTest/LanderRoverJoint" {
            info!("Manual input: Detaching LanderRoverJoint!");
            commands.trigger(lunco_sandbox_edit::commands::DetachJoint { target: entity });
            break;
        }
    }
}

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

