//! Schema discovery — tells API clients what commands exist.

use bevy::prelude::*;
use bevy::reflect::TypeInfo;
use crate::schema::{ApiSchema, CommandSchema, FieldSchema};

/// Builds the API schema by introspecting the ECS world.
pub fn discover_schema(world: &World) -> ApiSchema {
    let commands = discover_commands(world);
    ApiSchema { commands }
}

fn discover_commands(world: &World) -> Vec<CommandSchema> {
    let type_registry = world.resource::<AppTypeRegistry>();
    let registry_read = type_registry.read();

    registry_read.iter()
        .filter_map(|reg| {
            let info = reg.type_info();
            if !matches!(info, TypeInfo::Struct(_)) { return None; }
            let struct_info = match info {
                TypeInfo::Struct(s) => s,
                _ => return None,
            };
            let short_name = info.type_path_table().short_path().to_string();
            if short_name.starts_with("Api") || short_name.starts_with("Telemetry") {
                return None;
            }
            let fields: Vec<FieldSchema> = struct_info.iter().map(|f: &bevy::reflect::NamedField| FieldSchema {
                name: f.name().to_string(),
                type_name: f.type_path().to_string(),
            }).collect();
            Some(CommandSchema { name: short_name, fields })
        })
        .collect()
}

/// Plugin that registers schema discovery (no runtime systems needed).
pub struct ApiDiscoveryPlugin;
impl Plugin for ApiDiscoveryPlugin {
    fn build(&self, _app: &mut App) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_runs() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, lunco_core::LunCoCorePlugin));
        let schema = discover_schema(&app.world());
        // Schema should not crash; may be empty
        let _ = schema;
    }
}
