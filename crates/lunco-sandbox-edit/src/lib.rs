//! # LunCoSim Sandbox Editing Tools
//!
//! Provides a suite of in-scene editing tools for the LunCoSim sandbox:
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

// The headless-safe half — `catalog` (spawn registry), `commands`
// (SpawnCommandPlugin = runtime spawn/move + NetReplicate tagging), `spawn_meta`,
// `shader_doc`, `doc_resolve` and `SelectedEntities` — moved out to
// `lunco-scene-commands`, so a `--no-ui` server can link the command layer without
// linking the editor. Re-exported here under their old paths: everything below is
// the in-scene editor (gizmo/picking/egui), gated on `ui`, and it reaches for them
// as `crate::catalog::…` / `crate::SelectedEntities` exactly as before.
pub use lunco_scene_commands::{catalog, commands, doc_resolve, shader_doc, spawn_meta, SelectedEntities};

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
pub mod terrain_tools;

/// UI panels — WorkbenchPanel implementations (for editor mode).
#[cfg(feature = "ui")]
pub mod ui;

use bevy::prelude::*;

/// Master plugin for all sandbox editing tools.
#[cfg(feature = "ui")]
pub struct SandboxEditPlugin;

#[cfg(feature = "ui")]
impl Plugin for SandboxEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnState>()
            .init_resource::<SelectedEntities>()
            .init_resource::<InspectorTarget>()
            .init_resource::<catalog::SpawnCatalog>()
            .init_resource::<spawn::FootprintCache>()
            .insert_resource(lunco_core::DragModeActive { active: false })
            .init_resource::<lunco_core::SpawnToolActive>()
            .init_resource::<lunco_core::TerrainToolActive>()
            .init_resource::<lunco_core::WaypointToolActive>()
            .init_resource::<lunco_core::WaypointMenuOpen>()
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
        app.add_systems(Update, selection::handle_deselect_keys);

        // Terrain-sculpt tools — arm/disarm gate, brush sizing, cursor ghost.
        app.add_systems(Update, (
            terrain_tools::terrain_tool_state_system,
            terrain_tools::terrain_brush_size_input,
            terrain_tools::update_terrain_brush_ghost,
        ));

        // Scene picking is bevy_picking-driven (egui occlusion handled by the
        // framework's egui picking backend) — no hand-rolled gate, no manual
        // ray-casts. Selection, placement and terrain-sculpt observe the same
        // `Pointer<Click>`; each stands down when another tool owns the click.
        app.add_observer(selection::on_scene_click_select);
        app.add_observer(spawn::on_scene_click_spawn);
        app.add_observer(terrain_tools::on_scene_click_terrain);

        // Editor-only `SelectEntity` API command (Inspector highlight + gizmo) —
        // registered here, not in the headless `SpawnCommandPlugin`.
        selection::register_all_commands(app);
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
        // The gizmo crate reads a target's pose from `Transform` but its camera
        // from `GlobalTransform` — under big_space those differ by a whole cell,
        // so it drew the handles 2 km off-screen in the twin (and looked fine in
        // the sandbox only because that scene sits in the origin cell). The
        // `GizmoTarget` therefore lives on an unparented proxy whose `Transform`
        // IS its render-frame pose; the drag comes back as a delta.
        app.add_systems(Update, (gizmo::spawn_gizmo_proxies, gizmo::despawn_gizmo_proxies));
        app.add_systems(
            PostUpdate,
            gizmo::sync_gizmo_proxies.after(bevy::transform::TransformSystems::Propagate),
        );
        // `First` is strictly after the crate's `Last`, so this ordering can't be
        // lost to ambiguity the way a same-schedule system would be.
        app.add_systems(First, gizmo::apply_gizmo_proxy_drag);
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

        // NOTE: waypoints have no gizmo, and no plugin. A waypoint is a USD prim
        // referencing `vessels/markers/waypoint.usda` — the USD scene renders it, the
        // ordinary transform gizmo drags it, and Delete removes it. See
        // `ui::checkpoint_click`.

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
