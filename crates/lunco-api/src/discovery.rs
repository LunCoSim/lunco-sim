//! Schema discovery — tells API clients what commands exist.

use crate::queries::{ApiQueryRegistry, ApiVisibility};
use crate::schema::{ApiSchema, CommandSchema, FieldSchema};
use bevy::prelude::*;
use bevy::reflect::std_traits::ReflectDefault;
use bevy::reflect::{TypeInfo, TypeRegistration, TypeRegistry};
use std::collections::HashMap;

/// Why a command name could not be resolved to one public typed command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiCommandLookupError {
    /// No reflected type has this short name.
    NotFound,
    /// More than one reflected type has this short name.
    Ambiguous,
    /// A reflected type has this name, but it is not a `#[Command]` event.
    NotApiCommand,
    /// A valid command exists, but the current API visibility policy hides it.
    Hidden,
}

impl ApiCommandLookupError {
    /// Stable client-facing text for all name-resolution failures.
    pub fn message(self, name: &str) -> String {
        match self {
            Self::Ambiguous => {
                format!("Command '{name}' is ambiguous: more than one reflected type has this name")
            }
            Self::NotFound | Self::NotApiCommand | Self::Hidden => {
                format!("Command '{name}' not found or not API-accessible")
            }
        }
    }
}

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

/// Resolve one short command name through the same reflected API boundary used
/// by discovery and transports.
///
/// `TypeRegistry::get_with_short_type_path` intentionally returns `None` for an
/// ambiguous short name. Walking the registrations here lets us distinguish a
/// missing command from a plugin collision and prevents a schema from silently
/// choosing whichever registration happened to be visited first.
pub fn find_api_command<'a>(
    type_registry: &'a TypeRegistry,
    name: &str,
    visibility: Option<&ApiVisibility>,
) -> Result<&'a TypeRegistration, ApiCommandLookupError> {
    let matches: Vec<&TypeRegistration> = type_registry
        .iter()
        .filter(|registration| registration.type_info().type_path_table().short_path() == name)
        .collect();

    let registration = match matches.as_slice() {
        [] => return Err(ApiCommandLookupError::NotFound),
        [registration] => *registration,
        _ => return Err(ApiCommandLookupError::Ambiguous),
    };

    if !is_api_command(registration, None) {
        return Err(ApiCommandLookupError::NotApiCommand);
    }
    if visibility.is_some_and(|policy| policy.is_hidden(name)) {
        return Err(ApiCommandLookupError::Hidden);
    }
    Ok(registration)
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
    let mut short_name_counts = HashMap::<String, usize>::new();
    for registration in type_registry.iter() {
        *short_name_counts
            .entry(
                registration
                    .type_info()
                    .type_path_table()
                    .short_path()
                    .to_owned(),
            )
            .or_default() += 1;
    }

    let commands: Vec<CommandSchema> = type_registry
        .iter()
        .filter_map(|reg| {
            let info = reg.type_info();
            if !is_api_command(reg, visibility) {
                return None;
            }
            let short_name = info.type_path_table().short_path();
            if short_name_counts.get(short_name) != Some(&1) {
                warn!(
                    "[lunco-api] omitting ambiguous API command '{}' from discovery",
                    short_name
                );
                return None;
            }
            let struct_info = match info {
                TypeInfo::Struct(s) => s,
                _ => return None,
            };
            let fields: Vec<FieldSchema> = struct_info
                .iter()
                .map(|f: &bevy::reflect::NamedField| FieldSchema {
                    name: f.name().to_string(),
                    type_name: f.type_path().to_string(),
                })
                .collect();
            Some(CommandSchema {
                name: short_name.to_string(),
                defaulted: reg.data::<ReflectDefault>().is_some(),
                fields,
            })
        })
        .collect();
    let mut commands = commands;
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

    #[test]
    fn ambiguous_short_names_are_not_discoverable_or_routable() {
        mod left {
            use bevy::ecs::reflect::ReflectEvent;

            #[lunco_core::Command]
            pub struct Collision {
                pub value: u32,
            }
        }
        mod right {
            use bevy::ecs::reflect::ReflectEvent;

            #[lunco_core::Command]
            pub struct Collision {
                pub value: u32,
            }
        }

        let mut registry = TypeRegistry::new();
        registry.register::<left::Collision>();
        registry.register::<right::Collision>();

        assert!(discover_commands(&registry, None).is_empty());
        assert!(matches!(
            find_api_command(&registry, "Collision", None),
            Err(ApiCommandLookupError::Ambiguous)
        ));
    }
}
