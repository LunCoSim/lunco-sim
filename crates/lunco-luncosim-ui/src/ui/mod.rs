//! LunCoSim UI layer — everything that draws pixels, opens egui panels, or
//! drives an interactive camera.
//!
//! The entry point is [`LunCoSimUiPlugin`]. The application shell adds it only
//! for a windowed run; the shared simulator core and headless runner do not
//! compile this crate.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lunco_workbench_runtime_ui as runtime_ui;

use lunco_modelica_ui::{ModelicaUiConfig, ModelicaWorkbenchPlugin};
use lunco_usd_bevy_camera::camera_switch::{
    CameraSelectionOwner, CameraSelectionStatus, ObserveAvatar, ResumeCameraDirector,
};
use lunco_workbench_core::scene::{CurrentSceneName, CurrentScenePath};
use lunco_workbench_core::{MenuCtx, WorkbenchMenuRegistry, WorkbenchSnapshot};

/// Surface ⇄ Moon ⇄ Earth view-mode switcher (site-anchored scenes only).
mod celestial_time;
mod code_panel;
/// Generic consent window for missing Twin datasets.
mod dataset_provisioning;
mod models_palette;
/// Which floating viewport overlays are shown (persisted, off by default).
mod overlays;
/// Rhai behaviour editor — edit + save + hot-reload the script on the selected
/// prim, with a diagnostics list. The writable counterpart of `code_panel`.
mod rhai_editor_panel;
/// In-app rhai REPL panel (web + native). Empty unless the API bridge is
/// available — the file carries its own `#![cfg(…)]`.
mod rhai_repl_panel;
/// Explicit production-harness injection for the Scenarios menu failure path.
mod scenario_fixture;
/// Application-owned tutorial catalog menu. Tutorial behavior itself remains
/// authored Rhai and is launched through the generic scripting command.
mod tutorial_menu;
/// Native Velopack update checks and package installation. WASM has no native
/// process/update helper and intentionally does not compile this module.
#[cfg(all(feature = "updates", not(target_arch = "wasm32")))]
mod update;
/// Typed intent emitted by the authored terrain-progress surface.
#[derive(Event, Clone, Debug)]
struct DismissTerrainOverlay;

/// Transient lifecycle for generic authored dropdown controls. The dropdown
/// identity and option records come from the active runtime UI manifest and
/// typed exposure snapshot; this resource only controls presentation state.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
struct RuntimeUiDropdownState {
    open: Option<String>,
    /// Keep the press that opened the dropdown from immediately closing it
    /// when the matching pointer release reaches egui on the next frame.
    ignore_opening_click: bool,
}

impl RuntimeUiDropdownState {
    fn toggle(&mut self, key: &str) {
        if self.open.as_deref() == Some(key) {
            self.close();
        } else {
            self.open = Some(key.to_owned());
            self.ignore_opening_click = true;
        }
    }

    fn close(&mut self) {
        self.open = None;
        self.ignore_opening_click = false;
    }

    fn consume_opening_click(&mut self, clicked: bool) {
        if self.ignore_opening_click && clicked {
            self.ignore_opening_click = false;
        }
    }
}

/// The luncosim's interactive layer: egui workbench, bevy_picking, the USD Twin
/// browser + RTT viewport, the in-scene editor, materials, rover panels, and
/// explicit camera presentation controls.
///
/// Added by the app shell only for a windowed run. A headless server runs the
/// sim, physics, scene, cosim, and networking host (all in `LunCoSimCorePlugin`)
/// *without* any of this — headless mode omits the renderer and keeps only the
/// simulation-facing asset/type plugins, so nothing here (GPU / window / pointer)
/// is wired.
/// Initial scene request supplied by the application composition root.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct InitialScenePath(pub Option<String>);

/// Window icon bytes prepared by this crate's UI build script.
///
/// Packaging owns rasterization because the icon is also used for desktop
/// metadata. This crate owns only installing it on the live native window.
#[derive(Resource, Debug, Clone, Copy)]
pub struct WindowIconBytes(pub &'static [u8]);

/// Host metadata and initial presentation input for [`LunCoSimUiPlugin`].
#[derive(Debug, Clone)]
pub struct LunCoSimUiConfig {
    /// Product version stamped by the application build.
    pub product_version: &'static str,
    /// Source revision stamped by the application build.
    pub git_sha: &'static str,
    /// Public source repository for the stamped revision.
    pub repository_url: &'static str,
    /// Optional scene selected by the application's startup policy.
    pub initial_scene: Option<String>,
}

/// Interactive presentation for the luncosim application.
pub struct LunCoSimUiPlugin {
    /// Host metadata and the initial scene presentation request.
    pub config: LunCoSimUiConfig,
}

/// Install the retained runtime-authored HTML surface layer.
///
/// This is deliberately shared by the interactive workbench and the GPU
/// windowless recorder. The latter has no egui host, but it still has a real
/// Bevy UI render pass and a scene camera, so authored HUDs must use the same
/// HUI/Flair and exposure path in both modes.
pub fn add_runtime_ui_layer(app: &mut App) {
    app.add_plugins(runtime_ui::RuntimeUiPlugin)
        .add_systems(Update, sync_runtime_ui_capture_state)
        .add_systems(
            Update,
            update_runtime_ui_gates.before(runtime_ui::mount_runtime_ui_surfaces),
        );
}

fn sync_runtime_ui_capture_state(
    recording: Option<Res<lunco_capture::screenshot::OfflineRecordingState>>,
    mut state: ResMut<runtime_ui::RuntimeUiCaptureState>,
) {
    let active = recording.is_some_and(|recording| recording.active);
    if state.active != active {
        state.active = active;
    }
}

impl Plugin for LunCoSimUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_embodiment_core::roles::EmbodimentCorePlugin>() {
            app.add_plugins(lunco_embodiment_core::roles::EmbodimentCorePlugin);
        }
        app.insert_resource(lunco_workbench::BuildIdentity::new(
            self.config.product_version,
            self.config.git_sha,
            self.config.repository_url,
        ));
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, install_window_icon);
        app.insert_resource(InitialScenePath(self.config.initial_scene.clone()));
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
        if networked || no_throttle {
            // `ModelicaPlugin::sim_focus_pace` is the last writer of
            // `WinitSettings::unfocused_mode`. Keep the CLI's continuous-rate
            // contract in the shared pacing plane so that pacer cannot restore
            // reactive-low-power after the scene becomes idle.
            app.init_resource::<lunco_core::KeepAwake>();
            app.world_mut()
                .resource_mut::<lunco_core::KeepAwake>()
                .acquire();
        }
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

        add_runtime_ui_layer(app);
        scenario_fixture::install(app);
        crate::register_save_scenario_command(app);
        // A windowed host requires a presentation contract. Authored tracks and
        // LocalEmbodiment cameras remain authoritative; the USD projection may add
        // one Twin-scoped presentation camera/light for a standalone assembly
        // that has neither authored initial presentation.
        app.world_mut()
            .resource_mut::<lunco_usd_bevy_camera::camera_switch::CameraContractStatus>()
            .required = true;
        app.world_mut()
            .resource_mut::<lunco_usd_bevy_camera::camera_switch::StandalonePresentationState>()
            .enabled = true;
        app.init_resource::<RuntimeUiDropdownState>()
            .add_systems(lunco_core::SceneTeardown, reset_runtime_ui_dropdowns)
            .add_observer(reset_runtime_ui_dropdowns_on_twin_closed)
            .init_resource::<dataset_provisioning::DatasetProvisioningState>()
            .add_observer(dataset_provisioning::on_dataset_scope_ready)
            .add_observer(dataset_provisioning::on_dataset_scope_removed)
            .add_systems(Update, dataset_provisioning::poll_dataset_provisioning);
        app.add_plugins(bevy::pbr::wireframe::WireframePlugin::default())
            // bevy_picking's mesh backend: makes visible Mesh3d entities pickable,
            // so scene selection / possession / spawn-placement run as click observers.
            .add_plugins(bevy::picking::mesh_picking::MeshPickingPlugin)
            .add_plugins(lunco_workbench::WorkbenchPlugin)
            .add_plugins(lunco_workbench_guided_ui::GuidedOverlayPlugin)
            .add_plugins(lunco_workbench_browser::TwinBrowserPlugin);
        #[cfg(feature = "avatar-ui")]
        app.add_plugins(lunco_avatar_ui::AvatarUiPlugin);
        // An explicit scene launch is a presentation request for the simulator:
        // restore the scene/3D View even if this Twin was last closed in the
        // embedded Modelica workspace. The workspace owner consumes this once;
        // subsequent Twin switches still restore their own saved perspectives.
        let has_explicit_scene = app
            .world()
            .get_resource::<InitialScenePath>()
            .is_some_and(|scene| scene.0.is_some());
        if has_explicit_scene {
            app.insert_resource(
                lunco_workbench_state::WorkspaceStateRestorePolicy::with_initial_perspective(
                    "sandbox_view",
                ),
            );
        }
        #[cfg(all(feature = "updates", not(target_arch = "wasm32")))]
        app.add_plugins(update::UpdatePlugin);
        if args.iter().any(|arg| arg == "--windowed-ui") {
            app.insert_resource(lunco_workbench::OfflineRecordingPresentation {
                retain_workbench_chrome: true,
            });
        }
        app.add_systems(
            lunco_core::SceneTeardown,
            lunco_render_recovery::reset_render_recovery,
        );
        app.add_plugins(overlays::plugin)
            // Overlay visibility prefs + the Time-menu rows that drive them.
            // USD Twin browser plus the explicit Editor preview. The preview
            // owns a private, render-layer-isolated projection only after the
            // user selects a document in the Twin Browser; it never auto-mounts
            // the simulation's default scene and therefore cannot duplicate the
            // live world.
            .add_plugins(lunco_usd_viewport_runtime::UsdViewportPlugin)
            .add_plugins(lunco_usd_viewport_ui::UsdViewportUiPlugin)
            .add_plugins(lunco_usd_ui::UsdUiPlugin)
            .add_plugins(lunco_luncosim_edit_core::SceneEditPlugin)
            .add_plugins(lunco_luncosim_edit_ui::ui::SceneEditUiPlugin)
            .add_plugins(lunco_luncosim_edit_inspector_ui::SceneEditInspectorUiPlugin)
            .add_plugins(lunco_usd_prim_tree_ui::UsdPrimTreeUiPlugin)
            // NOTE: `ShaderMaterialPlugin` (the dynamic `ShaderMaterial` render
            // pipeline) used to be added here. It now lives inside
            // `lunco_render_bevy::LuncoRenderPlugin` — the one crate that may name
            // `bevy_pbr` — and adding it a second time panics Bevy.
            // See docs/architecture/render-decoupling.md.
            .add_plugins(tutorial_menu::TutorialMenuPlugin)
            // Rover panels. ONE closure: Bevy keys plugin uniqueness by type-name,
            // and every `|app| {…}` in this `build` shares the name `{{closure}}` — a
            // second one panics ("plugin already added"). So all app-level panel
            // registration goes here.
            .add_plugins(|app: &mut App| {
                use lunco_workbench_core::WorkbenchPanelAppExt;
                app.add_observer(on_runtime_ui_action)
                    .add_observer(on_dismiss_terrain_overlay)
                    .add_observer(dataset_provisioning::on_set_missing_asset_prompt_suppressed);
                // Rover-specific panels and the attach-a-model click flow.
                app.register_panel(code_panel::CodePanel);
                // Rhai behaviour editor (Editor). Its view-model is
                // produced each frame from the selection + ScriptRegistry.
                app.register_panel(rhai_editor_panel::RhaiEditorPanel);
                app.init_resource::<rhai_editor_panel::RhaiEditorVm>();
                app.add_systems(Update, rhai_editor_panel::produce_rhai_editor_vm);
                app.register_panel(models_palette::ModelsPalette);
                app.init_resource::<models_palette::ProgramCatalog>();
                app.add_systems(
                    Update,
                    models_palette::drain_program_catalog
                        .after(lunco_scene_catalog::catalog::drain_catalog_listing),
                );
                app.add_systems(
                    Update,
                    models_palette::sync_program_contracts
                        .after(models_palette::drain_program_catalog),
                );
                app.add_observer(models_palette::clear_program_catalog_on_twin_closed);
                // In-app rhai REPL — runs snippets against the live app through the
                // API bridge, on web + native. Gated on bridge availability.
                #[cfg(any(target_arch = "wasm32", feature = "transport-http"))]
                app.register_panel(rhai_repl_panel::RhaiReplPanel::default());
                app.init_resource::<models_palette::AttachState>();
                // Disarm on scene teardown — see `AttachState`.
                app.add_systems(
                    lunco_core::SceneTeardown,
                    |mut attach: ResMut<models_palette::AttachState>| {
                        if *attach != models_palette::AttachState::Idle {
                            *attach = models_palette::AttachState::Idle;
                        }
                    },
                );
                // Attach is bevy_picking-driven (observes the same `Pointer<Click>`
                // as selection; egui occlusion handled by the framework).
                app.add_observer(models_palette::on_scene_click_attach);
                app.add_systems(Update, models_palette::attach_escape_system);
            })
            .add_systems(
                Startup,
                (
                    init_current_scene_path,
                    register_sandbox_scenarios_menu,
                    register_camera_menu,
                    register_downloadable_assets_settings,
                ),
            )
            .add_observer(
                |t: On<lunco_usd_bevy_runtime_core::scene::LoadScene>,
                 current: Option<ResMut<CurrentScenePath>>,
                 current_name: Option<ResMut<CurrentSceneName>>,
                 hud: Option<ResMut<lunco_workbench_guided_ui::GuidedOverlay>>| {
                    if let Some(mut current) = current {
                        current.0 = t.event().path.clone();
                    }
                    if let Some(mut name) = current_name {
                        name.0 = std::path::Path::new(&t.event().path)
                            .file_name()
                            .and_then(|f| f.to_str())
                            .unwrap_or(&t.event().path)
                            .to_string();
                    }
                    // The overlay belongs to the scene that was on screen. A
                    // scene switch leaves hints, objectives, a spotlight ring or
                    // a half-finished coach card pointing at entities that no
                    // longer exist, and the "continue to the next lesson?" popup
                    // floating over a world it was never about.
                    //
                    // Cleared HERE — synchronously, on the LoadScene TRIGGER —
                    // rather than from a change-detection system: a lesson's
                    // `on_start` calls `load_scene` FIRST and then publishes its
                    // own hint/coach step, so anything that ran a frame later
                    // would wipe the incoming lesson's overlay instead of the
                    // outgoing one's. A still-running mission re-publishes its
                    // objectives on the next tick, so only stale state is lost.
                    if let Some(mut hud) = hud {
                        hud.hint.clear();
                        hud.objectives.clear();
                        hud.spotlight = None;
                        hud.tour = None;
                    }
                },
            )
            .add_observer(
                |_t: On<lunco_core::SceneTransitionStarted>,
                 mut current: ResMut<CurrentScenePath>,
                 mut current_name: ResMut<CurrentSceneName>| {
                    if matches!(_t.event().transition, lunco_core::SceneTransition::Clear) {
                        current.0.clear();
                        current_name.0.clear();
                    }
                },
            )
            // The sky clock remains native egui because the deliberately minimal
            // HUI contract has no equivalent text-input semantics for its UTC seek
            // field. Its state still flows through the typed SetClock command.
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (
                    celestial_time::draw_celestial_time
                        .in_set(lunco_workbench_core::ApplicationOverlayRenderSet)
                        .run_if(not(recording_offline))
                        .run_if(in_view_perspective)
                        .run_if(overlays::sky_clock_visible),
                    draw_runtime_ui_dropdowns
                        .in_set(lunco_workbench_core::ApplicationOverlayRenderSet)
                        .run_if(not(recording_offline)),
                ),
            );

        // Embed the FULL lunica workbench as the "Design" workspace via the
        // shared bundle — same clipboard bridge, autosave, worker, and panels
        // as standalone lunica, so the Design tab can't drift from the real
        // IDE. We pass only the one intentional embed knob: suppress the
        // first-run help overlay (lunica's onboarding coach-marks, out of
        // place inside a 3D physics demo). Welcome panel stays ON — it's the
        // same landing page lunica uses for the Design tab.
        app.add_plugins(ModelicaWorkbenchPlugin {
            config: ModelicaUiConfig {
                include_welcome_panel: true,
            },
        });

        // Forced window placement (`--window-pos`). Parses the flag and (when
        // present) inserts the resource, suppresses geometry persistence, and
        // registers the placer system — all in `lunco-workbench` so any binary
        // gets the same behaviour.
        lunco_workbench_window::wire_window_placement(app, &args);

        // URL-driven boot (wasm). Lets headless test harnesses drive the
        // workbench without firing canvas pointer events. See
        // [`luncosim_boot_from_url`].
        #[cfg(target_arch = "wasm32")]
        app.add_systems(bevy::prelude::Update, luncosim_boot_from_url);
    }
}

fn update_runtime_ui_gates(
    layout: Option<Res<WorkbenchSnapshot>>,
    overlays: Option<Res<overlays::OverlaySettings>>,
    recording: Option<Res<lunco_capture::screenshot::OfflineRecordingState>>,
    mut gates: ResMut<runtime_ui::RuntimeUiGates>,
    mut initialized: Local<bool>,
) {
    let changed = !*initialized
        || layout.as_ref().is_some_and(|value| value.is_changed())
        || overlays.as_ref().is_some_and(|value| value.is_changed())
        || recording.as_ref().is_some_and(|value| value.is_changed());
    if !changed {
        return;
    }
    *initialized = true;
    let in_view = layout.is_some_and(|value| {
        value.active_perspective() == Some(lunco_workbench_core::PerspectiveId("sandbox_view"))
    });
    let overlay_enabled = overlays.is_some_and(|value| value.view_switcher);
    let recording = recording.is_some_and(|value| value.active);
    gates.set("view_switcher", in_view && overlay_enabled && !recording);
}

fn recording_offline(
    recording: Option<Res<lunco_capture::screenshot::OfflineRecordingState>>,
) -> bool {
    recording.is_some_and(|recording| recording.active)
}

fn in_view_perspective(layout: Option<Res<WorkbenchSnapshot>>) -> bool {
    layout.is_some_and(|layout| {
        layout.active_perspective() == Some(lunco_workbench_core::PerspectiveId("sandbox_view"))
    })
}

fn on_runtime_ui_action(
    trigger: On<runtime_ui::RuntimeUiAction>,
    q_avatar: Query<
        Entity,
        (
            With<lunco_embodiment_core::roles::Embodiment>,
            With<lunco_embodiment_core::roles::LocalEmbodiment>,
        ),
    >,
    q_bodies: Query<(Entity, &lunco_core::CelestialBody)>,
    orbital_pin: Option<Res<lunco_celestial_spatial_core::OrbitalViewPin>>,
    manifest_state: Res<runtime_ui::RuntimeUiManifestState>,
    mut dropdowns: ResMut<RuntimeUiDropdownState>,
    mut commands: Commands,
) {
    match trigger.event().action.as_str() {
        "view.surface" => {
            if !orbital_pin.is_some_and(|pin| pin.active) {
                return;
            }
            if let Ok(target) = q_avatar.single() {
                commands.trigger(lunco_camera_core::ReturnFromOrbit { camera: target });
            }
        }
        "view.body.moon" => {
            if !runtime_focus_body(
                lunco_celestial::ephemeris_id::MOON,
                &q_bodies,
                &mut commands,
            ) {
                report_runtime_ui_failure(&mut commands, "Moon is not present in the loaded scene");
            }
        }
        "view.body.earth" => {
            if !runtime_focus_body(
                lunco_celestial::ephemeris_id::EARTH,
                &q_bodies,
                &mut commands,
            ) {
                report_runtime_ui_failure(
                    &mut commands,
                    "Earth is not present in the loaded scene",
                );
            }
        }
        "overlay.terrain.dismiss" => commands.trigger(DismissTerrainOverlay),
        action => {
            // The UI bridge remains domain-neutral. A Twin/Rhai program owns
            // the meaning of an authored action and reaches USD or simulation
            // state through the normal typed command/query/event surface.
            if action.trim().is_empty() {
                return;
            }
            let action = action.to_owned();
            if let Some(key) = manifest_state.dropdown_key_for_action(&action) {
                dropdowns.toggle(&key);
                return;
            }
            commands.trigger(lunco_scripting::commands::RunRhaiToolHook {
                tool: "runtime_ui".to_owned(),
                hook: "on_action".to_owned(),
                args: lunco_core::TelemetryValue::String(action.clone()),
            });
            commands.trigger(lunco_core::TelemetryEvent {
                name: "runtime.ui.action".to_owned(),
                source: 0,
                severity: lunco_core::Severity::Info,
                data: lunco_core::TelemetryValue::String(action),
                timestamp: 0.0,
            });
        }
    }
}

fn runtime_focus_body(
    ephemeris_id: i32,
    q_bodies: &Query<(Entity, &lunco_core::CelestialBody)>,
    commands: &mut Commands,
) -> bool {
    if let Some((target, _)) = q_bodies
        .iter()
        .find(|(_, body)| body.ephemeris_id == ephemeris_id)
    {
        commands.trigger(lunco_camera_core::FocusTarget {
            camera: None,
            target,
        });
        true
    } else {
        false
    }
}

fn report_runtime_ui_failure(commands: &mut Commands, message: &str) {
    warn!("[runtime-ui] {message}");
    lunco_core::trigger_error(commands, "runtime-ui-action-failed", message);
}

fn reset_runtime_ui_dropdowns(mut dropdowns: ResMut<RuntimeUiDropdownState>) {
    dropdowns.close();
}

fn reset_runtime_ui_dropdowns_on_twin_closed(
    _trigger: On<lunco_workspace::TwinClosed>,
    mut dropdowns: ResMut<RuntimeUiDropdownState>,
) {
    dropdowns.close();
}

/// Labels for the native Camera menu remain authored camera-domain commands.
/// The in-scene dropdown below is generic and reads its option records from the
/// runtime exposure snapshot.
const CAMERA_OBSERVE_AVATAR: &str = "Observe avatar";
const CAMERA_RESUME_DIRECTOR: &str = "Resume authored director";

fn runtime_ui_dropdown_options(
    ui: &mut egui::Ui,
    exposure: &lunco_core::exposure::ExposureSurface,
    definition: &runtime_ui::RuntimeUiDropdownDefinition,
    selected_key: Option<&str>,
) -> Option<String> {
    let Some(lunco_core::exposure::ExposureValue::Array(values)) =
        exposure.properties.get(&definition.source)
    else {
        ui.label("No authored options are available.");
        return None;
    };

    let mut selected = None;
    for value in values {
        let Some(fields) = runtime_ui::collection_item_fields(value, &definition.key) else {
            continue;
        };
        let Some(key) = fields
            .iter()
            .find(|(name, _)| name == &definition.key)
            .map(|(_, value)| value.as_str())
        else {
            continue;
        };
        let Some(label) = fields
            .iter()
            .find(|(name, _)| name == &definition.label)
            .map(|(_, value)| value.as_str())
        else {
            continue;
        };
        let Some(action) = fields
            .iter()
            .find(|(name, _)| name == &definition.action)
            .map(|(_, value)| value.as_str())
        else {
            continue;
        };
        if ui
            .selectable_label(selected_key == Some(key), label)
            .on_hover_text(key)
            .clicked()
        {
            selected = Some(action.to_owned());
        }
    }
    selected
}

fn runtime_ui_dimension(
    exposure: &lunco_core::exposure::ExposureSurface,
    source: &str,
) -> Option<f32> {
    let value = exposure.properties.get(source)?;
    let pixels = match value {
        lunco_core::exposure::ExposureValue::Number(value) => *value as f32,
        lunco_core::exposure::ExposureValue::Text(value) => value
            .trim()
            .strip_suffix("px")?
            .trim()
            .parse::<f32>()
            .ok()?,
        _ => return None,
    };
    pixels
        .is_finite()
        .then_some(pixels)
        .filter(|value| *value > 0.0)
}

/// Draw every open authored dropdown. This is a generic presentation mechanic:
/// the active manifest supplies the record field names and Rhai supplies the
/// records, actions, and dimensions. No camera or other domain state is read.
fn draw_runtime_ui_dropdowns(
    mut egui_ctx: EguiContexts,
    mut dropdowns: ResMut<RuntimeUiDropdownState>,
    exposures: Res<lunco_core::exposure::EngineExposures>,
    roots: Query<(&runtime_ui::RuntimeUiSurface, &Visibility)>,
    manifest_state: Res<runtime_ui::RuntimeUiManifestState>,
    layout: Option<Res<WorkbenchSnapshot>>,
    theme: Option<Res<lunco_theme::Theme>>,
    mut commands: Commands,
) {
    if !layout.is_some_and(|layout| {
        layout.active_perspective() == Some(lunco_workbench_core::PerspectiveId("sandbox_view"))
    }) {
        dropdowns.close();
        return;
    }
    let Some(open_key) = dropdowns.open.clone() else {
        return;
    };
    let Some(manifest) = manifest_state.manifest() else {
        dropdowns.close();
        return;
    };
    let Some((surface_definition, definition)) = manifest.dropdown_for_key(&open_key) else {
        dropdowns.close();
        return;
    };
    let Some(exposure) = exposures.surfaces.get(&surface_definition.namespace) else {
        dropdowns.close();
        return;
    };
    let Some(anchor) = roots.iter().find_map(|(surface, visibility)| {
        (surface.namespace() == surface_definition.namespace
            && matches!(*visibility, Visibility::Visible)
            && surface.is_mounted())
        .then(|| surface.applied_rect())
        .flatten()
    }) else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };

    let theme = theme
        .map(|theme| theme.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let popup_id = egui::Id::new(("runtime_ui_dropdown", open_key.as_str()));
    let mut open = true;
    let ignore_opening_click = dropdowns.ignore_opening_click;
    let selected_key = definition
        .selected_source
        .as_deref()
        .and_then(|source| exposure.properties.get(source))
        .and_then(lunco_core::exposure::ExposureValue::scalar_render);
    let width = runtime_ui_dimension(exposure, &definition.width_source);
    let max_height = runtime_ui_dimension(exposure, &definition.max_height_source);
    let (Some(width), Some(max_height)) = (width, max_height) else {
        report_runtime_ui_failure(
            &mut commands,
            "authored dropdown dimensions are unavailable or invalid",
        );
        dropdowns.close();
        return;
    };
    let mut selected_action = None;

    egui::Popup::new(
        popup_id,
        ctx.clone(),
        anchor,
        egui::LayerId::new(egui::Order::Foreground, popup_id),
    )
    .align(egui::RectAlign::BOTTOM_START)
    .gap(6.0)
    .open_bool(&mut open)
    .close_behavior(if ignore_opening_click {
        egui::PopupCloseBehavior::IgnoreClicks
    } else {
        egui::PopupCloseBehavior::CloseOnClickOutside
    })
    .layout(egui::Layout::top_down(egui::Align::Min))
    .frame(
        egui::Frame::new()
            .fill(theme.tokens.overlay_backdrop)
            .stroke(egui::Stroke::new(1.0, theme.tokens.overlay_border))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(8)),
    )
    .show(|ui| {
        let viewport_width = ctx.content_rect().width().max(1.0);
        ui.set_width(width.min(viewport_width));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .show(ui, |ui| {
                selected_action =
                    runtime_ui_dropdown_options(ui, exposure, definition, selected_key.as_deref());
            });
    });

    if selected_action.is_some() {
        open = false;
    }
    if !open {
        dropdowns.close();
    }
    dropdowns.consume_opening_click(ctx.input(|input| input.pointer.any_click()));
    if !open {
        dropdowns.close();
    }
    if let Some(action) = selected_action {
        commands.trigger(lunco_core::TelemetryEvent {
            name: "runtime.ui.action".to_owned(),
            source: 0,
            severity: lunco_core::Severity::Info,
            data: lunco_core::TelemetryValue::String(action),
            timestamp: 0.0,
        });
    }
}

/// Register an egui-hosted, keyboard-accessible route to the same semantic
/// camera actions used by the retained 3D overlay. HUI remains the compact
/// in-viewport presentation; it is not the sole way to operate an essential
/// camera mode because retained HTML currently exposes no accessibility tree.
fn register_camera_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_custom_menu("Camera", |ui, ctx| {
        let state = ctx.resource::<CameraSelectionStatus>().cloned();
        ui.label("Presentation");
        if let Some(state) = &state {
            let active = state.active_name.as_deref().unwrap_or("none");
            let owner = match state.owner {
                CameraSelectionOwner::None => "none",
                CameraSelectionOwner::Director => "director",
                CameraSelectionOwner::User => "operator",
                CameraSelectionOwner::Policy => "policy",
                CameraSelectionOwner::Generated => "generated",
            };
            ui.label(format!("Active: {active}  ·  Owner: {owner}"));
            if let Some(error) = &state.last_error {
                ui.colored_label(bevy_egui::egui::Color32::from_rgb(235, 130, 130), error);
            }
            if let Some(contract) =
                ctx.resource::<lunco_usd_bevy_camera::camera_switch::CameraContractStatus>()
            {
                for error in &contract.errors {
                    ui.colored_label(bevy_egui::egui::Color32::from_rgb(255, 110, 110), error);
                }
            }
            if state.avatar_available && ui.button(CAMERA_OBSERVE_AVATAR).clicked() {
                ctx.trigger(ObserveAvatar {});
                ui.close();
            }
            if state.director_available && ui.button(CAMERA_RESUME_DIRECTOR).clicked() {
                ctx.trigger(ResumeCameraDirector {});
                ui.close();
            }
            ui.separator();
            let dropdown = ctx
                .resource::<runtime_ui::RuntimeUiManifestState>()
                .and_then(|state| state.manifest())
                .and_then(|manifest| {
                    manifest
                        .surfaces
                        .iter()
                        .find(|surface| surface.namespace == "camera-status")
                        .and_then(|surface| surface.dropdowns.first())
                });
            let exposure = ctx
                .resource::<lunco_core::exposure::EngineExposures>()
                .and_then(|exposures| exposures.surfaces.get("camera-status"));
            if let (Some(dropdown), Some(exposure)) = (dropdown, exposure) {
                let selected_key = dropdown
                    .selected_source
                    .as_deref()
                    .and_then(|source| exposure.properties.get(source))
                    .and_then(lunco_core::exposure::ExposureValue::scalar_render);
                if let Some(action) =
                    runtime_ui_dropdown_options(ui, exposure, dropdown, selected_key.as_deref())
                {
                    ctx.trigger(runtime_ui::RuntimeUiAction {
                        action,
                        source: Entity::PLACEHOLDER,
                    });
                    ui.close();
                }
            }
        } else {
            ui.label("Camera state is not ready.");
        }
        let actions = [
            ("Surface view", "view.surface"),
            ("Orbit Moon", "view.body.moon"),
            ("Orbit Earth", "view.body.earth"),
        ];
        for (label, action) in actions {
            if ui.button(label).clicked() {
                ctx.trigger(runtime_ui::RuntimeUiAction {
                    action: action.to_owned(),
                    source: Entity::PLACEHOLDER,
                });
                ui.close();
            }
        }
        overlays::camera_menu_ui(ui, ctx);
    });
}

fn on_dismiss_terrain_overlay(
    _trigger: On<DismissTerrainOverlay>,
    mut status: ResMut<lunco_terrain_surface::TerrainGenStatus>,
) {
    status.user_dismissed = true;
    status.active = false;
}

// ── wasm URL-driven boot ──────────────────────────────────────────────────────

/// State machine for [`luncosim_boot_from_url`].
///
/// Lives in a `Local` so the boot work happens exactly once per app
/// lifetime — once `open_class` is satisfied the system runs and
/// no-ops in O(1).
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct LunCoSimBootState {
    parsed: bool,
    workspace: Option<String>,
    open_class: Option<String>,
    done: bool,
}

/// wasm-only `Update` system that reads `window.location.search` and:
///   - activates the perspective named by `?workspace=…` (once, on
///     first run);
///   - triggers an `OpenClass` for `?open=…` once `LibraryLoadState`
///     reaches `Ready`. Without that gate the trigger races source library
///     install and the workbench can't find the class.
///
/// Self-disables after both are applied. Useful for headless test
/// harnesses (e.g. `chrome-devtools-mcp`) which can't drive the egui
/// canvas via synthetic DOM events.
#[cfg(target_arch = "wasm32")]
fn luncosim_boot_from_url(
    mut commands: bevy::prelude::Commands,
    library: Option<bevy::prelude::Res<lunco_assets_core::library::LibraryLoadState>>,
    mut state: bevy::prelude::Local<LunCoSimBootState>,
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
        if let Some(ws) = state.workspace.as_ref() {
            let id: &'static str = Box::leak(ws.clone().into_boxed_str());
            commands
                .trigger(lunco_workbench_core::commands::ActivatePerspective { id: ws.clone() });
            bevy::log::info!("[luncosim_boot_from_url] activated perspective `{ws}`");
        }
        state.parsed = true;
    }

    // ── Per-frame poll: dispatch OpenClass once source library is ready ─────
    if let Some(qual) = state.open_class.clone() {
        let ready = matches!(
            library.as_deref(),
            Some(lunco_assets_core::library::LibraryLoadState::Ready { .. })
        );
        if !ready {
            return;
        }
        commands.trigger(lunco_modelica_ui_core::OpenClass {
            qualified: qual.clone(),
            ..Default::default()
        });
        bevy::log::info!(
            "[luncosim_boot_from_url] OpenClass({qual}) triggered (source library ready)"
        );
    }
    state.done = true;
}

fn init_current_scene_path(
    scene_path: Res<InitialScenePath>,
    mut current: ResMut<CurrentScenePath>,
    current_name: Option<ResMut<CurrentSceneName>>,
) {
    if let Some(path) = scene_path.0.as_deref() {
        current.0 = path.to_string();
        if let Some(mut name) = current_name {
            name.0 = std::path::Path::new(path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(path)
                .to_string();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn install_window_icon(
    windows: Query<Entity, With<bevy::window::PrimaryWindow>>,
    winit_windows: Option<NonSend<bevy_winit::WinitWindows>>,
    icon: Option<Res<WindowIconBytes>>,
    mut installed: Local<bool>,
) {
    if *installed {
        return;
    }
    let Some(winit_windows) = winit_windows else {
        return;
    };
    let Some(icon_bytes) = icon else {
        return;
    };
    for entity in &windows {
        let Some(window) = winit_windows.get_window(entity) else {
            continue;
        };
        match winit::window::Icon::from_rgba(icon_bytes.0.to_vec(), 64, 64) {
            Ok(icon) => {
                window.set_window_icon(Some(icon));
                *installed = true;
                info!("[window] installed LunCoSim icon");
            }
            Err(error) => warn!("[window] failed to install LunCoSim icon: {error}"),
        }
    }
}

/// Settings ▸ downloadable data — the generic view over
/// [`lunco_assets_core::datasets`].
///
/// The app never reaches the network on its own: every fetchable dataset is
/// DECLARED in an `Assets.toml` (a crate's, or an open Twin's) and downloaded
/// only from a click here. This panel knows nothing about ephemerides, terrain
/// or source library — it renders whatever the registry reports, so a new dataset needs a
/// manifest entry and no UI change at all.
fn register_downloadable_assets_settings(world: &mut World) {
    use bevy_egui::egui;
    use lunco_assets_datasets::{DatasetRegistry, DatasetState};
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_settings_submenu("Data & libraries", |ui, ctx| {
        ui.label(egui::RichText::new("Downloadable data").weak().small());
        let Some(mut settings) = ctx.resource::<lunco_settings::DownloadSettings>().cloned() else {
            return;
        };
        let original_settings = settings.clone();
        {
            ui.horizontal(|ui| {
                ui.label("Max parallel downloads:");
                ui.add(egui::Slider::new(
                    &mut settings.max_parallel_downloads,
                    lunco_settings::DownloadSettings::MAX_PARALLEL_DOWNLOADS_RANGE,
                ));
            });
            ui.horizontal(|ui| {
                ui.label("Max attempts per download:");
                ui.add(egui::Slider::new(
                    &mut settings.max_attempts,
                    lunco_settings::DownloadSettings::MAX_ATTEMPTS_RANGE,
                ));
            });
            ui.horizontal(|ui| {
                ui.label("Initial retry delay (seconds):");
                ui.add(egui::Slider::new(
                    &mut settings.retry_initial_delay_secs,
                    lunco_settings::DownloadSettings::RETRY_INITIAL_DELAY_SECS_RANGE,
                ));
            });
            ui.horizontal(|ui| {
                ui.label("Retry backoff multiplier:");
                ui.add(egui::Slider::new(
                    &mut settings.retry_backoff_multiplier,
                    lunco_settings::DownloadSettings::RETRY_BACKOFF_MULTIPLIER_RANGE,
                ));
            });
            ui.horizontal(|ui| {
                ui.label("Maximum retry delay (seconds):");
                ui.add(egui::Slider::new(
                    &mut settings.retry_max_delay_secs,
                    lunco_settings::DownloadSettings::RETRY_MAX_DELAY_SECS_RANGE,
                ));
            });
            settings.retry_max_delay_secs = settings
                .retry_max_delay_secs
                .max(settings.retry_initial_delay_secs);
            ui.label(
                egui::RichText::new(
                    "The same bounded policy is used for assets, source library, scenario HTTP, and app updates. Attempts include the first request; delays grow exponentially and stop at the configured maximum.",
                )
                .weak()
                .small(),
            );
            ui.add_space(8.0);
        }
        if let Some((root, suppressed)) = dataset_provisioning::active_project_prompt_setting(
            ctx.resource::<lunco_workspace::WorkspaceResource>(),
        ) {
            let mut suppressed = suppressed;
            if ui
                .checkbox(
                    &mut suppressed,
                    "Don't show missing-asset prompt at startup for this project",
                )
                .changed()
            {
                ctx.trigger(dataset_provisioning::SetMissingAssetPromptSuppressed {
                    root,
                    suppressed,
                });
            }
        } else {
            ui.label(
                egui::RichText::new("Open a project to configure its missing-asset prompt")
                    .weak()
                    .italics(),
            );
        }
        ui.add_space(8.0);
        if settings != original_settings {
            ctx.set_resource(settings);
        }
        let Some(registry) = ctx.resource::<DatasetRegistry>() else {
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
        // Snapshot: the rows below emit a typed request after painting.
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
                    lunco_assets_datasets::DatasetScope::Engine => e.group.clone(),
                    lunco_assets_datasets::DatasetScope::Twin { name, .. } => name.clone(),
                };
                (e.id.clone(), owner, e.name.clone(), e.state.clone())
            })
            .collect();
        // Registration order already groups by owner; sorting makes that a
        // guarantee rather than a coincidence, so the headings below can be
        // emitted on change instead of buffering the whole list.
        let mut rows = rows;
        rows.sort_by(|a, b| (&a.1, &a.2).cmp(&(&b.1, &b.2)));
        enum DatasetAction {
            Request(String),
            Cancel(String),
        }
        let mut requested: Option<DatasetAction> = None;
        // One stable section per existing owner keeps Settings compact without
        // inventing a second categorisation system for generic datasets.
        let mut index = 0;
        while index < rows.len() {
            let owner = rows[index].1.clone();
            let start = index;
            while index < rows.len() && rows[index].1 == owner {
                index += 1;
            }
            let slice = &rows[start..index];
            let installed = slice
                .iter()
                .filter(|row| matches!(row.3, DatasetState::Installed))
                .count();
            ui.add_space(4.0);
            egui::CollapsingHeader::new(format!("{} ({}/{})", owner, installed, slice.len()))
                .id_salt(("download-owner", owner.as_str()))
                .default_open(
                    slice
                        .iter()
                        .any(|row| !matches!(row.3, DatasetState::Installed)),
                )
                .show(ui, |ui| {
                    for (key, _, name, state) in slice {
                        ui.horizontal(|ui| {
                            ui.label(name);
                            match state {
                                DatasetState::Installed => {
                                    let (icon_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(18.0, 18.0),
                                        egui::Sense::hover(),
                                    );
                                    lunco_workbench_widgets::paint_icon(
                                        ui.painter(),
                                        lunco_workbench_widgets::UiIcon::Check,
                                        icon_rect,
                                        ui.visuals().weak_text_color(),
                                    );
                                    ui.label(egui::RichText::new("ready · cached").weak());
                                }
                                DatasetState::Downloading {
                                    bytes_done,
                                    bytes_total,
                                } => {
                                    if *bytes_total == 0 {
                                        ui.label("Downloading…");
                                    } else {
                                        ui.label(format!(
                                            "Downloading {:.1}/{:.1} MB",
                                            *bytes_done as f64 / 1_048_576.0,
                                            *bytes_total as f64 / 1_048_576.0
                                        ));
                                    }
                                    if lunco_workbench_widgets::icon_text_button(
                                        ui,
                                        lunco_workbench_widgets::UiIcon::Stop,
                                        "Cancel",
                                        "Cancel dataset download",
                                    )
                                    .clicked()
                                    {
                                        requested = Some(DatasetAction::Cancel(key.clone()));
                                    }
                                }
                                DatasetState::Processing { kind } => {
                                    ui.label(format!("Processing {kind}…"));
                                }
                                DatasetState::Cancelling => {
                                    ui.label("Stopping…");
                                }
                                DatasetState::Cancelled => {
                                    ui.label(egui::RichText::new("Cancelled").weak());
                                    if lunco_workbench_widgets::icon_text_button(
                                        ui,
                                        lunco_workbench_widgets::UiIcon::Refresh,
                                        "Retry",
                                        "Retry dataset download",
                                    )
                                    .clicked()
                                    {
                                        requested = Some(DatasetAction::Request(key.clone()));
                                    }
                                }
                                DatasetState::Missing => {
                                    ui.label(egui::RichText::new("not installed").weak());
                                    if lunco_workbench_widgets::icon_text_button(
                                        ui,
                                        lunco_workbench_widgets::UiIcon::Download,
                                        "Download",
                                        "Download this dataset",
                                    )
                                    .clicked()
                                    {
                                        requested = Some(DatasetAction::Request(key.clone()));
                                    }
                                }
                                DatasetState::Failed(error) => {
                                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                                    if lunco_workbench_widgets::icon_text_button(
                                        ui,
                                        lunco_workbench_widgets::UiIcon::Refresh,
                                        "Retry",
                                        "Retry dataset download",
                                    )
                                    .clicked()
                                    {
                                        requested = Some(DatasetAction::Request(key.clone()));
                                    }
                                }
                            }
                        });
                    }
                });
        }
        if let Some(action) = requested {
            match action {
                DatasetAction::Request(id) => {
                    ctx.trigger(lunco_assets_datasets::RequestDataset { id });
                }
                DatasetAction::Cancel(id) => {
                    ctx.trigger(lunco_assets_datasets::CancelDataset { id });
                }
            }
        }
    });
}

const SCENARIO_MENU_MIN_WIDTH: f32 = 300.0;
const SCENARIO_MENU_MAX_WIDTH: f32 = 420.0;
const SCENARIO_MENU_HEIGHT: f32 = 360.0;
const SCENARIO_REGISTRY_ERROR_EVENT: &str = "scenario-registry-unavailable";
const SCENARIO_REGISTRY_ERROR_LABEL: &str = "Scenarios unavailable";

fn scenario_registry_diagnostic(detail: impl Into<String>) -> String {
    format!("Twin registry unavailable: {}", detail.into())
}

fn scenario_registry_status_message(detail: &str) -> String {
    format!("{SCENARIO_REGISTRY_ERROR_EVENT} [source=0]: {detail}")
}

fn report_scenario_registry_error(ctx: &mut MenuCtx, detail: impl Into<String>) {
    let detail = scenario_registry_diagnostic(detail);
    let status_message = scenario_registry_status_message(&detail);
    let already_reported = ctx
        .resource::<lunco_status_core::status_bus::StatusBus>()
        .and_then(|bus| bus.history().next_back())
        .is_some_and(|event| {
            event.source == lunco_status_core::status_bus::TELEMETRY_SOURCE
                && event.level == lunco_status_core::status_bus::StatusLevel::Error
                && event.message == status_message
        });
    if !already_reported {
        ctx.trigger(lunco_core::TelemetryEvent {
            name: SCENARIO_REGISTRY_ERROR_EVENT.to_owned(),
            source: 0,
            severity: lunco_core::Severity::Error,
            data: lunco_core::TelemetryValue::String(detail),
            timestamp: 0.0,
        });
    }
}

fn render_scenario_registry_unavailable(ui: &mut bevy_egui::egui::Ui) {
    ui.label(SCENARIO_REGISTRY_ERROR_LABEL);
    ui.label(
        bevy_egui::egui::RichText::new("See Recent status for diagnostic details.")
            .weak()
            .small(),
    );
}

fn register_sandbox_scenarios_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_custom_menu("Scenarios", |ui, ctx| {
        ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
        ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);
        ui.label(
            bevy_egui::egui::RichText::new("Scenarios load a world or demo.")
                .weak()
                .small(),
        );
        ui.separator();
        let has_scene = ctx
            .resource::<CurrentScenePath>()
            .is_some_and(|path| !path.0.is_empty());

        ui.add_enabled_ui(has_scene, |ui| {
            if ui.button("Restart Scenario").clicked() {
                // `LoadScene` deliberately no-ops for the active `(stage, root)`.
                // RestartScene is the lifecycle verb that clears the current world,
                // invalidates the stage asset, and mounts a newly read source.
                ctx.trigger(lunco_usd_bevy_runtime_core::scene::RestartScene::default());
                ui.close();
            }
        });

        ui.separator();

        // ── Downloaded Twins (scenario-sync cache, G3) ───────────────────
        // Twins fetched from a server into the local cache — loadable offline
        // as a `twin://` root over the cache dir. Networking-only; the registry rebuilds from
        // `<cache>/scenarios/index.json` at boot and updates as downloads finish.
        #[cfg(feature = "networking")]
        {
            use lunco_networking_sync::scenario_sync::CachedTwinsRegistry;
            let entries = ctx
                .resource::<CachedTwinsRegistry>()
                .map(|r| r.entries.clone())
                .unwrap_or_default();
            ui.menu_button(format!("Downloaded Twins ({})", entries.len()), |ui| {
                ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
                ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);
                if entries.is_empty() {
                    ui.label(
                        bevy_egui::egui::RichText::new("(connect to a server to download one)")
                            .weak()
                            .italics(),
                    );
                }
                bevy_egui::egui::ScrollArea::vertical()
                    .max_height(SCENARIO_MENU_HEIGHT)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for entry in &entries {
                            let mb = (entry.total_bytes as f64) / (1024.0 * 1024.0);
                            let label = if entry.name.is_empty() {
                                format!("Downloaded twin  ({mb:.0} MB)")
                            } else {
                                format!("{}  ({mb:.0} MB)", entry.name)
                            };
                            if ui
                                .add_sized(
                                    [ui.available_width(), 0.0],
                                    bevy_egui::egui::Button::new(label).wrap(),
                                )
                                .clicked()
                            {
                                if let Some(scene) = entry.default_scene.clone() {
                                    // Mounts the cache dir as this twin's root and yields the
                                    // same `twin://<name>/<rel>` the host uses for the scene.
                                    let Some(twins) = ctx
                                        .resource::<lunco_assets_core::twin_source::TwinRoots>()
                                        .cloned()
                                    else {
                                        continue;
                                    };
                                    let path = match lunco_networking_sync::scenario_sync::mount_scenario_twin(
                                        &twins,
                                        &entry.scenario_id,
                                        &entry.name,
                                        &scene,
                                    ) {
                                        Ok(path) => path,
                                        Err(error) => {
                                            ctx.trigger(lunco_core::TelemetryEvent {
                                                name: "scenario-twin-mount-failed".into(),
                                                source: 0,
                                                severity: lunco_core::Severity::Error,
                                                data: lunco_core::TelemetryValue::String(
                                                    format!(
                                                        "could not mount downloaded scenario Twin: {error}"
                                                    ),
                                                ),
                                                timestamp: 0.0,
                                            });
                                            continue;
                                        }
                                    };
                                    ctx.trigger(lunco_usd_bevy_runtime_core::scene::LoadScene {
                                        path,
                                        root_prim: String::new(),
                                    });
                                    ui.close();
                                }
                            }
                        }
                    });
            });
        }

        ui.separator();

        let Some(roots) = ctx
            .resource::<lunco_assets_core::twin_source::TwinRoots>()
            .cloned()
        else {
            report_scenario_registry_error(ctx, "the Twin registry resource is unavailable");
            render_scenario_registry_unavailable(ui);
            return;
        };

        if ctx
            .resource::<scenario_fixture::ScenarioRegistryFixture>()
            .is_some_and(|fixture| fixture.unavailable)
        {
            report_scenario_registry_error(
                ctx,
                "the injected production-harness fixture is active",
            );
            render_scenario_registry_unavailable(ui);
            return;
        }

        let Some(manifest) = ctx.resource::<lunco_assets_core::discovery::AssetManifest>() else {
            render_scenario_registry_unavailable(ui);
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

        // Every loadable scene in the project. Which files those are is the
        // project's answer, not this menu's: each Twin declares `[usd] scenes`
        // in its `twin.toml`, the engine library uses its own `scenes/` layout.
        // See `discovery::list_scene_assets` for why the menu stopped deciding.
        let mut assets = match lunco_assets_core::discovery::list_scene_assets(manifest, &roots) {
            Ok(assets) => assets,
            Err(error) => {
                report_scenario_registry_error(ctx, error.to_string());
                render_scenario_registry_unavailable(ui);
                return;
            }
        };
        // Names copied out here so every click can dispatch through `MenuCtx`.
        let twin_names = match roots.names() {
            Ok(names) => names,
            Err(error) => {
                report_scenario_registry_error(ctx, error.to_string());
                render_scenario_registry_unavailable(ui);
                return;
            }
        };

        // Test scenes are hidden unless the user asks for them: they are rigs
        // `scripts/run_scene_tests.sh` runs for a verdict, and there are more of
        // them than there are scenes worth opening. Orthogonal to the globs
        // above — a project's `scenes` pattern says what IS a scene, this says
        // which of them this menu offers. The pref is one checkbox in the
        // Settings menu, so a test scene is never unreachable.
        let show_tests = ctx
            .resource::<lunco_luncosim_edit_ui::ui::asset_visibility::AssetVisibilitySettings>()
            .is_some_and(|s| s.show_test_assets);
        if !show_tests {
            assets.retain(|asset| !lunco_assets_core::discovery::is_test_asset(&asset.rel));
        }
        assets.sort_by(|a, b| a.stem.cmp(&b.stem));

        if assets.is_empty() {
            ui.label(
                bevy_egui::egui::RichText::new("(no scenes found)")
                    .weak()
                    .italics(),
            );
            return;
        }

        // Each scene's standard USD `doc` metadata, straight from the catalogue's
        // metadata store — the scan already read and parsed every project
        // `*.usda`, so re-reading them here would be a second parse of the
        // same default prim of the same file.
        //
        // The store fills asynchronously, so a scene not yet read simply
        // shows no tooltip this frame and gets one on the next redraw.
        let descs: Vec<Option<String>> = {
            let Some(store) = ctx.resource::<lunco_scene_catalog::catalog::AssetMetaStore>()
            else {
                return;
            };
            assets
                .iter()
                .map(|a| store.description(&a.asset_path).map(str::to_string))
                .collect()
        };

        let render =
            |ui: &mut bevy_egui::egui::Ui,
             ctx: &mut MenuCtx,
             items: &[(&lunco_assets_core::discovery::AssetFile, &Option<String>)]| {
                ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
                ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);
                bevy_egui::egui::ScrollArea::vertical()
                    .max_height(SCENARIO_MENU_HEIGHT)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (asset, desc) in items {
                            let label = clean_scene_name(&asset.stem);
                            let resp = ui.add_sized(
                                [ui.available_width(), 0.0],
                                bevy_egui::egui::Button::new(label).wrap(),
                            );
                            // Show the plain-language "what is this demo" blurb on hover.
                            // `on_hover_text` consumes and returns the `Response` (chaining API).
                            let resp = match desc {
                                Some(d) => resp.on_hover_text(d.as_str()),
                                None => resp,
                            };
                            if resp.clicked() {
                                    ctx.trigger(lunco_usd_bevy_runtime_core::scene::LoadScene {
                                    path: lunco_assets_core::engine_asset_uri(&asset.asset_path),
                                    root_prim: String::new(),
                                });
                                ui.close();
                            }
                        }
                    });
            };

        let paired: Vec<(&lunco_assets_core::discovery::AssetFile, &Option<String>)> =
            assets.iter().zip(descs.iter()).collect();
        let regular: Vec<_> = paired
            .iter()
            .copied()
            .filter(|(asset, _)| !lunco_assets_core::discovery::is_test_asset(&asset.rel))
            .collect();
        let tests: Vec<_> = paired
            .iter()
            .copied()
            .filter(|(asset, _)| lunco_assets_core::discovery::is_test_asset(&asset.rel))
            .collect();

        // Open Twins FIRST as submenus: the twin you have open is
        // the project you are working in, and its scenarios are what you came to
        // the menu for. The engine library is the reference collection below it.
        for name in &twin_names {
            let group: Vec<_> = regular
                .iter()
                .copied()
                .filter(|(a, _)| a.twin.as_deref() == Some(name.as_str()))
                .collect();
            if group.is_empty() {
                continue;
            }
            ui.menu_button(format!("{name}  ({})", group.len()), |ui| {
                render(ui, ctx, &group);
            });
        }

        let library: Vec<_> = regular
            .iter()
            .copied()
            .filter(|(a, _)| a.twin.is_none())
            .collect();
        if !library.is_empty() {
            ui.menu_button(format!("Library  ({})", library.len()), |ui| {
                render(ui, ctx, &library);
            });
        }
        if show_tests {
            ui.separator();
            ui.menu_button(format!("Test scenes  ({})", tests.len()), |ui| {
                if tests.is_empty() {
                    ui.label(
                        bevy_egui::egui::RichText::new("(no test scenes discovered)")
                            .weak()
                            .italics(),
                    );
                } else {
                    render(ui, ctx, &tests);
                }
            });
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

#[cfg(test)]
mod tests {
    use super::{
        runtime_ui_dimension, scenario_registry_diagnostic, scenario_registry_status_message,
        RuntimeUiDropdownState,
    };
    use lunco_core::exposure::{ExposureSurface, ExposureValue};
    use std::collections::HashMap;

    #[test]
    fn authored_dropdown_keeps_open_after_the_trigger_press() {
        let mut dropdowns = RuntimeUiDropdownState::default();
        dropdowns.toggle("surface::dropdown");
        assert_eq!(dropdowns.open.as_deref(), Some("surface::dropdown"));
        assert!(dropdowns.ignore_opening_click);

        dropdowns.consume_opening_click(true);
        assert_eq!(dropdowns.open.as_deref(), Some("surface::dropdown"));
        assert!(!dropdowns.ignore_opening_click);

        dropdowns.close();
        assert_eq!(dropdowns, RuntimeUiDropdownState::default());
    }

    #[test]
    fn authored_dropdown_dimensions_accept_typed_policy_values() {
        let mut properties = HashMap::new();
        properties.insert("width".to_owned(), ExposureValue::Text("280px".to_owned()));
        properties.insert("height".to_owned(), ExposureValue::Number(240.0));
        let exposure = ExposureSurface {
            properties,
            ..Default::default()
        };
        assert_eq!(runtime_ui_dimension(&exposure, "width"), Some(280.0));
        assert_eq!(runtime_ui_dimension(&exposure, "height"), Some(240.0));
    }

    #[test]
    fn scenario_registry_diagnostics_stay_out_of_the_menu_label() {
        let detail = scenario_registry_diagnostic("missing twin.toml");
        assert_eq!(
            super::SCENARIO_REGISTRY_ERROR_LABEL,
            "Scenarios unavailable"
        );
        assert_eq!(
            scenario_registry_status_message(&detail),
            "scenario-registry-unavailable [source=0]: Twin registry unavailable: missing twin.toml"
        );
        assert!(!super::SCENARIO_REGISTRY_ERROR_LABEL.contains("missing twin.toml"));
    }

    #[test]
    fn scenario_registry_fixture_is_transient_and_disabled_by_default() {
        let mut fixture = super::scenario_fixture::ScenarioRegistryFixture::default();
        assert!(!fixture.unavailable);
        fixture.unavailable = true;
        assert!(fixture.unavailable);
        fixture.unavailable = false;
        assert!(!fixture.unavailable);
    }
}
