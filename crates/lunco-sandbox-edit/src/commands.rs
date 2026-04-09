//! Command handler for spawn operations.
//!
//! Listens for `SPAWN_ENTITY:<entry_id>` [CommandMessage] events and spawns
//! the corresponding entity at the given world position. This enables spawning
//! via both the UI palette and the CLI.

use bevy::prelude::*;
use avian3d::prelude::*;
use big_space::prelude::Grid;
use lunco_core::architecture::CommandMessage;

use crate::catalog::{SpawnCatalog, spawn_procedural, spawn_usd_entry};

/// Observer that handles SPAWN_ENTITY commands.
///
/// Command format:
/// - `name`: `"SPAWN_ENTITY:<entry_id>"` (e.g., `"SPAWN_ENTITY:ball_dynamic"`)
/// - `args[0..3]`: world position (x, y, z)
pub fn on_spawn_entity_command(
    trigger: On<CommandMessage>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
) {
    let cmd = trigger.event();

    // Parse command name: "SPAWN_ENTITY:<entry_id>"
    let Some(entry_id) = cmd.name.strip_prefix("SPAWN_ENTITY:") else { return; };

    let entry = match catalog.get(entry_id) {
        Some(e) => e,
        None => {
            warn!("SPAWN_ENTITY: unknown entry '{}'", entry_id);
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

    let point = Vec3::new(cmd.args[0] as f32, cmd.args[1] as f32, cmd.args[2] as f32);

    info!("SPAWN_ENTITY: {} at {:?}", entry_id, point);

    match entry.source {
        crate::catalog::SpawnSource::Procedural(_) => {
            spawn_procedural(&mut commands, &mut meshes, &mut materials, entry, point, grid);
        }
        crate::catalog::SpawnSource::UsdFile(_) => {
            spawn_usd_entry(&mut commands, &asset_server, entry, point, grid);
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
    use super::*;

    #[test]
    fn test_command_name_parsing() {
        let cmd_name = "SPAWN_ENTITY:ball_dynamic";
        let entry_id = cmd_name.strip_prefix("SPAWN_ENTITY:");
        assert_eq!(entry_id, Some("ball_dynamic"));

        let cmd_name = "DRIVE_ROVER";
        let entry_id = cmd_name.strip_prefix("SPAWN_ENTITY:");
        assert_eq!(entry_id, None);
    }
}
