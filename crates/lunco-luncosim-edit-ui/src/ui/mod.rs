//! UI for the luncosim editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_control_core::ControlBinding;
use lunco_control_core::ControlLink;
use lunco_core::SceneMountState;
use lunco_embodiment_core::roles::{Embodiment, TheLocalEmbodiment};
use lunco_luncosim_edit_gizmo_ui as edit_gizmo;
use lunco_port_core::InputPorts;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_workbench::{ViewportPanel, WorkbenchAppExt};
use lunco_workbench_core::view_model::{ViewModelAppExt, ViewModelSet};
use lunco_workbench_core::{PerspectiveId, WorkbenchPanelAppExt};
use lunco_workbench_help_ui::{
    HelpMouse, HelpShortcut, LiveHelpSection, LiveHelpSections, PerspectiveHelp,
};

pub mod asset_visibility;
pub(crate) mod authoring_paths;
/// Unified human-facing authoring evidence: target chain, diagnostics, and
/// reusable temporary inspection layers.
pub mod authoring_review;
/// Screen-space labels a prim authored for itself (`lunco:billboard*`).
pub mod billboard_overlay;
/// Cinematic camera authoring — capture the current view as a `def Camera`
/// prim. The current camera-path contract is in
/// `docs/architecture/51-cinematic-camera.md`; capture and transport live in
/// the docked Cinematic panel.
pub mod cinematic;
/// Command Deck panel — the generic read+control surface for the selected
/// entity. Domain behavior is authored by the active Twin.
pub mod command_deck;
pub mod connection_canvas;
pub mod entity_list;
/// Joint State panel — live joint θ / ω / target / τ table for the selected
/// vessel (revolute joints + raycast wheels + steering), deep-review §2.7.
pub mod joint_state;
/// Universal runtime port inspection and manual control surface.
pub mod ports;
/// Generic right-click menus for USD-authored transparent markers.
pub mod scene_context;
/// Generic script-authored scene context-menu host.
pub mod scene_context_menu;
pub(crate) mod selection_context;
pub mod spawn_palette;
pub mod terrain_tools;
/// Bounded, terrain-conforming motion trails for topology-derived vehicles.
pub mod trail;
pub use trail::VehicleTrailPlugin;
#[derive(Clone, Default)]
pub(crate) struct EditorSessionSelection {
    /// Canonical composed prim paths. Bevy entities are only a live projection
    /// and are not retained across a preview reload.
    pub(crate) paths: Vec<String>,
    /// Canonical path of the Inspector's drilled target, if any.
    pub(crate) target_path: Option<String>,
}

/// Session-owned editor selection. `SelectedEntities` remains the shared ECS
/// highlight/gizmo projection, while this resource owns which preview lease
/// that projection belongs to and restores it after focus changes.
#[derive(Resource, Default)]
pub(crate) struct EditorSessionSelections {
    pub(crate) sessions: HashMap<lunco_usd_viewport_core::UsdPreviewId, EditorSessionSelection>,
    /// Live-scene selection remains entity-keyed because it is not an authored
    /// USD preview lease and never crosses into an Editor preview.
    live: LiveSceneSelection,
}

impl lunco_core::SceneResource for EditorSessionSelections {
    fn clear_scene(&mut self) {
        *self = Self {
            // Preview selection belongs to its document lease.
            sessions: std::mem::take(&mut self.sessions),
            ..Default::default()
        };
    }
}

#[derive(Clone, Default)]
struct LiveSceneSelection {
    entities: Vec<Entity>,
    target: Option<Entity>,
}

fn preview_path_for_entity(
    entity: Entity,
    preview: &lunco_usd_viewport_core::UsdPreviewSession,
    q_paths: &Query<(Entity, &UsdPrimPath)>,
    q_parents: &Query<&ChildOf>,
) -> Option<String> {
    q_paths.get(entity).ok().and_then(|(_, path)| {
        (path.stage_handle.id() == preview.stage_handle().id()
            && lunco_usd_bevy_scene::is_preview_entity(entity, preview.scene_root(), q_parents))
        .then(|| path.path.clone())
    })
}

fn preview_entity_for_path(
    path: &str,
    preview: &lunco_usd_viewport_core::UsdPreviewSession,
    q_paths: &Query<(Entity, &UsdPrimPath)>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut matches = q_paths.iter().filter_map(|(entity, prim)| {
        (prim.stage_handle.id() == preview.stage_handle().id()
            && prim.path == path
            && lunco_usd_bevy_scene::is_preview_entity(entity, preview.scene_root(), q_parents))
        .then_some(entity)
    });
    let entity = matches.next()?;
    matches.next().is_none().then_some(entity)
}

/// Keep the generic ECS selection projection scoped to the focused USD lease.
/// Without this boundary, focusing a second preview leaves the first preview's
/// entity in the Inspector even though every USD view-model has switched
/// documents. The session map is the UI state; `SelectedEntities` and
/// `SelectionTarget` are synchronized projections used by existing panels and
/// gizmo systems.
fn sync_editor_session_selection(
    viewport: Option<Res<lunco_usd_viewport_core::UsdViewportState>>,
    mut selected: ResMut<lunco_scene_selection::SelectedEntities>,
    mut inspector_target: ResMut<lunco_scene_selection::SelectionTarget>,
    q_paths: Query<(Entity, &UsdPrimPath)>,
    q_parents: Query<&ChildOf>,
    q_selected: Query<Entity, With<crate::selection::Selected>>,
    mut commands: Commands,
    mut sessions: ResMut<EditorSessionSelections>,
    mut last_preview: Local<Option<lunco_usd_viewport_core::UsdPreviewId>>,
) {
    let focused = viewport
        .as_deref()
        .and_then(|state| state.focused_preview_id());
    let open: std::collections::HashSet<_> = viewport
        .as_deref()
        .into_iter()
        .flat_map(|state| state.sessions().map(|session| session.id()))
        .collect();
    sessions
        .sessions
        .retain(|preview, _| open.contains(preview));

    if *last_preview != focused {
        if let Some(previous) = *last_preview {
            if open.contains(&previous) {
                if let Some(session) = viewport
                    .as_deref()
                    .and_then(|state| state.session(previous))
                {
                    let entry = sessions.sessions.entry(previous).or_default();
                    entry.paths = selected
                        .entities
                        .iter()
                        .filter_map(|entity| {
                            preview_path_for_entity(*entity, session, &q_paths, &q_parents)
                        })
                        .collect();
                    entry.target_path = inspector_target.part.and_then(|entity| {
                        preview_path_for_entity(entity, session, &q_paths, &q_parents)
                    });
                }
            }
        } else {
            sessions.live.entities.clone_from(&selected.entities);
            sessions.live.target = inspector_target.part;
        }

        for entity in q_selected.iter() {
            commands
                .entity(entity)
                .remove::<crate::selection::Selected>()
                .remove::<edit_gizmo::GizmoSelected>();
        }
        selected.entities.clear();
        inspector_target.part = None;

        if let Some(preview) = focused {
            let Some(session) = viewport.as_deref().and_then(|state| state.session(preview)) else {
                *last_preview = focused;
                return;
            };
            let entry = sessions.sessions.entry(preview).or_default();
            entry.paths.retain(|path| {
                preview_entity_for_path(path, session, &q_paths, &q_parents).is_some()
            });
            entry.target_path = entry.target_path.take().filter(|path| {
                preview_entity_for_path(path, session, &q_paths, &q_parents).is_some()
            });
            let paths = entry.paths.clone();
            let target_path = entry.target_path.clone();
            let restored: Vec<Entity> = paths
                .iter()
                .filter_map(|path| preview_entity_for_path(path, session, &q_paths, &q_parents))
                .collect();
            selected.entities.clone_from(&restored);
            inspector_target.part = target_path
                .as_deref()
                .and_then(|path| preview_entity_for_path(path, session, &q_paths, &q_parents));
            for entity in restored {
                commands
                    .entity(entity)
                    .try_insert((crate::selection::Selected, edit_gizmo::GizmoSelected));
            }
        } else {
            selected.entities.clone_from(&sessions.live.entities);
            inspector_target.part = sessions.live.target;
            for &entity in &sessions.live.entities {
                commands
                    .entity(entity)
                    .try_insert((crate::selection::Selected, edit_gizmo::GizmoSelected));
            }
        }
        *last_preview = focused;
        return;
    }

    if let Some(preview) = focused {
        let Some(session) = viewport.as_deref().and_then(|state| state.session(preview)) else {
            return;
        };
        // Keep the shared ECS projection inside the focused lease. A generic
        // SelectEntity command may arrive while a USD preview is focused; it
        // must not leak a live-scene entity into the authored Editor context.
        let valid: Vec<Entity> = selected
            .entities
            .iter()
            .copied()
            .filter(|entity| {
                preview_path_for_entity(*entity, session, &q_paths, &q_parents).is_some()
            })
            .collect();
        if valid != selected.entities {
            for entity in q_selected.iter().filter(|entity| !valid.contains(entity)) {
                commands
                    .entity(entity)
                    .remove::<crate::selection::Selected>()
                    .remove::<edit_gizmo::GizmoSelected>();
            }
            selected.entities = valid;
            if inspector_target.part.is_some_and(|entity| {
                preview_path_for_entity(entity, session, &q_paths, &q_parents).is_none()
            }) {
                inspector_target.part = None;
            }
        }
        let entry = sessions.sessions.entry(preview).or_default();
        entry.paths = selected
            .entities
            .iter()
            .filter_map(|entity| preview_path_for_entity(*entity, session, &q_paths, &q_parents))
            .collect();
        entry.target_path = inspector_target
            .part
            .and_then(|entity| preview_path_for_entity(entity, session, &q_paths, &q_parents));
    } else {
        sessions.live.entities.clone_from(&selected.entities);
        sessions.live.target = inspector_target.part;
    }
}

// `every_frame()` (an always-true gate handed to `add_view_model`) is gone: it
// said the right thing to a reader and the wrong thing to the tracker, which
// dutifully reported both users as broken gates every launch. The intent now
// lives in `add_view_model_every_frame`, which is the same decision made in the
// same place — minus the false positives.

/// Publish the data-driven input convention plus the current controlled endpoint's
/// binding into the existing View Help popup. This is presentation only: it
/// reads the public input-port surface and never changes control state.
fn refresh_view_help_controls(
    bindings: Res<lunco_input_core::ActiveInputBindings>,
    local_avatar: Res<TheLocalEmbodiment>,
    q_avatar: Query<
        &ControlLink,
        (
            With<Embodiment>,
            With<lunco_embodiment_core::roles::LocalEmbodiment>,
        ),
    >,
    q_names: Query<Ref<Name>>,
    q_callsigns: Query<&lunco_core::markers::Callsign>,
    q_catalog_ids: Query<&lunco_core::CatalogEntryId>,
    q_bindings: Query<Ref<ControlBinding>>,
    q_inputs: Query<Ref<InputPorts>>,
    mut help: ResMut<LiveHelpSections>,
    mut last_target: Local<Option<Entity>>,
    mut published: Local<bool>,
) {
    let target = local_avatar
        .0
        .and_then(|entity| q_avatar.get(entity).ok())
        .map(|link| link.target);
    let target_changed = *last_target != target;
    let endpoint_changed = target.is_some_and(|entity| {
        q_names.get(entity).is_ok_and(|name| name.is_changed())
            || q_bindings
                .get(entity)
                .is_ok_and(|binding| binding.is_changed())
            || q_inputs.get(entity).is_ok_and(|inputs| inputs.is_changed())
    });
    if *published && !bindings.is_changed() && !target_changed && !endpoint_changed {
        return;
    }

    let global_rows = match bindings.key_bindings() {
        Ok(rows) => rows
            .into_iter()
            .map(|(intent, keys)| (lunco_input_core::key_label(&keys), intent.to_string()))
            .collect(),
        Err(error) => {
            error!("[view-help] active keymap is invalid: {error}");
            return;
        }
    };
    let mut sections = vec![LiveHelpSection {
        title: "Global key → intent bindings".into(),
        rows: global_rows,
    }];

    if let Some(target) = target {
        let name = lunco_core::entity_display_name(
            q_names
                .get(target)
                .ok()
                .map(|name| Name::new(name.as_str().to_owned()))
                .as_ref(),
            q_callsigns.get(target).ok(),
            q_catalog_ids.get(target).ok(),
        );
        let name = if name.is_empty() {
            "controlled endpoint".into()
        } else {
            name
        };
        let mut rows = Vec::new();
        if let Ok(binding) = q_bindings.get(target) {
            for (intent, port, scale) in &binding.binds {
                let label = match bindings.label_for_intent(*intent) {
                    Ok(label) => label,
                    Err(error) => {
                        error!("[view-help] active keymap is invalid: {error}");
                        return;
                    }
                };
                rows.push((label, format!("{port}  ({scale:+})")));
            }
        }
        if rows.is_empty() {
            if let Ok(inputs) = q_inputs.get(target) {
                rows = inputs
                    .values
                    .keys()
                    .map(|port| ("—".into(), format!("{port}  (no local intent binding)")))
                    .collect();
            }
        }
        sections.push(LiveHelpSection {
            title: format!("Controlled: {name}"),
            rows,
        });
    }

    help.set(PerspectiveId::new("sandbox_view"), sections.clone());
    help.set(PerspectiveId::new("rover_build"), sections);
    *last_target = target;
    *published = true;
}

/// Registers scene editing panels and their control references. The Rhai
/// perspective policy assembles these capabilities into task layouts.
pub struct SceneEditUiPlugin;

impl Plugin for SceneEditUiPlugin {
    fn build(&self, app: &mut App) {
        use lunco_core::AppSceneExt;
        if !app.is_plugin_added::<lunco_embodiment_core::roles::EmbodimentCorePlugin>() {
            app.add_plugins(lunco_embodiment_core::roles::EmbodimentCorePlugin);
        }
        // The transform-gizmo transaction adapter is composed as a focused
        // sibling package. The remaining panel and scene interaction adapters
        // are UI-owned because they depend on the rendered viewport and its
        // egui preview lease. Generic editor mechanisms and command observers
        // are installed by `lunco-luncosim-edit-core::SceneEditPlugin`.
        app.add_plugins(edit_gizmo::SceneEditGizmoPlugin)
            .init_scene_resource::<crate::diagnostic_visuals::DiagnosticVisualStore>()
            .init_scene_resource::<scene_context_menu::SceneContextMenuState>()
            .init_scene_resource::<lunco_interaction_core::ArmedScriptTool>()
            .init_scene_resource::<crate::script_tools::ScenePointerDispatch>()
            .add_plugins(crate::script_tools::UsdCurveViewPlugin)
            .add_plugins(crate::perf_bridge::PerfBridgePlugin);
        app.add_systems(
            lunco_core::SceneTeardown,
            crate::script_tools::reset_scene_pointer_move_hooks,
        );
        app.add_observer(crate::script_tools::clear_scene_pointer_move_hook_on_document_closed);
        app.add_observer(crate::script_tools::clear_scene_pointer_move_hooks_on_twin_closed);
        app.world_mut()
            .resource_mut::<lunco_api::queries::ApiQueryRegistry>()
            .register(crate::diagnostic_visuals::DiagnosticVisualsQueryProvider);
        app.add_systems(Startup, crate::physics_viz::configure_gizmo_overlay);
        app.register_type::<crate::physics_viz::PhysicsArrows>();
        app.add_systems(
            PostUpdate,
            (
                crate::diagnostic_visuals::revoke_invalid_leases,
                crate::diagnostic_visuals::draw_diagnostic_visuals,
            )
                .chain()
                .after(bevy::transform::TransformSystems::Propagate)
                .after(lunco_viewport_core::SceneViewportSet::Reconcile)
                .before(bevy::camera::CameraUpdateSystems),
        );
        app.add_systems(
            Update,
            (
                crate::physics_viz::auto_mark_dynamic_bodies,
                crate::physics_viz::draw_physics_arrows,
                crate::joint_viz::draw_joint_viz,
                crate::joint_viz::draw_wheel_force_viz,
                crate::physics_gizmo::draw_physics_gizmo,
                crate::physics_gizmo::draw_frame_gizmo,
            ),
        );
        app.add_systems(Update, crate::selection::select_possessed_vessel);
        app.add_systems(
            Update,
            (
                crate::script_tools::disarm_script_tool_on_cancel,
                crate::script_tools::forget_missing_script_tool,
            ),
        );
        app.add_systems(
            PostUpdate,
            crate::script_tools::clear_scene_pointer_dispatch,
        );
        app.add_systems(
            PreUpdate,
            crate::script_tools::flush_scene_pointer_moves
                .after(bevy::picking::PickingSystems::Last)
                .before(lunco_core::RuntimeCycleSet::Ui),
        );
        app.add_observer(spawn_palette::on_spawn_state_requested);
        app.add_observer(terrain_tools::on_terrain_ui_action);
        app.add_observer(crate::selection::on_select_entity_target);
        app.add_systems(Update, crate::selection::handle_deselect_keys);
        app.add_observer(crate::script_tools::on_scene_pointer_event);
        app.add_observer(crate::script_tools::on_scene_pointer_move_event);
        app.add_observer(crate::script_tools::on_scene_pointer_enter_event);
        app.add_observer(scene_context_menu::on_script_ui_request);
        app.add_observer(scene_context_menu::clear_on_scene_transition);
        app.add_observer(scene_context_menu::clear_on_twin_closed);
        app.add_observer(crate::selection::on_usd_viewport_click);
        app.add_observer(crate::script_tools::on_scene_click_script_tool);
        crate::diagnostic_visuals::register_all_commands(app);
        crate::script_tools::register_all_commands(app);
        crate::selection::register_all_commands(app);
        app.add_systems(Update, crate::selection::draw_selection_bounds);

        // Camera-path overlay: state + the gizmo pass that draws it, and the
        // tracker that tells the panel's transport which path clock to drive.
        // Gate inputs for `usd_selection_view_changed`. `SceneEditPlugin` also
        // inits both, but this plugin is added independently of it (see
        // `lunco_luncosim::ui`), and a run condition that reads a missing resource
        // panics — the producers' own `Option<Res<_>>` tolerance does not cover
        // the gate.
        app.init_scene_resource::<lunco_scene_selection::SelectedEntities>();
        app.init_scene_resource::<lunco_scene_selection::SelectionTarget>();
        app.init_scene_resource::<EditorSessionSelections>();
        app.init_resource::<lunco_api::queries::ApiQueryRegistry>();
        app.world_mut()
            .resource_mut::<lunco_api::queries::ApiQueryRegistry>()
            .register(selection_context::InspectUsdSelectionProvider);
        app.add_systems(Update, sync_editor_session_selection.before(ViewModelSet));

        app.init_resource::<cinematic::CinematicViz>();
        app.init_scene_resource::<cinematic::CinematicTarget>();
        app.init_resource::<LiveHelpSections>();
        app.add_plugins(trail::VehicleTrailPlugin);
        app.add_systems(
            Update,
            (
                cinematic::track_active_camera_path,
                cinematic::draw_camera_paths,
            ),
        );
        // One `ControlLink` lookup and three `Ref` checks on a single entity,
        // then an early return — an O(1) live readout, the sanctioned
        // `every_frame` shape.
        app.add_view_model_every_frame(refresh_view_help_controls);
        app.register_panel(spawn_palette::SpawnPalette::default())
            .register_panel(entity_list::EntityList::default())
            .register_panel(ports::PortPanel::default())
            .register_panel(terrain_tools::ToolsPanel)
            .register_panel(cinematic::CinematicPanel)
            .register_panel(connection_canvas::UsdCanvasPanel)
            .register_panel(command_deck::CommandDeck)
            .register_panel(authoring_review::AuthoringReviewPanel)
            .register_panel(joint_state::JointStatePanel)
            .register_panel(ViewportPanel)
            // Task descriptions come from Rhai; domains supply control references.
            .register_perspective_help(
                PerspectiveId::new("sandbox_view"),
                PerspectiveHelp {
                    shortcuts: vec![],
                    mouse: vec![
                        HelpMouse {
                            interaction: "Left-Click input endpoint",
                            description: "Claim control; static objects do nothing",
                        },
                        HelpMouse {
                            interaction: "Shift+Left-Click",
                            description: "Select for inspection/gizmo in Build mode",
                        },
                        HelpMouse {
                            interaction: "Right-Drag",
                            description: "Orbit / rotate the camera",
                        },
                        HelpMouse {
                            interaction: "Scroll",
                            description: "Zoom in / out",
                        },
                    ],
                },
            )
            .register_perspective_help(
                PerspectiveId::new("rover_build"),
                PerspectiveHelp {
                    shortcuts: vec![
                        HelpShortcut {
                            keys: "Shift",
                            description: "Hold to place multiple (sticky spawn)",
                        },
                        HelpShortcut {
                            keys: "Ctrl+Z",
                            description: "Undo",
                        },
                    ],
                    mouse: vec![
                        HelpMouse {
                            interaction: "Left-Click",
                            description: "Replace selection · confirm placement",
                        },
                        HelpMouse {
                            interaction: "Shift+Left-Click",
                            description: "Extend selection",
                        },
                        HelpMouse {
                            interaction: "Ctrl+Left-Click",
                            description: "Remove only the clicked entity",
                        },
                        HelpMouse {
                            interaction: "Right-Drag",
                            description: "Orbit / rotate the camera",
                        },
                        HelpMouse {
                            interaction: "Scroll",
                            description: "Zoom in / out",
                        },
                    ],
                },
            )
            .register_perspective_help(
                PerspectiveId::new("analysis"),
                PerspectiveHelp {
                    shortcuts: vec![],
                    mouse: vec![
                        HelpMouse {
                            interaction: "Telemetry checkbox",
                            description: "Add or remove a signal in the active graph",
                        },
                        HelpMouse {
                            interaction: "Drag a telemetry channel onto Graphs",
                            description: "Compare signals in the same plot",
                        },
                    ],
                },
            )
            .register_perspective_help(
                PerspectiveId::new("terrain_sculpt"),
                PerspectiveHelp {
                    shortcuts: vec![
                        HelpShortcut {
                            keys: "Shift + ↑/↓",
                            description: "Grow / shrink brush radius",
                        },
                        HelpShortcut {
                            keys: "Alt + ↑/↓",
                            description: "Grow / shrink brush strength",
                        },
                        HelpShortcut {
                            keys: "Esc",
                            description: "Disarm the brush",
                        },
                    ],
                    mouse: vec![
                        HelpMouse {
                            interaction: "Left-Click",
                            description: "Sculpt (raise) · flatten to clicked height",
                        },
                        HelpMouse {
                            interaction: "Alt+Left-Click",
                            description: "Dig (invert the sculpt)",
                        },
                        HelpMouse {
                            interaction: "Ctrl+Left-Click",
                            description: "Flatten to the clicked height",
                        },
                        HelpMouse {
                            interaction: "Shift / Alt + Scroll",
                            description: "Brush radius / strength",
                        },
                        HelpMouse {
                            interaction: "Right-Drag",
                            description: "Orbit / rotate the camera",
                        },
                    ],
                },
            )
            .register_perspective_help(
                PerspectiveId::new("editor"),
                PerspectiveHelp {
                    shortcuts: vec![
                        HelpShortcut {
                            keys: "Ctrl+Z",
                            description: "Undo the last edit",
                        },
                        HelpShortcut {
                            keys: "Delete",
                            description: "Remove the selected part",
                        },
                        HelpShortcut {
                            keys: "Esc",
                            description: "Clear selection / gizmo",
                        },
                    ],
                    mouse: vec![
                        HelpMouse {
                            interaction: "Click a tree node",
                            description: "Select a part to inspect / edit",
                        },
                        HelpMouse {
                            interaction: "Left-Click",
                            description: "Replace selection",
                        },
                        HelpMouse {
                            interaction: "Shift+Left-Click",
                            description: "Extend selection",
                        },
                        HelpMouse {
                            interaction: "Ctrl+Left-Click",
                            description: "Remove only the clicked entity",
                        },
                        HelpMouse {
                            interaction: "Right-Drag",
                            description: "Orbit / rotate the camera",
                        },
                    ],
                },
            );

        // WP-8: the Entity list is a pure view over `EntityTreeView`, derived by
        // a change-gated producer instead of being rebuilt every egui frame.
        // …its system-entity filter remains a global persisted preference, while
        // grid scope is read from and written to the active Twin manifest through
        // the generic SetTwinSetting command.
        lunco_settings::AppSettingsExt::register_settings_section::<entity_list::EntityListSettings>(
            app,
        );
        app.add_systems(Startup, entity_list::register_settings_submenu);

        // Same shape for "show test scenes": a persisted pref in the Settings
        // menu, read by the browsers, off by default.
        lunco_settings::AppSettingsExt::register_settings_section::<
            asset_visibility::AssetVisibilitySettings,
        >(app);
        app.add_systems(Startup, asset_visibility::register_settings_submenu);
        entity_list::install_entity_tree_source_tracking(app);
        app.add_observer(entity_list::on_twin_closed);
        app.add_systems(
            Update,
            entity_list::poll_entity_tree_view_build
                .run_if(entity_list::entity_tree_task_pending)
                .before(ViewModelSet),
        );
        app.add_systems(
            Update,
            entity_list::mark_entity_tree_view_dirty
                .run_if(lunco_core_runtime::gate::tracked(
                    "entity_tree_topology_changed",
                    entity_list::scene_topology_changed,
                ))
                .after(entity_list::poll_entity_tree_view_build)
                .before(ViewModelSet),
        );
        app.add_view_model(
            entity_list::populate_entity_tree_view,
            entity_list::entity_tree_build_due,
        );

        // The universal port table is a live diagnostic/control surface. Its
        // producer samples requested live values at 10 Hz, while the shared
        // port-surface generation invalidates candidate discovery only at an
        // owner mutation boundary. The panel itself remains a pure view and
        // emits SetPorts/ReleasePort.
        app.init_scene_resource::<ports::PortView>()
            .init_scene_resource::<ports::PortPanelState>()
            .init_scene_resource::<ports::PortInspectionRequest>()
            .init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        app.add_view_model(ports::populate_port_view, ports::port_view_due);

        // USD connection canvas: the scene is derived from the live composed
        // stage by a main-thread producer (the stage is `!Send`). Its view-model
        // runs only while the canvas panel is visible; stage path batches then
        // keep unrelated authored changes from rebuilding the graph.
        app.init_scene_resource::<connection_canvas::UsdCanvasState>();
        connection_canvas::init_view_commands(app);
        app.init_scene_resource::<connection_canvas::LayoutJobs>();
        app.add_systems(Update, connection_canvas::update_layouts);
        app.add_systems(
            Update,
            connection_canvas::poll_view_files.run_if(connection_canvas::view_files_pending),
        );
        app.add_view_model(
            connection_canvas::produce_usd_canvas,
            connection_canvas::editor_canvas_changed,
        );

        // Command Deck view-model: selection + possession for its visible
        // panel. A hidden panel has no reader, so do not allocate its label or
        // query control state in the frame loop.
        app.init_scene_resource::<command_deck::CommandDeckView>();
        app.add_systems(
            Update,
            command_deck::populate_command_deck_view
                .in_set(ViewModelSet)
                .after(lunco_workbench_core::WorkbenchSnapshotPublishSet)
                .run_if(command_deck::command_deck_visible),
        );

        // One generic human-facing evidence surface. It reads the existing
        // selection, possession, camera, runtime-diagnostic, and diagnostic
        // lease owners; it does not add a second policy or status store. Its
        // gate ignores repeated writes that leave diagnostic facts unchanged.
        app.init_scene_resource::<authoring_review::AuthoringReviewView>();
        authoring_review::install_view_model_tracking(app);
        app.add_view_model(
            authoring_review::populate_authoring_review_view,
            authoring_review::authoring_review_view_due,
        );

        // Joint State view-model: live physics readouts are rebuilt only while
        // their panel is visible. Hidden dock tabs have no consumer and must
        // not scan the scene's joints and wheels every app frame.
        app.init_scene_resource::<joint_state::JointStateView>();
        app.add_systems(
            Update,
            joint_state::populate_joint_state_view
                .in_set(ViewModelSet)
                .after(lunco_workbench_core::WorkbenchSnapshotPublishSet)
                .run_if(joint_state::joint_state_visible),
        );

        // Debug-viz settings menu rows (joint + wheel-force gizmos).
        app.add_systems(Startup, register_debug_viz_settings);

        // Keep authored labels in the egui pass. Scene tools themselves are
        // discovered and dispatched by `script_tools`, so no product-specific
        // waypoint observer or overlay is registered here.
        app.init_resource::<scene_context::ActivePointerButton>()
            .add_observer(scene_context::apply_pointer_policy)
            .add_systems(
                PreUpdate,
                scene_context::sync_active_pointer_policy
                    .after(bevy::input::InputSystems)
                    .before(bevy::picking::PickingSystems::Backend),
            )
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (
                    billboard_overlay::draw_billboard_overlay,
                    scene_context_menu::draw_scene_context_menu,
                )
                    .after(lunco_theme::ThemeApplySet)
                    .before(lunco_workbench_core::WorkbenchRenderSet),
            );
        cinematic::register_all_commands(app);
    }
}

/// Register checkbox rows in the workbench Settings menu for the temporary
/// diagnostics. All rows update the shared lease store; the resource is not
/// persisted and is cleared at scene teardown.
fn register_debug_viz_settings(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut menus) = world.get_resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
    else {
        return;
    };
    menus.register_settings_submenu("Debug visualization", |ui, ctx| {
        ui.label(
            egui::RichText::new("Debug Visualization")
                .text_style(lunco_theme::TypographyRole::Label.text_style())
                .weak(),
        );
        let Some(mut store) = ctx
            .resource::<crate::diagnostic_visuals::DiagnosticVisualStore>()
            .cloned()
        else {
            return;
        };
        let root = ctx
            .resource::<SceneMountState>()
            .and_then(SceneMountState::active_root);
        let mut show_joints =
            store.builtin_enabled(crate::diagnostic_visuals::DiagnosticVisualKind::Joints);
        let mut show_wheel_forces =
            store.builtin_enabled(crate::diagnostic_visuals::DiagnosticVisualKind::WheelForces);
        let mut show_mass =
            store.builtin_enabled(crate::diagnostic_visuals::DiagnosticVisualKind::PhysicsMass);
        let mut show_forces =
            store.builtin_enabled(crate::diagnostic_visuals::DiagnosticVisualKind::PhysicsForces);
        let mut show_frames =
            store.builtin_enabled(crate::diagnostic_visuals::DiagnosticVisualKind::PhysicsFrames);
        let original = (
            show_joints,
            show_wheel_forces,
            show_mass,
            show_forces,
            show_frames,
        );
        ui.checkbox(&mut show_joints, "Show joints")
            .on_hover_text("Draw anchor dots + axis lines for every Avian joint");
        ui.checkbox(&mut show_wheel_forces, "Show wheel forces")
            .on_hover_text("Draw a force box + arrow at every wheel");
        ui.checkbox(&mut show_mass, "Selected-body mass")
            .on_hover_text(
                "CoM marker + inertia ellipsoid/axes for the selected \
                 vessel and its rigid-body parts",
            );
        ui.checkbox(&mut show_forces, "Selected-body forces")
            .on_hover_text(
                "Tire, normal-load, gravity and net-force arrows for the \
                 selected vessel and its rigid-body parts",
            );
        ui.checkbox(&mut show_frames, "Selected-body frames")
            .on_hover_text(
                "XYZ frame triads (RGB = XYZ) + revolute anchors for the \
                 selected vessel's rigid-body parts",
            );
        let current = (
            show_joints,
            show_wheel_forces,
            show_mass,
            show_forces,
            show_frames,
        );
        if current != original {
            use crate::diagnostic_visuals::DiagnosticVisualKind as Kind;
            store.set_builtin(Kind::Joints, show_joints, root);
            store.set_builtin(Kind::WheelForces, show_wheel_forces, root);
            store.set_builtin(Kind::PhysicsMass, show_mass, root);
            store.set_builtin(Kind::PhysicsForces, show_forces, root);
            store.set_builtin(Kind::PhysicsFrames, show_frames, root);
            ctx.set_resource(store);
        }
    });
}
