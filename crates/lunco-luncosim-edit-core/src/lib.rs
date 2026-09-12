//! # LunCoSim Scene Editing Tools
//!
//! Provides a suite of in-scene editing tools for the LunCoSim luncosim:
//!
//! - **Spawn System** — click-to-place rovers, props, and terrain
//! - **Scene tools** — picking, placement, and terrain tools
//! - **Undo** — Ctrl+Z / Ctrl+Shift+Z → `UndoDocument` / `RedoDocument` on the active
//!   document (see `commands::handle_undo_input`). Editor edits are USD ops, so undo is
//!   the *document's* typed-inverse history (journaled, networked) — there is no
//!   editor-side undo stack. USD's half of the verb lives in `lunco-usd`.
//!
//! Selection, transform gizmos, and egui/workbench panels are in the sibling
//! `lunco-luncosim-edit-ui` package. This package owns the editor mechanisms
//! and ECS state that the UI observes.
//!
//! ## Adding New Spawn Types
//!
//! Author a USD asset with `lunco:spawnable = true` and place it under the
//! project asset roots. `lunco-scene-commands` discovers it asynchronously and
//! publishes it through `ListSpawnCatalog`; no Rust catalog entry is required.

// The headless-safe half — spawn/picking tools, typed editor commands and
// scene-facing ECS state — stays here. Document-backed property/shader
// authoring is installed by the scene command host; egui panels, transform
// gizmos and immediate-mode diagnostics live in the sibling UI package.

pub mod spawn;
pub(crate) mod surface_pick;
pub mod terrain_picking;
pub mod terrain_tools;

use bevy::prelude::*;
use lunco_scene_catalog::catalog;
use lunco_scene_commands::{commands, SelectedEntities};

/// Master plugin for all luncosim editing tools.
pub struct SceneEditPlugin;

impl Plugin for SceneEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnState>()
            .init_resource::<SelectedEntities>()
            .init_resource::<catalog::SpawnCatalog>()
            .init_resource::<spawn::FootprintCache>()
            .init_resource::<spawn::SpawnDiagnostics>()
            .insert_resource(lunco_core::DragModeActive { active: false })
            .init_resource::<lunco_core::SpawnToolActive>()
            .init_resource::<lunco_core::TerrainToolActive>()
            .init_resource::<terrain_tools::TerrainToolState>();

        app.add_plugins(commands::SpawnCommandPlugin);

        // Non-UI systems
        app.add_systems(Update, spawn::update_spawn_ghost);
        app.add_systems(Update, spawn::spawn_tool_state_system);
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
            ),
        );

        // Scene picking is bevy_picking-driven (egui occlusion handled by the
        // framework's egui picking backend). Streamed DEM ground contributes
        // hits through the same backend set using GridSurfaceQuery; no tool
        // owns a separate click path. Selection, placement and terrain-sculpt
        // observe the same `Pointer<Click>` and stand down when another tool
        // owns the click.
        app.add_observer(spawn::on_scene_click_spawn);
        app.add_observer(terrain_tools::on_scene_click_terrain);
        // Streamed DEM tiles intentionally keep no CPU vertex copy, so the mesh
        // picking backend cannot hit open terrain. Feed the same Pointer<Click>
        // pipeline from the analytic GridSurfaceQuery instead of adding a second
        // click path to every terrain-aware tool.
        app.add_systems(
            PreUpdate,
            terrain_picking::emit_terrain_hits.in_set(bevy::picking::PickingSystems::Backend),
        );

        spawn::register_all_commands(app);

        // Ctrl+Z / Ctrl+Shift+Z → `UndoDocument` / `RedoDocument` on the active
        // document. The editor keeps NO private history: its edits are document
        // ops (gizmo drag → `TransformEntity` → one USD change set, delete →
        // `UsdOp::RemovePrim`, …), so undo is the Twin journal's undo — one
        // history, shared with the Inspector, the journal and every peer.
        app.add_systems(Update, commands::handle_undo_input);
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
