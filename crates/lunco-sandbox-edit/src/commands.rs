//! Command handler for spawn operations.
//!
//! Listens for `SpawnEntity` typed commands and spawns the corresponding entity
//! at the given world position. This enables spawning via both the UI palette
//! and external API clients.

use bevy::prelude::*;
use big_space::prelude::Grid;
use lunco_core::Command;
use crate::catalog::{SpawnCatalog, spawn_procedural, spawn_usd_entry};

/// Spawn an entity from the catalog at a given world position.
#[Command]
pub struct SpawnEntity {
    /// The grid entity to spawn under.
    pub target: Entity,
    /// The catalog entry ID (e.g. "ball_dynamic", "skid_rover").
    pub entry_id: String,
    /// World-space position (x, y, z).
    pub position: Vec3,
}

/// Observer that handles SpawnEntity commands.
pub fn on_spawn_entity_command(
    trigger: On<SpawnEntity>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
) {
    let cmd = trigger.event();

    let entry = match catalog.get(&cmd.entry_id) {
        Some(e) => e,
        None => {
            warn!("SPAWN_ENTITY: unknown entry '{}'", cmd.entry_id);
            return;
        }
    };

    let grid = match q_grids.get(cmd.target) {
        Ok(g) => g,
        Err(_) => {
            warn!("SPAWN_ENTITY: target entity is not a Grid");
            return;
        }
    };

    info!("SPAWN_ENTITY: {} at {:?}", cmd.entry_id, cmd.position);

    match entry.source {
        crate::catalog::SpawnSource::Procedural(_) => {
            spawn_procedural(&mut commands, &mut meshes, &mut materials, entry, cmd.position, grid);
        }
        crate::catalog::SpawnSource::UsdFile(_) => {
            spawn_usd_entry(&mut commands, &asset_server, entry, cmd.position, grid);
        }
    }
}

/// Plugin that registers the SPAWN_ENTITY command observer.
pub struct SpawnCommandPlugin;

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_spawn_entity_command);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_spawn_entity_struct_exists() {
        // Verify the struct can be constructed
        let cmd = super::SpawnEntity {
            target: bevy::prelude::Entity::PLACEHOLDER,
            entry_id: "test".to_string(),
            position: bevy::math::Vec3::ZERO,
        };
        assert_eq!(cmd.entry_id, "test");
    }
}
