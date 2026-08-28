//! # LunCoSim Scene Editing Tools
//!
//! Provides a suite of in-scene editing tools for the LunCoSim luncosim:
//!
//! - **Spawn System** — click-to-place rovers, props, and terrain
//! - **Selection** — Shift+click entities to select them with transform gizmo
//! - **Transform Gizmo** — translate/rotate selected entities
//! - **Inspector Panel** — view entity parameters (in `ui/` module)
//! - **Undo** — Ctrl+Z / Ctrl+Shift+Z → `UndoDocument` / `RedoDocument` on the active
//!   document (see `commands::handle_undo_input`). Editor edits are USD ops, so undo is
//!   the *document's* typed-inverse history (journaled, networked) — there is no
//!   editor-side undo stack. USD's half of the verb lives in `lunco-usd`.
//!
//! ## UI
//!
//! All UI panels live in the `ui/` subdirectory and are registered via
//! [`ui::SceneEditUiPlugin`]. This plugin should
//! be added alongside `SceneEditPlugin` for full functionality.
//!
//! ## Adding New Spawn Types
//!
//! Author a USD asset with `lunco:spawnable = true` and place it under the
//! project asset roots. `lunco-scene-commands` discovers it asynchronously and
//! publishes it through `ListSpawnCatalog`; no Rust catalog entry is required.

// The headless-safe half — `catalog` (spawn registry), `commands`
// (SpawnCommandPlugin = runtime spawn/move + NetReplicate tagging), `spawn_meta`,
// `shader_doc`, `doc_resolve` and `SelectedEntities` — moved out to
// `lunco-scene-commands`, so a `--no-ui` server can link the command layer without
// linking the editor. The in-scene editor imports those modules from their owning
// crate directly; this crate contains only gizmo/picking/egui concerns.

#[cfg(feature = "ui")]
pub mod gizmo;
#[cfg(feature = "ui")]
pub mod joint_viz;
#[cfg(feature = "ui")]
pub mod perf_bridge;
#[cfg(feature = "ui")]
pub mod physics_gizmo;
#[cfg(feature = "ui")]
pub mod physics_viz;
#[cfg(feature = "ui")]
pub mod script_tools;
#[cfg(feature = "ui")]
pub mod selection;
#[cfg(feature = "ui")]
pub mod spawn;
#[cfg(feature = "ui")]
pub(crate) mod surface_pick;
#[cfg(feature = "ui")]
pub mod terrain_picking;
#[cfg(feature = "ui")]
pub mod terrain_tools;

/// UI panels — `lunco-workbench::Panel` implementations (for editor mode).
#[cfg(feature = "ui")]
pub mod ui;

use bevy::prelude::*;
#[cfg(feature = "ui")]
use lunco_scene_commands::{catalog, commands, shader_doc, SelectedEntities};

/// Master plugin for all luncosim editing tools.
#[cfg(feature = "ui")]
pub struct SceneEditPlugin;

#[cfg(feature = "ui")]
impl Plugin for SceneEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnState>()
            .init_resource::<SelectedEntities>()
            .init_resource::<InspectorTarget>()
            .init_resource::<catalog::SpawnCatalog>()
            .init_resource::<spawn::FootprintCache>()
            .init_resource::<spawn::SpawnDiagnostics>()
            .insert_resource(lunco_core::DragModeActive { active: false })
            .init_resource::<lunco_core::SpawnToolActive>()
            .init_resource::<lunco_core::TerrainToolActive>()
            .init_resource::<lunco_core::WaypointToolActive>()
            .init_resource::<lunco_core::WaypointMenuOpen>()
            .init_resource::<lunco_core::ArmedScriptTool>()
            .init_resource::<gizmo::GizmoDragSession>()
            .init_resource::<terrain_tools::TerrainToolState>()
            // Shader source is a journaled domain: edits record to the Twin
            // journal + hot-reload. The recorder attaches when the journal appears.
            .init_resource::<shader_doc::ShaderRegistry>();
        app.add_systems(
            Update,
            shader_doc::wire_shader_journal_handle
                .run_if(resource_added::<lunco_doc_bevy::JournalResource>),
        );

        app.add_plugins(transform_gizmo_bevy::TransformGizmoPlugin);
        app.add_plugins(commands::SpawnCommandPlugin);
        app.add_plugins(perf_bridge::PerfBridgePlugin);

        // Non-UI systems
        app.add_systems(Update, spawn::update_spawn_ghost);
        app.add_systems(Update, spawn::spawn_tool_state_system);
        // Possession is the user's active vehicle context. Mirror it through
        // the one selection mutation so the Inspector follows the controlled
        // rover without inventing a second inspector-target mechanism.
        app.add_systems(Update, selection::select_possessed_vessel);
        app.add_observer(ui::spawn_palette::on_spawn_state_requested);
        app.add_observer(ui::terrain_tools::on_terrain_ui_action);
        app.add_observer(selection::on_select_entity_target);
        app.add_systems(Update, selection::handle_deselect_keys);
        // Selection → telemetry focus is NOT here: it moved down to the command
        // layer beside `SelectedEntities` itself
        // (`lunco_scene_commands::mirror_selection_to_telemetry_focus`, installed
        // by `SpawnCommandPlugin` above), so every host with the scene verbs gets
        // scoped telemetry instead of only this editor.

        // Terrain-sculpt tools — arm/disarm gate, brush sizing, cursor ghost.
        app.add_systems(
            Update,
            (
                terrain_tools::terrain_tool_state_system,
                terrain_tools::terrain_brush_size_input,
                terrain_tools::update_terrain_brush_ghost,
                // Script-authored click tools: keyboard exit + drop an armed
                // name whose tool has gone away (libraries hot-reload).
                script_tools::disarm_script_tool_on_cancel,
                script_tools::forget_missing_script_tool,
            ),
        );

        // Scene picking is bevy_picking-driven (egui occlusion handled by the
        // framework's egui picking backend). Streamed DEM ground contributes
        // hits through the same backend set using GridSurfaceQuery; no tool
        // owns a separate click path. Selection, placement and terrain-sculpt
        // observe the same `Pointer<Click>` and stand down when another tool
        // owns the click.
        app.add_observer(selection::on_scene_click_select);
        app.add_observer(spawn::on_scene_click_spawn);
        app.add_observer(terrain_tools::on_scene_click_terrain);
        app.add_observer(script_tools::on_scene_click_script_tool);
        // Streamed DEM tiles intentionally keep no CPU vertex copy, so the mesh
        // picking backend cannot hit open terrain. Feed the same Pointer<Click>
        // pipeline from the analytic GridSurfaceQuery instead of adding a second
        // click path to every terrain-aware tool.
        app.add_systems(
            PreUpdate,
            terrain_picking::emit_terrain_hits.in_set(bevy::picking::PickingSystems::Backend),
        );

        spawn::register_all_commands(app);

        // Editor-only `SelectEntity` API command (Inspector highlight + gizmo) —
        // registered here, not in the headless `SpawnCommandPlugin`.
        selection::register_all_commands(app);
        app.add_systems(Update, selection::draw_selection_bounds);

        // Capture and restore are lifecycle edges. The pose itself is driven in
        // InteractionSchedule below, which continues to run while physics is held.
        //
        // NOTE: TransformGizmoPlugin is added before this plugin, so its update_gizmos
        // system runs first in the Last schedule (systems run in registration order).
        app.add_systems(
            Last,
            (
                gizmo::capture_gizmo_start,
                gizmo::restore_gizmo_dynamic.after(gizmo::capture_gizmo_start),
            ),
        );
        app.add_systems(
            lunco_time::InteractionSchedule,
            (
                gizmo::apply_gizmo_proxy_drag.after(lunco_time::InteractionRestoreSet),
                gizmo::drive_gizmo_kinematic_pose
                    .after(gizmo::apply_gizmo_proxy_drag)
                    .before(lunco_time::InteractionRecordSet),
                lunco_physics::apply_kinematic_drives
                    .after(gizmo::drive_gizmo_kinematic_pose)
                    .before(lunco_time::InteractionRecordSet),
            ),
        );
        app.add_systems(Update, gizmo::sync_gizmo_camera);
        // The gizmo crate reads a target's pose from `Transform` but its camera
        // from `GlobalTransform` — under big_space those differ by a whole cell,
        // so it drew the handles 2 km off-screen in the twin (and looked fine in
        // the luncosim only because that scene sits in the origin cell). The
        // `GizmoTarget` therefore lives on an unparented proxy whose `Transform`
        // IS its render-frame pose; the drag comes back as a delta.
        app.add_systems(
            Update,
            (gizmo::spawn_gizmo_proxies, gizmo::despawn_gizmo_proxies),
        );
        app.add_systems(
            PostUpdate,
            gizmo::sync_gizmo_proxies.after(bevy::transform::TransformSystems::Propagate),
        );
        app.add_systems(Update, gizmo::drive_gizmo_drag_no_shift);
        // Publish the drag state as the core `GizmoDragging` marker so transform-
        // gizmo-free crates (avatar camera follow) can read it.
        app.add_systems(Update, gizmo::sync_gizmo_dragging_marker);
        // Ctrl+Z / Ctrl+Shift+Z → `UndoDocument` / `RedoDocument` on the active
        // document. The editor keeps NO private history: its edits are document
        // ops (gizmo drag → `MoveEntity` → `UsdOp::SetTranslate`, delete →
        // `UsdOp::RemovePrim`, …), so undo is the Twin journal's undo — one
        // history, shared with the Inspector, the journal and every peer.
        app.add_systems(Update, commands::handle_undo_input);

        // Physics-state arrows (velocity, force) for entities that
        // opt in via `PhysicsArrows`. Cheap when no entity opts in.
        app.init_resource::<physics_viz::GlobalPhysicsArrows>();
        app.register_type::<physics_viz::PhysicsArrows>();
        app.add_systems(Startup, physics_viz::configure_gizmo_overlay);
        app.add_systems(
            Update,
            (
                physics_viz::auto_mark_dynamic_bodies,
                physics_viz::draw_physics_arrows,
            ),
        );
        physics_viz::register_all_commands(app);

        // Joint + wheel-force visualization gizmos (toggled via
        // `ToggleJointViz` command — reachable from UI / API / Rhai).
        app.init_resource::<joint_viz::JointVizSettings>();
        app.add_systems(
            Update,
            (joint_viz::draw_joint_viz, joint_viz::draw_wheel_force_viz),
        );
        joint_viz::register_all_commands(app);

        // Selected-body dynamics gizmo: CoM + inertia ellipsoid + force
        // arrows, and body-frame triads (`TogglePhysicsGizmo` command /
        // workbench Settings menu). Draws only for the current selection;
        // off by default, so idle cost is two early-returns.
        app.init_resource::<physics_gizmo::PhysicsGizmoSettings>();
        app.add_systems(
            Update,
            (
                physics_gizmo::draw_physics_gizmo,
                physics_gizmo::draw_frame_gizmo,
            ),
        );
        physics_gizmo::register_all_commands(app);

        // NOTE: waypoints have no gizmo, and no plugin. A waypoint is a USD prim
        // referencing `vessels/markers/waypoint.usda` — the USD scene renders it, the
        // ordinary transform gizmo drags it, and Delete removes it. See
        // `ui::waypoint_click`.

        // NOTE: gizmo handle picking is provided by transform-gizmo-bevy's own
        // `TransformGizmoPickingPlugin` (added by `TransformGizmoPlugin`). Its
        // backend reports a target as hit ONLY when the cursor is actually over
        // a handle (`gizmo.pick_preview`), at picking order `0.0`.
        //
        // We deliberately do NOT add an "always report gizmo targets" backend.
        // An earlier version did (emitting every `GizmoTarget` at `f32::INFINITY`
        // every frame), which masked all real mesh hits in the `HoverMap`: once
        // one object was selected, every click resolved to that gizmo target
        // instead of the entity under the cursor — breaking Shift-click
        // multi-select and possessing a *different* rover. That override was a
        // leftover from the docked-egui-viewport era; with the full-window 3D
        // viewport + bevy_picking it is pure harm, so it's gone.
    }
}

/// Current state of the spawn system.
#[derive(Resource, Default, Clone)]
pub enum SpawnState {
    /// No spawn in progress.
    #[default]
    Idle,
    /// User has selected an entry from the palette, awaiting placement click.
    Selecting {
        /// ID of the catalog entry to spawn.
        entry_id: String,
    },
}

/// Which sub-part of the [`SelectedEntities`] the Inspector edits.
///
/// Selection targets a logical component root (a rover), but a component has
/// many material-bearing parts (4 wheels + body). This narrows editing to one
/// of them. `None` = "whole object" (edit the first shader holder + all PBR
/// materials in the subtree, the bulk default). Set by the Inspector's *Parts*
/// selector or by a viewport drill-click (clicking a part of the already-
/// selected object). The Inspector validates it against the current selection's
/// subtree each frame, so a stale part from a previous selection is ignored.
#[derive(Resource, Default)]
pub struct InspectorTarget {
    /// The targeted sub-part entity, or `None` for the whole object.
    pub part: Option<Entity>,
}
