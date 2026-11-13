//! LunCoSim UI layer — everything that draws pixels, opens egui panels, or
//! drives an interactive camera.
//!
//! The entry point is [`LunCoSimUiPlugin`]. The application shell adds it only
//! for a windowed run; the shared simulator core and headless runner do not
//! compile this crate.

use std::{collections::BTreeMap, sync::Arc};

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lunco_core::{AppSceneExt, SceneResource};
use lunco_workbench_runtime_ui as runtime_ui;

use lunco_modelica_ui::ModelicaWorkbenchPlugin;
use lunco_usd_bevy_camera::camera_switch::{CameraSelectionStatus, ObserveAvatar};
use lunco_workbench_core::presentation::{ViewportPlaceholder, ViewportPlaceholderAction};
use lunco_workbench_core::scene::{CurrentSceneName, CurrentScenePath};
use lunco_workbench_core::{
    PanelCtx, VIEW_PERSPECTIVE_ID, WorkbenchMenuRegistry, WorkbenchSnapshot,
};

mod camera_feed;
mod camera_feed_images;
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
/// Generic workbench host for application menus contributed by Rhai policies.
mod scripted_menus;
/// Typed intent emitted by the authored terrain-progress surface.
#[derive(Event, Clone, Debug)]
struct DismissTerrainOverlay;

/// Transient lifecycle for generic authored dropdown controls. The dropdown
/// identity and option records come from the active runtime UI manifest and
/// typed exposure snapshot; this resource only controls presentation state.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
struct RuntimeUiDropdownState {
    open: Option<String>,
    search: String,
    /// Keep the press that opened the dropdown from immediately closing it
    /// when the matching pointer release reaches egui on the next frame.
    ignore_opening_click: bool,
}

impl SceneResource for RuntimeUiDropdownState {}

fn install_runtime_ui_dropdown_state(app: &mut App) {
    app.init_scene_resource::<RuntimeUiDropdownState>()
        .add_observer(reset_runtime_ui_dropdowns_on_twin_closed);
}

impl RuntimeUiDropdownState {
    fn toggle(&mut self, key: &str) {
        if self.open.as_deref() == Some(key) {
            self.close();
        } else {
            self.open = Some(key.to_owned());
            self.search.clear();
            self.ignore_opening_click = true;
        }
    }

    fn close(&mut self) {
        self.clear_scene();
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
/// sim, physics, scene, cosim, and networking host (installed by the
/// simulation composition plugin)
/// *without* any of this — headless mode omits the renderer and keeps only the
/// simulation-facing asset/type plugins, so nothing here (GPU / window / pointer)
/// is wired.
/// Initial scene request supplied by the application composition root.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct InitialScenePath(pub Option<String>);

/// Window icon bytes prepared by this crate's packaging-only build script.
///
/// Packaging owns rasterization because the icon is also used for desktop
/// metadata. Ordinary UI builds do not install a generated icon.
#[cfg(feature = "package-icons")]
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

fn install_viewport_placeholder_actions(world: &mut World) {
    let mut placeholder = world.resource_mut::<ViewportPlaceholder>();
    set_welcome_actions(&mut placeholder, Vec::new(), None);
}

fn set_welcome_actions(
    placeholder: &mut ViewportPlaceholder,
    policy_actions: Vec<ViewportPlaceholderAction>,
    guidance: Option<String>,
) {
    placeholder.guidance = guidance;
    let mut policy_actions = policy_actions;
    policy_actions.push(ViewportPlaceholderAction {
        label: "Open Twin".into(),
        description: "Choose a Twin project folder from your computer.".into(),
        enabled: true,
        is_available: Arc::new(|| true),
        primary: false,
        activate: Arc::new(open_twin),
    });
    placeholder.actions = policy_actions;
}

/// Exact asset authorities, scoped to the workspace Twin identities.
#[derive(Resource, Default)]
struct PlaceholderTwinMounts(BTreeMap<lunco_workspace::TwinId, String>);

#[derive(Resource, Default)]
struct LastSceneCatalogFacts(
    Option<lunco_scripting_rhai_core::ui_bridge::ApplicationSceneCatalogChanged>,
);

fn remember_placeholder_twin(
    event: On<lunco_assets_runtime::TwinAssetMounted>,
    mut mounts: ResMut<PlaceholderTwinMounts>,
) {
    mounts
        .0
        .insert(event.event().twin, event.event().authority.clone());
}

fn forget_placeholder_twin(
    event: On<lunco_workspace::TwinClosed>,
    mut mounts: ResMut<PlaceholderTwinMounts>,
) {
    mounts.0.remove(&event.event().twin);
}

fn scenario_twin_facts(
    id: lunco_workspace::TwinId,
    twin: &lunco_twin::Twin,
    mounts: &PlaceholderTwinMounts,
    roots: Option<&lunco_assets_core::TwinRoots>,
) -> lunco_scripting_rhai_core::ui_bridge::SceneCatalogTwin {
    let name = twin.manifest.as_ref().map(|manifest| manifest.name.clone());
    let directory = twin
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    let mounted =
        mounts.0.get(&id).cloned().or_else(|| {
            roots.and_then(|roots| roots.authority_for_root(&twin.root).ok().flatten())
        });
    let default_scene = twin
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.usd.as_ref())
        .and_then(|usd| usd.default_scene.as_deref())
        .map(|path| path.replace('\\', "/"));
    lunco_scripting_rhai_core::ui_bridge::SceneCatalogTwin {
        id: id.raw(),
        source_identity: lunco_assets_core::logical_twin_source_identity(twin),
        source_ready: mounted.is_some(),
        name,
        directory,
        default_scene,
    }
}

fn publish_scene_catalog_facts(
    workspace: Res<lunco_workspace::WorkspaceResource>,
    mounts: Res<PlaceholderTwinMounts>,
    manifest: Option<Res<lunco_assets_runtime::discovery::AssetManifest>>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    metadata: Option<Res<lunco_scene_catalog::catalog::AssetMetaStore>>,
    #[cfg(feature = "networking")] downloads: Option<
        Res<lunco_networking_sync::scenario_sync::CachedTwinsRegistry>,
    >,
    current_scene: Res<CurrentScenePath>,
    visibility: Option<Res<lunco_luncosim_edit_ui::ui::asset_visibility::AssetVisibilitySettings>>,
    mut last_facts: ResMut<LastSceneCatalogFacts>,
    mut commands: Commands,
) {
    let inputs_changed = workspace.is_changed()
        || mounts.is_changed()
        || manifest
            .as_ref()
            .is_some_and(|manifest| manifest.is_changed())
        || roots.as_ref().is_some_and(|roots| roots.is_changed())
        || metadata
            .as_ref()
            .is_some_and(|metadata| metadata.is_changed())
        || current_scene.is_changed()
        || visibility
            .as_ref()
            .is_some_and(|visibility| visibility.is_changed())
        || {
            #[cfg(feature = "networking")]
            {
                downloads
                    .as_ref()
                    .is_some_and(|downloads| downloads.is_changed())
            }
            #[cfg(not(feature = "networking"))]
            {
                false
            }
        };
    if !inputs_changed {
        return;
    }

    use lunco_scripting_rhai_core::ui_bridge::{
        ApplicationSceneCatalogChanged, CachedScenarioTwin, SceneCatalogEntry,
    };
    let show_test_assets = visibility
        .as_ref()
        .is_some_and(|settings| settings.show_test_assets);
    let mut facts = ApplicationSceneCatalogChanged {
        current_scene: (!current_scene.0.is_empty())
            .then(|| lunco_assets_core::engine_asset_uri(&current_scene.0)),
        active_twin: None,
        twins: workspace
            .twins()
            .map(|(id, twin)| scenario_twin_facts(id, twin, &mounts, roots.as_deref()))
            .collect(),
        cached_twins: {
            #[cfg(feature = "networking")]
            {
                downloads
                    .as_deref()
                    .map(|registry| {
                        registry
                            .entries
                            .iter()
                            .map(|summary| CachedScenarioTwin {
                                scenario_id: summary.scenario_id_text(),
                                revision: summary.revision_text(),
                                name: summary.name.clone(),
                                default_scene: summary.default_scene.clone(),
                                total_bytes: summary.total_bytes,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            #[cfg(not(feature = "networking"))]
            {
                Vec::<CachedScenarioTwin>::new()
            }
        },
        download_sync_enabled: cfg!(feature = "networking"),
        show_test_assets,
        catalog_ready: manifest.as_ref().is_some_and(|manifest| manifest.ready()),
        catalog_error: None,
        scenes: Vec::new(),
    };
    facts.active_twin = workspace.active_twin.and_then(|id| {
        workspace
            .twin(id)
            .map(|twin| scenario_twin_facts(id, twin, &mounts, roots.as_deref()))
    });

    match (manifest.as_deref(), roots.as_deref()) {
        (None, _) => facts.catalog_error = Some("engine scene inventory is unavailable".into()),
        (Some(manifest), _) if !manifest.ready() => {}
        (Some(_manifest), None) => {
            facts.catalog_error = Some("Twin scene registry is unavailable".into());
        }
        (Some(manifest), Some(roots)) => {
            let twins = workspace.twins().map(|(_, twin)| twin).collect::<Vec<_>>();
            match lunco_assets_runtime::discovery::list_indexed_scene_assets(
                manifest, roots, &twins,
            ) {
                Ok(assets) => {
                    let mut scenes = Vec::with_capacity(assets.len());
                    let mut error = None;
                    for asset in assets {
                        let twin = match asset.twin_authority.as_deref() {
                            None => None,
                            Some(authority) => match roots
                                .logical_identity_for_authority(authority)
                                .ok()
                                .and_then(|identity| {
                                    facts
                                        .twins
                                        .iter()
                                        .find(|twin| twin.source_identity == identity)
                                }) {
                                Some(twin) => Some(twin.clone()),
                                None => {
                                    error = Some(format!(
                                        "scene `{}` has no open workspace Twin owner",
                                        asset.asset_path
                                    ));
                                    break;
                                }
                            },
                        };
                        let uri = lunco_assets_core::asset_path::load_asset_path(
                            &lunco_assets_core::engine_asset_uri(&asset.asset_path),
                            None,
                            None,
                            None,
                        )
                        .and_then(|path| {
                            lunco_assets_core::asset_path::source_uri(&path, Some(roots))
                        });
                        let uri = match uri {
                            Ok(uri) => uri,
                            Err(detail) => {
                                error = Some(detail.to_string());
                                break;
                            }
                        };
                        scenes.push(SceneCatalogEntry {
                            uri,
                            relative_path: asset.rel.clone(),
                            stem: asset.stem,
                            description: metadata
                                .as_deref()
                                .and_then(|metadata| metadata.description(&asset.asset_path))
                                .map(str::to_owned),
                            is_test: lunco_assets_runtime::discovery::is_test_asset(&asset.rel),
                            twin,
                        });
                    }
                    if let Some(error) = error {
                        facts.catalog_error = Some(error);
                    } else {
                        scenes.sort_by(|left, right| left.uri.cmp(&right.uri));
                        facts.scenes = scenes;
                    }
                }
                Err(error) => facts.catalog_error = Some(error.to_string()),
            }
        }
    }
    if last_facts.0.as_ref() != Some(&facts) {
        last_facts.0 = Some(facts.clone());
        commands.trigger(facts);
    }
}

fn sync_viewport_placeholder(
    workspace: Res<lunco_workspace::WorkspaceResource>,
    scripted_menus: Res<scripted_menus::ScriptedMenuBindings>,
    mut placeholder: ResMut<ViewportPlaceholder>,
) {
    if !workspace.is_changed() && !scripted_menus.is_changed() {
        return;
    }
    refresh_viewport_placeholder(&workspace, &scripted_menus, &mut placeholder);
}

pub(super) fn refresh_viewport_placeholder(
    workspace: &lunco_workspace::WorkspaceResource,
    scripted_menus: &scripted_menus::ScriptedMenuBindings,
    placeholder: &mut ViewportPlaceholder,
) {
    let active_twin_id = workspace.active_twin.map(lunco_workspace::TwinId::raw);
    let policy_actions = scripted_menus::viewport_actions_for(scripted_menus, active_twin_id);
    let guidance = scripted_menus::viewport_guidance_for(scripted_menus, active_twin_id);
    if active_twin_id.is_none() {
        set_welcome_actions(placeholder, policy_actions, guidance);
    } else {
        placeholder.guidance = guidance;
        placeholder.actions = policy_actions;
    }
}

fn open_twin(ctx: &mut PanelCtx<'_>) {
    ctx.trigger(lunco_workspace::open::OpenTwin {
        path: String::new(),
    });
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
    if !app.is_plugin_added::<lunco_capture::camera_feed::CameraFeedPlugin>() {
        app.add_plugins(lunco_capture::camera_feed::CameraFeedPlugin);
    }
    camera_feed_images::install(app);
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
        app.add_plugins(lunco_capture::camera_feed::CameraFeedPlugin)
            .init_scene_resource::<camera_feed::CameraFeedUiState>();
        #[cfg(not(target_family = "wasm"))]
        crate::debug_console::install(app);
        camera_feed::install(app);
        if !app.is_plugin_added::<lunco_embodiment_core::roles::EmbodimentCorePlugin>() {
            app.add_plugins(lunco_embodiment_core::roles::EmbodimentCorePlugin);
        }
        if app
            .world()
            .get_resource::<lunco_core::BuildIdentity>()
            .is_none()
        {
            app.insert_resource(lunco_core::BuildIdentity::new(
                self.config.product_version,
                self.config.git_sha,
                self.config.repository_url,
            ));
        }
        #[cfg(all(feature = "package-icons", not(target_arch = "wasm32")))]
        app.add_systems(Update, install_window_icon);
        app.insert_resource(InitialScenePath(self.config.initial_scene.clone()));
        // Winit frame pacing. Continuous while focused lets vsync (Fifo present /
        // requestAnimationFrame on web) act as the frame timer; ReactiveLowPower
        // keeps fans quiet when backgrounded and idle. Networked windows stay Continuous
        // unfocused so lightyear keepalives keep flowing (one of two side-by-side
        // windows is always unfocused; the default ~1 FPS throttle starves the
        // link past timeout). `--no-throttle` forces Continuous for automated
        // tests whose window sits unfocused. WinitSettings is read by the runner
        // each frame, so inserting it from this plugin (after DefaultPlugins) is
        // fine.
        let args: Vec<String> = std::env::args().collect();
        #[cfg(not(target_arch = "wasm32"))]
        crate::native_simulation_host::install(app);
        let networked = args.iter().any(|a| a == "--host" || a == "--connect");
        let no_throttle = args.iter().any(|a| a == "--no-throttle");
        if networked || no_throttle {
            // `ModelicaPlugin::sim_focus_pace` is the last writer of
            // `WinitSettings::unfocused_mode`. Keep the CLI's continuous-rate
            // contract in the shared pacing plane so that pacer cannot restore
            // reactive-low-power after the scene becomes idle.
            app.init_resource::<lunco_core_runtime::FramePacingDemand>();
            app.world_mut()
                .resource_mut::<lunco_core_runtime::FramePacingDemand>()
                .acquire_continuous();
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
        install_runtime_ui_dropdown_state(app);
        dataset_provisioning::install(app);
        app.insert_resource(lunco_workbench_guided_ui::GuidedHudVisibility {
            visible_in_perspective: Some(lunco_workbench_core::PerspectiveId::new(
                VIEW_PERSPECTIVE_ID,
            )),
        });
        // `lunco_render_bevy::LuncoRenderPlugin` owns the wireframe pass used by
        // per-prim contour display. Keep it at the render boundary so this UI
        // composition does not install a duplicate Bevy plugin.
        app
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
                    VIEW_PERSPECTIVE_ID,
                ),
            );
        }
        #[cfg(all(feature = "updates", not(target_arch = "wasm32")))]
        app.add_plugins(lunco_updater::UpdatePlugin);
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
            // Rover panels. ONE closure: Bevy keys plugin uniqueness by type-name,
            // and every `|app| {…}` in this `build` shares the name `{{closure}}` — a
            // second one panics ("plugin already added"). So all app-level panel
            // registration goes here.
            .add_plugins(|app: &mut App| {
                use lunco_workbench_core::WorkbenchPanelAppExt;
                install_viewport_placeholder_actions(app.world_mut());
                app.init_resource::<PlaceholderTwinMounts>()
                    .init_resource::<LastSceneCatalogFacts>()
                    .init_scene_resource::<PendingSceneLoadAction>()
                    .add_observer(remember_placeholder_twin)
                    .add_observer(forget_placeholder_twin)
                    .add_observer(on_scene_source_repair_ready)
                    .add_observer(on_scene_status_action)
                    .add_systems(
                        Update,
                        (publish_scene_catalog_facts, sync_viewport_placeholder).chain(),
                    );
                app.init_resource::<scripted_menus::ScriptedMenuBindings>();
                scripted_menus::register_all_commands(app);
                app.add_observer(on_runtime_ui_action)
                    .add_observer(on_guided_hud_action)
                    .add_observer(on_guided_stop_requested)
                    .add_observer(on_runtime_error_warning)
                    .add_observer(on_dismiss_terrain_overlay)
                    .add_observer(scripted_menus::on_script_ui_request)
                    .add_observer(scripted_menus::invoke_scripted_menu)
                    .add_observer(scripted_menus::clear_scripted_menus_on_twin_closed);
                // Rover-specific panels and the attach-a-model click flow.
                app.register_panel(code_panel::CodePanel);
                // Rhai behaviour editor (Editor). Its view-model is
                // produced each frame from the selection + ScriptRegistry.
                app.register_panel(rhai_editor_panel::RhaiEditorPanel);
                app.init_resource::<rhai_editor_panel::RhaiEditorVm>();
                app.add_systems(Update, rhai_editor_panel::produce_rhai_editor_vm);
                app.register_panel(models_palette::ModelsPalette::default());
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
                // Attach is bevy_picking-driven (observes the same `PointerClick`
                // as selection; egui occlusion handled by the framework).
                app.add_observer(models_palette::on_scene_click_attach);
                app.add_systems(Update, models_palette::attach_escape_system);
            })
            .add_systems(
                Startup,
                (
                    scripted_menus::register_navigation,
                    register_camera_menu,
                    register_downloadable_assets_settings,
                ),
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
            .add_observer(
                |transition: On<lunco_core::SceneTransitionCompleted>,
                 roots: Option<Res<lunco_assets_core::TwinRoots>>,
                 mut current: ResMut<CurrentScenePath>,
                 mut current_name: ResMut<CurrentSceneName>| {
                    match &transition.event().transition {
                        lunco_core::SceneTransition::Load { path, .. }
                        | lunco_core::SceneTransition::Restart { path, .. }
                        | lunco_core::SceneTransition::RuntimeReset { path, .. } => {
                            current.0 = match lunco_assets_core::asset_path::source_uri_for_address(
                                path,
                                roots.as_deref(),
                            ) {
                                Ok(uri) => uri,
                                Err(error) => {
                                    warn!("[scene] cannot publish the scene source URI: {error}");
                                    current.0.clear();
                                    current_name.0.clear();
                                    return;
                                }
                            };
                            current_name.0 = std::path::Path::new(path)
                                .file_name()
                                .and_then(|file| file.to_str())
                                .unwrap_or(path)
                                .to_owned();
                        }
                        lunco_core::SceneTransition::Clear => {
                            current.0.clear();
                            current_name.0.clear();
                        }
                    }
                },
            )
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (
                    camera_feed::draw_camera_feed
                        .in_set(lunco_workbench_core::ApplicationOverlayRenderSet)
                        .run_if(not(recording_offline))
                        .run_if(in_view_perspective),
                    lunco_viz::draw_graph_hud
                        .in_set(lunco_workbench_core::ApplicationOverlayRenderSet)
                        .run_if(not(recording_offline))
                        .run_if(in_view_perspective),
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

        // Share Lunica's panels, clipboard bridge, autosave and worker.
        // Rhai composes task layouts from these registered capabilities.
        app.add_plugins(ModelicaWorkbenchPlugin);

        #[cfg(feature = "sysml")]
        app.add_plugins(lunco_sysml_ui::SysmlUiPlugin);

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
        value.active_perspective()
            == Some(lunco_workbench_core::PerspectiveId::new(
                VIEW_PERSPECTIVE_ID,
            ))
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
        layout.active_perspective()
            == Some(lunco_workbench_core::PerspectiveId::new(
                VIEW_PERSPECTIVE_ID,
            ))
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
    q_bodies: Query<(Entity, &lunco_celestial::CelestialBody)>,
    orbital_pin: Option<Res<lunco_celestial_spatial_core::OrbitalViewPin>>,
    manifest_state: Res<runtime_ui::RuntimeUiManifestState>,
    mut dropdowns: ResMut<RuntimeUiDropdownState>,
    mut commands: Commands,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
) {
    if let Err(error) = trigger
        .event()
        .scope
        .validate(workspace.as_deref(), scene.as_deref())
    {
        warn!("[runtime-ui] {error}");
        return;
    }
    match trigger.event().action.as_str() {
        "view.surface" => {
            if let Ok(target) = q_avatar.single() {
                if orbital_pin.is_some_and(|pin| pin.active) {
                    commands.trigger(lunco_camera_core::ReturnFromOrbit { camera: target });
                } else {
                    commands.trigger(ObserveAvatar {});
                }
            } else {
                report_runtime_ui_failure(
                    &mut commands,
                    "the scene has no local avatar camera to observe",
                );
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
            let parameters = match runtime_ui_action_parameters_to_telemetry(
                &trigger.event().parameters,
                "runtime.ui.action.parameters",
            ) {
                Ok(parameters) => parameters,
                Err(error) => {
                    report_runtime_ui_failure(&mut commands, &error);
                    return;
                }
            };
            let action_args = lunco_telemetry_core::TelemetryValue::Map(BTreeMap::from([
                (
                    "action".to_owned(),
                    lunco_telemetry_core::TelemetryValue::String(action.clone()),
                ),
                ("parameters".to_owned(), parameters),
            ]));
            let binding = match trigger.event().scope.lifetime {
                lunco_core::InteractionLifetime::Owner => {
                    lunco_scripting_bridge_core::ToolHookBinding::for_owner(
                        "runtime_ui".to_owned(),
                        "on_action".to_owned(),
                        trigger.event().scope.owner_twin_id,
                        lunco_scripting_bridge_core::ToolHookBindingRetention::Invocation,
                    )
                }
                lunco_core::InteractionLifetime::Scene(_) => {
                    lunco_scripting_bridge_core::ToolHookBinding::for_scene(
                        "runtime_ui".to_owned(),
                        "on_action".to_owned(),
                        trigger.event().scope.owner_twin_id,
                        lunco_scripting_bridge_core::ToolHookBindingRetention::Invocation,
                        scene.as_deref(),
                    )
                }
            };
            let binding = match binding {
                Ok(binding) => binding,
                Err(error) => {
                    report_runtime_ui_failure(&mut commands, &error);
                    return;
                }
            };
            let entity = commands.spawn(binding).id();
            if trigger.event().source != Entity::PLACEHOLDER {
                commands
                    .entity(entity)
                    .insert(ChildOf(trigger.event().source));
            }
            let binding = Some(entity);
            commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
                binding,
                tool: "runtime_ui".to_owned(),
                hook: "on_action".to_owned(),
                args: action_args.clone(),
                owner_twin_id: trigger.event().scope.owner_twin_id,
            });
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: "runtime.ui.action".to_owned(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Info,
                data: action_args,
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
        }
    }
}

/// Dispatch a guided-HUD button through the same typed Rhai tool-hook path as
/// other authored UI actions. The guided presentation crate remains decoupled
/// from the scripting runtime and carries only the semantic action id.
fn on_guided_hud_action(
    trigger: On<lunco_workbench_guided_ui::GuidedHudActionRequested>,
    bindings: Query<&lunco_scripting_bridge_core::ToolHookBinding>,
    mut commands: Commands,
) {
    let action = trigger.event();
    let Ok(binding) = bindings.get(action.binding) else {
        warn!("[guided-hud] callback binding has been retired");
        return;
    };
    commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
        binding: Some(action.binding),
        tool: binding.tool.clone(),
        hook: binding.hook.clone(),
        args: lunco_telemetry_core::TelemetryValue::String(action.action.clone()),
        owner_twin_id: binding.owner_twin_id,
    });
}

/// Stop the scenario that owns the active coach tour. The guided overlay only
/// emits this intent; the application host owns scenario lifecycle commands.
fn on_guided_stop_requested(
    _trigger: On<lunco_workbench_guided_ui::GuidedStopRequested>,
    mut commands: Commands,
    mut overlay: ResMut<lunco_workbench_guided_ui::GuidedOverlay>,
    entities: Option<Res<lunco_api::registry::ApiEntityRegistry>>,
    scenarios: Query<(), With<lunco_scripting::ScriptedModel>>,
) {
    let Some(owner) = overlay.owner_script else {
        commands.trigger(lunco_notifications_core::ShowNotification {
            text: "This guided activity has no active scenario to stop.".into(),
            kind: "warn".into(),
            secs: 0.0,
        });
        return;
    };
    let Some(target) = entities
        .as_deref()
        .and_then(|registry| registry.resolve(&owner))
    else {
        overlay.clear();
        commands.trigger(lunco_notifications_core::ShowNotification {
            text: "The guided activity's scenario is no longer available.".into(),
            kind: "warn".into(),
            secs: 0.0,
        });
        return;
    };
    if scenarios.get(target).is_err() {
        overlay.clear();
        commands.trigger(lunco_notifications_core::ShowNotification {
            text: "The guided activity's scenario is no longer running.".into(),
            kind: "warn".into(),
            secs: 0.0,
        });
        return;
    }

    commands.trigger(lunco_scripting::StopScenario { target });
    overlay.clear();
}

/// Present recoverable runtime faults through the shared notification surface.
/// Domain menus keep their controls and selection state; the warning is owned
/// by the application-wide notification layer.
fn on_runtime_error_warning(trigger: On<lunco_core::RuntimeError>, mut commands: Commands) {
    if trigger.event().name == "scene-load-failed" {
        return;
    }
    commands.trigger(lunco_notifications_core::ShowNotification {
        text: trigger.event().message.clone(),
        kind: "warn".to_owned(),
        secs: 0.0,
    });
}

const USD_SCENE_STATUS_ACTION: &str = "USD scene";

#[derive(Resource, Default)]
struct PendingSceneLoadAction(Option<(String, String)>);

impl SceneResource for PendingSceneLoadAction {
    fn clear_scene(&mut self) {
        self.0 = None;
    }
}

fn on_scene_source_repair_ready(
    trigger: On<lunco_usd_bevy_runtime_core::SceneSourceRepairReady>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    mut pending: ResMut<PendingSceneLoadAction>,
    mut status: ResMut<lunco_status_core::status_bus::StatusBus>,
) {
    let event = trigger.event();
    let source_uri = roots.as_deref().and_then(|roots| {
        lunco_assets_core::asset_path::source_uri_for_address(&event.path, Some(roots)).ok()
    });
    let Some(source_uri) = source_uri else {
        pending.0 = None;
        status.push(
            USD_SCENE_STATUS_ACTION,
            lunco_status_core::status_bus::StatusLevel::Error,
            "The Twin source is no longer mounted. Reopen it before continuing.",
        );
        return;
    };
    pending.0 = Some((source_uri.clone(), event.root_prim.clone()));
    status.push(
        USD_SCENE_STATUS_ACTION,
        lunco_status_core::status_bus::StatusLevel::Attention,
        format!(
            "The replacement source is valid. The scene remains stopped; choose Continue to load `{}`.",
            source_uri
        ),
    );
}

fn on_scene_status_action(
    trigger: On<lunco_status_core::status_bus::StatusBarAction>,
    mut pending: ResMut<PendingSceneLoadAction>,
    mut commands: Commands,
) {
    if trigger.event().source != USD_SCENE_STATUS_ACTION {
        return;
    }
    let Some((path, root_prim)) = pending.0.take() else {
        return;
    };
    commands.trigger(lunco_usd_bevy_runtime_core::scene::LoadScene { path, root_prim });
}

fn runtime_ui_action_parameters_to_telemetry(
    value: &lunco_hooks::HookValue,
    path: &str,
) -> Result<lunco_telemetry_core::TelemetryValue, String> {
    use lunco_hooks::HookValue;
    use lunco_telemetry_core::TelemetryValue;

    let HookValue::Map(fields) = value else {
        return Err(format!("{path} must be a map, got {}", value.type_name()));
    };
    let mut result = BTreeMap::new();
    for (key, value) in fields {
        if result
            .insert(
                key.clone(),
                runtime_ui_action_value_to_telemetry(value, &format!("{path}.{key}"))?,
            )
            .is_some()
        {
            return Err(format!("{path} contains `{key}` more than once"));
        }
    }
    Ok(TelemetryValue::Map(result))
}

fn runtime_ui_action_value_to_telemetry(
    value: &lunco_hooks::HookValue,
    path: &str,
) -> Result<lunco_telemetry_core::TelemetryValue, String> {
    use lunco_hooks::HookValue;
    use lunco_telemetry_core::TelemetryValue;

    Ok(match value {
        HookValue::Unit => return Err(format!("{path} cannot contain unit values")),
        HookValue::Int(value) => TelemetryValue::I64(*value),
        HookValue::UInt(value) => TelemetryValue::U64(*value),
        HookValue::Float(value) => TelemetryValue::F64(*value),
        HookValue::Bool(value) => TelemetryValue::Bool(*value),
        HookValue::Str(value) => TelemetryValue::String(value.clone()),
        HookValue::Array(values) => TelemetryValue::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    runtime_ui_action_value_to_telemetry(value, &format!("{path}[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        HookValue::Map(fields) => {
            let mut result = BTreeMap::new();
            for (key, value) in fields {
                if result
                    .insert(
                        key.clone(),
                        runtime_ui_action_value_to_telemetry(value, &format!("{path}.{key}"))?,
                    )
                    .is_some()
                {
                    return Err(format!("{path} contains `{key}` more than once"));
                }
            }
            TelemetryValue::Map(result)
        }
        HookValue::Bytes(_) => {
            return Err(format!("{path} cannot contain byte buffers"));
        }
    })
}

fn runtime_focus_body(
    ephemeris_id: i32,
    q_bodies: &Query<(Entity, &lunco_celestial::CelestialBody)>,
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
    lunco_core::trigger_runtime_error(commands, "runtime-ui-action-failed", message);
}

fn reset_runtime_ui_dropdowns_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut dropdowns: ResMut<RuntimeUiDropdownState>,
) {
    if trigger.was_active {
        dropdowns.close();
    }
}

/// Labels for the native Camera menu remain authored camera-domain commands.
/// The in-scene dropdown below is generic and reads its option records from the
/// runtime exposure snapshot.
fn runtime_ui_dropdown_options(
    ui: &mut egui::Ui,
    exposure: &lunco_exposure_core::ExposureSurface,
    definition: &runtime_ui::RuntimeUiDropdownDefinition,
    selected_key: Option<&str>,
    search: &str,
) -> Option<String> {
    let Some(lunco_exposure_core::ExposureValue::Array(values)) =
        exposure.properties.get(&definition.source)
    else {
        ui.label("No authored options are available.");
        return None;
    };

    let mut selected = None;
    let search = search.to_lowercase();
    let mut previous_group = None;
    let mut shown = 0;
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
        let group = definition.group.as_ref().and_then(|field| {
            fields
                .iter()
                .find(|(name, _)| name == field)
                .map(|(_, value)| value.as_str())
        });
        if !search.is_empty()
            && ![key, label, group.unwrap_or("")]
                .iter()
                .any(|value| value.to_lowercase().contains(&search))
        {
            continue;
        }
        if let Some(group) = group {
            if previous_group.as_deref() != Some(group) {
                if shown > 0 {
                    ui.separator();
                }
                ui.strong(group);
                previous_group = Some(group.to_owned());
            }
        }
        shown += 1;
        if ui
            .selectable_label(selected_key == Some(key), label)
            .on_hover_text(key)
            .clicked()
        {
            selected = Some(action.to_owned());
        }
    }
    if shown == 0 {
        ui.weak("No matching options.");
    }
    selected
}

fn runtime_ui_dimension(
    exposure: &lunco_exposure_core::ExposureSurface,
    source: &str,
) -> Option<f32> {
    let value = exposure.properties.get(source)?;
    let pixels = match value {
        lunco_exposure_core::ExposureValue::Number(value) => *value as f32,
        lunco_exposure_core::ExposureValue::Text(value) => value
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
    exposures: Res<lunco_exposure_core::EngineExposures>,
    roots: Query<(Entity, &runtime_ui::RuntimeUiSurface, &Visibility)>,
    manifest_state: Res<runtime_ui::RuntimeUiManifestState>,
    layout: Option<Res<WorkbenchSnapshot>>,
    theme: Option<Res<lunco_theme::Theme>>,
    mut pick_gate: ResMut<lunco_workbench_core::scene_pick::ScenePickGate>,
    mut commands: Commands,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
) {
    if !layout.is_some_and(|layout| {
        layout.active_perspective()
            == Some(lunco_workbench_core::PerspectiveId::new(
                VIEW_PERSPECTIVE_ID,
            ))
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
    let Some((source, anchor)) = roots.iter().find_map(|(entity, surface, visibility)| {
        (surface.namespace() == surface_definition.namespace
            && matches!(*visibility, Visibility::Visible)
            && surface.is_mounted())
        .then(|| surface.applied_rect().map(|rect| (entity, rect)))
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
        .and_then(lunco_exposure_core::ExposureValue::scalar_render);
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

    let popup = egui::Popup::new(
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
        if let Some(hint) = &definition.search_hint {
            ui.add(
                egui::TextEdit::singleline(&mut dropdowns.search)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );
            ui.separator();
        }
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .show(ui, |ui| {
                selected_action = runtime_ui_dropdown_options(
                    ui,
                    exposure,
                    definition,
                    selected_key.as_deref(),
                    &dropdowns.search,
                );
            });
    });
    if let Some(popup) = popup {
        pick_gate.record_chrome_panel(popup.response.rect, popup.response.rect);
    }

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
        commands.trigger(runtime_ui::RuntimeUiAction {
            scope: runtime_ui::RuntimeUiActionScope::capture(
                workspace.as_deref(),
                scene.as_deref(),
            ),
            action,
            source,
            parameters: lunco_hooks::HookValue::Map(Vec::new()),
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
        if state.is_some() {
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
                .resource::<lunco_exposure_core::EngineExposures>()
                .and_then(|exposures| exposures.surfaces.get("camera-status"));
            if let (Some(dropdown), Some(exposure)) = (dropdown, exposure) {
                let selected_key = dropdown
                    .selected_source
                    .as_deref()
                    .and_then(|source| exposure.properties.get(source))
                    .and_then(lunco_exposure_core::ExposureValue::scalar_render);
                if let Some(action) =
                    runtime_ui_dropdown_options(ui, exposure, dropdown, selected_key.as_deref(), "")
                {
                    let scope = runtime_ui::RuntimeUiActionScope::capture(
                        ctx.resource::<lunco_workspace::WorkspaceResource>(),
                        ctx.resource::<lunco_core::SceneTransitionCoordinator>(),
                    );
                    ctx.trigger(runtime_ui::RuntimeUiAction {
                        scope,
                        action,
                        source: Entity::PLACEHOLDER,
                        parameters: lunco_hooks::HookValue::Map(Vec::new()),
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
                let scope = runtime_ui::RuntimeUiActionScope::capture(
                    ctx.resource::<lunco_workspace::WorkspaceResource>(),
                    ctx.resource::<lunco_core::SceneTransitionCoordinator>(),
                );
                ctx.trigger(runtime_ui::RuntimeUiAction {
                    scope,
                    action: action.to_owned(),
                    source: Entity::PLACEHOLDER,
                    parameters: lunco_hooks::HookValue::Map(Vec::new()),
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
    library: Option<bevy::prelude::Res<lunco_assets_runtime::library::LibraryLoadState>>,
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
            Some(lunco_assets_runtime::library::LibraryLoadState::Ready { .. })
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

#[cfg(all(feature = "package-icons", not(target_arch = "wasm32")))]
fn install_window_icon(
    mut created: MessageReader<bevy::window::WindowCreated>,
    windows: Query<Entity, With<bevy::window::PrimaryWindow>>,
    icon: Res<WindowIconBytes>,
    _main_thread: bevy::ecs::system::NonSendMarker,
) {
    bevy_winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
        for event in created.read() {
            if windows.get(event.window).is_err() {
                continue;
            }
            let Some(window) = winit_windows.get_window(event.window) else {
                warn!("[window] created primary window has no winit window");
                continue;
            };
            match winit::window::Icon::from_rgba(icon.0.to_vec(), 64, 64) {
                Ok(icon) => {
                    window.set_window_icon(Some(icon));
                    info!("[window] requested LunCoSim icon");
                }
                Err(error) => warn!("[window] failed to install LunCoSim icon: {error}"),
            }
        }
    });
}

/// Render one dataset as a compact card with its available action inline.
fn render_download_asset_row(
    ui: &mut bevy_egui::egui::Ui,
    row: &dataset_provisioning::ProvisioningRow,
    theme: Option<&lunco_theme::Theme>,
    actions: &mut Vec<(String, String, String, String)>,
) {
    use bevy_egui::egui;

    let status_color = theme.map_or_else(
        || ui.visuals().text_color(),
        |theme| {
            if row.ready {
                theme.tokens.success
            } else if !row.detail.is_empty() {
                theme.tokens.error
            } else if row.action == "cancel" {
                theme.tokens.accent
            } else {
                theme.tokens.text_subdued
            }
        },
    );
    let frame = match theme {
        Some(theme) => egui::Frame::group(ui.style())
            .corner_radius(egui::CornerRadius::same(theme.rounding.panel as u8))
            .inner_margin(egui::Margin::same(theme.spacing.window_padding as i8)),
        None => egui::Frame::group(ui.style()),
    };

    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(&row.label).strong());
                ui.colored_label(status_color, &row.status);
            });
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if row.action_display != "none"
                        && row.action != "dismiss"
                        && ui.button(&row.action_label).clicked()
                    {
                        actions.push((
                            row.action.clone(),
                            row.id.clone(),
                            row.scope.clone(),
                            String::new(),
                        ));
                    }
                    if !row.rebuild_action.is_empty()
                        && ui
                            .small_button(&row.rebuild_label)
                            .on_hover_text(
                                "Rerun every declared processing stage, even when cached output is current.",
                            )
                            .clicked()
                    {
                        actions.push((
                            row.rebuild_action.clone(),
                            row.id.clone(),
                            row.scope.clone(),
                            String::new(),
                        ));
                    }
                },
            );
        });

        if !row.ready {
            ui.horizontal_wrapped(|ui| {
                let metadata_style = lunco_theme::TypographyRole::Caption.text_style();
                ui.label(
                    egui::RichText::new(&row.download_status)
                        .text_style(metadata_style.clone())
                        .weak(),
                );
                if !row.preparation_status.is_empty() {
                    ui.label(
                        egui::RichText::new(&row.preparation_status)
                            .text_style(metadata_style)
                            .weak(),
                    );
                }
            });
        }

        ui.collapsing(&row.details_label, |ui| {
            if !row.detail.is_empty() {
                let error_color = theme.map_or_else(
                    || ui.visuals().error_fg_color,
                    |theme| theme.tokens.error,
                );
                ui.colored_label(error_color, &row.detail);
            }
            ui.label(egui::RichText::new("Source files").strong());
            ui.label(&row.source_files);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(&row.source_url).weak());
                if ui.link("Open source ↗").clicked() {
                    actions.push((
                        "open_web_url".to_owned(),
                        row.id.clone(),
                        row.scope.clone(),
                        row.source_url.clone(),
                    ));
                }
            });
        });
    });
}

/// Settings ▸ Data & libraries — the registry-backed view of downloadable
/// engine and Twin assets. Manifest additions appear here without UI changes.
fn register_downloadable_assets_settings(world: &mut World) {
    use bevy_egui::egui;
    use lunco_assets_datasets::DatasetRegistry;
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_settings_submenu("Data & libraries", |ui, ctx| {
        ui.set_min_width(560.0);
        ui.set_max_width(740.0);

        let theme = ctx.resource::<lunco_theme::Theme>().cloned();
        ui.label(
            egui::RichText::new("Downloads & libraries")
                .text_style(lunco_theme::TypographyRole::Section.text_style()),
        );
        ui.label(
            egui::RichText::new(
                "Scene assets download when needed. DejaVu Sans ships with native and web builds.",
            )
            .weak(),
        );

        let Some(mut settings) = ctx.resource::<lunco_settings::DownloadSettings>().cloned() else {
            return;
        };
        let original_settings = settings.clone();
        let mut actions = Vec::new();
        let item_spacing = theme
            .as_ref()
            .map_or(ui.spacing().item_spacing.y, |theme| theme.spacing.item_spacing);

        let registry = ctx.resource::<DatasetRegistry>();
        if let Some(registry) = registry {
            if !registry.is_scope_scanned(&lunco_assets_datasets::DatasetScope::Engine) {
                ui.add_space(item_spacing);
                ui.label(egui::RichText::new("Loading asset catalog…").weak());
            } else if registry.entries().is_empty() {
                ui.add_space(item_spacing);
                ui.label(egui::RichText::new("No downloadable assets are declared.").weak());
            } else if let Some(projection) =
                ctx.resource::<dataset_provisioning::DatasetProvisioningState>()
            {
                let groups = projection.settings_groups.clone();
                let rows: Vec<_> = groups
                    .iter()
                    .filter_map(|group| group.rows.as_ref().ok())
                    .flat_map(|rows| rows.iter())
                    .collect();
                let ready = rows.iter().filter(|row| row.ready).count();
                let in_progress = rows.iter().filter(|row| row.action == "cancel").count();
                let has_scene = ctx
                    .resource::<lunco_assets_datasets::SceneDatasetRequirements>()
                    .is_some_and(|requirements| requirements.scene.is_some());

                ui.add_space(item_spacing);
                ui.horizontal_wrapped(|ui| {
                    if let Some(theme) = theme.as_ref() {
                        ui.colored_label(theme.tokens.success, format!("{ready} ready"));
                        ui.separator();
                        ui.colored_label(
                            theme.tokens.accent,
                            format!("{in_progress} in progress"),
                        );
                        ui.separator();
                        ui.label(format!("{} assets", rows.len()));
                    } else {
                        ui.label(format!("{ready} ready"));
                        ui.separator();
                        ui.label(format!("{in_progress} in progress"));
                        ui.separator();
                        ui.label(format!("{} assets", rows.len()));
                    }
                    if has_scene
                        && ui
                            .small_button("Reload scene")
                            .on_hover_text("Use rebuilt assets in the current scene.")
                            .clicked()
                    {
                        actions.push((
                            "reload_asset_scene".to_owned(),
                            String::new(),
                            String::new(),
                            String::new(),
                        ));
                    }
                });

                let filter_id = egui::Id::new("downloadable-data-filter");
                let mut filter = ui.data_mut(|data| {
                    data.get_persisted::<String>(filter_id).unwrap_or_default()
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut filter)
                            .hint_text("Search assets or source files")
                            .desired_width(f32::INFINITY),
                    );
                    if !filter.is_empty() && ui.small_button("Clear").clicked() {
                        filter.clear();
                    }
                });
                ui.data_mut(|data| data.insert_persisted(filter_id, filter.clone()));
                let filter = filter.to_lowercase();
                let mut visible_groups = false;

                for group in groups {
                    let rows = match group.rows {
                        Ok(rows) => rows,
                        Err(error) => {
                            visible_groups = true;
                            let color = theme.as_ref().map_or_else(
                                || ui.visuals().error_fg_color,
                                |theme| theme.tokens.error,
                            );
                            ui.colored_label(color, error);
                            continue;
                        }
                    };
                    let rows: Vec<_> = rows
                        .iter()
                        .filter(|row| {
                            filter.is_empty()
                                || format!("{} {} {}", group.owner, row.label, row.source_files)
                                    .to_lowercase()
                                    .contains(&filter)
                        })
                        .collect();
                    if rows.is_empty() {
                        continue;
                    }
                    visible_groups = true;
                    let ready = rows.iter().filter(|row| row.ready).count();
                    egui::CollapsingHeader::new(format!(
                        "{}  ·  {ready}/{} ready",
                        group.owner,
                        rows.len()
                    ))
                    .id_salt(("asset-scope", group.scope.clone(), group.owner.clone()))
                    .default_open(
                        group.owner == "fonts"
                            || group.scope.starts_with("twin:")
                            || !filter.is_empty(),
                    )
                    .show(ui, |ui| {
                        for row in rows {
                            render_download_asset_row(ui, row, theme.as_ref(), &mut actions);
                            if let Some(theme) = theme.as_ref() {
                                ui.add_space(theme.spacing.item_spacing);
                            }
                        }
                    });
                }
                if !visible_groups {
                    ui.label(egui::RichText::new("No assets match this search.").weak());
                }
            } else {
                ui.label("Asset presentation is unavailable.");
            }
        } else {
            ui.label(
                egui::RichText::new("Dataset registry is not installed.")
                    .weak()
                    .italics(),
            );
        }

        ui.separator();
        egui::CollapsingHeader::new("Advanced settings").show(ui, |ui| {
            egui::CollapsingHeader::new("Transfer settings").show(ui, |ui| {
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
                        "These shared limits apply to asset, source-library, scenario, and app-update requests. Attempts include the first request; delays grow exponentially up to the configured cap.",
                    )
                    .weak(),
                );
            });

            if let Some(mut processing) =
                ctx.resource::<lunco_settings::AssetProcessingSettings>().cloned()
            {
                let original = processing.clone();
                egui::CollapsingHeader::new("Texture preparation").show(ui, |ui| {
                    let mut mib = processing.texture_decode_max_bytes as f64 / 1048576.0;
                    ui.horizontal(|ui| {
                        ui.label("Decoder memory limit:");
                        if ui
                            .add(
                                egui::DragValue::new(&mut mib)
                                    .speed(64.0)
                                    .range(1.0..=8796093022207.0)
                                    .suffix(" MiB"),
                            )
                            .changed()
                        {
                            processing.texture_decode_max_bytes = (mib * 1048576.0).round() as u64;
                        }
                        if ui.small_button("1 GiB").clicked() {
                            processing.texture_decode_max_bytes = 1 << 30;
                        }
                        if ui.small_button("2 GiB").clicked() {
                            processing.texture_decode_max_bytes = 2 << 30;
                        }
                    });
                    ui.label(
                        "Default: 1 GiB per decoder. Applies to new preparation jobs; resampling uses additional memory.",
                    );
                    ui.label(
                        "Twin override: assets.texture_decode_max_bytes in Twin settings (bytes). An asset's max_decode_bytes takes precedence.",
                    );
                });
                if processing != original {
                    ctx.set_resource(processing);
                }
            }
        });

        if settings != original_settings {
            ctx.set_resource(settings);
        }
        for (action, id, scope, url) in actions {
            use lunco_telemetry_core::TelemetryValue;
            ctx.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
                tool: "runtime_ui".to_owned(),
                hook: "on_action".to_owned(),
                binding: None,
                owner_twin_id: None,
                args: TelemetryValue::Map(std::collections::BTreeMap::from([
                    ("action".into(), TelemetryValue::String(action)),
                    (
                        "parameters".into(),
                        TelemetryValue::Map(std::collections::BTreeMap::from([
                            ("id".into(), TelemetryValue::String(id)),
                            ("scope".into(), TelemetryValue::String(scope)),
                            ("url".into(), TelemetryValue::String(url)),
                        ])),
                    ),
                ])),
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{RuntimeUiDropdownState, runtime_ui_dimension};
    use lunco_exposure_core::{ExposureSurface, ExposureValue};
    use std::collections::HashMap;

    #[derive(bevy::prelude::Resource, Default)]
    struct RequestedScene(Option<(String, String)>);

    fn record_requested_scene(
        trigger: bevy::prelude::On<lunco_usd_bevy_runtime_core::scene::LoadScene>,
        mut requested: bevy::prelude::ResMut<RequestedScene>,
    ) {
        requested.0 = Some((
            trigger.event().path.clone(),
            trigger.event().root_prim.clone(),
        ));
    }

    #[test]
    fn repaired_scene_waits_for_the_recent_events_action() {
        use bevy::prelude::*;
        use lunco_status_core::status_bus::{StatusBarAction, StatusBus, StatusLevel};

        let roots = lunco_assets_core::TwinRoots::default();
        let authority = roots
            .register_authority("demo", std::env::temp_dir())
            .expect("register Twin source");
        let mut app = App::new();
        app.insert_resource(super::PendingSceneLoadAction::default())
            .insert_resource(StatusBus::default())
            .insert_resource(roots)
            .insert_resource(RequestedScene::default())
            .add_observer(super::on_scene_source_repair_ready)
            .add_observer(super::on_scene_status_action)
            .add_observer(record_requested_scene);

        app.world_mut()
            .trigger(lunco_usd_bevy_runtime_core::SceneSourceRepairReady {
                path: lunco_assets_core::twin_uri(&authority, "scenes/mission.usda"),
                root_prim: "/Mission".to_owned(),
            });
        app.world_mut().flush();

        assert_eq!(app.world().resource::<RequestedScene>().0, None);
        let status = app.world().resource::<StatusBus>();
        let event = status
            .history()
            .last()
            .expect("repair action is in Recent Events");
        assert_eq!(event.source, "USD scene");
        assert_eq!(event.level, StatusLevel::Attention);
        assert!(event.message.contains("scene remains stopped"));
        assert!(event.message.contains("choose Continue"));
        assert!(event.message.contains("twin://demo/scenes/mission.usda"));
        assert!(!event.message.contains(&authority));

        app.world_mut().trigger(StatusBarAction {
            source: "USD scene",
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<RequestedScene>().0,
            Some((
                "twin://demo/scenes/mission.usda".to_owned(),
                "/Mission".to_owned(),
            ))
        );
        assert_eq!(
            app.world().resource::<super::PendingSceneLoadAction>().0,
            None
        );
    }

    #[test]
    fn scene_lifecycle_retires_dropdown_state_and_preserves_inactive_twin_closure() {
        use bevy::prelude::*;
        let mut app = App::new();
        super::install_runtime_ui_dropdown_state(&mut app);
        app.add_systems(
            lunco_core::SceneTeardown,
            |mut state: ResMut<RuntimeUiDropdownState>| {
                state.toggle("shutdown_dropdown");
            },
        );
        for _ in 0..3 {
            app.world_mut()
                .resource_mut::<RuntimeUiDropdownState>()
                .toggle("active_dropdown");
            let closed = lunco_workspace::TwinClosed {
                twin: lunco_workspace::TwinId::new(7),
                root: Default::default(),
                was_active: false,
            };
            app.world_mut().trigger(closed.clone());
            assert_eq!(
                app.world()
                    .resource::<RuntimeUiDropdownState>()
                    .open
                    .as_deref(),
                Some("active_dropdown")
            );
            lunco_core::run_scene_teardown(app.world_mut());
            assert_eq!(
                *app.world().resource::<RuntimeUiDropdownState>(),
                RuntimeUiDropdownState::default()
            );
            app.world_mut()
                .resource_mut::<RuntimeUiDropdownState>()
                .toggle("active_dropdown");
            app.world_mut().trigger(lunco_workspace::TwinClosed {
                was_active: true,
                ..closed
            });
            assert_eq!(
                *app.world().resource::<RuntimeUiDropdownState>(),
                RuntimeUiDropdownState::default()
            );
        }
    }

    #[derive(bevy::prelude::Resource, Default)]
    struct GuidedStopTarget(Option<bevy::prelude::Entity>);

    fn record_guided_stop_target(
        trigger: bevy::prelude::On<lunco_scripting::StopScenario>,
        mut target: bevy::prelude::ResMut<GuidedStopTarget>,
    ) {
        target.0 = Some(trigger.event().target);
    }

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
    fn guided_stop_targets_the_authored_scenario_and_clears_the_coach() {
        let mut app = bevy::prelude::App::new();
        let owner = lunco_core::GlobalEntityId::from_raw(120);
        let target = app
            .world_mut()
            .spawn(lunco_scripting::ScriptedModel::default())
            .id();
        let mut entities = lunco_api::registry::ApiEntityRegistry::default();
        entities.assign(target, owner);
        app.insert_resource(entities)
            .insert_resource(GuidedStopTarget::default())
            .insert_resource(lunco_workbench_guided_ui::GuidedOverlay {
                owner_script: Some(owner),
                tour: Some(lunco_workbench_guided_ui::TourStep {
                    title: "Authored scenario".into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .add_observer(super::on_guided_stop_requested)
            .add_observer(record_guided_stop_target);

        app.world_mut()
            .trigger(lunco_workbench_guided_ui::GuidedStopRequested);
        app.world_mut().flush();

        assert_eq!(app.world().resource::<GuidedStopTarget>().0, Some(target));
        assert!(
            app.world()
                .resource::<lunco_workbench_guided_ui::GuidedOverlay>()
                .tour
                .is_none()
        );
    }
}
