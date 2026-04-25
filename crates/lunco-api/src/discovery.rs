//! Schema discovery — tells API clients what commands exist.

use bevy::prelude::*;
use bevy::reflect::{TypeInfo, TypeRegistry};
use crate::queries::ApiVisibility;
use crate::schema::{ApiSchema, CommandSchema, FieldSchema};

/// Discover LunCo commands from the type registry.
/// Filters to only types from `lunco_*` crates that have `ReflectEvent`.
/// Hidden commands (per [`ApiVisibility`]) are filtered out — they remain
/// reflectable and dispatchable inside the app, but external API
/// consumers see them as if they did not exist.
pub(crate) fn discover_commands(
    type_registry: &TypeRegistry,
    visibility: Option<&ApiVisibility>,
) -> Vec<CommandSchema> {
    type_registry.iter()
        .filter_map(|reg| {
            let info = reg.type_info();
            if !matches!(info, TypeInfo::Struct(_)) { return None; }
            let Some(_) = reg.data::<bevy::ecs::reflect::ReflectEvent>() else { return None; };
            let struct_info = match info { TypeInfo::Struct(s) => s, _ => return None };
            let short_name = info.type_path_table().short_path().to_string();
            if short_name.starts_with("Api") || short_name.starts_with("Telemetry") { return None; }
            let full_path = info.type_path_table().path();
            if !full_path.contains("lunco_") { return None; }
            // Visibility filter — last gate before the command becomes
            // part of the externally-advertised schema.
            if visibility.is_some_and(|v| v.is_hidden(&short_name)) {
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

/// Builds the API schema by introspecting the ECS world.
pub fn discover_schema(world: &World) -> ApiSchema {
    let type_registry = world.resource::<AppTypeRegistry>();
    let registry_read = type_registry.read();
    let visibility = world.get_resource::<ApiVisibility>();
    let commands = discover_commands(&registry_read, visibility);
    ApiSchema { commands }
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
