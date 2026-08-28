//! Sandbox UI layer — everything that draws pixels, opens egui panels, or
//! drives an interactive camera.
//!
//! This whole module is `#[cfg(feature = "ui")]` (declared in `lib.rs`), so a
//! headless `--no-ui` / `lunco-luncosim-server` build never compiles it. The
//! entry point is [`SandboxUiPlugin`]: the app shell adds it only when running
//! windowed (`ui` feature present AND not `--no-ui`). The shared sim/physics/
//! cosim/networking core (`SandboxCorePlugin`) and the headless runner
//! (`SandboxHeadlessPlugin`) live in `lib.rs` and carry no UI.
//!
//! Mirrors the `ui/` + `*UiPlugin` convention every library crate already uses
//! (`SceneEditUiPlugin`, `UsdUiPlugin`, `ModelicaUiPlugin`, …) — the app crate
//! is now structurally identical to them.

use bevy::prelude::*;

use lunco_modelica::{ModelicaUiConfig, ModelicaWorkbenchPlugin};
use lunco_usd_bevy::camera_switch::{
    CameraSelectionOwner, CameraSelectionStatus, ObserveAvatar, ResumeCameraDirector, SetUserCamera,
};
use lunco_workbench::{CurrentSceneName, CurrentScenePath, MenuCtx};

const DIAGNOSTICS_STATUS_SOURCE: &str = "Diagnostics";

#[derive(Resource, Default)]
struct DiagnosticsStatusMirror {
    signature: Option<String>,
}

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
/// Generic retained HUI/Flair exposure boundary shared by runtime-authored
/// templates and engine value producers.
mod runtime_exposure;
/// Native Velopack update checks and package installation. WASM has no native
/// process/update helper and intentionally does not compile this module.
#[cfg(not(target_arch = "wasm32"))]
mod update;
/// Typed intent emitted by the authored terrain-progress surface.
#[derive(Event, Clone, Debug)]
struct DismissTerrainOverlay;

/// The luncosim's interactive layer: egui workbench, bevy_picking, the USD Twin
/// browser + RTT viewport, the in-scene editor, materials, rover panels, and
/// explicit camera presentation controls.
///
/// Added by the app shell only for a windowed run. A headless server runs the
/// sim, physics, scene, cosim, and networking host (all in `SandboxCorePlugin`)
/// *without* any of this — headless mode omits the renderer and keeps only the
/// simulation-facing asset/type plugins, so nothing here (GPU / window / pointer)
/// is wired.
pub(crate) struct SandboxUiPlugin;

/// Install the retained runtime-authored HTML surface layer.
///
/// This is deliberately shared by the interactive workbench and the GPU
/// windowless recorder. The latter has no egui host, but it still has a real
/// Bevy UI render pass and a scene camera, so authored HUDs must use the same
/// HUI/Flair and exposure path in both modes.
pub(crate) fn add_runtime_ui_layer(app: &mut App) {
    app.add_plugins((
        bevy_hui::HuiPlugin,
        bevy_flair::FlairPlugin,
        runtime_exposure::RuntimeUiManifestPlugin,
    ))
    .init_resource::<runtime_exposure::RuntimeUiRenderState>()
    .init_resource::<runtime_exposure::RuntimeUiGates>()
    .add_systems(Startup, runtime_exposure::load_runtime_ui_manifest)
    .add_systems(
        Update,
        (
            runtime_exposure::sync_runtime_ui_manifest,
            update_runtime_ui_gates,
            runtime_exposure::mount_runtime_ui_surfaces
                .after(runtime_exposure::sync_runtime_ui_manifest)
                .after(update_runtime_ui_gates)
                .before(bevy_hui::HuiSystems::Build),
            runtime_exposure::bind_runtime_ui_to_camera
                .after(runtime_exposure::sync_runtime_ui_manifest),
            runtime_exposure::attach_runtime_ui_names
                .after(runtime_exposure::sync_runtime_ui_manifest)
                .before(bevy_flair::style::StyleSystems::Prepare),
            runtime_exposure::hand_runtime_ui_styling_to_flair
                .after(bevy_hui::HuiSystems::Style)
                .after(runtime_exposure::sync_runtime_ui_manifest)
                // HUI emits HtmlStyle during its style pass; the bridge must
                // hand that tree to Flair before Flair snapshots selectors and
                // computes authored properties, otherwise a newly mounted
                // surface paints one frame with raw HUI text.
                .before(bevy_flair::style::StyleSystems::Prepare),
            runtime_exposure::apply_runtime_ui_exposures
                .after(runtime_exposure::sync_runtime_ui_manifest)
                // Exposure values populate HUI TemplateProperties and inline
                // style inputs. They must be visible to the same style pass
                // that computes the retained tree; applying them after Style
                // paints one frame of raw text before the authored panel is
                // laid out and styled.
                .after(bevy_hui::HuiSystems::Build)
                .before(bevy_hui::HuiSystems::Style),
        ),
    )
    .add_systems(
        PostUpdate,
        (
            runtime_exposure::apply_runtime_ui_placement_after_style
                .after(bevy_flair::style::StyleSystems::ApplyComputedProperties)
                .after(bevy::ui::UiSystems::Propagate)
                .before(bevy::ui::UiSystems::Content),
            runtime_exposure::report_runtime_ui_readiness
                .after(runtime_exposure::apply_runtime_ui_placement_after_style)
                .after(bevy::ui::UiSystems::PostLayout),
        ),
    );
    runtime_exposure::install_runtime_ui_render_readiness(app);
}

impl Plugin for SandboxUiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(lunco_workbench::BuildIdentity::new(
            crate::PRODUCT_VERSION,
            crate::GIT_SHA,
            crate::REPOSITORY_URL,
        ));
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, crate::apply_luncosim_window_icon);
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
        // A windowed host requires an authored presentation contract. The USD
        // projection owns validation; this flag only declares the host's
        // admission requirement and never creates a camera.
        app.world_mut()
            .resource_mut::<lunco_usd_bevy::camera_switch::CameraContractStatus>()
            .required = true;
        app.init_resource::<dataset_provisioning::DatasetProvisioningState>()
            .add_observer(dataset_provisioning::on_dataset_scope_ready)
            .add_observer(dataset_provisioning::on_dataset_scope_removed)
            .add_systems(Update, dataset_provisioning::poll_dataset_provisioning);
        app.add_plugins(bevy::pbr::wireframe::WireframePlugin::default())
            // bevy_picking's mesh backend: makes visible Mesh3d entities pickable,
            // so scene selection / possession / spawn-placement run as click observers.
            .add_plugins(bevy::picking::mesh_picking::MeshPickingPlugin)
            .add_plugins(lunco_workbench::WorkbenchPlugin);
        app.init_resource::<DiagnosticsStatusMirror>()
            .add_systems(Update, sync_diagnostics_status);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(update::UpdatePlugin);
        if args.iter().any(|arg| arg == "--windowed-ui") {
            app.insert_resource(lunco_workbench::OfflineRecordingPresentation {
                retain_workbench_chrome: true,
            });
        }
        lunco_workbench::install_render_recovery_teardown(app, lunco_core::SceneTeardown);
        app.add_plugins(overlays::plugin)
            // Overlay visibility prefs + the Time-menu rows that drive them.
            // USD Twin browser. NOTE: the USD *viewport preview*
            // (`UsdViewportPlugin`) is intentionally NOT added here. It is an
            // editor tool that OWNS its own scene — it parses the active USD doc
            // into a second `UsdStageAsset` and mounts a private `scene_root`. The
            // luncosim is a sim app: its single scene is the live `LoadScene` world,
            // viewed by the window camera. Adding the preview built the scene a
            // SECOND time (doubled crater meshes / rocks). A view must not own a
            // scene — see `docs/architecture/usd-source-of-truth.md`.
            .add_plugins(lunco_usd::ui::UsdUiPlugin)
            .add_plugins(lunco_luncosim_edit::SceneEditPlugin)
            .add_plugins(lunco_luncosim_edit::ui::SceneEditUiPlugin)
            // NOTE: `ShaderMaterialPlugin` (the dynamic `ShaderMaterial` render
            // pipeline) used to be added here. It now lives inside
            // `lunco_render_bevy::LuncoRenderPlugin` — the one crate that may name
            // `bevy_pbr` — and adding it a second time panics Bevy.
            // See docs/architecture/render-decoupling.md.
            // The shared tutorial launcher: registry + 🎓 menu + panel +
            // Start/Skip/SetSubsystemEnabled + progress + onboarding + F1.
            // Tutorials compose from assets/tutorials/luncosim.usda (data, not code).
            .add_plugins(lunco_tutorial::TutorialPlugin {
                app: "luncosim".into(),
            })
            // Rover panels. ONE closure: Bevy keys plugin uniqueness by type-name,
            // and every `|app| {…}` in this `build` shares the name `{{closure}}` — a
            // second one panics ("plugin already added"). So all app-level panel
            // registration goes here.
            .add_plugins(|app: &mut App| {
                use lunco_workbench::WorkbenchAppExt;
                app.add_observer(on_runtime_ui_action)
                    .add_observer(on_dismiss_terrain_overlay)
                    .add_observer(dataset_provisioning::on_set_missing_asset_prompt_suppressed);
                app.add_systems(
                    Update,
                    runtime_exposure::register_runtime_ui_input_regions
                        .after(runtime_exposure::apply_runtime_ui_exposures),
                );
                // Rover-specific panels and the attach-a-model click flow.
                app.register_panel(code_panel::CodePanel);
                // Rhai behaviour editor (Object Builder). Its view-model is
                // produced each frame from the selection + ScriptRegistry.
                app.register_panel(rhai_editor_panel::RhaiEditorPanel);
                app.init_resource::<rhai_editor_panel::RhaiEditorVm>();
                app.add_systems(Update, rhai_editor_panel::produce_rhai_editor_vm);
                app.register_panel(models_palette::ModelsPalette);
                app.init_resource::<models_palette::ProgramCatalog>();
                app.add_systems(Startup, models_palette::refresh_program_catalog_startup);
                app.add_systems(
                    Update,
                    models_palette::refresh_program_catalog_manifest
                        .run_if(resource_changed::<lunco_assets::discovery::AssetManifest>),
                );
                app.add_observer(models_palette::refresh_program_catalog_twin_added);
                app.add_observer(models_palette::refresh_program_catalog_twin_closed);
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
            // View is the default simulation presentation. A loaded Twin must
            // show its authored 3D camera immediately; the Modelica workbench
            // remains an explicit operator choice instead of covering the scene
            // during startup.
            .add_systems(
                Startup,
                |mut layout: ResMut<lunco_workbench::WorkbenchLayout>| {
                    layout.activate_perspective(lunco_workbench::PerspectiveId("sandbox_view"));
                },
            )
            .add_systems(
                Startup,
                (
                    init_current_scene_path,
                    register_sandbox_scenarios_menu,
                    register_camera_menu,
                    register_diagnostics_menu,
                    register_downloadable_assets_settings,
                    register_graphics_settings,
                ),
            )
            .add_observer(
                |t: On<lunco_usd::LoadScene>,
                 current: Option<ResMut<CurrentScenePath>>,
                 current_name: Option<ResMut<CurrentSceneName>>,
                 hud: Option<ResMut<lunco_workbench::tutorial_overlay::TutorialHud>>,
                 pending: Option<ResMut<lunco_tutorial::PendingAdvance>>| {
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
                    if let Some(mut pending) = pending {
                        pending.0 = None;
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
                celestial_time::draw_celestial_time
                    .in_set(lunco_workbench::ApplicationOverlayRenderSet)
                    .run_if(not(recording_offline))
                    .run_if(in_view_perspective)
                    .run_if(overlays::sky_clock_visible),
            );

        // Tutorial TRACKS come from the curriculum layer `TutorialCorePlugin`
        // composes, not from here: a lesson is an executable scenario, so
        // `StartTutorial { id }` must resolve on a headless/API host too.
        // Registering tracks from the UI plugin once made the whole `basic`
        // track exist only in the windowed build — over the API it answered
        // `unknown id` and nothing loaded.

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

fn update_runtime_ui_gates(
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
    overlays: Option<Res<overlays::OverlaySettings>>,
    recording: Option<Res<lunco_workbench::screenshot::OfflineRecordingState>>,
    mut gates: ResMut<runtime_exposure::RuntimeUiGates>,
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
        value.active_perspective() == Some(lunco_workbench::PerspectiveId("sandbox_view"))
    });
    let overlay_enabled = overlays.is_some_and(|value| value.view_switcher);
    let recording = recording.is_some_and(|value| value.active);
    gates.set("view_switcher", in_view && overlay_enabled && !recording);
}

fn recording_offline(
    recording: Option<Res<lunco_workbench::screenshot::OfflineRecordingState>>,
) -> bool {
    recording.is_some_and(|recording| recording.active)
}

fn in_view_perspective(layout: Option<Res<lunco_workbench::WorkbenchLayout>>) -> bool {
    layout.is_some_and(|layout| {
        layout.active_perspective() == Some(lunco_workbench::PerspectiveId("sandbox_view"))
    })
}

fn on_runtime_ui_action(
    trigger: On<runtime_exposure::RuntimeUiAction>,
    q_avatar: Query<Entity, (With<lunco_core::Avatar>, With<lunco_core::LocalAvatar>)>,
    q_control: Query<
        &lunco_controller::ControllerLink,
        (With<lunco_core::Avatar>, With<lunco_core::LocalAvatar>),
    >,
    q_bodies: Query<(Entity, &lunco_core::CelestialBody)>,
    orbital_pin: Option<Res<lunco_celestial::OrbitalViewPin>>,
    mut commands: Commands,
) {
    match trigger.event().action {
        runtime_exposure::RuntimeUiActionKind::ViewSurface => {
            if !orbital_pin.is_some_and(|pin| pin.active) {
                return;
            }
            if let Ok(target) = q_avatar.single() {
                commands.trigger(lunco_avatar::ReturnFromOrbit { target });
            }
        }
        runtime_exposure::RuntimeUiActionKind::ViewBodyMoon => {
            if !runtime_focus_body(
                lunco_celestial::ephemeris_id::MOON,
                &q_bodies,
                &mut commands,
            ) {
                report_runtime_ui_failure(&mut commands, "Moon is not present in the loaded scene");
            }
        }
        runtime_exposure::RuntimeUiActionKind::ViewBodyEarth => {
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
        runtime_exposure::RuntimeUiActionKind::DismissTerrainOverlay => {
            commands.trigger(DismissTerrainOverlay)
        }
        runtime_exposure::RuntimeUiActionKind::ToggleAutopilot => {
            let Ok(link) = q_control.single() else {
                report_runtime_ui_failure(
                    &mut commands,
                    "No locally controlled vessel is available",
                );
                return;
            };
            commands.trigger(lunco_luncosim_edit::ui::waypoint_click::ToggleAutopilot {
                vessel: link.vessel_entity,
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
        commands.trigger(lunco_avatar::FocusTarget {
            avatar: None,
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

/// Register an egui-hosted, keyboard-accessible route to the same semantic
/// camera actions used by the retained 3D overlay. HUI remains the compact
/// in-viewport presentation; it is not the sole way to operate an essential
/// camera mode because retained HTML currently exposes no accessibility tree.
fn register_camera_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_custom_menu("Camera", |ui, ctx| {
        let state = ctx.resource::<CameraSelectionStatus>().cloned();
        ui.label("Presentation");
        if let Some(state) = &state {
            let active = state.active_name.as_deref().unwrap_or("none");
            let owner = match state.owner {
                CameraSelectionOwner::None => "none",
                CameraSelectionOwner::Director => "director",
                CameraSelectionOwner::User => "operator",
            };
            ui.label(format!("Active: {active}  ·  Owner: {owner}"));
            if let Some(error) = &state.last_error {
                ui.colored_label(bevy_egui::egui::Color32::from_rgb(235, 130, 130), error);
            }
            if let Some(contract) =
                ctx.resource::<lunco_usd_bevy::camera_switch::CameraContractStatus>()
            {
                for error in &contract.errors {
                    ui.colored_label(bevy_egui::egui::Color32::from_rgb(255, 110, 110), error);
                }
            }
            if state.avatar_available && ui.button("Observe avatar").clicked() {
                ctx.trigger(ObserveAvatar {});
                ui.close();
            }
            if state.director_available && ui.button("Resume authored director").clicked() {
                ctx.trigger(ResumeCameraDirector {});
                ui.close();
            }
            ui.separator();
            if state.cameras.is_empty() {
                ui.label("No authored window camera is available.");
                ui.label("This scene has no presentation until one is authored.");
            } else {
                ui.label("Operator camera");
                for name in &state.cameras {
                    let active = state.active_name.as_deref() == Some(name.as_str());
                    if ui.selectable_label(active, name).clicked() {
                        ctx.trigger(SetUserCamera { name: name.clone() });
                        ui.close();
                    }
                }
            }
        } else {
            ui.label("Camera state is not ready.");
        }
        let actions = [
            (
                "Surface view",
                runtime_exposure::RuntimeUiActionKind::ViewSurface,
            ),
            (
                "Orbit Moon",
                runtime_exposure::RuntimeUiActionKind::ViewBodyMoon,
            ),
            (
                "Orbit Earth",
                runtime_exposure::RuntimeUiActionKind::ViewBodyEarth,
            ),
        ];
        for (label, action) in actions {
            if ui.button(label).clicked() {
                ctx.trigger(runtime_exposure::RuntimeUiAction { action });
                ui.close();
            }
        }
    });
}

/// Mirror the current owning-boundary diagnostics onto the workbench status
/// bus. The resources remain authoritative; this is only the UI projection so
/// the status bar and its Diagnostics history show the same errors that the API
/// and the explicit menu expose.
fn sync_diagnostics_status(
    runtime: Option<Res<lunco_core::RuntimeDiagnostics>>,
    lint: Option<Res<lunco_lint::LintReport>>,
    mut mirror: ResMut<DiagnosticsStatusMirror>,
    mut bus: ResMut<lunco_workbench::status_bus::StatusBus>,
) {
    let mut entries = Vec::new();
    if let Some(runtime) = runtime {
        entries.extend(runtime.findings.iter().map(|finding| {
            (
                runtime_severity_rank(finding.severity),
                format!(
                    "[runtime/{}] {} — {}",
                    finding.code, finding.subject, finding.message
                ),
            )
        }));
    }
    if let Some(lint) = lint {
        entries.extend(lint.findings.iter().map(|finding| {
            (
                lint_severity_rank(finding.severity),
                format!("[lint/{}] {}", finding.rule, finding.line()),
            )
        }));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let signature = entries
        .iter()
        .map(|(rank, message)| format!("{rank}:{message}"))
        .collect::<Vec<_>>()
        .join("\n");
    if mirror.signature.as_deref() == Some(signature.as_str()) {
        return;
    }
    mirror.signature = Some(signature);

    if entries.is_empty() {
        bus.push(
            DIAGNOSTICS_STATUS_SOURCE,
            lunco_workbench::status_bus::StatusLevel::Info,
            "No active runtime or authoring diagnostics",
        );
        return;
    }

    let errors = entries.iter().filter(|(rank, _)| *rank == 3).count();
    let warnings = entries.iter().filter(|(rank, _)| *rank == 2).count();
    let level = if errors > 0 {
        lunco_workbench::status_bus::StatusLevel::Error
    } else if warnings > 0 {
        lunco_workbench::status_bus::StatusLevel::Warn
    } else {
        lunco_workbench::status_bus::StatusLevel::Info
    };
    let summary = match (errors, warnings) {
        (0, 0) => format!("{} informational diagnostics", entries.len()),
        (0, warnings) => format!(
            "{warnings} warning(s), {} informational diagnostic(s)",
            entries.len() - warnings
        ),
        (errors, 0) => format!(
            "{errors} error(s), {} informational diagnostic(s)",
            entries.len() - errors
        ),
        (errors, warnings) => format!(
            "{errors} error(s), {warnings} warning(s), {} informational diagnostic(s)",
            entries.len() - errors - warnings
        ),
    };
    bus.push(
        DIAGNOSTICS_STATUS_SOURCE,
        level,
        format!("{summary}: {}", entries[0].1),
    );
}

fn runtime_severity_rank(severity: lunco_core::DiagnosticSeverity) -> u8 {
    match severity {
        lunco_core::DiagnosticSeverity::Error => 3,
        lunco_core::DiagnosticSeverity::Warning => 2,
        lunco_core::DiagnosticSeverity::Info => 1,
    }
}

fn lint_severity_rank(severity: lunco_lint::LintSeverity) -> u8 {
    match severity {
        lunco_lint::LintSeverity::Error => 3,
        lunco_lint::LintSeverity::Warn => 2,
        lunco_lint::LintSeverity::Info => 1,
    }
}

/// Show the complete current diagnostic set, including the exact lint finding
/// and source subject. Error and warning rows use the shared theme tokens so a
/// malformed scene is visibly distinct from ordinary status history.
fn register_diagnostics_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_custom_menu("Diagnostics", |ui, ctx| {
        let Some(theme) = ctx.resource::<lunco_theme::Theme>() else {
            ui.label("Diagnostics theme is unavailable.");
            return;
        };
        let error = theme.tokens.error;
        let warning = theme.tokens.warning;
        let info = theme.tokens.text_subdued;
        let runtime = ctx.resource::<lunco_core::RuntimeDiagnostics>();
        let lint = ctx.resource::<lunco_lint::LintReport>();
        let runtime_count = runtime.map(|report| report.findings.len()).unwrap_or(0);
        let lint_count = lint.map(|report| report.findings.len()).unwrap_or(0);
        ui.label(format!(
            "Runtime: {runtime_count} · Authoring lint: {lint_count}"
        ));

        if runtime_count == 0 && lint_count == 0 {
            ui.label("No active diagnostics.");
            return;
        }
        ui.separator();
        bevy_egui::egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                if let Some(runtime) = runtime {
                    for finding in &runtime.findings {
                        let color = match finding.severity {
                            lunco_core::DiagnosticSeverity::Error => error,
                            lunco_core::DiagnosticSeverity::Warning => warning,
                            lunco_core::DiagnosticSeverity::Info => info,
                        };
                        ui.colored_label(
                            color,
                            format!(
                                "[runtime/{}] {} — {}",
                                finding.code, finding.subject, finding.message
                            ),
                        );
                    }
                }
                if let Some(lint) = lint {
                    for finding in &lint.findings {
                        let color = match finding.severity {
                            lunco_lint::LintSeverity::Error => error,
                            lunco_lint::LintSeverity::Warn => warning,
                            lunco_lint::LintSeverity::Info => info,
                        };
                        ui.colored_label(color, finding.line());
                    }
                }
            });
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

fn init_current_scene_path(
    scene_path: Res<crate::ScenePath>,
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
    layout.register_settings_submenu("Data & libraries", |ui, ctx| {
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
                    "The same bounded policy is used for assets, MSL, scenario HTTP, and app updates. Attempts include the first request; delays grow exponentially and stop at the configured maximum.",
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
                    lunco_assets::datasets::DatasetScope::Engine => e.group.clone(),
                    lunco_assets::datasets::DatasetScope::Twin { name, .. } => name.clone(),
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
                                    lunco_workbench::paint_icon(
                                        ui.painter(),
                                        lunco_workbench::UiIcon::Check,
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
                                    if lunco_workbench::icon_text_button(
                                        ui,
                                        lunco_workbench::UiIcon::Stop,
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
                                    if lunco_workbench::icon_text_button(
                                        ui,
                                        lunco_workbench::UiIcon::Refresh,
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
                                    if lunco_workbench::icon_text_button(
                                        ui,
                                        lunco_workbench::UiIcon::Download,
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
                                    if lunco_workbench::icon_text_button(
                                        ui,
                                        lunco_workbench::UiIcon::Refresh,
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
                    ctx.trigger(lunco_assets::datasets::RequestDataset { id });
                }
                DatasetAction::Cancel(id) => {
                    ctx.trigger(lunco_assets::datasets::CancelDataset { id });
                }
            }
        }
    });
}

/// Add luncosim's local-light policy to the workbench-owned Graphics group.
/// The quality setting and terrain rows live in `lunco-workbench`; this row is
/// app-specific because possession is the policy owner for rover headlights.
fn register_graphics_settings(world: &mut World) {
    use crate::light_policy::LocalLightShadows;
    use bevy_egui::egui;

    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings_submenu("Graphics", |ui, ctx| {
        ui.label(egui::RichText::new("Local light shadows").weak().small());
        let Some(current) = ctx.resource::<crate::light_policy::ShadowCastingSettings>() else {
            ui.label(egui::RichText::new("(local-light policy unavailable)").weak());
            return;
        };
        let mut settings = *current;
        egui::ComboBox::from_id_salt("graphics.local_light_shadows")
            .selected_text(match settings.local_lights {
                LocalLightShadows::Off => "Off",
                LocalLightShadows::All => "All",
                LocalLightShadows::PossessedOnly => "Possessed vessel only",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut settings.local_lights, LocalLightShadows::Off, "Off");
                ui.selectable_value(&mut settings.local_lights, LocalLightShadows::All, "All");
                ui.selectable_value(
                    &mut settings.local_lights,
                    LocalLightShadows::PossessedOnly,
                    "Possessed vessel only",
                );
            });
        if settings != *current {
            ctx.set_resource(settings);
        }
        ui.label(
            egui::RichText::new(
                "Controls rover headlights and fill lamps; the Rendering quality choice controls the shared sun shadow atlas.",
            )
            .weak()
            .small(),
        );
    });
}

const SCENARIO_MENU_MIN_WIDTH: f32 = 300.0;
const SCENARIO_MENU_MAX_WIDTH: f32 = 420.0;
const SCENARIO_MENU_HEIGHT: f32 = 360.0;

fn register_sandbox_scenarios_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_custom_menu("Scenarios", |ui, ctx| {
        let Some(theme) = ctx.resource::<lunco_theme::Theme>() else {
            ui.label("(theme unavailable)");
            return;
        };
        let error_color = theme.tokens.error;
        ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
        ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);
        ui.label(
            bevy_egui::egui::RichText::new(
                "Scenarios load a world or demo. Tutorials are guided lessons layered on a world.",
            )
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
                ctx.trigger(lunco_usd::RestartScene::default());
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
        render_tutorials_submenu(ui, ctx);

        // ── Downloaded Twins (scenario-sync cache, G3) ───────────────────
        // Twins fetched from a server into the local cache — loadable offline
        // as a `twin://` root over the cache dir. Networking-only; the registry rebuilds from
        // `<cache>/scenarios/index.json` at boot and updates as downloads finish.
        #[cfg(feature = "networking")]
        {
            use lunco_networking::scenario_sync::CachedTwinsRegistry;
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
                                        .resource::<lunco_assets::twin_source::TwinRoots>()
                                        .cloned()
                                    else {
                                        continue;
                                    };
                                    let path = match lunco_networking::scenario_sync::mount_scenario_twin(
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
                                    ctx.trigger(lunco_usd::LoadScene {
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
            .resource::<lunco_assets::twin_source::TwinRoots>()
            .cloned()
        else {
            ui.label(
                bevy_egui::egui::RichText::new("(no TwinRoots resource)")
                    .weak()
                    .italics(),
            );
            return;
        };

        let Some(manifest) = ctx.resource::<lunco_assets::discovery::AssetManifest>() else {
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

        // Every loadable scene in the project. WHICH files those are is the
        // project's answer, not this menu's: each Twin declares `[usd] scenes`
        // in its `twin.toml`, the engine library uses its own `scenes/` layout.
        // See `discovery::list_scene_assets` for why the menu stopped deciding.
        let mut assets = match lunco_assets::discovery::list_scene_assets(manifest, &roots) {
            Ok(assets) => assets,
            Err(error) => {
                ui.label(
                    bevy_egui::egui::RichText::new(format!(
                        "(Twin registry unavailable: {error})"
                    ))
                    .color(error_color),
                );
                return;
            }
        };
        // Names copied out here so every click can dispatch through `MenuCtx`.
        let twin_names = match roots.names() {
            Ok(names) => names,
            Err(error) => {
                ui.label(
                    bevy_egui::egui::RichText::new(format!(
                        "(Twin registry unavailable: {error})"
                    ))
                    .color(error_color),
                );
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
            .resource::<lunco_luncosim_edit::ui::asset_visibility::AssetVisibilitySettings>()
            .is_some_and(|s| s.show_test_assets);
        if !show_tests {
            assets.retain(|asset| !lunco_assets::discovery::is_test_asset(&asset.rel));
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

        // Each scene's `lunco:description`, straight from the catalogue's
        // metadata store — the scan already read and parsed every project
        // `*.usda`, so re-reading them here would be a second parse of the
        // same default prim of the same file. (It used to be exactly that:
        // a `SceneDescCache` that lazily re-parsed on first hover.)
        //
        // The store fills asynchronously, so a scene not yet read simply
        // shows no tooltip this frame and gets one on the next redraw.
        let descs: Vec<Option<String>> = {
            let Some(store) = ctx.resource::<lunco_scene_commands::catalog::AssetMetaStore>()
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
             items: &[(&lunco_assets::discovery::AssetFile, &Option<String>)]| {
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
                                ctx.trigger(lunco_usd::LoadScene {
                                    path: lunco_assets::engine_asset_uri(&asset.asset_path),
                                    root_prim: String::new(),
                                });
                                ui.close();
                            }
                        }
                    });
            };

        let paired: Vec<(&lunco_assets::discovery::AssetFile, &Option<String>)> =
            assets.iter().zip(descs.iter()).collect();
        let regular: Vec<_> = paired
            .iter()
            .copied()
            .filter(|(asset, _)| !lunco_assets::discovery::is_test_asset(&asset.rel))
            .collect();
        let tests: Vec<_> = paired
            .iter()
            .copied()
            .filter(|(asset, _)| lunco_assets::discovery::is_test_asset(&asset.rel))
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
            ui.menu_button(format!("◉ {name}  ({})", group.len()), |ui| {
                render(ui, ctx, &group);
            });
        }

        let library: Vec<_> = regular
            .iter()
            .copied()
            .filter(|(a, _)| a.twin.is_none())
            .collect();
        if !library.is_empty() {
            ui.menu_button(format!("▤ Library  ({})", library.len()), |ui| {
                render(ui, ctx, &library);
            });
        }
        if show_tests {
            ui.separator();
            // U+1F9EA is missing from the bundled fallback and rendered as
            // tofu on clean installs; use the same supported tool glyph as
            // the rest of the test/build UI.
            ui.menu_button(format!("⚒ Test scenes  ({})", tests.len()), |ui| {
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

/// Render the "🎓 Tutorials" submenu inside the Scenarios menu. Lists every
/// registered tutorial with a completion tick, a difficulty chip, and its blurb
/// on hover; clicking starts it. Kept next to the scenes list so the menu is the
/// single place to launch either a raw scene or a guided lesson.
fn render_tutorials_submenu(ui: &mut bevy_egui::egui::Ui, ctx: &mut MenuCtx) {
    use bevy_egui::egui;

    let registry = ctx.resource::<lunco_tutorial::TutorialRegistry>().cloned();
    let progress = ctx
        .resource::<lunco_tutorial::TutorialProgress>()
        .cloned()
        .unwrap_or_default();

    ui.menu_button("Tutorials", |ui| {
        ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
        ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);
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

        egui::ScrollArea::vertical()
            .max_height(SCENARIO_MENU_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for meta in registry.ordered() {
                    let done = progress.completed.iter().any(|c| c == &meta.id);
                    // Completed or fresh, then the title and a dim difficulty chip.
                    let label = format!(
                        "{} {}  ·  {}",
                        if done { "[done]" } else { "[new]" },
                        meta.title,
                        meta.difficulty
                    );
                    let resp =
                        ui.add_sized([ui.available_width(), 0.0], egui::Button::new(label).wrap());
                    // Hover tip: the plain-language "what this teaches" blurb.
                    let resp = resp.on_hover_text(meta.blurb.as_str());
                    if resp.clicked() {
                        ctx.trigger(lunco_tutorial::StartTutorial {
                            id: meta.id.to_string(),
                        });
                        ui.close();
                    }
                }
            });
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
