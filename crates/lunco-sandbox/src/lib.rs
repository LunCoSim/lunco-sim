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
// The USD-reading terrain systems read the LIVE canonical stage via `StageView`
// (which implements `UsdRead`).
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
use lunco_celestial::{CelestialConfig, SiteAnchor};
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
/// None`: the asset stores still initialise (so USD visual sync can populate the
/// meshes avian colliders read), but no GPU device is created and nothing is
/// drawn — `ScheduleRunnerPlugin` (added by [`SandboxHeadlessPlugin`]) ticks the
/// app in winit's place.
///
/// NB: with no backend, `RenderPlugin` does NOT build the render world — it skips
/// `ExtractPlugin`/`SyncWorldPlugin` entirely, while still installing the render-
/// sync component hooks that expect them. [`SandboxHeadlessPlugin`] adds
/// `SyncWorldPlugin` back to keep despawns from aborting; see the note there.
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
                // NB: the web render-scale cap lives in index.html (capping
                // `devicePixelRatio` before winit init). Bevy's
                // `WindowResolution::with_scale_factor_override` is IGNORED under
                // `fit_canvas_to_parent`, so it can't be done here.
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
    // Locally-registered Twins (native same-machine): lets a client that already
    // has the host's Twin load `twin://` host-identically instead of `scenario://`.
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
    // If this scenario's Twin is registered locally (native same-machine dev),
    // the client already booted on that Twin's default scene — the SAME asset
    // path the host loaded (`twin://<name>/<scene>`), so every prim already
    // shares the host's `GlobalEntityId` (identity = `hash(namespace:source:path)`,
    // `source` = asset path) and possession + client prediction bind across the
    // wire. Do NOT re-load it: a redundant teardown+reload of the live scene
    // races avian's island solver (`assert!(island.body_count > 0)` → client
    // panic). Mark the revision handled and keep the host-identical scene. A
    // client WITHOUT the Twin (web) has no `twin://` to load and falls through to
    // the `scenario://` cache copy — whose different `source` gives each prim a
    // per-peer gid (the identity-binding limitation tracked separately).
    if m.twin_scene.is_some() && twins.names().contains(&m.name) {
        info!(
            "[net] scenario twin '{}' is local — keeping the host-identical twin:// scene (no scenario:// swap)",
            m.name
        );
        *last_loaded = Some(m.revision);
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
/// it: prims re-added, rovers churned, and `reap_orphaned_wheel_joints` despawned
/// a wheel joint whose bodies were already gone, tripping avian's
/// `assert!(island.joint_count > 0)` about a second after boot.
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
    mut registry: ResMut<lunco_usd::UsdDocumentRegistry>,
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
    registry: Option<ResMut<lunco_sandbox_edit::shader_doc::ShaderRegistry>>,
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
            serde_json::from_value::<lunco_sandbox_edit::shader_doc::ShaderOp>(op)
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
const LUNCO_POLICY_TYPE: &str = "LuncoPolicy";

/// Read every composed `LuncoPolicy` prim across all live stages into the desired
/// policy set — the "policy is a projected USD prim" extractor. Reads the
/// **composed** stage, so an opinion authored at any layer (global/twin/scene)
/// resolves to one effective policy per seam. A prim missing `seam` or `source` is
/// skipped (incompletely authored). Pure over the stages, so it's unit-testable
/// without a running app.
#[cfg(feature = "networking")]
fn extract_usd_policies(
    canonical: &lunco_usd_bevy::CanonicalStages,
) -> Vec<lunco_networking::scripted_policy::PolicyDef> {
    let mut out = Vec::new();
    for (_, cs) in canonical.iter() {
        let view = cs.view();
        for prim in view.prim_paths() {
            if view.prim_type_name(&prim).as_deref() != Some(LUNCO_POLICY_TYPE) {
                continue;
            }
            let seam = view.value::<String>(&prim, "lunco:policy:seam").unwrap_or_default();
            let source = view.value::<String>(&prim, "lunco:policy:source").unwrap_or_default();
            if seam.is_empty() || source.is_empty() {
                continue;
            }
            out.push(lunco_networking::scripted_policy::PolicyDef {
                seam,
                entry: view.value::<String>(&prim, "lunco:policy:entry").unwrap_or_default(),
                source,
                deterministic: view
                    .value::<bool>(&prim, "lunco:policy:deterministic")
                    .unwrap_or(true),
            });
        }
    }
    out
}

/// **Policy projection** — activation half of "policy is a USD prim". On any
/// composed-stage change, read the `LuncoPolicy` prims and project them into the
/// live hook registry via
/// [`project_policies`](lunco_networking::scripted_policy::project_policies): a new
/// prim registers its rhai hook (and, at [`MERGE_SEAM`](lunco_networking::scripted_policy::MERGE_SEAM),
/// flips the journal merge strategy); a removed prim retracts it. Because a policy
/// prim rides the USD doc-op journal, cross-peer propagation is (journal sync →
/// each peer recomposes → each peer's projector re-registers) — no bespoke policy
/// broadcast. Change-gated on total stage generation + stage count so it runs only
/// when the composed stage actually moved.
#[cfg(feature = "networking")]
fn project_usd_policies(
    canonical: NonSend<lunco_usd_bevy::CanonicalStages>,
    mut registry: ResMut<lunco_networking::scripted_policy::ScriptedPolicyRegistry>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    mut last: Local<Option<(usize, u64)>>,
) {
    let signal = (canonical.len(), canonical.iter().map(|(_, cs)| cs.generation()).sum());
    if *last == Some(signal) {
        return;
    }
    *last = Some(signal);
    let desired = extract_usd_policies(&canonical);
    lunco_networking::scripted_policy::project_policies(desired, &mut registry, journal.as_deref());
}

/// **Environment-settings projection** — the read half of persisting
/// `SetEnvironmentLight` render knobs (exposure / bloom / ambient / earthshine)
/// onto the `LuncoEnvironment` settings prim (see
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
    ambient: Option<ResMut<bevy::light::GlobalAmbientLight>>,
    mut q_earthshine: Query<&mut DirectionalLight, With<lunco_environment::Earthshine>>,
    mut last: Local<Option<(usize, u64)>>,
) {
    let signal = (canonical.len(), canonical.iter().map(|(_, cs)| cs.generation()).sum());
    if *last == Some(signal) {
        return;
    }
    *last = Some(signal);

    let mut ambient = ambient;
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
            if let Some(br) = view.value::<f32>(&prim, "lunco:env:ambientBrightness") {
                if let Some(a) = ambient.as_mut() {
                    a.brightness = br;
                }
            }
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

/// Convenience command: author (or hot-replace) a rhai policy as a `LuncoPolicy`
/// USD prim under `/World/Policies/<name>` in ONE call, instead of hand-issuing the
/// underlying `ApplyUsdOp`s. Because it authors USD doc ops, the policy **journals →
/// syncs to every peer → the projector activates it** (registers the rhai hook; at
/// `MERGE_SEAM` flips the merge strategy). Re-issuing with the same `name` (or later
/// editing `lunco:policy:source`) **hot-replaces the hook live** — dynamic rhai
/// editing with no file system, converging across the network.
///
/// This is the ergonomic surface over the canonical form (a `LuncoPolicy` prim); the
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
    registry: Res<lunco_usd::UsdDocumentRegistry>,
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
            type_name: Some("LuncoPolicy".into()),
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
            name: "lunco:policy:source".into(),
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

/// Save a live-edited rhai scenario's current source back onto its USD prim's
/// `lunco:script` attribute — the missing half of scenario authoring.
///
/// The LOAD path reads `lunco:script` off a prim into a running scenario; until
/// now a hot-edited scenario had no way *back* to the document. This resolves the
/// scripted entity's live source (from [`ScriptRegistry`](lunco_scripting::ScriptRegistry)),
/// its prim path, and the editable scene document backing it, then authors the
/// source onto `lunco:script` via [`SetAttribute`](lunco_usd::UsdOp::SetAttribute)
/// (whose `string` type authors the value RAW — no hand-escaping) — which journals,
/// and on `SaveDocument` writes through to the `.usda`.
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

    // 3. Author the source onto `lunco:script` (root layer → durable in the .usda
    //    on SaveDocument). A `string` value is authored RAW — `SetAttribute` handles
    //    the escaping (writer-side), so the whole rhai source round-trips verbatim
    //    with no hand-escaping here. Through `ApplyUsdOp` so it journals like any edit.
    commands.trigger(lunco_usd::ApplyUsdOp {
        doc: scene_doc,
        op: lunco_usd::UsdOp::SetAttribute {
            edit_target: lunco_usd::LayerId::root(),
            path: upp.path.clone(),
            name: "lunco:script".into(),
            type_name: "string".into(),
            value: source,
        },
    });
    info!(
        "[save-scenario] {target}: scenario source written onto `{}` (doc {}) — journals; SaveDocument persists to disk",
        upp.path, scene_doc.0
    );
}

lunco_core::register_commands!(on_set_rhai_policy, on_save_scenario);

#[cfg(all(test, feature = "networking", not(target_arch = "wasm32")))]
mod policy_projection_tests {
    use super::extract_usd_policies;
    use lunco_usd_bevy::{CanonicalStage, CanonicalStages, StageRecipe};

    /// A `LuncoPolicy` prim authored in the scene USD is read into a `PolicyDef` —
    /// the "settable in USD" half of proper policies. The projector then hands this
    /// to `project_policies`, so a scene-authored (or journal-synced) policy
    /// activates its rhai hook with no bespoke broadcast.
    #[test]
    fn extracts_lunco_policy_prims_from_composed_stage() {
        const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n\
            \x20   def LuncoPolicy \"takeover\"\n    {\n\
            \x20       string lunco:policy:seam = \"control.authority.take\"\n\
            \x20       string lunco:policy:entry = \"may_take_control\"\n\
            \x20       string lunco:policy:source = \"fn may_take_control(ctx){true}\"\n\
            \x20       bool lunco:policy:deterministic = false\n    }\n}\n";

        let mut stages = CanonicalStages::default();
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("build stage");
        stages.insert(bevy::asset::AssetId::invalid(), cs);

        let policies = extract_usd_policies(&stages);
        assert_eq!(policies.len(), 1, "one LuncoPolicy prim → one PolicyDef");
        let p = &policies[0];
        assert_eq!(p.seam, "control.authority.take");
        assert_eq!(p.entry, "may_take_control");
        assert!(p.source.contains("may_take_control"), "source carried verbatim");
        assert!(!p.deterministic, "authored deterministic=false is read");
    }

    /// **Live rhai editing (no file system).** Editing a `LuncoPolicy`'s `source`
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
            \x20   def LuncoPolicy \"drive\"\n    {\n\
            \x20       string lunco:policy:seam = \"rover.drive\"\n\
            \x20       string lunco:policy:entry = \"drive\"\n\
            \x20       string lunco:policy:source = \"fn drive(c){1}\"\n\
            \x20       bool lunco:policy:deterministic = true\n    }\n}\n";

        let id = bevy::asset::AssetId::invalid();
        let mut stages = CanonicalStages::default();
        stages.insert(
            id,
            CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
                .expect("build stage"),
        );
        assert_eq!(extract_usd_policies(&stages)[0].source, "fn drive(c){1}");

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
            .author_attribute(&prim, "lunco:policy:source", "string", new_src)
            .expect("author live edit");

        assert_eq!(
            extract_usd_policies(&stages)[0].source,
            "fn drive(c){2}",
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

        // Convenience command: `SetRhaiPolicy` authors a `LuncoPolicy` prim as USD
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
            // Celestial stack (doc 43): dormant by default — the solar
            // hierarchy spawns only when a loaded scene authors a site anchor
            // (`lunco:anchor:*` on its root prim, e.g. the moonbase Twin);
            // the sandbox avatar keeps the FloatingOrigin either way. Comms
            // connectivity (antenna sight-lines → `comms:*` ports) is always
            // on; it needs no hierarchy.
            .insert_resource(lunco_celestial::CelestialConfig {
                spawn_hierarchy: false,
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
            .add_systems(Update, enable_celestial_on_site_anchor)
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
            // Policy projection: activate `LuncoPolicy` prims from the composed
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

        // USD→DEM bridge: an authored terrain prim with `lunco:assetMode="dem"`
        // gets a DEM heightfield built onto it; its `materialType` authors the
        // material via the universal ShaderMaterial path. Core (not GUI-gated):
        // the headless server needs the collider for deterministic physics.
        app.add_systems(
            Update,
            (
                bridge_usd_dem_terrain,
                refresh_layered_terrain_layers,
                cache_terrain_document,
                refresh_docbacked_terrain_from_doc,
                track_ground_collider_pending,
            ),
        );
        // Authoring tier: doc-backed terrains route live edits to their USD document's
        // runtime layer (journaled, non-destructive) instead of mutating the runtime
        // layer stack directly. Document-free terrains are handled in lunco-terrain-surface.
        app.init_resource::<TerrainEditPrimSeq>()
            .add_observer(on_brush_terrain_authored)
            .add_observer(on_flatten_terrain_authored)
            .add_observer(on_place_crater_authored)
            .add_observer(on_place_rock_authored)
            .add_observer(on_remove_terrain_edit_authored)
            // Doc-backed crater/rock tuning authors to USD (→ project → regen), instead
            // of the direct stack-mutation path (which handles document-free terrains).
            .add_observer(on_obstacle_spec_authored);
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
        // Environment-settings projection: apply a persisted `LuncoEnvironment`
        // prim's render knobs (exposure/bloom/ambient/earthshine) to the live
        // render state on stage change. UI-gated (render/camera state); core
        // persistence (authoring the prim) happens in `lunco-sandbox-edit`.
        #[cfg(feature = "ui")]
        app.add_systems(Update, project_env_settings);

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
    unexamined: Query<(), (With<lunco_usd::UsdPrimPath>, Without<DemBridged>)>,
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
/// Read the authored `(role, rel-path, weight)` triples off `sdf` via any
/// [`UsdRead`] source — the live `StageView` or the flattened reader — so the
/// binding is identical whichever plane supplied the stage.
#[cfg(feature = "ui")]
fn read_authored_layer_maps<R: UsdRead>(
    reader: &R,
    sdf: &openusd::sdf::Path,
    roles: &'static [LayerRole],
) -> Vec<(&'static LayerRole, String, f32)> {
    roles
        .iter()
        .filter_map(|role| {
            let map_attr = format!("lunco:terrain:layer:{}:map", role.name);
            let rel = reader.scalar::<String>(sdf, &map_attr)?;
            let weight = reader
                .real_f32(sdf, &format!("lunco:terrain:layer:{}:weight", role.name))
                .unwrap_or(1.0);
            Some((role, rel, weight))
        })
        .collect()
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
    const SOURCE: &str = "terrain";
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
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
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
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else {
            commands.entity(entity).try_insert(TerrainLayersBound);
            continue;
        };

        // Read the LIVE canonical stage (built on demand from the asset's recipe)
        // — the source of truth — through the `UsdRead` read body
        // (`read_authored_layer_maps`).
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
        let authored: Vec<(&LayerRole, String, f32)> =
            read_authored_layer_maps(&cs.view(), &sdf, ROLES);

        if authored.is_empty() {
            // No layer authored — stop re-scanning this terrain.
            commands.entity(entity).try_insert(TerrainLayersBound);
            continue;
        }
        // Resolve against whatever the SCENE came from. A `scenario://` scene (a
        // twin downloaded from a host) carries layer paths relative to itself, and
        // the client normally has the local demo twin open — binding those layers
        // under `twin://sandbox/…` silently loads the wrong (missing) textures.
        // `AssetPath::path()` strips the source, so the parent dir is the bare
        // `<scenario-id>`. Otherwise resolve relative to the open Twin.
        let asset_path = asset_server.get_path(id);
        let scenario_root = asset_path.as_ref().and_then(|p| {
            if !matches!(
                p.source(),
                bevy::asset::io::AssetSourceId::Name(n)
                    if &**n == lunco_assets::scenario_source::SCENARIO_SCHEME
            ) {
                return None;
            }
            p.path().parent().and_then(|d| d.to_str()).map(str::to_owned)
        });
        let base_uri = if let Some(root) = &scenario_root {
            format!("scenario://{root}")
        } else {
            let Some((twin_name, _)) = twins.primary() else { continue };
            format!("twin://{twin_name}")
        };
        // Wait for the material to exist before binding (created async by the USD
        // shader system); retry next frame until it does.
        let Some(mut material) = mats.get_mut(&mat3d.0) else { continue };

        for (role, rel, weight) in authored {
            let uri = format!("{base_uri}/{rel}");
            (role.set_slot)(&mut material, asset_server.load(&uri));
            for w in role.weights {
                material.set(w, ParamValue::F32(weight));
            }
            info!("[usd-dem] bound terrain {} layer '{rel}' (weight {weight}) → {uri}", role.name);
        }
        commands.entity(entity).try_insert(TerrainLayersBound);
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
struct UsdLayerAttrs<'a, R: UsdRead> {
    reader: &'a R,
    sdf: openusd::sdf::Path,
}

impl<R: UsdRead> lunco_terrain_surface::LayerAttrSource for UsdLayerAttrs<'_, R> {
    fn get_f32(&self, name: &str) -> Option<f32> {
        self.reader.real_f32(&self.sdf, name)
    }
    fn get_i64(&self, name: &str) -> Option<i64> {
        // `TryFrom<Value>` is strict per variant, so probe both authored widths:
        // `int64` (the Inspector authors seeds full-range) and hand-authored `int`.
        self.reader
            .scalar::<i64>(&self.sdf, name)
            .or_else(|| self.reader.scalar::<i32>(&self.sdf, name).map(|v| v as i64))
    }
    fn get_string(&self, name: &str) -> Option<String> {
        self.reader.scalar::<String>(&self.sdf, name)
    }
    fn get_bool(&self, name: &str) -> Option<bool> {
        self.reader.scalar::<bool>(&self.sdf, name)
    }
}

/// The `dem` (ground) child layer prim of a layered terrain, if authored.
fn find_dem_layer<R: UsdRead>(
    reader: &R,
    terrain: &openusd::sdf::Path,
) -> Option<openusd::sdf::Path> {
    reader
        .children(terrain)
        .into_iter()
        .find(|c| reader.scalar::<String>(c, "lunco:layer").as_deref() == Some("dem"))
}

/// Parse the non-ground child layer prims (`craters`/`rocks`/`shader`/…) into the
/// composable [`TerrainLayerStack`](lunco_terrain_surface::TerrainLayerStack) via the
/// registry. Shared by the bridge (initial build) and the live-edit refresh.
fn parse_terrain_layer_stack<R: UsdRead>(
    reader: &R,
    terrain: &openusd::sdf::Path,
    registry: &lunco_terrain_surface::TerrainLayerParserRegistry,
) -> lunco_terrain_surface::TerrainLayerStack {
    let mut stack = lunco_terrain_surface::TerrainLayerStack::default();
    // Runtime edit prims (`lunco:layer = "edit"`) — one prim per edit — aggregate into
    // the single `EditsLayer` (the runtime projection tier), folded on top at the end.
    let mut edits: Vec<(lunco_terrain_surface::LayerId, lunco_terrain_surface::EditKind)> = Vec::new();
    // Whether the scene authors its OWN overzoom prim (even a zeroed/disabled one
    // counts — that's an explicit opt-out of the default sub-DEM detail).
    let mut authored_overzoom = false;
    // CANONICAL child order. `children()` iterates a hash map, so its order varies
    // per process AND per parse (bridge vs composed re-parse). The stack's fold
    // order feeds `SurfaceOracle::content_key` — unsorted, every launch minted a
    // fresh surface key for identical content, invalidating the entire tile/derived
    // map cache (cold-bake storm on every boot) and reordering non-commutative
    // edits. Sorting by path makes stack order — and thus the key and the composed
    // surface — a pure function of the document.
    let mut children: Vec<_> = reader.children(terrain).into_iter().collect();
    children.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for child in children {
        let attrs = UsdLayerAttrs { reader, sdf: child.clone() };
        // An edit prim (carries the packed `lunco:edit`)? Aggregate into the single
        // edits layer, keyed by its prim path (its stable identity).
        if let Some(edit) =
            lunco_terrain_surface::parse_edit(lunco_terrain_surface::LayerId::new(child.as_str()), &attrs)
        {
            edits.push(edit);
            continue;
        }
        // Otherwise a normal composable layer prim (`lunco:layer = …`).
        let Some(layer_type) = reader.scalar::<String>(&child, "lunco:layer") else {
            continue;
        };
        if layer_type == "dem" {
            continue;
        }
        if layer_type == "overzoom" {
            authored_overzoom = true;
        }
        if !registry.knows(&layer_type) {
            warn!("[usd-dem] child layer '{layer_type}' has no registered terrain layer parser");
            continue;
        }
        if let Some(layer) = registry.parse(&layer_type, &attrs) {
            // Identity = the layer prim's path: unique, stable, already in hand. Lets
            // several same-kind layers coexist and be addressed individually.
            stack.push_layer(child.as_str(), layer);
        }
    }
    // Sub-DEM detail defaults ON: without it the ground between the finest shader
    // grain (~12 cm) and the DEM data resolution (~5 m) is empty in every channel
    // and reads as flat plastic one step from the camera. Authoring an `overzoom`
    // prim — including a zeroed one — takes over from the default.
    if !authored_overzoom {
        stack.push_layer("overzoom/default", lunco_terrain_surface::default_overzoom_layer());
    }
    if !edits.is_empty() {
        stack.push_layer(
            lunco_terrain_surface::EDITS_LAYER_ID,
            std::sync::Arc::new(lunco_terrain_surface::EditsLayer::from_edits(edits)),
        );
    }
    stack
}

/// Seed the shared [`ObstacleFieldSpec`] from the USD-authored `craters`/`rocks` child
/// layer prims so the Inspector's "Craters & Rocks" panel opens showing the scene's
/// ACTUAL values (density, size, ratios) instead of the resource defaults. Mirrors the
/// `SizeDist` the layer parsers build — `sizeMin`/`sizeMax` attrs with the parsers'
/// defaults (`craters` → 2/60, `rocks` → 0.2/(mode*4).max(2.5)) and the same
/// min ≤ mode ≤ max clamp — so a subsequent panel edit starts from the authored
/// look rather than jumping. Writes the resource only (no `UpdateObstacleFieldSpec`,
/// no re-stamp — the terrain already built from the same USD stack).
fn sync_obstacle_spec_from_usd<R: UsdRead>(
    reader: &R,
    terrain: &openusd::sdf::Path,
    spec: &mut lunco_obstacle_field::spec::ObstacleFieldSpec,
) {
    use lunco_obstacle_field::spec::SizeDist;
    for child in reader.children(terrain) {
        match reader.scalar::<String>(&child, "lunco:layer").as_deref() {
            Some("craters") => {
                let density = reader.real_f32(&child, "density").unwrap_or(0.0);
                let mode = reader.real_f32(&child, "sizeMode").unwrap_or(22.0);
                spec.craters.enabled = density > 0.0;
                spec.craters.density = density;
                spec.craters.depth_ratio = reader.real_f32(&child, "depthRatio").unwrap_or(0.4);
                spec.craters.rim_height_ratio =
                    reader.real_f32(&child, "rimRatio").unwrap_or(0.18);
                let size_min = reader.real_f32(&child, "sizeMin").unwrap_or(2.0);
                let size_max = reader.real_f32(&child, "sizeMax").unwrap_or(60.0);
                spec.craters.size =
                    SizeDist::new(size_min.min(mode), mode, size_max.max(mode), 0.7);
                if let Some(seed) = reader
                    .scalar::<i64>(&child, "seed")
                    .or_else(|| reader.scalar::<i32>(&child, "seed").map(|v| v as i64))
                {
                    spec.seed = seed as u64;
                }
            }
            Some("rocks") => {
                let density = reader.real_f32(&child, "density").unwrap_or(0.0);
                let mode = reader.real_f32(&child, "sizeMode").unwrap_or(0.6);
                spec.rocks.enabled = density > 0.0;
                spec.rocks.density = density;
                let size_min = reader.real_f32(&child, "sizeMin").unwrap_or(0.2);
                let size_max = reader
                    .real_f32(&child, "sizeMax")
                    .unwrap_or((mode * 4.0).max(2.5));
                spec.rocks.size =
                    SizeDist::new(size_min.min(mode), mode, size_max.max(mode), 0.6);
                spec.rocks.dynamic_fraction =
                    reader.real_f32(&child, "dynamicFrac").unwrap_or(0.0);
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
///
/// **Document-free terrains only** (`Without<DocBackedTerrain>`). A doc-backed
/// terrain re-bakes from its registry document instead
/// ([`refresh_docbacked_terrain_from_doc`]) — the source of truth — so it doesn't
/// depend on the twin stage asset being reloaded (its `LiveRebuildExempt` marker
/// deliberately suppresses that reload). Routing exactly one path per terrain
/// avoids a double re-parse.
fn refresh_layered_terrain_layers(
    mut ev: MessageReader<AssetEvent<lunco_usd::UsdStageAsset>>,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    registry: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    q: Query<
        (Entity, &lunco_usd::UsdPrimPath),
        (
            With<lunco_terrain_surface::DemTerrainSurface>,
            Without<lunco_terrain_surface::DocBackedTerrain>,
        ),
    >,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
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
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        // Read the LIVE canonical stage (reflects the in-place edit that raised
        // this Modified event).
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else {
            // No live stage (asset carries no recipe / build failed) — skip.
            continue;
        };
        let stack = parse_terrain_layer_stack(&cs.view(), &sdf, &registry);
        // Despawn-safe: a scene reload can despawn this terrain between queue
        // time and apply_deferred — no-op instead of panicking.
        commands.entity(entity).try_insert(stack);
    }
}

/// Caches the backing USD **document** on a doc-projected DEM terrain: the raw
/// `DocumentId` handle of the live scene the terrain belongs to, plus the
/// [`DocBackedTerrain`](lunco_terrain_surface::DocBackedTerrain) marker. Its presence
/// is the switch that routes live edits to the **authoring tier** (author a USD op →
/// journal → project). Its *absence* means a document-free terrain (quick
/// `SpawnDemTerrain`, headless, tests — those carry no `UsdPrimPath`, so they never
/// match here), whose edits apply **directly** to the runtime layer.
///
/// Resolution is uniform: every doc-backed scene — twin default (`--scene` / workspace
/// Twin) and live-imported (`OpenFile`) alike — is a doc-backed twin scene, so the doc
/// is recovered from
/// [`DocBackedTwinScenes`](lunco_usd::twin_projection::DocBackedTwinScenes) via the
/// stage's `twin://<name>/<rel>` asset path. Retries each frame (guarded by
/// `Without<TerrainDocument>`) until the doc mounts; once resolved, it stops.
#[derive(Component)]
struct TerrainDocument {
    /// Raw `DocumentId` of the backing doc (rebuilt as `DocumentId` at the authoring
    /// boundary). The document is the edit authority; edits author there and project in.
    doc: u64,
}

/// Monotonic suffix for authored edit prim names (`edit_<n>` / `rock_<n>`), unique per
/// session so a removed edit's name is never reused. Starts at 0 but is re-seeded past
/// any existing children at every authoring site ([`seed_edit_seq_past_children`]) — a
/// runtime overlay restored from `.lunco/runtime/…` carries last session's prims, and
/// reusing a taken name would make the `AddPrim` fail (the edit silently dropped).
#[derive(Resource, Default)]
struct TerrainEditPrimSeq(u64);

/// Advance `seq` past every `edit_<n>` / `rock_<n>` child already present under
/// `terrain_path` in the composed (`base ⊕ runtime`) document, so the next authored
/// name can never collide with a restored or historical prim. Runs at authoring time
/// (not doc-mount time) so it cannot race the `DocumentOpened` runtime-overlay
/// restore; `composed_arc` is memoized by generation, so this is a cheap child walk.
fn seed_edit_seq_past_children(
    registry: &lunco_usd::UsdDocumentRegistry,
    doc: lunco_doc::DocumentId,
    terrain_path: &str,
    seq: &mut TerrainEditPrimSeq,
) {
    let Some(host) = registry.host(doc) else { return };
    let Ok(sdf) = openusd::sdf::Path::new(terrain_path) else { return };
    let composed = host.document().composed_arc();
    for child in composed.children(&sdf) {
        let Some(name) = child.as_str().rsplit('/').next() else { continue };
        for prefix in ["edit_", "rock_"] {
            if let Some(n) = name.strip_prefix(prefix).and_then(|s| s.parse::<u64>().ok()) {
                seq.0 = seq.0.max(n + 1);
            }
        }
    }
}

/// Author one edit onto every **doc-backed** terrain as USD ops on its document's
/// **runtime** layer — non-destructive, ephemeral over the base DEM (Omniverse
/// session-layer pattern): an `AddPrim` for the edit prim + a `SetAttribute` with the
/// packed edit. `registry.apply` records both to the journal (undo / sync), then
/// the twin projection re-projects the composed `base ⊕ runtime` → `parse_edit` → the one
/// `EditsLayer`. The direct-path observer in lunco-terrain-surface handles document-FREE
/// terrains (`Without<DocBackedTerrain>`), so exactly one path fires per terrain.
fn author_terrain_edit(
    kind: lunco_terrain_surface::EditKind,
    terrains: &Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: &mut lunco_usd::UsdDocumentRegistry,
    seq: &mut TerrainEditPrimSeq,
) {
    for (prim_path, td) in terrains {
        let doc = lunco_doc::DocumentId::new(td.doc);
        seed_edit_seq_past_children(registry, doc, &prim_path.path, seq);
        let name = format!("edit_{}", seq.0);
        seq.0 += 1;
        let edit_prim = format!("{}/{name}", prim_path.path.trim_end_matches('/'));
        // 1. The edit prim, on the ephemeral runtime layer (non-destructive).
        let added = registry.apply(
            doc,
            lunco_usd::UsdOp::AddPrim {
                edit_target: lunco_usd::LayerId::runtime(),
                parent_path: prim_path.path.clone(),
                name,
                type_name: None,
                reference: None,
            },
        );
        if let Err(e) = added {
            warn!("[terrain-edit] AddPrim {edit_prim} failed — edit dropped: {e:?}");
            continue;
        }
        // 2. The packed edit parameters (shared schema with `parse_edit`).
        let (attr, ty, value) = lunco_terrain_surface::edit_attr_write(&kind);
        let _ = registry.apply(
            doc,
            lunco_usd::UsdOp::SetAttribute {
                edit_target: lunco_usd::LayerId::runtime(),
                path: edit_prim,
                name: attr.to_string(),
                type_name: ty.to_string(),
                value,
            },
        );
    }
}

fn on_brush_terrain_authored(
    trigger: On<lunco_terrain_surface::BrushTerrain>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_usd::UsdDocumentRegistry>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let Some(mut registry) = registry else { return };
    author_terrain_edit(
        lunco_terrain_surface::EditKind::Brush {
            center: [ev.x as f64, ev.z as f64],
            radius: ev.radius as f64,
            amplitude: ev.amplitude as f64,
        },
        &terrains,
        &mut registry,
        &mut seq,
    );
}

fn on_flatten_terrain_authored(
    trigger: On<lunco_terrain_surface::FlattenTerrain>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_usd::UsdDocumentRegistry>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let Some(mut registry) = registry else { return };
    author_terrain_edit(
        lunco_terrain_surface::EditKind::Flatten {
            center: [ev.x as f64, ev.z as f64],
            radius: ev.radius as f64,
            target_y: ev.target_y as f64,
        },
        &terrains,
        &mut registry,
        &mut seq,
    );
}

fn on_place_crater_authored(
    trigger: On<lunco_terrain_surface::PlaceCrater>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_usd::UsdDocumentRegistry>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let Some(mut registry) = registry else { return };
    author_terrain_edit(
        lunco_terrain_surface::EditKind::Crater {
            center: [ev.x as f64, ev.z as f64],
            radius: ev.radius as f64,
            depth: ev.depth_or_default(),
        },
        &terrains,
        &mut registry,
        &mut seq,
    );
}

/// Doc-backed manual rock placement: author ONE `lunco:layer = "rock"` child prim
/// (x/z/size/seed attrs) on the runtime layer. The stack re-parse picks it up via
/// the `rock` parser — a single addressable boulder, removable by its prim path.
fn on_place_rock_authored(
    trigger: On<lunco_terrain_surface::PlaceRock>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_usd::UsdDocumentRegistry>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
) {
    let ev = trigger.event();
    let Some(mut registry) = registry else { return };
    for (prim_path, td) in &terrains {
        let doc = lunco_doc::DocumentId::new(td.doc);
        seed_edit_seq_past_children(&registry, doc, &prim_path.path, &mut seq);
        let name = format!("rock_{}", seq.0);
        seq.0 += 1;
        let rock_prim = format!("{}/{name}", prim_path.path.trim_end_matches('/'));
        if let Err(e) = registry.apply(
            doc,
            lunco_usd::UsdOp::AddPrim {
                edit_target: lunco_usd::LayerId::runtime(),
                parent_path: prim_path.path.clone(),
                name,
                type_name: None,
                reference: None,
            },
        ) {
            warn!("[terrain-edit] AddPrim {rock_prim} failed — rock dropped: {e:?}");
            continue;
        }
        let attrs: [(&str, &str, String); 5] = [
            // RAW content — `SetAttribute` authors `string` values verbatim
            // (no literal parsing); hand-quoting embeds the quotes.
            ("lunco:layer", "string", "rock".to_string()),
            ("x", "float", format!("{}", ev.x)),
            ("z", "float", format!("{}", ev.z)),
            ("size", "float", format!("{}", ev.size_or_default())),
            ("seed", "int64", format!("{}", ev.seed_or_default() as i64)),
        ];
        for (attr, ty, value) in attrs {
            let _ = registry.apply(
                doc,
                lunco_usd::UsdOp::SetAttribute {
                    edit_target: lunco_usd::LayerId::runtime(),
                    path: rock_prim.clone(),
                    name: attr.to_string(),
                    type_name: ty.to_string(),
                    value,
                },
            );
        }
    }
}

/// Remove a doc-backed terrain edit by authoring a `RemovePrim` of its edit prim — the
/// removal `id` IS the prim path. Document-free removal is handled directly in
/// lunco-terrain-surface. Applies to the doc that owns the prim; others reject harmlessly.
fn on_remove_terrain_edit_authored(
    trigger: On<lunco_terrain_surface::RemoveTerrainLayer>,
    terrains: Query<&TerrainDocument, With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_usd::UsdDocumentRegistry>>,
) {
    let Some(mut registry) = registry else { return };
    let path = trigger.event().id.clone();
    for td in &terrains {
        let _ = registry.apply(
            lunco_doc::DocumentId::new(td.doc),
            lunco_usd::UsdOp::RemovePrim { edit_target: lunco_usd::LayerId::runtime(), path: path.clone() },
        );
    }
}

fn cache_terrain_document(
    terrains: Query<
        (Entity, &lunco_usd::UsdPrimPath),
        (With<lunco_terrain_surface::DemTerrainSurface>, Without<TerrainDocument>),
    >,
    twin_scenes: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (entity, terrain_path) in &terrains {
        // Recover the backing document from `DocBackedTwinScenes` via the stage's
        // `twin://<name>/<rel>` asset path. Both twin default scenes (`--scene` /
        // workspace Twin) and live-imported (`OpenFile`) scenes are doc-backed twin
        // scenes now, so this one path covers both.
        let doc = asset_server.get_path(terrain_path.stage_handle.id()).and_then(|asset_path| {
            let rel_path = asset_path.path().to_string_lossy();
            let (name, rel) = rel_path.split_once('/')?;
            twin_scenes.doc_for(name, rel)
        });
        let Some(doc) = doc else {
            continue; // not mounted yet (retry next frame), or document-free.
        };
        info!("[terrain-doc] terrain {entity} → doc {} (DocBackedTerrain attached)", doc.0);
        // `LiveRebuildExempt`: an authored crater/rock/edit is an attribute-only doc
        // change; without this the twin projection would despawn + re-instantiate the
        // terrain (a full DEM re-read) per edit. The exempt marker suppresses that
        // reload; `refresh_docbacked_terrain_from_doc` re-bakes off the registry doc.
        commands.entity(entity).try_insert((
            TerrainDocument { doc: doc.0 },
            lunco_terrain_surface::DocBackedTerrain,
            lunco_usd::twin_projection::LiveRebuildExempt,
        ));
    }
}

/// Last registry-document generation a doc-backed terrain re-baked at, so
/// [`refresh_docbacked_terrain_from_doc`] re-parses only when the document moved.
#[derive(Component)]
struct TerrainDocGeneration(u64);

/// Re-bake a doc-backed DEM terrain from its backing registry document whenever
/// that document's generation advances (an authored crater/rock/edit op). Reads
/// the composed (`base ⊕ runtime`) layer straight from the registry — the source
/// of truth — and re-parses the composable `TerrainLayerStack` in place;
/// `regenerate_dem_layers` then re-stamps off the retained base grid (no GeoTIFF
/// re-read, no entity despawn).
///
/// This is the twin-scene counterpart to the asset-event
/// [`refresh_layered_terrain_layers`] (now document-free only): a doc-backed terrain's
/// `LiveRebuildExempt` marker suppresses the twin stage reload, so the registry
/// generation is the re-bake trigger. One re-bake path keyed on the document, not the
/// projected asset — covering twin default and live-imported (`OpenFile`) scenes alike.
fn refresh_docbacked_terrain_from_doc(
    registry: Option<Res<lunco_usd::UsdDocumentRegistry>>,
    parser: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    mut terrains: Query<
        (
            Entity,
            &lunco_usd::UsdPrimPath,
            &TerrainDocument,
            Option<&mut TerrainDocGeneration>,
            Has<lunco_terrain_surface::DemBaseGrid>,
        ),
        With<lunco_terrain_surface::DemTerrainSurface>,
    >,
    mut commands: Commands,
) {
    // Brings the `Document::generation` trait method into scope (method
    // resolution only — the name isn't bound, so it can't clash).
    use lunco_doc::Document as _;
    let Some(registry) = registry else { return };
    for (entity, prim_path, td, tracker, has_base_grid) in &mut terrains {
        let doc = lunco_doc::DocumentId::new(td.doc);
        let Some(host) = registry.host(doc) else { continue };
        let cur_gen = host.document().generation();
        match tracker {
            Some(mut g) => {
                if g.0 == cur_gen {
                    continue; // document unchanged since our last re-bake
                }
                g.0 = cur_gen; // live edit — re-bake from composed below
            }
            None => {
                // First sight. The initial bridge parse (`bridge_usd_dem_terrain`) read
                // the BASE stage only, so a runtime overlay restored from
                // `.lunco/runtime/…` on `DocumentOpened` (e.g. a crater/rock layer the
                // user disabled last session) is NOT reflected in the just-built terrain.
                // If such an overlay exists we MUST re-bake from the composed (base ⊕
                // runtime) doc — otherwise the persisted disable is silently ignored and
                // the terrain shows the base values on every launch. `start_dem_restamp`
                // needs the retained `DemBaseGrid`, so wait for the async DEM build to
                // deposit it before triggering. With no runtime overlay the bridge parse
                // is authoritative → seed + skip (no wasted startup re-stamp).
                let has_runtime_override = host
                    .document()
                    .runtime_data()
                    .iter()
                    .any(|(_, spec)| spec.ty == openusd::sdf::SpecType::Prim);
                if has_runtime_override && !has_base_grid {
                    continue; // retry next frame, once the base grid is built
                }
                commands.entity(entity).try_insert(TerrainDocGeneration(cur_gen));
                if !has_runtime_override {
                    continue; // nothing persisted to re-apply
                }
                // fall through: re-parse composed + insert stack → one startup re-bake
            }
        }
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        // `composed_arc` is memoized by generation, so this shares the SAME recompose
        // the twin overlay serialize already paid for this edit (was a second full
        // O(stage) layer merge on the main thread per brush stroke).
        let composed = host.document().composed_arc();
        let stack = parse_terrain_layer_stack(composed.as_ref(), &sdf, &parser);
        // Despawn-safe: a scene reload can despawn this terrain between queue
        // time and apply_deferred — no-op instead of panicking.
        commands.entity(entity).try_insert(stack);
    }
}

/// Author one attribute onto a prim's **runtime** layer (non-destructive override).
fn author_layer_attr(
    registry: &mut lunco_usd::UsdDocumentRegistry,
    doc: lunco_doc::DocumentId,
    path: &str,
    name: &str,
    type_name: &str,
    value: String,
) {
    let _ = registry.apply(
        doc,
        lunco_usd::UsdOp::SetAttribute {
            edit_target: lunco_usd::LayerId::runtime(),
            path: path.to_string(),
            name: name.to_string(),
            type_name: type_name.to_string(),
            value,
        },
    );
}

/// Inspector crater/rock tuning on a **doc-backed** terrain: author the changed params
/// onto its USD `craters`/`rocks` layer prims (runtime layer) rather than mutating the
/// `TerrainLayerStack` directly. The USD mutation then drives everything automatically
/// — the registry document's generation advances → `refresh_docbacked_terrain_from_doc`
/// re-parses the stack from the composed (`base ⊕ runtime`) doc → `start_dem_restamp`
/// re-bakes off the retained base grid (off-thread, debounced; no GeoTIFF re-read). The
/// terrain's `LiveRebuildExempt` marker suppresses the twin whole-scene reload this edit
/// would otherwise trigger. This is the USD-source-of-truth path; the direct
/// `on_obstacle_spec_rebuild_layers` handles only document-free terrains
/// (`Without<DocBackedTerrain>`), so exactly one path fires.
fn on_obstacle_spec_authored(
    trigger: On<lunco_obstacle_field::plugin::UpdateObstacleFieldSpec>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_usd::UsdDocumentRegistry>>,
) {
    use lunco_usd_bevy::UsdRead as _;
    let Some(mut registry) = registry else { return };
    let spec = &trigger.event().spec;
    // The USD crater/rock layer parsers use `density > 0` as the on/off signal
    // (`parse_crater_layer`/`parse_rock_layer` drop the layer at density ≤ 0), so the
    // Inspector's `enabled` checkbox must fold into the authored density here — else an
    // unchecked-but-nonzero layer re-parses as still-on and stays visible. Author the
    // EFFECTIVE density (0 when disabled); the live in-memory spec keeps the real value,
    // so re-checking restores it within the session.
    let crater_density = if spec.craters.enabled { spec.craters.density } else { 0.0 };
    let rock_density = if spec.rocks.enabled { spec.rocks.density } else { 0.0 };
    for (prim_path, td) in &terrains {
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        let doc = lunco_doc::DocumentId::new(td.doc);
        // Enumerate the terrain's child layer prims from the composed (base ⊕ runtime)
        // document — the stage asset no longer carries a flattened reader. `composed()`
        // is owned, so the registry borrow ends here and `author_layer_attr` below can
        // take it mutably.
        let Some(composed) = registry.host(doc).map(|h| h.document().composed()) else { continue };
        let layers: Vec<(String, String)> = composed
            .children(&sdf)
            .into_iter()
            .filter_map(|child| {
                composed
                    .scalar::<String>(&child, "lunco:layer")
                    .map(|ty| (child.as_str().to_string(), ty))
            })
            .collect();
        for (path, layer_type) in layers {
            match layer_type.as_str() {
                "craters" => {
                    info!("[obstacle-usd] authoring craters density={crater_density} (enabled={}) sizeMode={} seed={:#x} → {path} (doc {})", spec.craters.enabled, spec.craters.size.mode, spec.seed, td.doc);
                    author_layer_attr(&mut registry, doc, &path, "density", "float", crater_density.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMode", "float", spec.craters.size.mode.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMin", "float", spec.craters.size.min.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMax", "float", spec.craters.size.max.to_string());
                    author_layer_attr(&mut registry, doc, &path, "depthRatio", "float", spec.craters.depth_ratio.to_string());
                    author_layer_attr(&mut registry, doc, &path, "rimRatio", "float", spec.craters.rim_height_ratio.to_string());
                    // The u64 seed bit-casts through int64; `parse_crater_layer` casts back
                    // (`s as u64`), so the full Reseed range round-trips. Without this attr
                    // every doc-driven re-parse falls back to the parser default and the
                    // crater layout silently flips between the resource seed and 0xC0FFEE.
                    author_layer_attr(&mut registry, doc, &path, "seed", "int64", (spec.seed as i64).to_string());
                }
                "rocks" => {
                    author_layer_attr(&mut registry, doc, &path, "density", "float", rock_density.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMode", "float", spec.rocks.size.mode.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMin", "float", spec.rocks.size.min.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMax", "float", spec.rocks.size.max.to_string());
                    author_layer_attr(&mut registry, doc, &path, "dynamicFrac", "float", spec.rocks.dynamic_fraction.to_string());
                    author_layer_attr(&mut registry, doc, &path, "seed", "int64", (spec.seed as i64).to_string());
                }
                _ => {}
            }
        }
    }
}

fn bridge_usd_dem_terrain(
    q: Query<(Entity, &lunco_usd::UsdPrimPath), Without<DemBridged>>,
    // Live terrains already realized from a PRIOR instantiation pass. A stage
    // recompose (runtime-overlay restore, doc-backing) hands every prim a fresh
    // ECS entity; the previous pass's terrain survives long enough to double
    // the DEM build. Two live terrains for one authored prim stream two
    // collider rings from two oracles — the rover rides whichever surface is
    // higher (a stale smooth ring over the cratered fresh one reads as
    // "floating over every crater").
    q_prior_terrains: Query<
        (Entity, &lunco_usd::UsdPrimPath),
        Or<(
            With<lunco_terrain_surface::DemTerrainRequest>,
            With<lunco_terrain_surface::DemHeightField>,
        )>,
    >,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    twins: Res<lunco_assets::twin_source::TwinRoots>,
    asset_server: Res<AssetServer>,
    registry: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    mut obstacle_spec: ResMut<lunco_obstacle_field::ObstacleFieldSpec>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
) {
    for (entity, prim_path) in &q {
        // Read the LIVE canonical stage (built on demand from the asset's recipe)
        // — the source of truth. Wait until it is available before reading attrs.
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        if canonical.get(id).is_none() {
            // No live stage (asset carries no recipe / build failed) — retry next frame.
            continue;
        }
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else {
            commands.entity(entity).try_insert(DemBridged);
            continue;
        };
        commands.entity(entity).try_insert(DemBridged); // examined — don't re-scan
        // Newest pass wins: retire any prior terrain realized for this same
        // authored prim (same path + same stage asset). Its LOD tiles, ring
        // tiles, and scatter are reaped by their respective orphan reapers.
        for (prior, prior_path) in &q_prior_terrains {
            if prior != entity
                && prior_path.path == prim_path.path
                && prior_path.stage_handle.id() == prim_path.stage_handle.id()
            {
                warn!(
                    "[usd-dem] retiring duplicate terrain entity {prior} for {} \
                     (superseded by a re-composed instantiation pass)",
                    prim_path.path
                );
                commands.entity(prior).try_despawn();
            }
        }
        // Directory of the scene asset this prim came from (e.g.
        // `twins/moonbase`), used to resolve a relative `demSource` when NO
        // Twin is open — the web autoload path (LoadScene from the staged asset
        // tree) has no `twin://` root, so the DEM is resolved against the
        // scene's own folder instead. `None` for in-memory stages.
        let asset_path = asset_server.get_path(id);
        // Did this scene arrive over the wire as a `scenario://` twin? Then its
        // `demSource` is relative to the SCENARIO, and any locally-open twin is
        // an unrelated scene that must not capture the lookup.
        let from_scenario = asset_path.as_ref().is_some_and(|p| {
            matches!(
                p.source(),
                bevy::asset::io::AssetSourceId::Name(n)
                    if &**n == lunco_assets::scenario_source::SCENARIO_SCHEME
            )
        });
        let scene_dir = asset_path.and_then(|p| p.path().parent().map(|d| d.to_path_buf()));
        let cs = canonical.get(id).expect("checked above");
        bridge_dem_prim_read(
            &cs.view(), entity, prim_path, &sdf, &twins, scene_dir.as_deref(), from_scenario,
            &registry, obstacle_spec.bypass_change_detection(), &mut commands,
        );
    }
}

/// The DEM-bridge read body, generic over the read source ([`UsdRead`]) — reads
/// the authored `lunco:assetMode` / child-layer / anchor attributes off either the
/// live [`StageView`](lunco_usd_bevy::StageView) or the flattened `sdf::Data`,
/// identically, and attaches the terrain request + composed stack + georef.
/// Extracted from `bridge_usd_dem_terrain` for the dual-source cutover.
#[allow(clippy::too_many_arguments)]
fn bridge_dem_prim_read<R: UsdRead>(
    reader: &R,
    entity: Entity,
    prim_path: &lunco_usd::UsdPrimPath,
    sdf: &openusd::sdf::Path,
    twins: &lunco_assets::twin_source::TwinRoots,
    scene_dir: Option<&std::path::Path>,
    from_scenario: bool,
    registry: &lunco_terrain_surface::TerrainLayerParserRegistry,
    obstacle_spec: &mut lunco_obstacle_field::spec::ObstacleFieldSpec,
    commands: &mut Commands,
) {
    // A DEM-backed terrain: `lunco:assetMode = "dem"` (or "layered"). Its surface
    // is COMPOSED from child LAYER prims (`lunco:layer = "dem" | "craters" |
    // "rocks" | "shader" | …`) — add a layer by adding a prim. The `dem` (ground)
    // layer supplies the heightmap source + window; the rest stamp/scatter/shade.
    let asset_mode = reader.scalar::<String>(sdf, "lunco:assetMode");
    if !matches!(asset_mode.as_deref(), Some("dem") | Some("layered")) {
        return;
    }

    // The ground (`dem`) layer + the composable stack (craters/rocks/shader/…),
    // parsed from the child layer prims (helpers shared with the live-edit refresh).
    let dem_layer_sdf = find_dem_layer(reader, sdf);
    let stack = parse_terrain_layer_stack(reader, sdf, registry);
    // Seed the Inspector's shared spec from the authored values so the panel opens
    // showing THIS scene's craters/rocks, not the resource defaults (caller passes
    // `bypass_change_detection` so it doesn't look like a runtime edit).
    sync_obstacle_spec_from_usd(reader, sdf, obstacle_spec);

    // DEM/ground parameters: prefer a `dem` child layer prim (plain attr names);
    // fall back to the Terrain prim's own `lunco:terrain:*` attrs (back-compat).
    let dem = dem_layer_sdf.clone();
    let attr_f32 = |name: &str, legacy: &str| -> Option<f32> {
        dem.as_ref().and_then(|d| reader.real_f32(d, name)).or_else(|| reader.real_f32(sdf, legacy))
    };
    let attr_i32 = |name: &str, legacy: &str| -> Option<i32> {
        dem.as_ref()
            .and_then(|d| reader.scalar::<i32>(d, name))
            .or_else(|| reader.scalar::<i32>(sdf, legacy))
    };
    let attr_bool = |name: &str, legacy: &str| -> Option<bool> {
        dem.as_ref()
            .and_then(|d| reader.scalar::<bool>(d, name))
            .or_else(|| reader.scalar::<bool>(sdf, legacy))
    };

    let rel = dem
        .as_ref()
        .and_then(|d| reader.scalar::<String>(d, "demSource"))
        .or_else(|| reader.scalar::<String>(sdf, "lunco:terrain:demSource"));
    let Some(rel) = rel else {
        warn!("[usd-dem] prim {} is a DEM terrain but has no dem-layer demSource", prim_path.path);
        return;
    };
    // Resolve the DEM source to a byte-readable URI.
    //
    // A `scenario://` scene wins outright: its `demSource` is relative to the
    // DOWNLOADED twin, and the client almost always has an unrelated twin open
    // (it boots the local demo before joining), which would otherwise capture the
    // lookup and resolve the DEM under `assets/scenes/sandbox/` — the terrain then
    // fails to build and the twin arrives with no ground.
    //
    // `AssetPath::path()` strips the source, so `scene_dir` is the bare
    // `<scenario-id>`. Native reads an absolute path out of the scenario cache;
    // web keeps it cache-relative, which is what the wasm DEM reader probes
    // against OPFS (`<cache>/scenarios/<id>/…`).
    //
    // Otherwise prefer an open Twin's root (native Open-Twin flow, absolute fs
    // path), then fall back to the scene's own asset directory — the web autoload
    // path loads the scene from the staged `assets/` tree with no `twin://` root,
    // so `terrain/…` resolves to `twins/<name>/terrain/…` (asset-relative; the
    // wasm DEM reader fetches it same-origin under `assets/`).
    let uri = if from_scenario {
        let Some(dir) = scene_dir else {
            warn!("[usd-dem] scenario DEM source '{rel}' has no scene directory");
            return;
        };
        let base = {
            #[cfg(not(target_arch = "wasm32"))]
            {
                lunco_assets::cache_dir().join("scenarios").join(dir)
            }
            #[cfg(target_arch = "wasm32")]
            {
                dir.to_path_buf()
            }
        };
        base.join(&rel).to_string_lossy().replace('\\', "/")
    } else if let Some((_, root)) = twins.primary() {
        root.join(&rel).to_string_lossy().to_string()
    } else if let Some(dir) = scene_dir {
        dir.join(&rel).to_string_lossy().replace('\\', "/")
    } else {
        warn!("[usd-dem] cannot resolve DEM source '{rel}': no open Twin and no scene directory");
        return;
    };
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
    // `colliderRing` = stream a per-body collider ring vs one static collider.
    // The static full-DEM collider is Nyquist-gated at the DEM base spacing
    // (~3.9 m), so it fades every crater below ~12 m radius FLAT in physics while
    // the 0.65 m near tiles render deep bowls (rovers visibly float above / sink
    // into what they see). Analytic height-modifier layers (craters / edits /
    // overzoom) live ENTIRELY below that limit — a static collider therefore CANNOT
    // represent them, so whenever the terrain both streams fine visuals (`lodViz`)
    // AND carries such layers, force the ring (it samples the oracle at each tile's
    // own resolution, matching the surface exactly). Only when there are no height
    // layers does the authored attr decide (default = `lodViz`); an explicit
    // `colliderRing = false` still keeps the static collider for a plain DEM.
    let has_height_layers = stack
        .0
        .iter()
        .any(|e| matches!(e.layer.id(), "craters" | "edits" | "overzoom"));
    let collider_ring = if lod_viz && has_height_layers {
        true
    } else {
        attr_bool("colliderRing", "lunco:terrain:colliderRing").unwrap_or(lod_viz)
    };
    // (`detailUpsample` is retired: craters/edits are ANALYTIC modifiers on the
    // surface oracle now, sampled at each consumer's own resolution — grid
    // upscaling has nothing left to buy.)

    let layer_count = stack.0.len();
    commands.entity(entity).try_insert((
        lunco_terrain_surface::DemTerrainRequest {
            uri,
            half_window,
            target_res,
            lod_viz,
            collider_ring,
            with_default_material: false,
        },
        stack,
        lunco_terrain_surface::DemTerrainSurface,
    ));
    // Georeference (#5): the `lunco:anchor:*` lat/lon/height anchor + the stage
    // `metersPerUnit`. The terrain math is metres, so a non-1 `metersPerUnit`
    // is recorded but flagged loudly (we don't rescale the DEM). Attach a
    // `TerrainGeoref` whenever any of these are authored.
    let anchor_lat = reader.real(sdf, "lunco:anchor:lat");
    let anchor_lon = reader.real(sdf, "lunco:anchor:lon");
    let anchor_height = reader.real(sdf, "lunco:anchor:height");
    let meters_per_unit = reader.real(sdf, "metersPerUnit");
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
        commands.entity(entity).try_insert(georef);
        info!(
            "[usd-dem] georef: lat {:.4} lon {:.4} height {:.1} m (mpu {})",
            georef.center_lat_deg, georef.center_lon_deg, georef.anchor_height_m, georef.meters_per_unit
        );
    }
    info!(
        "[usd-dem] bridged layered terrain prim {} → DEM '{rel}' (target_res {target_res}, \
         lod_viz {lod_viz}, collider_ring {collider_ring}, {layer_count} composed layer(s))",
        prim_path.path
    );
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
/// Doc 43: a loaded scene that authors a site anchor (`lunco:anchor:*` on its
/// root prim — e.g. the moonbase Twin) turns the solar-system view on. The
/// hierarchy spawn is idempotent and the avatar keeps the FloatingOrigin
/// (`spawn_observer_camera` stays false), so this is purely additive: the Moon
/// globe appears under the georeferenced terrain, Earth/Sun in the sky, and
/// scrolling out of the scene reaches the solar system.
fn enable_celestial_on_site_anchor(
    q_added: Query<(), Added<SiteAnchor>>,
    config: Option<ResMut<CelestialConfig>>,
) {
    let Some(mut config) = config else { return };
    if !q_added.is_empty() && !config.spawn_hierarchy {
        info!("[celestial] site-anchored scene loaded → enabling the solar hierarchy");
        config.spawn_hierarchy = true;
    }
}

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
//  `release` port → `lunco_sandbox_edit::commands::ReleaseActuator` → DetachJoint.
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

