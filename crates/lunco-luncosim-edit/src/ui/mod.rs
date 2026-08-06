//! UI for the luncosim editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use bevy::prelude::*;
use lunco_controller::ControllerLink;
use lunco_core::{Avatar, ControlBinding, InputPorts};
use lunco_workbench::{
    HelpMouse, HelpShortcut, LiveHelpSection, LiveHelpSections, PanelId, Perspective,
    PerspectiveId, ViewportPanel, WorkbenchAppExt, WorkbenchLayout, VIEWPORT_PANEL_ID,
};

pub mod asset_visibility;
/// Read-only node graph of the selected vessel's authored autopilot program.
pub mod autopilot_canvas;
/// Screen-space labels a prim authored for itself (`lunco:billboard*`).
pub mod billboard_overlay;
/// Interactive checkpoint authoring — Alt+LMB append + right-click context
/// menu, routing through the existing `SetAutopilotBehavior`/`EngageAutopilot`
/// commands (no new journal domain).
pub mod checkpoint_click;
/// Cinematic camera authoring — capture the current view as a `def Camera`
/// prim (doc 50). The transport that replays it is the floating HUD in
/// `lunco-luncosim`, not a panel: View mode has no dock.
pub mod cinematic;
/// Command Deck panel — the read+control surface for the selected vessel
/// (possession status, autopilot engage/disengage, checkpoint list). Pure
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
pub mod usd_mount;
pub mod usd_params;
pub mod usd_prim_tree;
pub mod usd_variants;

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
) -> bool {
    selection.is_changed() || target.is_changed() || revision.is_changed()
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
    q_avatar: Query<&ControllerLink, With<Avatar>>,
    q_names: Query<Ref<Name>>,
    q_bindings: Query<Ref<ControlBinding>>,
    q_inputs: Query<Ref<InputPorts>>,
    mut help: ResMut<LiveHelpSections>,
    mut global_rows: Local<Option<Vec<(String, String)>>>,
    mut last_target: Local<Option<Entity>>,
    mut published: Local<bool>,
) {
    let target = q_avatar.iter().next().map(|link| link.vessel_entity);
    let target_changed = *last_target != target;
    let endpoint_changed = target.is_some_and(|entity| {
        q_names.get(entity).is_ok_and(|name| name.is_changed())
            || q_bindings
                .get(entity)
                .is_ok_and(|binding| binding.is_changed())
            || q_inputs.get(entity).is_ok_and(|inputs| inputs.is_changed())
    });
    if *published && !target_changed && !endpoint_changed {
        return;
    }

    let global_rows = global_rows.get_or_insert_with(|| {
        lunco_controller::default_key_bindings()
            .into_iter()
            .map(|(intent, keys)| (lunco_controller::key_label(&keys), format!("{intent:?}")))
            .collect()
    });
    let mut sections = vec![LiveHelpSection {
        title: "Global key → intent bindings".into(),
        rows: global_rows.clone(),
    }];

    if let Some(target) = target {
        let name = q_names
            .get(target)
            .map(|name| name.as_str().to_owned())
            .unwrap_or_else(|_| "controlled endpoint".into());
        let mut rows = q_bindings
            .get(target)
            .map(|binding| {
                binding
                    .binds
                    .iter()
                    .map(|(intent, port, scale)| {
                        (
                            lunco_controller::default_key_label(*intent),
                            format!("{port}  ({scale:+})"),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
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

    help.set(PerspectiveId("sandbox_view"), sections);
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
/// 3D camera (tagged `WorkbenchViewportCamera`) is confined to that
/// rect each frame by `lunco_workbench::apply_workbench_viewport`, and
/// the panel paints its theme backdrop around it.
pub struct SandboxEditUiPlugin;

impl Plugin for SandboxEditUiPlugin {
    fn build(&self, app: &mut App) {
        // Camera-path overlay: state + the gizmo pass that draws it, and the
        // tracker that tells the panel's transport which path clock to drive.
        // Gate inputs for `usd_selection_view_changed`. `SandboxEditPlugin` also
        // inits both, but this plugin is added independently of it (see
        // `lunco_luncosim::ui`), and a run condition that reads a missing resource
        // panics — the producers' own `Option<Res<_>>` tolerance does not cover
        // the gate.
        app.init_resource::<lunco_scene_commands::SelectedEntities>();
        app.init_resource::<crate::InspectorTarget>();

        app.init_resource::<cinematic::CinematicViz>();
        app.init_resource::<cinematic::CinematicTarget>();
        app.init_resource::<LiveHelpSections>();
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
                    title: "🎬 View",
                    description: "Full-screen 3D observation & control mode. Fly the \
                                  camera around the scene and claim an endpoint's \
                                  public input ports. The live sections below show \
                                  the global key map and the controlled endpoint's map.",
                    shortcuts: vec![
                        HelpShortcut { keys: "Shift", description: "Camera speed boost" },
                        HelpShortcut { keys: "+ / −", description: "Zoom in / out" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click input endpoint", description: "Claim control; static objects do nothing" },
                        HelpMouse { interaction: "Shift+Left-Click", description: "Select for inspection/gizmo in Build mode" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                        HelpMouse { interaction: "Scroll", description: "Zoom in / out" },
                    ],
                    has_tour: false,
                },
            )
            .register_perspective(SchemaPerspective)
            .register_perspective_help(
                PerspectiveId("lunica_schema"),
                lunco_workbench::PerspectiveHelp {
                    title: "🔗 Lunica Schema",
                    description: "Full-window view of the composed USD connection graph. Nodes and wires are derived from the live stage's typed properties and authored connections.",
                    shortcuts: vec![
                        HelpShortcut { keys: "F", description: "Fit the complete graph" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Pan / zoom", description: "Navigate the graph canvas" },
                        HelpMouse { interaction: "Drag a node", description: "Inspect or reposition a graph node" },
                    ],
                    has_tour: false,
                },
            )
            .register_perspective(BuildPerspective)
            .register_perspective_help(
                PerspectiveId("rover_build"),
                lunco_workbench::PerspectiveHelp {
                    title: "🏗 Build",
                    description: "3D scene editor. Spawn objects from the palette, \
                                  select and transform them, and assemble the scene.",
                    shortcuts: vec![
                        HelpShortcut { keys: "W / A / S / D", description: "Move camera" },
                        HelpShortcut { keys: "Q / E", description: "Move camera down / up" },
                        HelpShortcut { keys: "Shift", description: "Hold to place multiple (sticky spawn)" },
                        HelpShortcut { keys: "Delete", description: "Delete the selected object" },
                        HelpShortcut { keys: "Ctrl+Z", description: "Undo" },
                        HelpShortcut { keys: "Esc", description: "Cancel placement · clear selection / gizmo" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click", description: "Select object · confirm placement" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Select + transform gizmo (drag to move)" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                        HelpMouse { interaction: "Scroll", description: "Zoom in / out" },
                    ],
                    has_tour: false,
                },
            )
            // TODO(perspectives): re-introduce 🏔 Terrain and 🧩 Object Builder once
            // the authoring flows behind them are ready. Registration is commented
            // out — NOT deleted: `TerrainPerspective` / `ObjectBuilderPerspective`
            // and their help entries stay intact, so re-enabling is uncommenting
            // this block.
            /*
            .register_perspective(TerrainPerspective)
            .register_perspective_help(
                PerspectiveId("terrain_sculpt"),
                lunco_workbench::PerspectiveHelp {
                    title: "🏔 Terrain",
                    description: "Sculpt the surface. Arm a brush in the Tools palette, \
                                  then click the terrain to raise, dig, or flatten it. \
                                  Edits re-bake the visuals and the collider live.",
                    shortcuts: vec![
                        HelpShortcut { keys: "Shift + ↑/↓", description: "Grow / shrink brush radius" },
                        HelpShortcut { keys: "Alt + ↑/↓", description: "Grow / shrink brush strength" },
                        HelpShortcut { keys: "Esc", description: "Disarm the brush" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click", description: "Sculpt (raise) · flatten to clicked height" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Dig (invert the sculpt)" },
                        HelpMouse { interaction: "Ctrl+Left-Click", description: "Flatten to the clicked height" },
                        HelpMouse { interaction: "Shift / Alt + Scroll", description: "Brush radius / strength" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                    ],
                    has_tour: false,
                },
            )
            .register_perspective(ObjectBuilderPerspective)
            .register_perspective_help(
                PerspectiveId("object_builder"),
                lunco_workbench::PerspectiveHelp {
                    title: "🧩 Object Builder",
                    description: "Assemble and edit objects from parts. Navigate the \
                                  object's structure in the tree, attach components from \
                                  the palette, and tune the selected prim's parameters in \
                                  the Inspector.",
                    shortcuts: vec![
                        HelpShortcut { keys: "Ctrl+Z", description: "Undo the last edit" },
                        HelpShortcut { keys: "Delete", description: "Remove the selected part" },
                        HelpShortcut { keys: "Esc", description: "Clear selection / gizmo" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Click a tree node", description: "Select a part to inspect / edit" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Select + transform gizmo (drag to move)" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                    ],
                    has_tour: false,
                },
            )
            */
            ;

        // WP-8: the Entity list is a pure view over `EntityTreeView`, derived by
        // a change-gated producer instead of being rebuilt every egui frame.
        // …and its "show system entities" filter is a persisted pref exposed in the
        // workbench Settings menu, not a panel-local toolbar.
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
            resource_changed::<lunco_usd_bevy::UsdStageRevision>,
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
            resource_changed::<lunco_usd_bevy::UsdStageRevision>,
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

        // Ctrl+LMB drops a mission waypoint by AUTHORING A USD PRIM (`ApplyUsdOp`) —
        // no checkpoint command, no checkpoint domain. Moving, deleting, undoing and
        // inspecting it are then the ordinary prim paths. See `checkpoint_click`.
        app.init_resource::<checkpoint_click::WaypointContextMenuState>()
            .init_resource::<checkpoint_click::WaypointPlacement>()
            .init_resource::<scene_context::SceneContextMenuState>()
            // An armed placement names the vessel whose route it edits, and a
            // context menu names the waypoint it opened on. Both are entities of
            // the scene being unloaded — carried across a reload they leave the
            // next scene's first ground click captured by a tool aimed at a
            // vessel that no longer exists, with the possession and selection
            // observers standing down for it (`WaypointToolActive`).
            .add_systems(
                lunco_usd_bevy::scene_lifecycle::SceneTeardown,
                (
                    |mut placement: ResMut<checkpoint_click::WaypointPlacement>,
                     mut menu: ResMut<checkpoint_click::WaypointContextMenuState>| {
                        if placement.0.is_some() {
                            placement.0 = None;
                        }
                        *menu = checkpoint_click::WaypointContextMenuState::default();
                    },
                    |mut menu: ResMut<scene_context::SceneContextMenuState>| {
                        *menu = scene_context::SceneContextMenuState::default();
                    },
                    |q_reached: Query<Entity, With<lunco_autopilot::usd_tree::ReachedWaypoints>>,
                     mut commands: Commands| {
                        for entity in q_reached.iter() {
                            commands.entity(entity).remove::<lunco_autopilot::usd_tree::ReachedWaypoints>();
                        }
                    },
                ),
            )
            .add_observer(checkpoint_click::on_scene_click_checkpoint)
            .add_observer(checkpoint_click::on_scene_right_click_waypoint)
            .add_observer(scene_context::on_scene_right_click_context)
            // Consumes the ground click that follows a Move / Insert-after.
            .add_observer(checkpoint_click::on_scene_click_place_waypoint)
            // egui DRAWING belongs in the egui pass, not `Update`. bevy_egui brackets
            // a context's begin/end pass here, so a widget built outside it never joins
            // egui's input pass: the context menu PAINTED but nothing in it could be
            // clicked. (The overlay got away with `Update` only because it is
            // paint-only — no widgets, no interaction.)
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (
                    // The two WORLD overlays paint into `Order::Background` and must
                    // register their layer BEFORE the workbench builds its chrome in
                    // that same order — that is what puts the dock in front of them
                    // instead of a waypoint label in front of the Inspector. See
                    // `billboard_overlay::world_overlay_layer`.
                    (
                        checkpoint_click::draw_waypoint_overlay,
                        // USD-authored labels (`lunco:billboard`).
                        billboard_overlay::draw_billboard_overlay,
                    )
                        .before(lunco_workbench::WorkbenchRenderSet),
                    checkpoint_click::draw_waypoint_context_menu,
                    scene_context::draw_scene_context_menu,
                    // Crosshair + Esc-to-cancel while a placement is armed.
                    checkpoint_click::handle_waypoint_placement_mode,
                ),
            )
            .add_systems(
                Update,
                (
                    // USD-authored marker policies are translated once into
                    // native mesh-picking behavior.
                    scene_context::apply_pointer_policies,
                    // The route line is real 3D geometry, not an egui overlay stroke.
                    checkpoint_click::sync_waypoint_path_mesh,
                    checkpoint_click::sync_waypoint_marker_visuals,
                    checkpoint_click::handle_autopilot_toggle_hotkey,
                    // Grabbing the controls takes the vessel back from its autopilot.
                    checkpoint_click::manual_input_disengages_autopilot,
                    // Mirrors an armed placement into the shared tool gate so
                    // possession/selection stand down for that one click.
                    checkpoint_click::sync_waypoint_tool_active,
                    // `Cancel` intent (Esc/Backspace, from the data keymap) → the
                    // CancelWaypointEdit command. Backs out of ANY waypoint mode.
                    checkpoint_click::cancel_waypoint_edit_on_intent,
                ),
            );
        checkpoint_click::register_all_commands(app);
        cinematic::register_all_commands(app);

        app.add_observer(on_select_progress);
        app.add_observer(on_spawn_progress);
    }
}

fn trigger_tutorial_next(commands: &mut Commands) {
    commands.trigger(lunco_core::TelemetryEvent {
        name: "cmd:TutorialNext".to_string(),
        source: 0,
        severity: lunco_core::Severity::Info,
        data: lunco_core::TelemetryValue::Bool(true),
        timestamp: 0.0,
    });
}

fn on_select_progress(
    _trigger: On<crate::selection::SelectEntity>,
    hud: Option<Res<lunco_workbench::tutorial_overlay::TutorialHud>>,
    mut commands: Commands,
) {
    if hud.is_some_and(|h| h.tour.as_ref().and_then(|t| t.require.as_deref()) == Some("select")) {
        trigger_tutorial_next(&mut commands);
    }
}

fn on_spawn_progress(
    _trigger: On<lunco_core::SpawnEntity>,
    hud: Option<Res<lunco_workbench::tutorial_overlay::TutorialHud>>,
    mut commands: Commands,
) {
    if hud.is_some_and(|h| h.tour.as_ref().and_then(|t| t.require.as_deref()) == Some("spawn")) {
        trigger_tutorial_next(&mut commands);
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
        "🎬 View".into()
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

/// Full-window Lunica schema mode — the live composed USD connection graph.
///
/// This is a real workbench perspective rather than a runtime panel insertion
/// into View. View deliberately has no egui centre so the Bevy camera receives
/// direct pointer input; inserting a canvas there leaves the dock parked and
/// produces a blank presentation surface. Giving the canvas its own centre
/// slot makes the authored `ActivatePerspective` command deterministic and
/// lets the canvas fit the complete graph to its actual widget rectangle.
pub struct SchemaPerspective;

impl Perspective for SchemaPerspective {
    fn id(&self) -> PerspectiveId {
        PerspectiveId("lunica_schema")
    }
    fn title(&self) -> String {
        "🔗 Lunica Schema".into()
    }
    fn restores_cached_layout(&self) -> bool {
        false
    }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(None);
        layout.set_right_inspector(None);
        layout.set_bottom(None);
        layout.set_center(vec![connection_canvas::USD_CANVAS_PANEL_ID]);
        layout.set_active_center_panel(connection_canvas::USD_CANVAS_PANEL_ID);
    }
}

/// Build mode — structure + telemetry left, 3D centre, Inspector/spawn right,
/// and graph instances below.
///
/// Telemetry is a singleton, therefore it has exactly one dock location. Graph
/// instances own the bottom dock; placing Telemetry in both used the same egui
/// widget ids twice and produced the red collision diagnostics.
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
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser_tabs(vec![PanelId("entity_list"), PanelId("telemetry_browser")]);
        layout.set_center(vec![VIEWPORT_PANEL_ID]);
        layout.set_right_inspector_tabs(vec![
            PanelId("sandbox_inspector"),
            PanelId("command_deck"),
            PanelId("sandbox_environment"),
            // Optional — only renders if the host binary registers a
            // panel with this id (the rover binary does, modelica
            // workbench doesn't). The workbench filters unknown ids.
            PanelId("rover_code"),
            PanelId("spawn_palette"),
        ]);
        // The default Graphs instance is opened by the host after this
        // perspective activates. Its own `PanelSlot::Bottom` creates the sole
        // bottom dock without duplicating a singleton panel.
        layout.set_bottom(None);
    }
}

/// Object Builder mode — assemble and edit objects from parts.
///
/// Distinct from Build (which leads with the spawn palette for dropping loose
/// props into a scene): this leads with the **object's structure** — the entity
/// tree on the left, so you navigate and select a rover's rocker → bogie → wheel
/// — with the component palette beneath it for attaching parts, the 3D view in the
/// centre, and the Inspector on the right to tune the selected prim's parameters.
/// The panels are the proven ones (tree / palette / viewport / inspector); this is
/// the workspace that arranges them for building rather than observing.
///
/// The connection canvas and rhai editor that will also live here are separate,
/// larger additions; this establishes the perspective they dock into.
pub struct ObjectBuilderPerspective;

impl Perspective for ObjectBuilderPerspective {
    fn id(&self) -> PerspectiveId {
        PerspectiveId("object_builder")
    }
    // 🧩 renders in the bundled fallback (unlike 🏗, which tofus — see welcome.rs).
    fn title(&self) -> String {
        "🧩 Object Builder".into()
    }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        // Structure first: the USD prim tree (the object's authoring hierarchy)
        // to navigate/select parts, the entity list as an alternate view, and the
        // palette to add parts. (Unknown ids are filtered.)
        layout.set_side_browser_tabs(vec![
            usd_prim_tree::USD_PRIM_TREE_PANEL_ID,
            PanelId("entity_list"),
            PanelId("spawn_palette"),
        ]);
        // Three central tabs: the 3D build view, the connection canvas, and the
        // Rhai behaviour editor. The canvas rewires co-sim connections and joints;
        // the editor edits the selected prim's script; the 3D view places and
        // transforms parts. Viewport first so it's the default tab (its 3D renders
        // through the empty tab). `rhai_editor` is registered by the luncosim binary
        // (the workbench filters the id in apps that don't register it).
        layout.set_center(vec![
            VIEWPORT_PANEL_ID,
            connection_canvas::USD_CANVAS_PANEL_ID,
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
        "🏔 Terrain".into()
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
