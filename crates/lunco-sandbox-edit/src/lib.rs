//! # LunCoSim Sandbox Editing Tools
//!
//! Provides a suite of in-scene editing tools for the LunCoSim sandbox:
//!
//! - **Spawn System** — click-to-place rovers, props, and terrain
//! - **Selection** — click entities to select them
//! - **Transform Gizmo** — translate/rotate selected entities
//! - **Inspector Panel** — view entity parameters
//! - **Undo** — Ctrl+Z to revert spawns
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

pub mod catalog;
pub mod commands;
pub mod gizmo;
pub mod inspector;
pub mod palette;
pub mod pickup;
pub mod selection;
pub mod spawn;
pub mod undo;

use bevy::prelude::*;

pub use undo::UndoStack;

/// Master plugin for all sandbox editing tools.
pub struct SandboxEditPlugin;

impl Plugin for SandboxEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnState>()
            .init_resource::<SelectedEntity>()
            .init_resource::<UndoStack>()
            .init_resource::<catalog::SpawnCatalog>();

        app.add_plugins(transform_gizmo_bevy::TransformGizmoPlugin);
        app.add_plugins(commands::SpawnCommandPlugin);

        // Systems registered individually to avoid chain() tuple limits
        // Palette panel MUST run in EguiPrimaryContextPass to access ctx_mut()
        app.add_systems(bevy_egui::EguiPrimaryContextPass, palette::spawn_palette_panel);
        app.add_systems(Update, spawn::update_spawn_ghost);
        app.add_systems(Update, spawn::handle_spawn_placement);
        app.add_systems(Update, selection::handle_entity_selection);
        app.add_systems(Update, gizmo::sync_gizmo_mode);
        app.add_systems(Update, gizmo::sync_gizmo_target);
        // Inspector panel MUST run in EguiPrimaryContextPass to access ctx_mut()
        app.add_systems(bevy_egui::EguiPrimaryContextPass, inspector::inspector_panel);
        app.add_systems(Update, pickup::sync_pickup_enabled);
        app.add_systems(Update, undo::handle_undo_input);
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

/// Tracks which entity is currently selected and what tool is active.
#[derive(Resource, Default)]
pub struct SelectedEntity {
    /// The selected entity, if any.
    pub entity: Option<Entity>,
    /// Current gizmo/tool mode.
    pub mode: ToolMode,
    /// Whether the physics pickup tool is currently active.
    pub is_picking_up: bool,
}

/// Tool mode determining what the user can do with the selected entity.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    /// Click to select entities.
    #[default]
    Select,
    /// Translate (move) selected entity.
    Translate,
    /// Rotate selected entity.
    Rotate,
    /// Grab and throw dynamic rigid bodies.
    Pickup,
}
