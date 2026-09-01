//! UI for the luncosim editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, ControlBinding, InputPorts, TheLocalAvatar};
use lunco_workbench::twin_browser::TWIN_BROWSER_PANEL_ID;
use lunco_workbench::{
    HelpMouse, HelpShortcut, LiveHelpSection, LiveHelpSections, PanelId, Perspective,
    PerspectiveId, ViewportPanel, WorkbenchAppExt, WorkbenchLayout, VIEWPORT_PANEL_ID,
};

pub mod asset_visibility;
pub(crate) mod authoring_paths;
/// Read-only node graph of the selected vessel's authored autopilot program.
pub mod autopilot_canvas;
/// Screen-space labels a prim authored for itself (`lunco:billboard*`).
pub mod billboard_overlay;
/// Cinematic camera authoring — capture the current view as a `def Camera`
/// prim. The current camera-path contract is in
/// `docs/architecture/51-cinematic-camera.md`; capture and transport live in
/// the docked Cinematic panel.
pub mod cinematic;
/// Command Deck panel — the read+control surface for the selected vessel
/// (possession status, autopilot engage/disengage, waypoint list). Pure
/// reader: every mutation dispatches a typed command (§4.2).
pub mod command_deck;
pub mod connection_canvas;
pub mod entity_list;
pub mod inspector;
/// Joint State panel — live joint θ / ω / target / τ table for the selected
/// vessel (revolute joints + raycast wheels + steering), deep-review §2.7.
pub mod joint_state;
/// Generic right-click menus for USD-authored transparent markers.
pub mod scene_context;
pub mod spawn_palette;
pub mod terrain_tools;
/// Bounded, terrain-conforming motion trails for topology-derived vehicles.
pub mod trail;
pub use trail::VehicleTrailPlugin;
pub mod usd_animation;
pub mod usd_joint;
pub mod usd_mount;
pub mod usd_params;
pub mod usd_prim_tree;
pub mod usd_variants;
/// Interactive waypoint authoring — PlaceWaypoint intent and primary-pointer
/// append menu. Document-backed routes use the existing USD authoring funnel;
/// runtime-only routes use the existing live behavior command (no new waypoint
/// domain).
pub mod waypoint_click;

/// Schedule slot (in `Update`) for the UI *view-model* producers — the
/// change-driven systems that derive render-ready state into resources for the
/// egui panels to read (WP-8). `Update` runs before `EguiPrimaryContextPass`, so
/// resources written here are visible to the panels the same frame.
///
/// The private field is the enforcement: the set cannot be named at a call site,
/// so the only way into it is [`ViewModelAppExt::add_view_model`], which demands
/// a gate. A label plus a doc line asking for one was the arrangement that cost
/// 12 ms a frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewModelSet(());

/// Register a view-model producer, **gate required**.
///
/// `ViewModelSet` used to be a label plus a doc line ("gate each with its own
/// `run_if`"), which is advice a registration can silently ignore — and two
/// did, for 12 ms a frame. Here the gate is an argument: you cannot register a
/// producer without stating when it runs.
///
/// A producer that genuinely must run every frame registers through
/// [`ViewModelAppExt::add_view_model_every_frame`], which puts the claim at the
/// call site where review can see it, next to the reason.
///
/// See `docs/architecture/42-ui-frame-discipline.md` §6.
pub trait ViewModelAppExt {
    /// Add `producer` to [`ViewModelSet`] in `Update`, gated on `gate`.
    fn add_view_model<P, M, C, CM>(&mut self, producer: P, gate: C) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
        C: SystemCondition<CM> + Send + 'static;

    /// Add a producer that runs **every frame, on purpose** — an O(1) live
    /// readout with nothing to gate on.
    ///
    /// Separate from [`add_view_model`](Self::add_view_model) so the
    /// effectiveness tracker keeps meaning what it says. Registering these with
    /// an always-true gate made the tracker report them as "this run condition is
    /// not gating" on every launch — three warnings a run, two of which described
    /// a decision rather than a defect, which is exactly how a real one
    /// (`populate_inspector_view` at 296/300) gets read past. Declaring the
    /// intent in the CALL makes the log's remaining entries all actionable.
    fn add_view_model_every_frame<P, M>(&mut self, producer: P) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>;
}

impl ViewModelAppExt for App {
    fn add_view_model<P, M, C, CM>(&mut self, producer: P, gate: C) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
        C: SystemCondition<CM> + Send + 'static,
    {
        // Every view-model gate is tracked HERE rather than at each call site:
        // a gate that silently degrades to always-true costs exactly what it was
        // added to save, and nothing catches that at compile time (effectiveness
        // is a runtime property). Wrapping at the one registration point means a
        // new view model cannot be added without its gate being measured — no
        // per-site discipline required, and no way to forget.
        //
        // Named after the PRODUCER, not the condition. Naming it after the
        // condition reported `fn() -> bool` for every gate built by a factory
        // (`every_frame()` and friends return a fn pointer, so the item type —
        // and with it the path — is gone). The producer is a plain fn item whose
        // `type_name` is its full path, and "which view model is rebuilding" is
        // the more useful thing to read in a log line anyway.
        self.add_systems(
            Update,
            producer
                .in_set(ViewModelSet(()))
                .run_if(lunco_core::gate::tracked(std::any::type_name::<P>(), gate)),
        )
    }

    fn add_view_model_every_frame<P, M>(&mut self, producer: P) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
    {
        // No gate and no tracker — there is nothing to measure. The claim being
        // made is "this producer is O(1) and has no input worth watching", and
        // the place to check that claim is review of the call, not a runtime
        // report that can only ever say "it ran every frame" (it will).
        self.add_systems(Update, producer.in_set(ViewModelSet(())))
    }
}

/// Gate for the producers that read the **selected prim out of the composed
/// stage** (`usd_params`, `usd_variants`, `usd_mount`).
///
/// Their inputs are exactly three: what is selected, what is drilled into, and
/// the USD projection itself. All three are change-detected, and the stage walk
/// they do on a miss is the expensive part — the same `CanonicalStages` lookups
/// that made `produce_usd_canvas` 11 ms a frame. Nothing here early-returns
/// cheaply, so nothing here may run ungated.
pub fn usd_selection_view_changed(
    selection: Res<lunco_scene_commands::SelectedEntities>,
    target: Res<crate::InspectorTarget>,
    revision: Res<lunco_usd_bevy::UsdStageRevision>,
    viewport: Option<Res<lunco_usd::ui::viewport::UsdViewportState>>,
) -> bool {
    selection.is_changed()
        || target.is_changed()
        || revision.is_changed()
        || viewport.is_some_and(|state| state.is_changed())
}

/// Return whether an entity belongs to the focused Editor preview subtree.
/// The preview root itself is part of that scope; all other entities must be
/// descendants through Bevy's authoritative hierarchy.
pub(crate) fn is_editor_preview_entity(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
) -> bool {
    if entity == root {
        return true;
    }
    let mut current = entity;
    while let Ok(parent) = parents.get(current) {
        current = parent.parent();
        if current == root {
            return true;
        }
    }
    false
}

/// Resolve a USD editor selection against one explicit preview lease.
///
/// `SelectedEntities` is shared with the live scene because it is the generic
/// entity-selection projection. USD panels must not treat it as a document
/// identity, however: the same path can exist in several isolated previews
/// and a live entity can be selected while a preview is focused. This helper
/// applies the existing lease root and stage handle before a panel derives any
/// authored view-model.
pub(crate) fn selected_entity_in_preview(
    session: &lunco_usd::ui::viewport::UsdPreviewSession,
    selected: Option<&lunco_scene_commands::SelectedEntities>,
    target: Option<&crate::InspectorTarget>,
    q_paths: &Query<&lunco_usd_bevy::UsdPrimPath>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let belongs = |entity: Entity| {
        q_paths.get(entity).is_ok_and(|path| {
            path.stage_handle.id() == session.stage_handle().id()
                && is_editor_preview_entity(entity, session.scene_root(), q_parents)
        })
    };

    target
        .and_then(|value| value.part)
        .filter(|entity| belongs(*entity))
        .or_else(|| {
            selected
                .and_then(lunco_scene_commands::SelectedEntities::primary)
                .filter(|entity| belongs(*entity))
        })
}

#[derive(Clone, Default)]
struct EditorSessionSelection {
    entities: Vec<Entity>,
    target: Option<Entity>,
}

/// Session-owned editor selection. `SelectedEntities` remains the shared ECS
/// highlight/gizmo projection, while this resource owns which preview lease
/// that projection belongs to and restores it after focus changes.
#[derive(Resource, Default)]
struct EditorSessionSelections {
    sessions: HashMap<lunco_usd::ui::viewport::UsdPreviewId, EditorSessionSelection>,
    live: EditorSessionSelection,
}

/// Keep the generic ECS selection projection scoped to the focused USD lease.
/// Without this boundary, focusing a second preview leaves the first preview's
/// entity in the Inspector even though every USD view-model has switched
/// documents. The session map is the UI state; `SelectedEntities` and
/// `InspectorTarget` are synchronized projections used by existing panels and
/// gizmo systems.
fn sync_editor_session_selection(
    viewport: Option<Res<lunco_usd::ui::viewport::UsdViewportState>>,
    mut selected: ResMut<lunco_scene_commands::SelectedEntities>,
    mut inspector_target: ResMut<crate::InspectorTarget>,
    q_parents: Query<&ChildOf>,
    q_selected: Query<Entity, With<crate::selection::Selected>>,
    mut commands: Commands,
    mut sessions: ResMut<EditorSessionSelections>,
    mut last_preview: Local<Option<lunco_usd::ui::viewport::UsdPreviewId>>,
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

    let belongs = |preview: lunco_usd::ui::viewport::UsdPreviewId, entity: Entity| {
        viewport
            .as_deref()
            .and_then(|state| state.session(preview))
            .is_some_and(|session| {
                crate::ui::is_editor_preview_entity(entity, session.scene_root(), &q_parents)
            })
    };

    if *last_preview != focused {
        if let Some(previous) = *last_preview {
            if open.contains(&previous) {
                let entry = sessions.sessions.entry(previous).or_default();
                entry.entities = selected
                    .entities
                    .iter()
                    .copied()
                    .filter(|entity| belongs(previous, *entity))
                    .collect();
                entry.target = inspector_target
                    .part
                    .filter(|entity| belongs(previous, *entity));
            }
        } else {
            sessions.live.entities.clone_from(&selected.entities);
            sessions.live.target = inspector_target.part;
        }

        for entity in q_selected.iter() {
            commands
                .entity(entity)
                .remove::<crate::selection::Selected>()
                .remove::<crate::gizmo::GizmoSelected>();
        }
        selected.entities.clear();
        inspector_target.part = None;

        if let Some(preview) = focused {
            let entry = sessions.sessions.entry(preview).or_default();
            entry.entities.retain(|entity| belongs(preview, *entity));
            selected.entities.clone_from(&entry.entities);
            inspector_target.part = entry.target.filter(|entity| belongs(preview, *entity));
            for &entity in &entry.entities {
                commands
                    .entity(entity)
                    .try_insert((crate::selection::Selected, crate::gizmo::GizmoSelected));
            }
        } else {
            selected.entities.clone_from(&sessions.live.entities);
            inspector_target.part = sessions.live.target;
            for &entity in &sessions.live.entities {
                commands
                    .entity(entity)
                    .try_insert((crate::selection::Selected, crate::gizmo::GizmoSelected));
            }
        }
        *last_preview = focused;
        return;
    }

    if let Some(preview) = focused {
        let entry = sessions.sessions.entry(preview).or_default();
        entry.entities = selected
            .entities
            .iter()
            .copied()
            .filter(|entity| belongs(preview, *entity))
            .collect();
        entry.target = inspector_target
            .part
            .filter(|entity| belongs(preview, *entity));
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
    bindings: Res<lunco_controller::InputBindingsSettings>,
    local_avatar: Res<TheLocalAvatar>,
    q_avatar: Query<&ControllerLink, (With<Avatar>, With<lunco_core::LocalAvatar>)>,
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
        .map(|link| link.vessel_entity);
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
            .map(|(intent, keys)| (lunco_controller::key_label(&keys), intent.to_string()))
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

    help.set(PerspectiveId("sandbox_view"), sections.clone());
    help.set(PerspectiveId("rover_build"), sections);
    *last_target = target;
    *published = true;
}

/// Plugin that registers all luncosim editing UI panels, the workbench
/// 3D viewport placeholder, and two workspace presets:
///
/// - **View** (default) — just the 3D scene, no panels.
/// - **Build** — 3D + Entities, Inspector, Spawn palette around the edges.
///
/// The user switches via the workspace tabs in the transport bar.
/// `ViewportPanel` reserves the centre slot in both perspectives; the
/// 3D camera (declared by the canonical `lunco_render::SceneCamera` intent) is
/// confined to that
/// rect each frame by `lunco_workbench::apply_workbench_viewport`, and
/// the panel paints its theme backdrop around it.
pub struct SceneEditUiPlugin;

impl Plugin for SceneEditUiPlugin {
    fn build(&self, app: &mut App) {
        // Camera-path overlay: state + the gizmo pass that draws it, and the
        // tracker that tells the panel's transport which path clock to drive.
        // Gate inputs for `usd_selection_view_changed`. `SceneEditPlugin` also
        // inits both, but this plugin is added independently of it (see
        // `lunco_luncosim::ui`), and a run condition that reads a missing resource
        // panics — the producers' own `Option<Res<_>>` tolerance does not cover
        // the gate.
        app.init_resource::<lunco_scene_commands::SelectedEntities>();
        app.init_resource::<crate::InspectorTarget>();
        app.init_resource::<EditorSessionSelections>();
        app.add_systems(
            Update,
            sync_editor_session_selection.before(ViewModelSet(())),
        );

        app.init_resource::<cinematic::CinematicViz>();
        app.init_resource::<cinematic::CinematicTarget>();
        app.init_resource::<LiveHelpSections>();
        app.add_plugins(trail::VehicleTrailPlugin);
        app.add_systems(
            Update,
            (
                cinematic::track_active_camera_path,
                cinematic::draw_camera_paths,
            ),
        );
        // One `ControllerLink` lookup and three `Ref` checks on a single entity,
        // then an early return — an O(1) live readout, the sanctioned
        // `every_frame` shape.
        app.add_view_model_every_frame(refresh_view_help_controls);
        app.register_panel(spawn_palette::SpawnPalette)
            .register_panel(inspector::Inspector)
            .register_panel(inspector::EnvironmentPanel)
            .register_panel(entity_list::EntityList)
            .register_panel(terrain_tools::ToolsPanel)
            .register_panel(cinematic::CinematicPanel)
            .register_panel(connection_canvas::UsdCanvasPanel)
            .register_panel(autopilot_canvas::AutopilotCanvasPanel)
            .register_panel(usd_prim_tree::UsdPrimTreePanel)
            .register_panel(command_deck::CommandDeck)
            .register_panel(joint_state::JointStatePanel)
            .register_panel(ViewportPanel)
            // Order matters for auto-activation — View first so it's
            // the default when the rover binary boots.
            .register_perspective(ViewPerspective)
            .register_perspective_help(
                PerspectiveId("sandbox_view"),
                lunco_workbench::PerspectiveHelp {
                    description: "Full-screen 3D observation & control mode. Fly the \
                                  camera around the scene and claim an endpoint's \
                                  public input ports. The live sections below show \
                                  the global key map and the controlled endpoint's map.",
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
                    has_tour: false,
                },
            )
            .register_perspective(BuildPerspective)
            .register_perspective_help(
                PerspectiveId("rover_build"),
                lunco_workbench::PerspectiveHelp {
                    description: "3D scene editor. Spawn objects from the palette, \
                                  select and transform them, and assemble the scene.",
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
                            description: "Select object · confirm placement",
                        },
                        HelpMouse {
                            interaction: "Shift+Left-Click",
                            description: "Select + transform gizmo (drag to move)",
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
                    has_tour: false,
                },
            )
            .register_perspective(TerrainPerspective)
            .register_perspective_help(
                PerspectiveId("terrain_sculpt"),
                lunco_workbench::PerspectiveHelp {
                    description: "Sculpt the surface. Arm a brush in the Tools palette, \
                                  then click the terrain to raise, dig, or flatten it. \
                                  Edits re-bake the visuals and the collider live.",
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
                    has_tour: false,
                },
            )
            .register_perspective(EditorPerspective)
            .register_perspective_help(
                PerspectiveId("editor"),
                lunco_workbench::PerspectiveHelp {
                    description: "Assemble and edit a USD document. Choose the \
                                  document in the Twin Browser, navigate its structure, \
                                  attach components from \
                                  the palette, and tune the selected prim's parameters in \
                                  the Inspector.",
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
                            interaction: "Shift+Left-Click",
                            description: "Select + transform gizmo (drag to move)",
                        },
                        HelpMouse {
                            interaction: "Right-Drag",
                            description: "Orbit / rotate the camera",
                        },
                    ],
                    has_tour: false,
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
        app.add_systems(Startup, entity_list::register_settings_menu);

        // Same shape for "show test scenes": a persisted pref in the Settings
        // menu, read by the browsers, off by default.
        lunco_settings::AppSettingsExt::register_settings_section::<
            asset_visibility::AssetVisibilitySettings,
        >(app);
        app.add_systems(Startup, asset_visibility::register_settings_menu);
        app.init_resource::<entity_list::EntityTreeView>();
        app.add_observer(entity_list::on_twin_closed);
        app.add_view_model(
            entity_list::populate_entity_tree_view,
            entity_list::scene_topology_changed,
        );

        // WP-8: the Inspector reads query-derived sun / camera / joint state
        // (which `PanelCtx` can't gather in paint) from `InspectorView`,
        // produced each frame by an exclusive system before the egui pass.
        app.init_resource::<inspector::InspectorView>();
        app.init_resource::<inspector::ShaderSchemaCache>();
        app.add_observer(inspector::on_inspector_component_edit)
            .add_observer(inspector::on_projection_edit_requested)
            .add_observer(inspector::on_usd_attribute_edit_requested)
            .add_observer(inspector::on_usd_variant_edit_requested)
            .add_observer(inspector::on_mount_snap_requested)
            .add_observer(inspector::on_mount_detach_requested)
            .add_observer(inspector::on_shader_swap_requested)
            .add_observer(inspector::on_shader_create_requested)
            .add_observer(inspector::on_shader_import_requested)
            .add_observer(inspector::on_shader_parameters_requested)
            .add_observer(inspector::on_pbr_material_requested)
            .add_observer(inspector::on_modelica_parameter_requested);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_observer(inspector::on_attach_at_socket_requested);
        app.add_view_model(
            inspector::populate_inspector_view,
            inspector::inspector_inputs_changed,
        );

        // USD connection canvas: the scene is derived from the live composed
        // stage by a main-thread producer (the stage is `!Send`).
        //
        // This used to run ungated on the claim that it "early-returns cheaply
        // when the topology is stable". It does not: the early-return compares a
        // hash that costs ~20 000 composed-stage lookups and a sorted `Vec<String>`
        // to compute — 11 ms/frame on `sandbox_scene.usda`, the single largest
        // item in the frame. A gate derived from the OUTPUT can never be cheaper
        // than the output. `UsdStageRevision` is stamped by the writers instead.
        // The internal hash stays, now purely as the idempotence guard it should
        // always have been (a bumped revision does not imply a changed topology).
        app.init_resource::<connection_canvas::UsdCanvasState>();
        app.add_view_model(
            connection_canvas::produce_usd_canvas,
            connection_canvas::editor_canvas_changed,
        );

        // Autopilot graph: a small O(1) read of the selected vessel's derived
        // behaviour spec. The canvas's layout only rebuilds when that source
        // changes, never while the simulation is ticking.
        app.init_resource::<autopilot_canvas::AutopilotCanvasState>();
        app.add_observer(autopilot_canvas::on_write_mission_requested)
            .add_observer(autopilot_canvas::on_create_mission_requested);
        app.add_view_model_every_frame(autopilot_canvas::produce_autopilot_canvas);

        // USD prim tree: same main-thread producer pattern (the stage is
        // `!Send`), same gate for the same reason.
        app.init_resource::<usd_prim_tree::UsdPrimTreeView>();
        app.add_view_model(
            usd_prim_tree::produce_usd_prim_tree,
            usd_prim_tree::editor_prim_tree_changed,
        );

        // USD parameter sliders: harvest the selected prim's customData-ranged
        // attributes for the Inspector's data-driven Parameters section. Walks
        // the composed stage on every run, so it is gated on its three inputs
        // (`usd_selection_view_changed`) rather than run every frame.
        app.init_resource::<usd_params::UsdParamView>();
        app.add_view_model(
            usd_params::produce_usd_param_view,
            usd_selection_view_changed,
        );

        // Variant sets: which configurations the selected prim ships (a rover's
        // `drivetrain`, a scenario scene's `terrain` site) and which composes
        // now — the Inspector's ⎇ Variants picker. Same stage walk, same gate.
        app.init_resource::<usd_variants::UsdVariantView>();
        app.add_view_model(
            usd_variants::produce_usd_variant_view,
            usd_selection_view_changed,
        );

        // Mount snap: resolve each socket the selected host advertises + the
        // placement that lands its part's plug on the socket (Inspector 🔩 Mount).
        // Same stage walk, same gate.
        app.init_resource::<usd_mount::UsdMountView>();
        app.add_view_model(
            usd_mount::produce_usd_mount_view,
            usd_selection_view_changed,
        );

        // Standard USD Physics joint authoring: the producer reads the
        // composed joint once per selection/stage revision, while the Inspector
        // dispatches the same typed USD operations as Rhai and the API.
        app.init_resource::<usd_joint::UsdJointView>();
        app.add_view_model(
            usd_joint::produce_usd_joint_view,
            usd_selection_view_changed,
        );
        app.init_resource::<usd_animation::UsdAnimationView>();
        app.add_view_model(
            usd_animation::produce_usd_animation_view,
            usd_selection_view_changed,
        );

        // Command Deck view-model: selection + possession + behaviour-spec
        // readout for the currently-selected vessel. Cheap O(1) single-entity
        // lookups each `Update` (the sanctioned live-readout exception to §7),
        // so no change-gate — same shape as the avatar status producer.
        app.init_resource::<command_deck::CommandDeckView>();
        app.add_view_model_every_frame(command_deck::populate_command_deck_view);

        // Joint State view-model: the selected vessel's joints and wheels are
        // live physics (θ / ω / τ change every tick), so this is an explicit
        // every-frame producer — bounded by the vessel's joint count, the same
        // scale as the joint_viz gizmo pass. Its first branch returns before
        // iterating any joint or wheel query when nothing is selected.
        app.init_resource::<joint_state::JointStateView>();
        app.add_view_model_every_frame(joint_state::populate_joint_state_view);

        // Debug-viz settings menu rows (joint + wheel-force gizmos).
        app.add_systems(Startup, register_debug_viz_settings);

        // PlaceWaypoint + LMB (Alt+LMB in the bundled keymap) drops a mission
        // waypoint by AUTHORING A USD PRIM (`ApplyUsdOp`) —
        // no dedicated waypoint command or parallel waypoint state. Moving,
        // deleting, undoing and inspecting it are ordinary prim paths. See
        // `waypoint_click`.
        app.init_resource::<waypoint_click::WaypointContextMenuState>()
            .init_resource::<waypoint_click::WaypointPlacement>()
            .init_resource::<waypoint_click::WaypointClickDedup>()
            .init_resource::<waypoint_click::RouteVisualProjection>()
            .init_resource::<waypoint_click::RouteProjectionRebuildRequested>()
            // An armed placement names the vessel whose route it edits, and a
            // context menu names the waypoint it opened on. Both are entities of
            // the scene being unloaded — carried across a reload they leave the
            // next scene's first ground click captured by a tool aimed at a
            // vessel that no longer exists, with the possession and selection
            // observers standing down for it (`WaypointToolActive`).
            .add_systems(
                lunco_core::SceneTeardown,
                (
                    |mut placement: ResMut<waypoint_click::WaypointPlacement>,
                     mut menu: ResMut<waypoint_click::WaypointContextMenuState>| {
                        if placement.0.is_some() {
                            placement.0 = None;
                        }
                        *menu = waypoint_click::WaypointContextMenuState::default();
                    },
                    |mut dedup: ResMut<waypoint_click::WaypointClickDedup>| {
                        dedup.clear();
                    },
                    |q_reached: Query<Entity, With<lunco_autopilot::usd_tree::ReachedWaypoints>>,
                     mut commands: Commands| {
                        for entity in q_reached.iter() {
                            commands.entity(entity).remove::<lunco_autopilot::usd_tree::ReachedWaypoints>();
                        }
                    },
                    waypoint_click::clear_route_visual_projection,
                ),
            )
            .add_observer(waypoint_click::on_scene_click_waypoint)
            .add_observer(waypoint_click::on_scene_right_click_waypoint)
            .add_observer(waypoint_click::on_append_waypoint_placement_requested)
            // Consumes the ground click that follows a Move / Insert-after.
            .add_observer(waypoint_click::on_scene_click_place_waypoint)
            // Keep the native picking backend synchronized with each USD-authored
            // interaction policy, including live reauthoring and re-projection.
            .add_observer(scene_context::apply_pointer_policy)
            // egui DRAWING belongs in the egui pass, not `Update`. bevy_egui brackets
            // a context's begin/end pass here, so a widget built outside it never joins
            // egui's input pass: the context menu PAINTED but nothing in it could be
            // clicked. (The overlay got away with `Update` only because it is
            // paint-only — no widgets, no interaction.)
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (
                    // The two WORLD overlays append to egui's root Background paint
                    // list BEFORE the workbench builds its chrome. A custom egui
                    // Background layer has no deterministic order against that
                    // root list; this schedule edge is the actual 3D → tags → UI
                    // composition boundary.
                    // USD-authored labels (`lunco:billboard`) use each prim's
                    // propagated render pose; route projection remains the sole
                    // producer for terrain-grid ribbon geometry and marker state.
                    billboard_overlay::draw_billboard_overlay
                        .before(lunco_workbench::WorkbenchRenderSet),
                    waypoint_click::draw_waypoint_context_menu
                        .in_set(lunco_workbench::ApplicationOverlayRenderSet),
                    // Crosshair + Esc-to-cancel while a placement is armed.
                    waypoint_click::handle_waypoint_placement_mode
                        .in_set(lunco_workbench::ApplicationOverlayRenderSet),
                ),
            )
            .add_systems(
                Update,
                (
                    // Route interpretation and terrain projection are a
                    // change-driven producer. The mesh and marker consumers
                    // run only when this snapshot changes.
                    waypoint_click::arm_route_projection_rebuild
                        .before(waypoint_click::project_waypoint_markers_to_surface)
                        .before(waypoint_click::rebuild_waypoint_route_projection),
                    waypoint_click::project_waypoint_markers_to_surface
                        .run_if(waypoint_click::route_projection_rebuild_is_pending),
                    waypoint_click::rebuild_waypoint_route_projection
                        .after(waypoint_click::project_waypoint_markers_to_surface)
                        .run_if(waypoint_click::route_projection_rebuild_is_pending),
                    waypoint_click::sync_route_visual_meshes
                        .after(waypoint_click::rebuild_waypoint_route_projection)
                        .run_if(resource_changed::<waypoint_click::RouteVisualProjection>),
                    waypoint_click::handle_autopilot_toggle_intent,
                    inspector::delete_selected_on_intent,
                    // Grabbing the controls takes the vessel back from its autopilot.
                    waypoint_click::manual_input_disengages_autopilot,
                    // Mirrors an armed placement into the shared tool gate so
                    // possession/selection stand down for that one click.
                    waypoint_click::sync_waypoint_tool_active,
                    // `Cancel` intent (Esc/Backspace, from the data keymap) → the
                    // CancelWaypointEdit command. Backs out of ANY waypoint mode.
                    waypoint_click::cancel_waypoint_edit_on_intent,
                ),
            );
        waypoint_click::register_all_commands(app);
        cinematic::register_all_commands(app);
    }
}

/// Register checkbox rows in the workbench Settings menu for the joint
/// and wheel-force gizmos. Mutates [`joint_viz::JointVizSettings`]
/// directly; the resource is not persisted (debug toggle, defaults off).
fn register_debug_viz_settings(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, ctx| {
        ui.label(egui::RichText::new("Debug Visualization").weak().small());
        let Some(mut settings) = ctx
            .resource::<crate::joint_viz::JointVizSettings>()
            .copied()
        else {
            return;
        };
        let original_settings = settings;
        let Some(mut gizmo) = ctx
            .resource::<crate::physics_gizmo::PhysicsGizmoSettings>()
            .copied()
        else {
            return;
        };
        let original_gizmo = gizmo;
        ui.checkbox(&mut settings.show_joints, "Show joints")
            .on_hover_text("Draw anchor dots + axis lines for every Avian joint");
        ui.checkbox(&mut settings.show_wheel_forces, "Show wheel forces")
            .on_hover_text("Draw a force box + arrow at every wheel");
        ui.checkbox(&mut gizmo.show_mass, "Selected-body mass")
            .on_hover_text(
                "CoM marker + inertia ellipsoid/axes for the selected \
                 vessel and its rigid-body parts",
            );
        ui.checkbox(&mut gizmo.show_forces, "Selected-body forces")
            .on_hover_text(
                "Tire, normal-load, gravity and net-force arrows for the \
                 selected vessel and its rigid-body parts",
            );
        ui.checkbox(&mut gizmo.show_frames, "Selected-body frames")
            .on_hover_text(
                "XYZ frame triads (RGB = XYZ) + revolute anchors for the \
                 selected vessel's rigid-body parts",
            );
        if settings != original_settings {
            ctx.set_resource(settings);
        }
        if gizmo != original_gizmo {
            ctx.set_resource(gizmo);
        }
    });
}

/// Rover luncosim's default workspace — full-screen 3D, no panels.
///
/// All slots empty — the workbench renders **nothing** in the
/// centre, so Bevy's 3D scene gets the pointer events directly. This
/// is the only way to keep gizmos draggable without render-to-texture:
/// any egui surface in the central area (even a transparent
/// `ViewportPanel` tab) marks the rect as egui-interactive and
/// blocks Bevy input.
pub struct ViewPerspective;

impl Perspective for ViewPerspective {
    fn id(&self) -> PerspectiveId {
        PerspectiveId("sandbox_view")
    }
    fn title(&self) -> String {
        // The icon belongs to the perspective tab beside Build and Lunica;
        // the top-level View menu intentionally remains a plain menu label.
        "◉ View".into()
    }
    fn restores_cached_layout(&self) -> bool {
        false
    }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(None);
        layout.set_right_inspector(None);
        layout.set_bottom(None);
        layout.set_center(vec![]);
    }
}

/// Build mode — Entities over Telemetry on the left, 3D centre, and Inspector
/// over Spawn on the right.
pub struct BuildPerspective;

impl Perspective for BuildPerspective {
    fn id(&self) -> PerspectiveId {
        PerspectiveId("rover_build")
    }
    // ⚒ (U+2692) instead of 🏗 (U+1F3D7) — the latter tofus in the
    // bundled DejaVu fallback; ⚒ renders everywhere (see welcome.rs).
    fn title(&self) -> String {
        "⚒ Build".into()
    }
    fn layout_revision(&self) -> u32 {
        // Revision 2 adds the default Graphs instance to the bottom-center
        // split. Existing cached Build layouts must be rebuilt once so the
        // default is deterministic for every workspace.
        2
    }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser_stacked(
            vec![PanelId("entity_list")],
            vec![PanelId("telemetry_browser")],
        );
        layout.set_center(vec![VIEWPORT_PANEL_ID]);
        layout.set_right_inspector_stacked(
            vec![PanelId("sandbox_inspector")],
            vec![PanelId("spawn_palette")],
        );
        layout.set_bottom(None);
    }
}

/// Editor mode — edit one explicit USD assembly document.
///
/// Distinct from Build (which edits the live simulation scene): Editor leads
/// with the selected document's USD structure and isolated USD preview. The
/// document is selected explicitly in the Twin Browser; the preview subtree,
/// prim tree, Inspector, and all authoring commands then share that document
/// identity. No live scene stage is inferred from entity counts.
///
/// The Rhai editor lives beside this build surface. The USD connection canvas
/// is opened from the Connections entry in the Lunica/Twin navigation so the
/// graph has one discoverable home instead of another top-level perspective.
pub struct EditorPerspective;

impl Perspective for EditorPerspective {
    fn id(&self) -> PerspectiveId {
        PerspectiveId("editor")
    }
    fn title(&self) -> String {
        "Editor".into()
    }
    fn show_in_switcher(&self) -> bool {
        false
    }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        // Structure first: the USD prim tree (the assembly's authoring hierarchy)
        // to navigate/select parts, the entity list as an alternate view, and the
        // palette to add parts. (Unknown ids are filtered.)
        layout.set_side_browser_tabs(vec![
            TWIN_BROWSER_PANEL_ID,
            usd_prim_tree::USD_PRIM_TREE_PANEL_ID,
            PanelId("entity_list"),
            PanelId("spawn_palette"),
        ]);
        // Central tabs: the isolated USD document preview and the Rhai
        // behaviour editor. The
        // USD connection graph is opened from the Connections entry in the
        // Lunica/Twin navigation, so it is not a second Build workflow.
        layout.set_center(vec![
            lunco_usd::ui::USD_VIEWPORT_PANEL_ID,
            PanelId("rhai_editor"),
        ]);
        // The Inspector alone on the right — parameter editing is the point here.
        layout.set_right_inspector_tabs(vec![
            PanelId("sandbox_inspector"),
            PanelId("sandbox_environment"),
        ]);
        layout.set_bottom(None);
    }
}

/// Terrain sculpt mode — Tools palette left, 3D centre, Inspector + Entities
/// tabbed right. The Tools palette arms a brush; clicking the terrain sculpts
/// it (possession + selection stand down while a brush is armed).
pub struct TerrainPerspective;

impl Perspective for TerrainPerspective {
    fn id(&self) -> PerspectiveId {
        PerspectiveId("terrain_sculpt")
    }
    fn title(&self) -> String {
        "Terrain".into()
    }
    fn show_in_switcher(&self) -> bool {
        false
    }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser_tabs(vec![PanelId("tools_palette")]);
        layout.set_center(vec![VIEWPORT_PANEL_ID]);
        layout.set_right_inspector_tabs(vec![
            PanelId("sandbox_inspector"),
            PanelId("sandbox_environment"),
            PanelId("entity_list"),
        ]);
        layout.set_bottom(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_perspective_uses_editor_identity() {
        let perspective = EditorPerspective;

        assert_eq!(perspective.id(), PerspectiveId("editor"));
        assert_eq!(perspective.title(), "Editor");
    }
}
