//! API request executor — processes `ApiRequest` and produces `ApiResponse`.
//!
//! Uses Bevy's `AppTypeRegistry` to discover all typed commands (`Event + Reflect`)
//! for schema discovery. Commands are triggered as `ApiCommandEvent` which carries
//! the command name and JSON params.
//!
//! Domain observers can observe both:
//! - `On<DriveRover>` for internal triggers
//! - `On<ApiCommandEvent>` for API triggers (downcast the command)

use bevy::prelude::*;
use bevy::reflect::{TypeRegistry, NamedField};
use crate::{
    registry::ApiEntityRegistry,
    schema::{ApiErrorCode, ApiRequest, ApiResponse, ApiSchema, CommandSchema, FieldSchema},
};

/// Events that transport adapters send to request API operations.
#[derive(Event, Debug)]
pub struct ApiRequestEvent {
    pub request: ApiRequest,
    pub correlation_id: u64,
}

/// Events that the executor sends back to transports with results.
#[derive(Event, Debug)]
pub struct ApiResponseEvent {
    pub response: ApiResponse,
    pub correlation_id: u64,
}

/// A deserialized command from the API, carrying the command name and raw JSON params.
///
/// Domain observers that want to be API-accessible should observe `On<ApiCommandEvent>`
/// and parse the params themselves. This avoids the complexity of dynamic event triggering.
#[derive(Event, Debug, Clone)]
pub struct ApiCommandEvent {
    /// The command type name (e.g. "DriveRover").
    pub command: String,
    /// Raw JSON params.
    pub params: serde_json::Value,
}

/// System counter for generating unique IDs.
#[derive(Resource, Default)]
pub struct ApiIdCounter { next: u64 }
impl ApiIdCounter { pub fn next(&mut self) -> u64 { let id = self.next; self.next += 1; id } }

/// Observer that processes API requests and produces responses.
pub fn api_request_observer(
    trigger: On<ApiRequestEvent>,
    mut commands: Commands,
    mut id_counter: ResMut<ApiIdCounter>,
    registry: Res<ApiEntityRegistry>,
    type_registry: Res<AppTypeRegistry>,
    q_meta: Query<(Option<&Name>, Option<&lunco_core::RoverVessel>, Option<&lunco_core::CelestialBody>)>,
) {
    let req = trigger.event();
    eprintln!("[lunco-api] Processing request: {:?}", req.request);
    let correlation_id = req.correlation_id;

    let response = {
        let type_reg = type_registry.read();
        execute_request(&req.request, &mut commands, &mut id_counter, &registry, &type_reg, &q_meta)
    };

    commands.trigger(ApiResponseEvent { response, correlation_id });
}

/// Execute a single API request against the ECS world.
fn execute_request(
    request: &ApiRequest,
    commands: &mut Commands,
    id_counter: &mut ApiIdCounter,
    registry: &ApiEntityRegistry,
    type_registry: &TypeRegistry,
    q_meta: &Query<(Option<&Name>, Option<&lunco_core::RoverVessel>, Option<&lunco_core::CelestialBody>)>,
) -> ApiResponse {
    match request {
        ApiRequest::ExecuteCommand { command, params } => {
            // Validate command exists in type registry (Event + Reflect types)
            let exists = type_registry.iter().any(|reg| {
                let info = reg.type_info();
                let short_name = info.type_path_table().short_path();
                // Check if type is a struct with matching name (all #[Command] types are registered)
                matches!(info, bevy::reflect::TypeInfo::Struct(_)) && short_name == command.as_str()
            });

            if !exists {
                return ApiResponse::error(ApiErrorCode::CommandNotFound, format!("Command '{}' not found", command));
            }

            // Trigger as ApiCommandEvent — domain observers parse params
            let command_id = id_counter.next();
            commands.trigger(ApiCommandEvent {
                command: command.clone(),
                params: params.clone(),
            });

            ApiResponse::command_accepted(command_id)
        }
        ApiRequest::QueryEntity { id } => {
            match registry.resolve(id) {
                Some(e) => {
                    let (name, rover, body) = q_meta.get(e).unwrap_or((None, None, None));
                    let kind = if rover.is_some() { "rover" } else if body.is_some() { "planet" } else { "unknown" };
                    ApiResponse::ok(serde_json::json!({
                        "entity": e.index().to_string(),
                        "api_id": id.to_string(),
                        "name": name.map(|n| n.as_str()).unwrap_or(""),
                        "type": kind,
                    }))
                },
                None => ApiResponse::error(ApiErrorCode::EntityNotFound, format!("Entity {} not found", id)),
            }
        }
        ApiRequest::ListEntities => {
            let entities: Vec<serde_json::Value> = registry.entities()
                .map(|(api_id, entity)| {
                    let (name, rover, body) = q_meta.get(entity).unwrap_or((None, None, None));
                    let kind = if rover.is_some() { "rover" } else if body.is_some() { "planet" } else { "unknown" };
                    serde_json::json!({
                        "api_id": api_id.to_string(),
                        "entity_index": entity.index().to_string(),
                        "name": name.map(|n| n.as_str()).unwrap_or(""),
                        "type": kind,
                    })
                })
                .collect();
            eprintln!("[lunco-api] Listing {} entities", entities.len());
            ApiResponse::ok(serde_json::json!({ "entities": entities, "count": entities.len() }))
        }
        ApiRequest::DiscoverSchema => {
            let cmds = discover_commands(type_registry);
            ApiResponse::ok(serde_json::to_value(&ApiSchema { commands: cmds }).unwrap_or_default())
        }
        ApiRequest::SubscribeTelemetry { filter: _ } => {
            ApiResponse::ok(serde_json::json!({ "message": "Subscription created" }))
        }
    }
}

/// Discover all typed commands from the type registry.
fn discover_commands(type_registry: &TypeRegistry) -> Vec<CommandSchema> {
    eprintln!("[lunco-api] discover_commands: scanning type registry");

    // First, list all Struct types to debug
    let struct_types: Vec<_> = type_registry.iter()
        .filter_map(|reg| {
            if matches!(reg.type_info(), bevy::reflect::TypeInfo::Struct(_)) {
                Some(reg.type_info().type_path_table().short_path().to_string())
            } else {
                None
            }
        })
        .collect();
    eprintln!("[lunco-api] Found {} Struct types: {:?}", struct_types.len(), struct_types);

    let commands: Vec<CommandSchema> = type_registry.iter()
        .filter_map(|reg| {
            let info = reg.type_info();
            if !matches!(info, bevy::reflect::TypeInfo::Struct(_)) { return None; }
            let struct_info = match info {
                bevy::reflect::TypeInfo::Struct(s) => s,
                _ => return None,
            };
            let short_name = info.type_path_table().short_path().to_string();
            if short_name.starts_with("Api") || short_name.starts_with("Telemetry") { return None; }
            let fields: Vec<FieldSchema> = struct_info.iter().map(|f: &NamedField| FieldSchema {
                name: f.name().to_string(),
                type_name: f.type_path().to_string(),
            }).collect();
            Some(CommandSchema { name: short_name, fields })
        })
        .collect();

    eprintln!("[lunco-api] Discovered {} commands", commands.len());
    for cmd in &commands {
        eprintln!("[lunco-api]   - {} ({} fields)", cmd.name, cmd.fields.len());
    }

    commands
}

/// Plugin that registers the API executor observer.
pub struct ApiExecutorPlugin;

impl Plugin for ApiExecutorPlugin {
    fn build(&self, app: &mut App) {
        eprintln!("[lunco-api] Registering ApiExecutorPlugin");
        app.init_resource::<ApiIdCounter>()
            .add_observer(api_request_observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_id_generation() {
        let mut counter = ApiIdCounter::default();
        assert_eq!(counter.next(), 0);
        assert_eq!(counter.next(), 1);
        assert_eq!(counter.next(), 2);
    }
}
