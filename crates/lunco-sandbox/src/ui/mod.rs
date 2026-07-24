//! Sandbox UI layer — everything that draws pixels, opens egui panels, or
//! drives an interactive camera.
//!
//! This whole module is `#[cfg(feature = "ui")]` (declared in `lib.rs`), so a
//! headless `--no-ui` / `lunco-sandbox-server` build never compiles it. The
//! entry point is [`SandboxUiPlugin`]: the app shell adds it only when running
//! windowed (`ui` feature present AND not `--no-ui`). The shared sim/physics/
//! cosim/networking core (`SandboxCorePlugin`) and the headless runner
//! (`SandboxHeadlessPlugin`) live in `lib.rs` and carry no UI.
//!
//! Mirrors the `ui/` + `*UiPlugin` convention every library crate already uses
//! (`SandboxEditUiPlugin`, `UsdUiPlugin`, `ModelicaUiPlugin`, …) — the app crate
//! is now structurally identical to them.

use bevy::prelude::*;
use big_space::prelude::*;
use leafwing_input_manager::prelude::*;

use lunco_avatar::{
    AdaptiveNearPlane, FreeFlightCamera, IntentAnalogState, ProvisionalAvatarCamera,
};
use lunco_core::{Avatar, LocalAvatar};
use lunco_modelica::{ModelicaUiConfig, ModelicaWorkbenchPlugin};
use lunco_workbench::auto_tag_workbench_3d_cameras;

/// Surface ⇄ Moon ⇄ Earth view-mode switcher (site-anchored scenes only).
mod celestial_time;
mod code_panel;
mod models_palette;
/// Which floating viewport overlays are shown (persisted, off by default).
mod overlays;
/// Rhai behaviour editor — edit + save + hot-reload the script on the selected
/// prim, with a diagnostics list. The writable counterpart of `code_panel`.
mod rhai_editor_panel;
mod rhai_repl_panel;
/// Driver cockpit overlay for the View perspective — attitude/tilt, nav readout,
/// and the physics-real transport band. Paints only while possessing a vessel.
mod rover_hud;
/// Centered "Downloading <scenario>" overlay during scenario-sync asset fetch.
/// Networking-only — the file carries its own `#![cfg(feature = "networking")]`.
#[cfg(feature = "networking")]
mod scenario_download;
/// Centered "Generating terrain…" overlay during the initial DEM bake.
mod terrain_progress;
mod view_mode;

/// The sandbox's interactive layer: egui workbench, bevy_picking, the USD Twin
/// browser + RTT viewport, the in-scene editor, materials, rover panels, and
/// the fallback free-flight camera.
///
/// Added by the app shell only for a windowed run. A headless server runs the
/// sim, physics, scene, cosim, and networking host (all in `SandboxCorePlugin`)
/// *without* any of this — the render plugins still load in `backends: None`
/// mode so the asset stores exist, but nothing here (GPU / window / pointer)
/// is wired.
pub(crate) struct SandboxUiPlugin;

impl Plugin for SandboxUiPlugin {
    fn build(&self, app: &mut App) {
        // Winit frame pacing. Continuous while focused lets vsync (Fifo present /
        // requestAnimationFrame on web) act as the frame timer; ReactiveLowPower
        // keeps fans quiet when backgrounded. Networked windows stay Continuous
        // unfocused so lightyear keepalives keep flowing (one of two side-by-side
        // windows is always unfocused; the default ~1 FPS throttle starves the
        // link past timeout). `--no-throttle` forces Continuous for automated
        // tests whose window sits unfocused. WinitSettings is read by the runner
        // each frame, so inserting it from this plugin (after DefaultPlugins) is
        // fine.
        let args: Vec<String> = std::env::args().collect();
        let networked = args.iter().any(|a| a == "--host" || a == "--connect");
        let no_throttle = args.iter().any(|a| a == "--no-throttle");
        {
            use bevy::winit::{UpdateMode, WinitSettings};
            app.insert_resource(WinitSettings {
                focused_mode: UpdateMode::Continuous,
                unfocused_mode: if networked || no_throttle {
                    UpdateMode::Continuous
                } else {
                    UpdateMode::reactive_low_power(std::time::Duration::from_secs(1))
                },
            });
        }

        app.add_plugins(bevy::pbr::wireframe::WireframePlugin::default())
            // bevy_picking's mesh backend: makes visible Mesh3d entities pickable,
            // so scene selection / possession / spawn-placement run as click observers.
            .add_plugins(bevy::picking::mesh_picking::MeshPickingPlugin)
            .add_plugins(lunco_workbench::WorkbenchPlugin)
            // Overlay visibility prefs + the Time-menu rows that drive them.
            .add_plugins(overlays::plugin)
            // USD Twin browser. NOTE: the USD *viewport preview*
            // (`UsdViewportPlugin`) is intentionally NOT added here. It is an
            // editor tool that OWNS its own scene — it parses the active USD doc
            // into a second `UsdStageAsset` and mounts a private `scene_root`. The
            // sandbox is a sim app: its single scene is the live `LoadScene` world,
            // viewed by the window camera. Adding the preview built the scene a
            // SECOND time (doubled crater meshes / rocks). A view must not own a
            // scene — see `docs/usd-source-of-truth-ecs-projection-design.md`.
            .add_plugins(lunco_usd::ui::UsdUiPlugin)
            .add_plugins(lunco_sandbox_edit::SandboxEditPlugin)
            .add_plugins(lunco_sandbox_edit::ui::SandboxEditUiPlugin)
            // NOTE: `ShaderMaterialPlugin` (the dynamic `ShaderMaterial` render
            // pipeline) used to be added here. It now lives inside
            // `lunco_render_bevy::LuncoRenderPlugin` — the one crate that may name
            // `bevy_pbr` — and adding it a second time panics Bevy.
            // See docs/architecture/render-decoupling.md.
            // The shared tutorial launcher: registry + 🎓 menu + panel +
            // Start/Skip/SetSubsystemEnabled + progress + onboarding + F1.
            // Tutorials load from assets/tutorials/sandbox/tutorials.json (data, not code).
            .add_plugins(lunco_tutorial::TutorialPlugin {
                app: "sandbox".into(),
            })
            // Rover panels. ONE closure: Bevy keys plugin uniqueness by type-name,
            // and every `|app| {…}` in this `build` shares the name `{{closure}}` — a
            // second one panics ("plugin already added"). So all app-level panel
            // registration goes here.
            .add_plugins(|app: &mut App| {
                use lunco_settings::AppSettingsExt;
                use lunco_workbench::WorkbenchAppExt;
                app.register_settings_section::<lunco_settings::DownloadSettings>();
                // Rover-specific panels and the attach-a-model click flow.
                app.register_panel(code_panel::CodePanel);
                // Rhai behaviour editor (Object Builder). Its view-model is
                // produced each frame from the selection + ScriptRegistry.
                app.register_panel(rhai_editor_panel::RhaiEditorPanel);
                app.init_resource::<rhai_editor_panel::RhaiEditorVm>();
                app.add_systems(Update, rhai_editor_panel::produce_rhai_editor_vm);
                app.register_panel(models_palette::ModelsPalette);
                // In-app rhai REPL — runs snippets against the live app through the
                // API bridge, on web + native. Gated on bridge availability.
                #[cfg(any(target_arch = "wasm32", feature = "transport-http"))]
                app.register_panel(rhai_repl_panel::RhaiReplPanel::default());
                app.init_resource::<models_palette::AttachState>();
                // Attach is bevy_picking-driven (observes the same `Pointer<Click>`
                // as selection; egui occlusion handled by the framework).
                app.add_observer(models_palette::on_scene_click_attach);
                app.add_systems(Update, models_palette::attach_escape_system);
            })
            // ModelicaPlugin's AnalyzePerspective registers before SandboxEditUiPlugin's
            // workspaces; without this nudge we'd boot into the Modelica layout.
            // Activate the 3D-only View workspace by default.
            .add_systems(
                Startup,
                |mut layout: ResMut<lunco_workbench::WorkbenchLayout>| {
                    layout.activate_perspective(lunco_workbench::PerspectiveId("sandbox_view"));
                },
            )
            .insert_resource(CurrentScenePath::default())
            .add_systems(
                Startup,
                (
                    init_current_scene_path,
                    register_sandbox_scenarios_menu,
                    register_downloadable_assets_settings,
                ),
            )
            .add_observer(
                |t: On<lunco_usd::LoadScene>,
                 mut current: ResMut<CurrentScenePath>,
                 current_name: Option<ResMut<lunco_workbench::CurrentSceneName>>| {
                    current.0 = t.event().path.clone();
                    if let Some(mut name) = current_name {
                        name.0 = std::path::Path::new(&t.event().path)
                            .file_name()
                            .and_then(|f| f.to_str())
                            .unwrap_or(&t.event().path)
                            .to_string();
                    }
                },
            )
            // Confine window-targeting cameras to the ViewportPanel rect (prevents
            // the full-window 3D bleed-on-pass-skip bug). RTT cameras are skipped.
            .add_systems(Update, auto_tag_workbench_3d_cameras)
            // Sharpest shadow filter (hard airless-Moon terminator) on each camera.
            .add_systems(Update, force_hard_shadow_filtering)
            // Fallback free-flight camera when the scene authors none — interactive
            // only; a headless server has no user to control.
            .add_systems(
                PostUpdate,
                spawn_fallback_avatar.after(avian3d::prelude::PhysicsSystems::Writeback),
            )
            // Centered "Generating terrain…" card during the initial DEM bake
            // (heightmap decode + crater stamp), so the black startup viewport
            // reads as progress. Clears itself once the bake finishes.
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (
                    terrain_progress::draw_terrain_progress,
                    // Surface ⇄ Moon ⇄ Earth switcher — appears only when the
                    // celestial hierarchy is live (the scene declared bodies).
                    //
                    // NOT while recording: these two are EDITOR chrome — they
                    // exist so an operator can retarget the view and scrub the
                    // sky clock. An offline take is film output, and a scene
                    // that declares celestial bodies (which any scene with a
                    // real sun now does) would otherwise burn a clock readout
                    // and a view switcher into every frame.
                    //
                    // All three are window-space `egui::Area`s, so they are also
                    // gated on the View perspective (`in_view_perspective`) —
                    // in an authoring perspective they painted straight across
                    // the docked panels.
                    // Both are also OPT-IN now (`OverlaySettings`, off by default,
                    // toggled from the Time menu). Permanent chrome over the
                    // viewport should be something you asked for; the sky clock's
                    // controls live in that menu regardless, so switching the pill
                    // off costs no capability.
                    view_mode::draw_view_mode_switcher
                        .run_if(not(recording_offline))
                        .run_if(in_view_perspective)
                        .run_if(overlays::view_switcher_visible),
                    // Sky clock: rate + couple/detach for the CELESTIAL clock only
                    // (not the sim transport). Same visibility gate.
                    celestial_time::draw_celestial_time
                        .run_if(not(recording_offline))
                        .run_if(in_view_perspective)
                        .run_if(overlays::sky_clock_visible),
                    // Driver cockpit: attitude/tilt (bottom-left) + nav/controls
                    // (bottom-right). Only while possessing a vessel, and only in
                    // View. Transport (pause + rate) lives on the workbench
                    // toolbar, next to the pause button that already owns
                    // `TimeTransport`.
                    rover_hud::draw_rover_hud.run_if(in_view_perspective),
                ),
            );
        // G2: "Downloading <scenario>" overlay during scenario-sync asset fetch.
        // Networking-only — the module is `#[cfg(feature = "networking")]`.
        #[cfg(feature = "networking")]
        app.add_systems(
            bevy_egui::EguiPrimaryContextPass,
            scenario_download::draw_scenario_download,
        );

        // Deterministic per-view-mode exposure for celestial scenes (orbital
        // = calibrated EV, surface = 4 stops open for the earthshine-lit
        // polar site).
        app.add_systems(Update, mode_exposure);

        // Extra tutorial TRACKS — Basic (rover fundamentals) and Space School
        // (the IKI event scenario). `TutorialPlugin` is added once above for the
        // primary "sandbox" app (its build() does the one-time command/panel/
        // observer setup); adding it again would panic on duplicate-plugin. So
        // these extra apps just contribute their JSON catalog into the shared
        // `TutorialRegistry` via `register_tutorial`.
        // Data-driven: a new track is a `tutorials.json` + `.rhai`.
        //
        // Only tracks this app SHIPS. Space School is not one of them — it is a
        // TWIN (`sim/tutorials/tutorials.json`), loaded by `sync_twin_tutorials`
        // when that twin is opened. Registering it here too would double-register
        // the same lesson ids against a stale bundled copy of the scene.
        {
            use lunco_tutorial::TutorialAppExt;
            for track in ["basic"] {
                let path = format!("{track}/tutorials.json");
                match lunco_assets::tutorials::tutorial_source(&path) {
                    Some(src) => {
                        match serde_json::from_str::<Vec<lunco_tutorial::TutorialMeta>>(&src) {
                            Ok(metas) => {
                                for m in metas {
                                    app.register_tutorial(m);
                                }
                            }
                            Err(e) => {
                                bevy::log::warn!("tutorials manifest '{path}' failed to parse: {e}")
                            }
                        }
                    }
                    None => bevy::log::warn!("no tutorials manifest at 'assets/tutorials/{path}'"),
                }
            }
        }

        // Embed the FULL lunica workbench as the "Design" workspace via the
        // shared bundle — same clipboard bridge, autosave, worker, and panels
        // as standalone lunica, so the Design tab can't drift from the real
        // IDE. We pass only the one intentional embed knob: suppress the
        // first-run help overlay (lunica's onboarding coach-marks, out of
        // place inside a 3D physics demo). Welcome panel stays ON — it's the
        // same landing page lunica uses for the Design tab.
        app.add_plugins(ModelicaWorkbenchPlugin {
            config: ModelicaUiConfig {
                include_help_overlay: false,
                include_welcome_panel: true,
            },
        });

        // WEB: load sequentially — bake the terrain FIRST, then fetch + parse the
        // ~2 MB MSL bundle. The browser has ONE thread; letting the MSL download +
        // chunked decompress/deserialize run concurrently with terrain generation
        // stole the thread and stalled the "Generating terrain" phase. `DeferMslLoad`
        // holds the whole MSL bootstrap (network fetch included) until
        // `release_msl_after_terrain` clears it once the DEM has baked. Native has
        // real threads → no deferral (the gate is never inserted there).
        #[cfg(target_arch = "wasm32")]
        {
            app.init_resource::<lunco_modelica::msl_remote::DeferMslLoad>();
            app.add_systems(bevy::prelude::Update, release_msl_after_terrain);
        }

        // Forced window placement (`--window-pos`). Parses the flag and (when
        // present) inserts the resource, suppresses geometry persistence, and
        // registers the placer system — all in `lunco-workbench` so any binary
        // gets the same behaviour.
        lunco_workbench::wire_window_placement(app, &args);

        // URL-driven boot (wasm). Lets headless test harnesses drive the
        // workbench without firing canvas pointer events. See
        // [`sandbox_boot_from_url`].
        #[cfg(target_arch = "wasm32")]
        app.add_systems(bevy::prelude::Update, sandbox_boot_from_url);
    }
}

/// Inserts the sharpest shadow filter (`Hardware2x2`) on every 3D camera as it
/// appears. USD- and Avatar-spawned cameras land async over many frames; the
/// `Without<ShadowFilteringMethod>` filter catches each exactly once.
/// True while an offline take is capturing frames — the signal that "this
/// viewport is the film, not the editor". Chrome that exists for an operator
/// (view switcher, sky-clock scrubber) hides behind it; instrumentation that
/// describes the VEHICLE does not.
fn recording_offline(
    state: Option<Res<lunco_workbench::screenshot::OfflineRecordingState>>,
) -> bool {
    state.is_some_and(|s| s.active)
}

/// True only in the 🎬 View perspective — full-screen 3D, no dock.
///
/// The floating overlays (driver HUD, view switcher, sky clock) are raw
/// `egui::Area`s in window space: they know nothing about the dock and paint
/// straight over whatever panels an authoring perspective has open. That reads as
/// a broken layer while building, so they exist only where the whole window IS
/// the viewport.
fn in_view_perspective(layout: Option<Res<lunco_workbench::WorkbenchLayout>>) -> bool {
    layout.is_some_and(|l| {
        l.active_perspective() == Some(lunco_workbench::PerspectiveId("sandbox_view"))
    })
}

fn force_hard_shadow_filtering(
    mut commands: Commands,
    q: Query<Entity, (With<Camera3d>, Without<bevy::light::ShadowFilteringMethod>)>,
) {
    for e in &q {
        commands
            .entity(e)
            .try_insert(bevy::light::ShadowFilteringMethod::Hardware2x2);
    }
}

/// Per-view-mode exposure for celestial scenes. One physical scene spans ~14
/// stops: a sunlit globe from orbit is correct at the calibrated EV 15, while
/// the polar moonbase under a grazing sun is earthshine-lit — pitch black at
/// that same fixed EV. Histogram auto-exposure was tried and metered the
/// mostly-black space background instead (whole frame blown white), so this
/// is DETERMINISTIC: orbital → the calibrated EV, surface → 4 stops open
/// (earthshine-readable), eased like eye adaptation. Inert without the
/// celestial hierarchy — studio scenes keep their authored EV.
fn mode_exposure(
    // "Is there a sky?" is now the SCENE's answer, not a host flag: a celestial
    // hierarchy exists iff the scene declared bodies (`LunCoCelestialBodyAPI`).
    q_hierarchy: Query<(), With<lunco_celestial::SolarSystemRoot>>,
    pin: Option<Res<lunco_celestial::OrbitalViewPin>>,
    sun: Option<Res<lunco_environment::LunarSun>>,
    sun_dir: Option<Res<lunco_celestial::SunDirectionWorld>>,
    mut q_exposure: Query<
        (
            &mut bevy::camera::Exposure,
            Option<&CellCoord>,
            &GlobalTransform,
            Option<&bevy::ecs::hierarchy::ChildOf>,
        ),
        With<lunco_core::Avatar>,
    >,
    q_grids: Query<&Grid>,
) {
    let Some(sun) = sun else { return };
    // No celestial hierarchy (studio / flat sandbox scenes) → keep the authored EV.
    if q_hierarchy.is_empty() {
        return;
    }
    let orbital = pin.is_some_and(|p| p.active);
    for (mut exposure, cell, _gtf, child_of) in &mut q_exposure {
        // The camera must be mounted under a celestial `Grid` before its
        // geographic placement settles. Hold exposure until mounted.
        let mounted = cell.is_some()
            && child_of
                .and_then(|c| q_grids.get(c.parent()).ok())
                .is_some();
        if !mounted {
            warn_once!(
                "mode_exposure: avatar camera has no CellCoord/Grid parent — holding \
                 exposure until the mount settles."
            );
            continue;
        }
        let target = if orbital {
            sun.exposure_ev100
        } else {
            let to_sun_site_enu = sun_dir.as_ref().map(|d| -d.0).unwrap_or(Vec3::Y);
            let elev = to_sun_site_enu.y.clamp(-1.0, 1.0).asin();
            let sunlit = ((elev + 0.02) / 0.02).clamp(0.0, 1.0);
            9.0 + (sun.exposure_ev100 - 9.0) * sunlit
        };
        exposure.ev100 = target;
    }
}

/// Grace period before [`spawn_fallback_avatar`] steps in (USD load is async).
const FALLBACK_AVATAR_GRACE_SECS: f32 = 2.0;

/// Spawns a default avatar if no USD-defined Avatar was loaded.
///
/// This acts as a fallback when the scene file doesn't contain an Avatar
/// prim, ensuring the user always has a controllable camera.
///
/// USD asset loading is async — checking for `Camera3d` on frame 1 is too
/// eager and would spawn a fallback even when the scene *will* provide
/// one a few frames later, leaving the world with two cameras + two
/// `FloatingOrigin`s (which big_space resets every frame, killing perf
/// and breaking propagation). Wait a grace period; if the scene didn't
/// publish a camera by then, spawn the fallback exactly once.
fn spawn_fallback_avatar(
    time: Res<Time>,
    q_cameras: Query<Entity, With<Camera3d>>,
    q_grids: Query<Entity, With<Grid>>,
    active_sun: Res<lunco_environment::LunarSun>,
    mut commands: Commands,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    // A USD-spawned camera ends the wait immediately.
    if q_cameras.iter().next().is_some() {
        *done = true;
        return;
    }
    // Otherwise let USD have its grace window before we step in.
    if time.elapsed_secs() < FALLBACK_AVATAR_GRACE_SECS {
        return;
    }
    let Some(grid) = q_grids.iter().next() else {
        return;
    };

    info!("No USD camera after {FALLBACK_AVATAR_GRACE_SECS}s, spawning fallback FreeFlightCamera");
    commands.spawn((
        // `SceneCamera` is what marks this as THE scene camera. It is not cosmetic:
        // `lunco-celestial`'s `update_globe_lod` now selects the camera with
        // `With<SceneCamera>` (it used to use `With<Camera3d>`, which forced every
        // domain crate to link a GPU stack merely to ask "which camera is the scene
        // one?"). Without it this fallback camera would stream NO globe tiles.
        // The `Camera3d` below is redundant in a render build — the `SceneCamera`
        // binder inserts it — but harmless, and it keeps this spawn readable.
        // See docs/architecture/render-decoupling.md.
        //
        // `agx()` (NOT `default()`): this fallback and the USD avatar camera
        // (`lunco-usd-sim/src/lib.rs`, `SceneCamera::agx()`) MUST share the SAME
        // tone curve. `SceneCamera::default()` is TonyMcMapface; the avatar is
        // AgX. While the active window camera flips between the two (provisional
        // → USD takeover, stage recompose re-instantiating the avatar prim), a
        // mismatch re-grades the whole frame through a different curve — a
        // uniform global lift that reads as "brightness jumps after load" even
        // with EV and sun lux flat. `agx()` keeps the grade identical across a
        // switch.
        lunco_render::SceneCamera::agx(),
        Camera3d::default(),
        // NO SMAA on this (workbench) camera: SMAA's post-process resolve does
        // not survive the full-window-3D + egui-overlay compositing, so it
        // renders a blank/black viewport (and crashes outright without the
        // `smaa_luts` feature). MSAA (the `Camera3d` default) covers geometry
        // edges. See the matching note on the USD avatar camera in lunco-usd-sim.
        //
        // Exposure read from the active-scene `LunarSun` resource — the SAME
        // source as the sun illuminance, so they stay matched. Tunable live via
        // SetEnvironmentLight / the Inspector.
        bevy::camera::Exposure {
            ev100: active_sun.exposure_ev100,
        },
        FreeFlightCamera {
            yaw: -2.245559,
            pitch: -0.303039,
            damping: None,
        },
        AdaptiveNearPlane,
        // Provisional: the authored USD Avatar camera (if the scene has one)
        // takes over and despawns this in the same flush it spawns — see
        // `ProvisionalAvatarCamera`. Without the marker, a slow (web/HTTP) scene
        // load that finishes *after* this stand-in appears leaves two order-0
        // window cameras → camera-order ambiguity + duplicate GizmoCamera.
        ProvisionalAvatarCamera,
        Transform::from_translation(Vec3::new(-30.0, 15.0, -20.0)),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        Avatar,
        LocalAvatar,
        // Nested: a tuple bundle tops out at 15 elements and adding `SceneCamera`
        // (the render-decoupling intent marker, above) pushed this spawn to 16.
        // Grouping the input triple is semantically identical — a nested tuple is
        // just as much a `Bundle`.
        (
            IntentAnalogState::default(),
            ActionState::<lunco_core::UserIntent>::default(),
            lunco_controller::get_avatar_input_map(),
        ),
        ChildOf(grid),
    ));
    *done = true;
}

// ── wasm URL-driven boot ──────────────────────────────────────────────────────

/// State machine for [`sandbox_boot_from_url`].
///
/// Lives in a `Local` so the boot work happens exactly once per app
/// lifetime — once `open_class` is satisfied the system runs and
/// no-ops in O(1).
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SandboxBootState {
    parsed: bool,
    workspace: Option<String>,
    open_class: Option<String>,
    done: bool,
}

/// wasm-only `Update` system that reads `window.location.search` and:
///   - activates the perspective named by `?workspace=…` (once, on
///     first run);
///   - triggers an `OpenClass` for `?open=…` once `MslLoadState`
///     reaches `Ready`. Without that gate the trigger races MSL
///     install and the workbench can't find the class.
///
/// Self-disables after both are applied. Useful for headless test
/// harnesses (e.g. `chrome-devtools-mcp`) which can't drive the egui
/// canvas via synthetic DOM events.
#[cfg(target_arch = "wasm32")]
fn sandbox_boot_from_url(
    mut commands: bevy::prelude::Commands,
    mut layout: Option<bevy::prelude::ResMut<lunco_workbench::WorkbenchLayout>>,
    msl: Option<bevy::prelude::Res<lunco_assets::msl::MslLoadState>>,
    mut state: bevy::prelude::Local<SandboxBootState>,
) {
    if state.done {
        return;
    }

    // ── First-run: parse URL, kick the workspace switch ──────────
    if !state.parsed {
        let search = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default();
        for kv in search.trim_start_matches('?').split('&') {
            let mut parts = kv.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let val_enc = parts.next().unwrap_or("");
            let val = js_sys::decode_uri_component(val_enc)
                .map(|j| j.as_string().unwrap_or_else(|| val_enc.to_string()))
                .unwrap_or_else(|_| val_enc.to_string());
            match key {
                "workspace" => state.workspace = Some(val),
                "open" => state.open_class = Some(val),
                _ => {}
            }
        }
        if let (Some(ws), Some(layout)) = (state.workspace.as_ref(), layout.as_mut()) {
            let id: &'static str = Box::leak(ws.clone().into_boxed_str());
            layout.activate_perspective(lunco_workbench::PerspectiveId(id));
            bevy::log::info!("[sandbox_boot_from_url] activated perspective `{ws}`");
        }
        state.parsed = true;
    }

    // ── Per-frame poll: dispatch OpenClass once MSL is ready ─────
    if let Some(qual) = state.open_class.clone() {
        let ready = matches!(
            msl.as_deref(),
            Some(lunco_assets::msl::MslLoadState::Ready { .. })
        );
        if !ready {
            return;
        }
        commands.trigger(lunco_modelica::ui::commands::OpenClass {
            qualified: qual.clone(),
            ..Default::default()
        });
        bevy::log::info!("[sandbox_boot_from_url] OpenClass({qual}) triggered (MSL ready)");
    }
    state.done = true;
}

/// Tracks the currently loaded scene path, so the user can restart it.
#[derive(Resource, Clone, Default)]
pub(crate) struct CurrentScenePath(pub(crate) String);

fn init_current_scene_path(
    scene_path: Res<crate::ScenePath>,
    mut commands: Commands,
    current_name: Option<ResMut<lunco_workbench::CurrentSceneName>>,
) {
    commands.insert_resource(CurrentScenePath(scene_path.0.clone()));
    if let Some(mut name) = current_name {
        name.0 = std::path::Path::new(&scene_path.0)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(&scene_path.0)
            .to_string();
    }
}

/// Settings ▸ downloadable data — the generic view over
/// [`lunco_assets::datasets`].
///
/// The app never reaches the network on its own: every fetchable dataset is
/// DECLARED in an `Assets.toml` (a crate's, or an open Twin's) and downloaded
/// only from a click here. This panel knows nothing about ephemerides, terrain
/// or MSL — it renders whatever the registry reports, so a new dataset needs a
/// manifest entry and no UI change at all.
fn register_downloadable_assets_settings(world: &mut World) {
    use bevy_egui::egui;
    use lunco_assets::datasets::{DatasetRegistry, DatasetState};
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, world| {
        ui.label(egui::RichText::new("Downloadable data").weak().small());
        if let Some(mut settings) = world.get_resource_mut::<lunco_settings::DownloadSettings>() {
            ui.horizontal(|ui| {
                ui.label("Max parallel downloads:");
                ui.add(egui::Slider::new(
                    &mut settings.max_parallel_downloads,
                    1..=10,
                ));
            });
            ui.add_space(8.0);
        }
        let Some(registry) = world.get_resource::<DatasetRegistry>() else {
            ui.label(
                egui::RichText::new("(dataset registry not installed)")
                    .weak()
                    .italics(),
            );
            return;
        };
        if registry.entries().is_empty() {
            ui.label(
                egui::RichText::new("(nothing declared — no Assets.toml registered)")
                    .weak()
                    .italics(),
            );
            return;
        }
        // Snapshot: the rows below need `&mut World` to request a download.
        //
        // The heading is WHO declared it — the LunCo library that owns the
        // dataset ("celestial", "ephemeris", "modelica") or the twin's own
        // name. `scope.label()` says "engine" for every engine dataset, which
        // is true and useless: a user looking for Earth imagery is looking for
        // the celestial library, not for the fact that it isn't a twin's.
        let rows: Vec<(String, String, String, DatasetState)> = registry
            .entries()
            .iter()
            .map(|e| {
                let owner = match &e.scope {
                    lunco_assets::datasets::DatasetScope::Engine => e.group.clone(),
                    lunco_assets::datasets::DatasetScope::Twin { name, .. } => name.clone(),
                };
                (e.key.clone(), owner, e.name.clone(), e.state.clone())
            })
            .collect();
        // Registration order already groups by owner; sorting makes that a
        // guarantee rather than a coincidence, so the headings below can be
        // emitted on change instead of buffering the whole list.
        let mut rows = rows;
        rows.sort_by(|a, b| a.1.cmp(&b.1));
        let mut requested: Option<String> = None;
        let mut heading: Option<&str> = None;
        for (key, owner, name, state) in &rows {
            if heading != Some(owner.as_str()) {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(owner).weak().small());
                heading = Some(owner.as_str());
            }
            ui.horizontal(|ui| {
                ui.label(name.as_str());
                match state {
                    DatasetState::Installed => {
                        ui.label(egui::RichText::new("✔ cached").weak());
                    }
                    DatasetState::Downloading {
                        bytes_done,
                        bytes_total,
                    } => {
                        let mb = |b: &u64| *b as f64 / (1024.0 * 1024.0);
                        ui.label(if *bytes_total > 0 {
                            format!("⬇ {:.1}/{:.1} MB", mb(bytes_done), mb(bytes_total))
                        } else {
                            format!("⬇ {:.1} MB", mb(bytes_done))
                        });
                    }
                    DatasetState::Missing | DatasetState::Failed(_) => {
                        if let DatasetState::Failed(e) = state {
                            ui.label(egui::RichText::new("⚠").color(egui::Color32::RED))
                                .on_hover_text(e.clone());
                        }
                        if ui
                            .button("⬇ Download")
                            .on_hover_text(
                                "Fetches this dataset and caches it — engine data in the \
                                 shared cache, a twin's data in that twin's .cache. \
                                 Downloads only happen from this button.",
                            )
                            .clicked()
                        {
                            requested = Some(key.clone());
                        }
                    }
                }
            });
        }
        if let Some(key) = requested {
            if let Some(mut registry) = world.get_resource_mut::<DatasetRegistry>() {
                registry.request(&key);
            }
        }
    });
}

fn register_sandbox_scenarios_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_custom_menu("Scenarios", |ui, world| {
        let current_path = world
            .get_resource::<CurrentScenePath>()
            .map(|c| c.0.clone());

        ui.add_enabled_ui(current_path.is_some(), |ui| {
            if ui.button("🔄 Restart Scenario").clicked() {
                if let Some(path) = current_path {
                    world.trigger(lunco_usd::LoadScene {
                        path,
                        root_prim: String::new(),
                    });
                }
                ui.close();
            }
        });

        ui.separator();

        // ── Tutorials submenu ────────────────────────────────────────────
        // A dedicated entry so users can jump straight into any interactive
        // lesson (same list the Tutorials panel shows). Each entry starts the
        // tutorial by id via `StartTutorial`, which loads its scene + attaches
        // the orchestrator script. Hovering an entry reveals its blurb — the
        // plain-language "what does this teach" tip.
        render_tutorials_submenu(ui, world);

        // ── Downloaded Twins (scenario-sync cache, G3) ───────────────────
        // Twins fetched from a server into the local cache — loadable offline
        // as a `twin://` root over the cache dir. Networking-only; the registry rebuilds from
        // `<cache>/scenarios/index.json` at boot and updates as downloads finish.
        #[cfg(feature = "networking")]
        {
            use lunco_networking::scenario_sync::CachedTwinsRegistry;
            let entries = world
                .get_resource::<CachedTwinsRegistry>()
                .map(|r| r.entries.clone())
                .unwrap_or_default();
            ui.menu_button(format!("📦 Downloaded Twins ({})", entries.len()), |ui| {
                if entries.is_empty() {
                    ui.label(
                        bevy_egui::egui::RichText::new("(connect to a server to download one)")
                            .weak()
                            .italics(),
                    );
                }
                for entry in &entries {
                    let mb = (entry.total_bytes as f64) / (1024.0 * 1024.0);
                    let label = if entry.name.is_empty() {
                        format!("Downloaded twin  ({mb:.0} MB)")
                    } else {
                        format!("{}  ({mb:.0} MB)", entry.name)
                    };
                    if ui.button(label).clicked() {
                        if let Some(scene) = entry.default_scene.clone() {
                            // Mounts the cache dir as this twin's root and yields the
                            // same `twin://<name>/<rel>` the host uses for the scene.
                            let twins = world
                                .resource::<lunco_assets::twin_source::TwinRoots>()
                                .clone();
                            let path = lunco_networking::scenario_sync::mount_scenario_twin(
                                &twins,
                                &entry.scenario_id,
                                &entry.name,
                                &scene,
                            );
                            world.trigger(lunco_usd::LoadScene {
                                path,
                                root_prim: String::new(),
                            });
                            ui.close();
                        }
                    }
                }
            });
        }

        ui.separator();

        let Some(roots) = world.get_resource::<lunco_assets::twin_source::TwinRoots>() else {
            ui.label(
                bevy_egui::egui::RichText::new("(no TwinRoots resource)")
                    .weak()
                    .italics(),
            );
            return;
        };

        let Some(manifest) = world.get_resource::<lunco_assets::discovery::AssetManifest>() else {
            return;
        };
        // On the web the listing arrives by fetch. "Not loaded yet" is not "no
        // scenes" — say which, rather than showing an empty menu that looks final.
        if !manifest.ready() {
            ui.label(
                bevy_egui::egui::RichText::new("(loading asset list…)")
                    .weak()
                    .italics(),
            );
            return;
        }

        // The Scenarios menu contains loadable scene entry layers only. Twins
        // declare these through `[usd] scenes`; reference-only USD layers and
        // other source types remain available in the Library browser.
        let mut assets = lunco_assets::discovery::list_scene_assets(manifest, roots);
        // Names copied out here: `roots`/`manifest` borrow the world, and every
        // click below needs `&mut World` to trigger the load.
        let twin_names = roots.names();

        // Test assets are hidden unless the user asks for them: they are rigs
        // `scripts/run_scene_tests.sh` runs for a verdict, and there are more of
        // them than there are scenes worth opening. `is_test_asset` keys on the
        // `tests/` DIRECTORY — so a `scenarios/tests/` script or a `scenes/tests/`
        // scene is hidden the same way. The pref is one checkbox in the Settings
        // menu, so a test asset is never unreachable.
        let show_tests = world
            .get_resource::<lunco_sandbox_edit::ui::asset_visibility::AssetVisibilitySettings>()
            .is_some_and(|s| s.show_test_assets);
        if !show_tests {
            assets.retain(|asset| !lunco_assets::discovery::is_test_asset(&asset.rel));
        }
        assets.sort_by(|a, b| a.stem.cmp(&b.stem));

        if assets.is_empty() {
            ui.label(
                bevy_egui::egui::RichText::new("(no assets found)")
                    .weak()
                    .italics(),
            );
            return;
        }

        // Each asset's `lunco:description`, straight from the catalogue's
        // metadata store — the scan already read and parsed every project
        // `*.usda`, so re-reading them here would be a second parse of the
        // same default prim of the same file. (It used to be exactly that:
        // a `SceneDescCache` that lazily re-parsed on first hover.) Only scenes
        // carry a description; `.rhai`/`.mo`/`.btxml`/`.wgsl` files map to
        // `None` and simply show no tooltip.
        //
        // The store fills asynchronously, so a scene not yet read simply
        // shows no tooltip this frame and gets one on the next redraw.
        let descs: Vec<Option<String>> = {
            let store = world.resource::<lunco_sandbox_edit::catalog::AssetMetaStore>();
            assets
                .iter()
                .map(|a| store.description(&a.asset_path).map(str::to_string))
                .collect()
        };

        let paired: Vec<(&lunco_assets::discovery::AssetFile, &Option<String>)> =
            assets.iter().zip(descs.iter()).collect();

        // Open Twins FIRST as submenus: the twin you have open is
        // the project you are working in, and its scenarios are what you came to
        // the menu for. The engine library is the reference collection below it.
        for name in &twin_names {
            let group: Vec<_> = paired
                .iter()
                .copied()
                .filter(|(a, _)| a.twin.as_deref() == Some(name.as_str()))
                .collect();
            if group.is_empty() {
                continue;
            }
            ui.menu_button(format!("🌍 {name}  ({})", group.len()), |ui| {
                render_scene_buttons(ui, world, &group);
            });
        }

        let library: Vec<_> = paired
            .iter()
            .copied()
            .filter(|(a, _)| a.twin.is_none())
            .collect();
        if !library.is_empty() {
            ui.menu_button(format!("📚 Library  ({})", library.len()), |ui| {
                render_scene_buttons(ui, world, &library);
            });
        }
    });
}

/// Render buttons for loadable scene entry layers.
fn render_scene_buttons(
    ui: &mut bevy_egui::egui::Ui,
    world: &mut World,
    items: &[(&lunco_assets::discovery::AssetFile, &Option<String>)],
) {
    for (asset, desc) in items {
        let label = clean_scene_name(&asset.stem);
        let resp = ui.button(label);
        // Show the plain-language "what is this" blurb on hover. Only scenes
        // carry one (from the spawn catalogue); other types simply show none.
        let resp = match desc {
            Some(d) => resp.on_hover_text(d.as_str()),
            None => resp,
        };
        if resp.clicked() {
            world.trigger(lunco_usd::LoadScene {
                path: asset.asset_path.clone(),
                root_prim: String::new(),
            });
            ui.close();
        }
    }
}

/// Render the "🎓 Tutorials" submenu inside the Scenarios menu. Lists every
/// registered tutorial with a completion tick, a difficulty chip, and its blurb
/// on hover; clicking starts it. Kept next to the scenes list so the menu is the
/// single place to launch either a raw scene or a guided lesson.
fn render_tutorials_submenu(ui: &mut bevy_egui::egui::Ui, world: &mut World) {
    use bevy_egui::egui;

    let registry = world
        .get_resource::<lunco_tutorial::TutorialRegistry>()
        .cloned();
    let progress = world
        .get_resource::<lunco_tutorial::TutorialProgress>()
        .cloned()
        .unwrap_or_default();

    ui.menu_button("🎓 Tutorials", |ui| {
        let Some(registry) = registry else {
            ui.label(
                egui::RichText::new("(tutorials unavailable)")
                    .weak()
                    .italics(),
            );
            return;
        };
        if registry.tutorials.is_empty() {
            ui.label(
                egui::RichText::new("(no tutorials registered)")
                    .weak()
                    .italics(),
            );
            return;
        }

        for meta in registry.ordered() {
            let done = progress.completed.iter().any(|c| c == &meta.id);
            // ✓ completed · 🎓 fresh, then the title and a dim difficulty chip.
            let label = format!(
                "{} {}  ·  {}",
                if done { "✓" } else { "🎓" },
                meta.title,
                meta.difficulty
            );
            let resp = ui.button(label);
            // Hover tip: the plain-language "what this teaches" blurb.
            let resp = resp.on_hover_text(meta.blurb.as_str());
            if resp.clicked() {
                world.trigger(lunco_tutorial::StartTutorial {
                    id: meta.id.to_string(),
                });
                ui.close();
            }
        }
    });
}

fn clean_scene_name(stem: &str) -> String {
    stem.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// WEB: release the [`DeferMslLoad`](lunco_modelica::msl_remote::DeferMslLoad)
/// gate once the terrain has finished its first bake, so MSL loads *after* the
/// terrain instead of contending for the single browser thread during
/// generation. "Baked" = the terrain's [`TerrainGenStatus`] went active (a build
/// started) and then idle (it finished). A timeout is the fallback for a scene
/// that authors no DEM terrain, so MSL still eventually loads.
#[cfg(target_arch = "wasm32")]
fn release_msl_after_terrain(
    time: Res<Time>,
    status: Res<lunco_terrain_surface::TerrainGenStatus>,
    defer: Option<Res<lunco_modelica::msl_remote::DeferMslLoad>>,
    mut seen_active: Local<bool>,
    mut elapsed: Local<f32>,
    mut commands: Commands,
) {
    if defer.is_none() {
        return; // already released
    }
    *elapsed += time.delta_secs();
    if status.active {
        *seen_active = true;
    }
    let terrain_done = *seen_active && !status.active;
    // 45 s fallback: a scene with no DEM terrain never flips `active`, but MSL
    // must still load eventually (the Design tab needs it).
    if terrain_done || *elapsed > 45.0 {
        commands.remove_resource::<lunco_modelica::msl_remote::DeferMslLoad>();
        info!(
            "[sandbox] releasing deferred MSL load ({})",
            if terrain_done {
                "terrain baked"
            } else {
                "timeout"
            }
        );
    }
}
