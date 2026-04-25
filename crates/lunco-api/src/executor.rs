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
use bevy::reflect::TypeRegistry;
use std::io::Cursor;
use crate::{
    registry::ApiEntityRegistry,
    queries::ApiQueryRegistry,
    schema::{ApiErrorCode, ApiRequest, ApiResponse, ApiSchema},
    discovery::discover_commands,
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
/// This is used internally by the API layer to bridge requests into simulation events.
#[derive(Event, Debug, Clone, Reflect)]
pub struct ApiCommandEvent {
    pub command: String,
    #[reflect(ignore)]
    pub params: serde_json::Value,
}

/// Request to execute a script snippet.
#[derive(Event, Debug, Clone, Reflect)]
pub struct ScriptRequestEvent {
    pub language: String,
    pub code: String,
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
    query_registry: Res<ApiQueryRegistry>,
    type_registry: Res<AppTypeRegistry>,
    q_meta: Query<(Option<&Name>, Option<&lunco_core::RoverVessel>, Option<&lunco_core::CelestialBody>)>,
    q_cameras: Query<Entity, With<Camera3d>>,
) {
    let req = trigger.event();
    let correlation_id = req.correlation_id;

    let maybe_response = {
        let type_reg = type_registry.read();
        execute_request(&req.request, &mut commands, &mut id_counter, &registry, &query_registry, &type_reg, &q_meta, &q_cameras, correlation_id)
    };

    // None means the response is deferred (e.g. waiting for ScreenshotCaptured).
    if let Some(response) = maybe_response {
        commands.trigger(ApiResponseEvent { response, correlation_id });
    }
}

/// Dynamic dispatcher: converts generic [ApiCommandEvent] into pure simulation events.
///
/// This system listens for all API-triggered commands and uses reflection to
/// fire the specific [Event] types (e.g. `DriveRover`).
pub fn api_command_dispatcher(
    trigger: On<ApiCommandEvent>,
    mut commands: Commands,
    type_registry: Res<AppTypeRegistry>,
    registry: Res<ApiEntityRegistry>,
) {
    let event = trigger.event();
    let type_reg = type_registry.read();

    // 1. Find type registration by short name (e.g. "DriveRover")
    let Some(registration) = type_reg.get_with_short_type_path(&event.command) else {
        warn!("[lunco-api] Command '{}' not found in type registry", event.command);
        return;
    };

    // 2. Resolve IDs: recursively find fields that should be Entities and look them up in the registry
    let mut resolved_params = event.params.clone();
    resolve_ids_in_json(&mut resolved_params, &registry);

    // 3. Deserialize JSON into reflected struct
    let reflect_deserializer = bevy::reflect::serde::TypedReflectDeserializer::new(registration, &type_reg);
    
    use serde::de::DeserializeSeed;
    match reflect_deserializer.deserialize(resolved_params.clone()) {
        Ok(_reflected) => {
            // 4. Trigger the event dynamically via commands.queue to access World
            let cmd_name = event.command.clone();
            
            commands.queue(move |world: &mut World| {
                let registry = world.resource::<AppTypeRegistry>().clone();
                let type_reg = registry.read();
                
                let Some(registration) = type_reg.get_with_short_type_path(&cmd_name) else { return };
                let Some(reflect_event) = registration.data::<bevy::ecs::reflect::ReflectEvent>() else { return };
                
                // Re-deserialize inside the world queue where we have access to everything
                let reflect_deserializer = bevy::reflect::serde::TypedReflectDeserializer::new(registration, &type_reg);
                if let Ok(reflected) = reflect_deserializer.deserialize(resolved_params) {
                    reflect_event.trigger(world, reflected.as_ref(), &type_reg);
                }
            });
        },
        Err(e) => {
            warn!("[lunco-api] Deserialization error for '{}': {}", event.command, e);
        }
    }
}

/// Recursively finds fields that look like stable IDs and resolves them to Bevy Entity indices.
///
/// Note: This is a heuristic. We assume fields named 'target', 'entity', or 'body'
/// that contain a large number or numeric string are meant to be GlobalEntityIds.
fn resolve_ids_in_json(value: &mut serde_json::Value, registry: &ApiEntityRegistry) {
    use lunco_core::GlobalEntityId;

    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                // Heuristic: fields that typically store Entity IDs
                if k == "target" || k == "entity" || k == "body" || k == "parent" {
                    if let Some(id_u64) = v.as_u64() {
                        if let Some(entity) = registry.resolve(&GlobalEntityId(id_u64)) {
                            // Replace with raw Bevy index for reflection deserializer
                            *v = serde_json::json!(entity.index().index());
                        }
                    } else if let Some(id_str) = v.as_str() {
                        if let Ok(id_u64) = id_str.parse::<u64>() {
                            if let Some(entity) = registry.resolve(&GlobalEntityId(id_u64)) {
                                *v = serde_json::json!(entity.index().index());
                            }
                        }
                    }
                } else {
                    resolve_ids_in_json(v, registry);
                }
            }
        }
        serde_json::Value::Array(list) => {
            for v in list.iter_mut() {
                resolve_ids_in_json(v, registry);
            }
        }
        _ => {}
    }
}

/// Execute a single API request against the ECS world.
/// Returns `None` when the response is deferred (e.g. waiting for ScreenshotCaptured).
fn execute_request(
    request: &ApiRequest,
    commands: &mut Commands,
    id_counter: &mut ApiIdCounter,
    registry: &ApiEntityRegistry,
    query_registry: &ApiQueryRegistry,
    type_registry: &TypeRegistry,
    q_meta: &Query<(Option<&Name>, Option<&lunco_core::RoverVessel>, Option<&lunco_core::CelestialBody>)>,
    _q_cameras: &Query<Entity, With<Camera3d>>,
    correlation_id: u64,
) -> Option<ApiResponse> {
    match request {
        ApiRequest::ExecuteCommand { command, params } => {
            // Special-case: CaptureScreenshot — response depends on save_to_file param.
            if command == "CaptureScreenshot" {
                let save_to_file = params
                    .get("save_to_file")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Spawn Bevy's screenshot capture entity directly here
                // instead of relying on a domain-side observer. Earlier
                // the executor only triggered `ApiCommandEvent` and
                // hoped a `CaptureScreenshot` observer downstream would
                // call `Screenshot::primary_window()`. That observer
                // only ships in `lunco-avatar`; binaries that don't pull
                // it in (modelica_workbench, hello_workbench) silently
                // never produced a screenshot — curl would just hang.
                // Doing the spawn here keeps the screenshot path
                // self-contained in lunco-api.
                use bevy::render::view::screenshot::Screenshot;
                if save_to_file {
                    let path = format!("screenshot_{}.png",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs());
                    commands.insert_resource(PendingScreenshotRequest {
                        correlation_id: None,   // response already sent
                        save_path: Some(path.clone()),
                    });
                    commands.spawn(Screenshot::primary_window());
                    return Some(ApiResponse::ok(serde_json::json!({ "path": path })));
                } else {
                    // Raw-PNG mode: defer response until ScreenshotCaptured fires.
                    commands.insert_resource(PendingScreenshotRequest {
                        correlation_id: Some(correlation_id),
                        save_path: None,
                    });
                    commands.spawn(Screenshot::primary_window());
                    return None; // response deferred
                }
            }

            // Query registry — endpoints that *return data* (vs typed
            // Reflect commands which are fire-and-forget). Domain crates
            // register providers via `ApiQueryRegistry::register`. The
            // provider runs deferred via `commands.queue` so it can take
            // `&mut World`; the response is fired back via
            // `ApiResponseEvent` when the queue flushes.
            if let Some(provider) = query_registry.get(command) {
                let params = params.clone();
                commands.queue(move |world: &mut World| {
                    let response = provider.execute(world, &params);
                    world.commands().trigger(ApiResponseEvent {
                        response,
                        correlation_id,
                    });
                });
                return None; // response deferred
            }

            // Validate command exists and has ReflectEvent
            let registration = type_registry.get_with_short_type_path(command);
            let has_reflect_event = registration.map(|r| r.data::<bevy::ecs::reflect::ReflectEvent>().is_some()).unwrap_or(false);

            if !has_reflect_event {
                return Some(ApiResponse::error(ApiErrorCode::CommandNotFound, format!("Command '{}' not found or not API-accessible", command)));
            }

            // Trigger as ApiCommandEvent — handled by api_command_dispatcher
            let command_id = id_counter.next();
            commands.trigger(ApiCommandEvent {
                command: command.clone(),
                params: params.clone(),
            });

            Some(ApiResponse::command_accepted(command_id))
        }
        ApiRequest::ExecuteScript { language, code } => {
            commands.trigger(ScriptRequestEvent {
                language: language.clone(),
                code: code.clone(),
            });
            Some(ApiResponse::ok(serde_json::json!({ "status": "sent_to_engine" })))
        }
        ApiRequest::QueryEntity { id } => {
            Some(match registry.resolve(id) {
                Some(e) => {
                    let (name, rover, body) = q_meta.get(e).unwrap_or((None, None, None));
                    let kind = if rover.is_some() { "rover" } else if body.is_some() { "planet" } else { "unknown" };
                    ApiResponse::ok(serde_json::json!({
                        "api_id": id,
                        "name": name.map(|n| n.as_str()).unwrap_or(""),
                        "type": kind,
                    }))
                },
                None => ApiResponse::error(ApiErrorCode::EntityNotFound, format!("Entity {} not found", id)),
            })
        }
        ApiRequest::ListEntities => {
            let entities: Vec<serde_json::Value> = registry.entities()
                .into_iter()
                .map(|(api_id, entity)| {
                    let (name, rover, body) = q_meta.get(entity).unwrap_or((None, None, None));
                    let kind = if rover.is_some() { "rover" } else if body.is_some() { "planet" } else { "unknown" };
                    serde_json::json!({
                        "api_id": api_id,
                        "name": name.map(|n| n.as_str()).unwrap_or(""),
                        "type": kind,
                    })
                })
                .collect();
            Some(ApiResponse::ok(serde_json::json!({ "entities": entities, "count": entities.len() })))
        }
        ApiRequest::DiscoverSchema => {
            let cmds = discover_commands(type_registry);
            Some(ApiResponse::ok(serde_json::to_value(&ApiSchema { commands: cmds }).unwrap_or_default()))
        }
        ApiRequest::SubscribeTelemetry { filter: _ } => {
            Some(ApiResponse::ok(serde_json::json!({ "message": "Subscription created" })))
        }
    }
}

use bevy::render::view::screenshot::ScreenshotCaptured;

/// Pending screenshot request — set before the screenshot is triggered so the
/// ScreenshotCaptured observer knows what to do with the image.
#[derive(Resource, Default)]
pub struct PendingScreenshotRequest {
    /// correlation_id of the HTTP request waiting for the screenshot (raw-PNG mode).
    /// None when save_to_file is true (response is already sent).
    pub correlation_id: Option<u64>,
    /// When Some, save to this path and do not return bytes to the caller.
    pub save_path: Option<String>,
}

/// Plugin that registers the API executor observer.
pub struct ApiExecutorPlugin;

impl Plugin for ApiExecutorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiIdCounter>()
            .init_resource::<PendingScreenshotRequest>()
            .add_observer(api_request_observer)
            .add_observer(api_command_dispatcher)
            .add_observer(save_screenshot);
    }
}

fn save_screenshot(
    trigger: On<ScreenshotCaptured>,
    mut pending: ResMut<PendingScreenshotRequest>,
    mut commands: Commands,
) {
    let event = trigger.event();
    let correlation_id = pending.correlation_id.take();
    let save_path = pending.save_path.take();

    let Ok(dyn_img) = event.image.clone().try_into_dynamic() else {
        error!("[lunco-api] Screenshot: failed to convert image");
        return;
    };

    if let Some(path) = save_path {
        // save_to_file mode — write to disk, response already sent
        if let Err(e) = dyn_img.save(&path) {
            error!("[lunco-api] Failed to save screenshot to '{}': {}", path, e);
        }
    } else if let Some(cid) = correlation_id {
        // raw-PNG mode — encode and send back via the deferred HTTP response
        let mut png_bytes: Vec<u8> = Vec::new();
        if let Ok(()) = dyn_img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png) {
            commands.trigger(ApiResponseEvent {
                response: crate::schema::ApiResponse::Screenshot { png_bytes },
                correlation_id: cid,
            });
        } else {
            error!("[lunco-api] Failed to encode screenshot as PNG");
        }
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
