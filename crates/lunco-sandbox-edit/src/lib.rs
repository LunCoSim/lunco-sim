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
//! [`ui::SandboxEditUiPlugin`](ui::SandboxEditUiPlugin). This plugin should
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

pub mod catalog;
pub mod commands;
pub mod gizmo;
pub mod selection;
pub mod spawn;
pub mod undo;

/// UI panels — WorkbenchPanel implementations (for editor mode).
pub mod ui;

/// Overlay panels for 3D-embedded mode.
pub mod overlay;

use bevy::prelude::*;

pub use undo::{UndoStack, UndoAction};

/// Master plugin for all sandbox editing tools.
pub struct SandboxEditPlugin;

impl Plugin for SandboxEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnState>()
            .init_resource::<SelectedEntity>()
            .init_resource::<UndoStack>()
            .init_resource::<catalog::SpawnCatalog>()
            .insert_resource(lunco_core::DragModeActive { active: false });

        app.add_plugins(transform_gizmo_bevy::TransformGizmoPlugin);
        app.add_plugins(commands::SpawnCommandPlugin);

        // Non-UI systems
        app.add_systems(Update, spawn::update_spawn_ghost);
        app.add_systems(Update, spawn::handle_spawn_placement);
        // Selection system is registered in the binary with proper ordering (before avatar possession)

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
        app.add_systems(Update, gizmo::sync_gizmo_camera);
        app.add_systems(Update, undo::handle_undo_input);

        // Custom picking backend: always report selected `GizmoTarget`s as
        // hit by the pointer. Counteracts the gizmo's internal
        // `gizmo_picking_backend` gate (`any_gizmo_hovered`) which would
        // otherwise leave the gizmo inert when the cursor is over an
        // egui dock surface. We can't disable that feature on
        // `transform-gizmo-bevy 0.9` because its `gizmo_picking_backend`
        // feature gate is incomplete (unconditional `use bevy_picking::...`
        // imports), so we replicate the always-on behaviour here.
        app.add_systems(
            bevy::prelude::PreUpdate,
            always_pick_gizmo_targets.in_set(bevy_picking::PickingSystems::Backend),
        );
    }
}

/// Picking backend that always reports `GizmoTarget` entities as hit
/// by the pointer. See the comment in `SandboxEditPlugin::build` for
/// why this is necessary.
fn always_pick_gizmo_targets(
    q_targets: Query<bevy::prelude::Entity, bevy::prelude::With<transform_gizmo_bevy::GizmoTarget>>,
    pointers: Query<(&bevy_picking::pointer::PointerId, &bevy_picking::pointer::PointerLocation)>,
    mut output: bevy::prelude::MessageWriter<bevy_picking::backend::PointerHits>,
) {
    let targets: Vec<bevy::prelude::Entity> = q_targets.iter().collect();
    if targets.is_empty() {
        return;
    }
    for (pointer_id, pointer_location) in &pointers {
        if pointer_location.location.is_none() {
            continue;
        }
        let hits: Vec<(bevy::prelude::Entity, bevy_picking::backend::HitData)> = targets
            .iter()
            .map(|e| (*e, bevy_picking::backend::HitData::new(*e, 0.0, None, None)))
            .collect();
        // High order so we sit above other picking backends in the
        // HoverMap; the gizmo just needs to see the target as hit
        // (it does its own handle hit-testing internally).
        output.write(bevy_picking::backend::PointerHits::new(*pointer_id, hits, f32::INFINITY));
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

/// Tracks which entity is currently selected.
#[derive(Resource, Default)]
pub struct SelectedEntity {
    /// The selected entity, if any.
    pub entity: Option<Entity>,
}
