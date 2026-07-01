//! # LunCoSim Sandbox Editing Tools
//!
//! Provides a suite of in-scene editing tools for the LunCoSim sandbox:
//!
//! - **Spawn System** — click-to-place rovers, props, and terrain
//! - **Selection** — Shift+click entities to select them with transform gizmo
//! - **Transform Gizmo** — translate/rotate selected entities
//! - **Inspector Panel** — view entity parameters (in `ui/` module)
//! - **Undo** — Ctrl+Z to revert spawns
//!
//! ## UI
//!
//! All UI panels live in the `ui/` subdirectory and are registered via
//! [`ui::SandboxEditUiPlugin`]. This plugin should
//! be added alongside `SandboxEditPlugin` for full functionality.
//!
//! ## Adding New Spawn Types
//!
//! Add entries to `SpawnCatalog::default()` in `catalog.rs`:
//!
//! ```ignore
//! catalog.add(SpawnableEntry {
//!     id: "my_rover".into(),
//!     display_name: "My Rover".into(),
//!     category: SpawnCategory::Rover,
//!     source: SpawnSource::UsdFile("vessels/rovers/my_rover.usda".into()),
//!     default_transform: Transform::default(),
//! });
//! ```

// Headless-safe: `catalog` (spawn registry) + `commands` (SpawnCommandPlugin =
// runtime spawn/move + NetReplicate tagging) are the only parts a `--no-ui`
// server needs. Everything below is the in-scene editor (gizmo/picking/egui),
// gated on `ui`.
pub mod catalog;
pub mod commands;
#[cfg(feature = "ui")]
pub mod gizmo;
#[cfg(feature = "ui")]
pub mod joint_viz;
#[cfg(feature = "ui")]
pub mod perf_bridge;
#[cfg(feature = "ui")]
pub mod physics_viz;
#[cfg(feature = "ui")]
pub mod selection;
#[cfg(feature = "ui")]
pub mod spawn;
#[cfg(feature = "ui")]
pub mod undo;

/// UI panels — WorkbenchPanel implementations (for editor mode).
#[cfg(feature = "ui")]
pub mod ui;

use bevy::prelude::*;

#[cfg(feature = "ui")]
pub use undo::{UndoStack, UndoAction};

/// Master plugin for all sandbox editing tools.
#[cfg(feature = "ui")]
pub struct SandboxEditPlugin;

#[cfg(feature = "ui")]
impl Plugin for SandboxEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnState>()
            .init_resource::<SelectedEntities>()
            .init_resource::<InspectorTarget>()
            .init_resource::<UndoStack>()
            .init_resource::<catalog::SpawnCatalog>()
            .insert_resource(lunco_core::DragModeActive { active: false })
            .init_resource::<lunco_core::SpawnToolActive>();

        app.add_plugins(transform_gizmo_bevy::TransformGizmoPlugin);
        app.add_plugins(commands::SpawnCommandPlugin);
        app.add_plugins(perf_bridge::PerfBridgePlugin);

        // Non-UI systems
        app.add_systems(Update, spawn::update_spawn_ghost);
        app.add_systems(Update, spawn::spawn_tool_state_system);
        app.add_systems(Update, selection::handle_deselect_keys);

        // Scene picking is bevy_picking-driven (egui occlusion handled by the
        // framework's egui picking backend) — no hand-rolled gate, no manual
        // ray-casts. Selection and placement observe the same `Pointer<Click>`.
        app.add_observer(selection::on_scene_click_select);
        app.add_observer(spawn::on_scene_click_spawn);

        // Editor-only `SelectEntity` API command (Inspector highlight + gizmo) —
        // registered here, not in the headless `SpawnCommandPlugin`.
        app.add_observer(selection::on_select_entity);
        app.register_type::<selection::SelectEntity>();
        app.add_systems(Update, selection::draw_selection_bounds);

        // Gizmo systems run in Last schedule (after transform-gizmo-bevy's update_gizmos):
        // 1. capture_gizmo_start - makes body kinematic when drag starts
        // 2. sync_gizmo_transforms - syncs Position + GlobalTransform from Transform
        // 3. restore_gizmo_dynamic - restores dynamic when drag ends
        //
        // NOTE: TransformGizmoPlugin is added before this plugin, so its update_gizmos
        // system runs first in the Last schedule (systems run in registration order).
        app.add_systems(Last, (
            gizmo::capture_gizmo_start,
            gizmo::sync_gizmo_transforms.after(gizmo::capture_gizmo_start),
            gizmo::restore_gizmo_dynamic.after(gizmo::sync_gizmo_transforms),
        ));
        app.add_systems(
            PostUpdate,
            gizmo::restore_dragged_transform
                .after(avian3d::schedule::PhysicsSystems::Writeback)
                .before(bevy::transform::TransformSystems::Propagate),
        );
        app.add_systems(Update, gizmo::sync_gizmo_camera);
        // Publish the drag state as the core `GizmoDragging` marker so transform-
        // gizmo-free crates (avatar camera follow) can read it.
        app.add_systems(Update, gizmo::sync_gizmo_dragging_marker);
        app.add_systems(Update, undo::handle_undo_input);

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
#[derive(Resource, Default)]
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

/// Tracks which entities are currently selected.
#[derive(Resource, Default, Clone)]
pub struct SelectedEntities {
    /// The selected entities. The last one added is the "primary" selection.
    pub entities: Vec<Entity>,
}

impl SelectedEntities {
    /// Returns the primary selected entity, if any.
    pub fn primary(&self) -> Option<Entity> {
        self.entities.last().copied()
    }
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
