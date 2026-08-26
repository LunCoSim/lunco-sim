//! Schema discovery — tells API clients what commands exist.

use crate::queries::{ApiQueryRegistry, ApiVisibility};
use crate::schema::{ApiSchema, CommandSchema, FieldSchema};
use bevy::prelude::*;
use bevy::reflect::{TypeInfo, TypeRegistration, TypeRegistry};

/// True when a type registration is an externally callable LunCo command.
///
/// This is the predicate shared by discovery and dispatch. Keeping it here
/// prevents the executor from accepting an arbitrary reflected event that is
/// not part of the public command surface.
pub fn is_api_command(registration: &TypeRegistration, visibility: Option<&ApiVisibility>) -> bool {
    let info = registration.type_info();
    let is_marked_command = matches!(
        info,
        TypeInfo::Struct(struct_info)
            if struct_info
                .get_attribute::<lunco_core::ApiCommandMarker>()
                .is_some()
    );
    if !is_marked_command
        || registration
            .data::<bevy::ecs::reflect::ReflectEvent>()
            .is_none()
    {
        return false;
    }
    let short_name = info.type_path_table().short_path();
    !visibility.is_some_and(|v| v.is_hidden(short_name))
}

/// Discover LunCo commands from the type registry.
/// Filters to only types emitted by the `#[Command]` macro that have
/// `ReflectEvent`.
/// Hidden commands (per [`ApiVisibility`]) are filtered out — they remain
/// reflectable and dispatchable inside the app, but external API
/// consumers see them as if they did not exist.
/// Public so other crates (e.g. the scripting authoring catalog) can reuse the
/// canonical command-reflection walk instead of duplicating it.
pub fn discover_commands(
    type_registry: &TypeRegistry,
    visibility: Option<&ApiVisibility>,
) -> Vec<CommandSchema> {
    let mut commands: Vec<CommandSchema> = type_registry
        .iter()
        .filter_map(|reg| {
            let info = reg.type_info();
            if !is_api_command(reg, visibility) {
                return None;
            }
            let struct_info = match info {
                TypeInfo::Struct(s) => s,
                _ => return None,
            };
            let short_name = info.type_path_table().short_path().to_string();
            let fields: Vec<FieldSchema> = struct_info
                .iter()
                .map(|f: &bevy::reflect::NamedField| FieldSchema {
                    name: f.name().to_string(),
                    type_name: f.type_path().to_string(),
                })
                .collect();
            Some(CommandSchema {
                name: short_name,
                fields,
            })
        })
        .collect();
    commands.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    commands
}

/// Discover data-returning query providers registered by the runtime.
///
/// Providers are intentionally represented by their stable names here. Their
/// parameter and response contracts live with the provider implementation, and
/// the result is still self-describing JSON. Sorting makes the wire schema
/// stable despite the registry's hash-map storage.
pub fn discover_queries(registry: Option<&ApiQueryRegistry>) -> Vec<String> {
    let mut queries = registry
        .map(|registry| registry.names().map(str::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    queries.sort_unstable();
    queries
}

/// Builds the API schema by introspecting the ECS world.
pub fn discover_schema(world: &World) -> ApiSchema {
    let type_registry = world.resource::<AppTypeRegistry>();
    let registry_read = type_registry.read();
    let visibility = world.get_resource::<ApiVisibility>();
    let commands = discover_commands(&registry_read, visibility);
    let queries = discover_queries(world.get_resource::<ApiQueryRegistry>());
    ApiSchema { commands, queries }
}

/// Plugin that registers schema discovery (no runtime systems needed).
pub struct ApiDiscoveryPlugin;
impl Plugin for ApiDiscoveryPlugin {
    fn build(&self, _app: &mut App) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[lunco_core::Command(default)]
    struct PluginCommand {
        value: u32,
    }

    #[derive(Event, Reflect, Clone, Debug)]
    #[reflect(Event)]
    struct ReflectedEvent;

    #[test]
    fn test_discovery_runs() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, lunco_core::LunCoCorePlugin));
        let schema = discover_schema(app.world());
        // Schema should not crash; may be empty
        let _ = schema;
    }

    #[test]
    fn command_marker_is_the_authoritative_api_boundary() {
        let mut registry = TypeRegistry::new();
        registry.register::<PluginCommand>();
        registry.register::<ReflectedEvent>();

        let commands = discover_commands(&registry, None);

        assert_eq!(
            commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            vec!["PluginCommand"]
        );
    }
}
