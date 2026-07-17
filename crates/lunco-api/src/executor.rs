//! API request executor — processes `ApiRequest` and produces `ApiResponse`.
//!
//! Uses Bevy's `AppTypeRegistry` to discover all typed commands (`Event + Reflect`)
//! for schema discovery. Commands are triggered as `ApiCommandEvent` which carries
//! the command name and JSON params.
//!
//! Domain observers can observe both:
//! - `On<SetPorts>` for internal triggers
//! - `On<ApiCommandEvent>` for API triggers (downcast the command)

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use crate::{
    registry::ApiEntityRegistry,
    queries::{ApiQueryRegistry, ApiVisibility},
    schema::{ApiErrorCode, ApiRequest, ApiResponse, ApiSchema},
    discovery::discover_commands,
    subscription::TelemetrySubscriptions,
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
    /// Request id minted at ingress; the dispatcher sets `ActiveCommandId`
    /// to this around the observer trigger so a result-reporting handler
    /// records its outcome under the id the caller can poll.
    pub id: u64,
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
    visibility: Res<ApiVisibility>,
    type_registry: Res<AppTypeRegistry>,
    cmd_results: Res<lunco_core::CommandResults>,
    mut subscriptions: ResMut<TelemetrySubscriptions>,
    q_meta: Query<(Option<&Name>, Has<lunco_fsw::FlightSoftware>, Option<&lunco_core::CelestialBody>)>,
    // Which commands answer later, on the correlation id. Populated by whichever crate owns
    // them (`register_deferred_command`), never by name here.
    deferred_commands: Option<Res<DeferredCommands>>,
) {
    let req = trigger.event();
    let correlation_id = req.correlation_id;

    let maybe_response = {
        let type_reg = type_registry.read();
        execute_request(&req.request, &mut commands, &mut id_counter, &registry, &query_registry, &visibility, &type_reg, &cmd_results, &mut subscriptions, &q_meta, deferred_commands.as_deref(), correlation_id)
    };

    // None means the response is deferred — a deferred command or query provider will
    // answer on this correlation id later. See `DeferredCommands`.
    if let Some(response) = maybe_response {
        commands.trigger(ApiResponseEvent { response, correlation_id });
    }
}

/// Project every dispatched command onto the shared [`TelemetryEvent`] bus as a
/// `cmd:<CommandName>` event, so a rhai scenario (or a tutorial's declarative
/// `mission(me)`) can react to *any* UI/API action with `wait_for("cmd:SpawnEntity")`
/// or an objective's `requires_event: "cmd:PossessVessel"` — no per-command glue.
///
/// This is the observer-triggered analog of the `project_events` registrar
/// (which reads buffered *messages*); `ApiCommandEvent` is fired via
/// `commands.trigger`, so it needs a dedicated observer rather than a
/// `MessageReader`. Fires for the command as *requested* (before reflection/authz
/// resolution) — the signal is "the user asked for X", which is what a tutorial
/// step keys off. `source: 0` (no single emitter); payload = the command name.
pub fn project_command_events(trigger: On<ApiCommandEvent>, mut commands: Commands) {
    let name = &trigger.event().command;
    commands.trigger(lunco_core::TelemetryEvent {
        name: format!("cmd:{name}"),
        source: 0,
        severity: lunco_core::Severity::Info,
        data: lunco_core::TelemetryValue::String(name.clone()),
        timestamp: 0.0,
    });
}

/// Can `params` actually become this command? `Ok(())` if it deserializes AND
/// is constructible; `Err(message)` otherwise.
///
/// The two ways a command dies silently, checked here so a caller learns about
/// them from its own HTTP response instead of from a log line it can't see:
///
/// 1. **Deserialize failure** — a misspelled/mistyped field.
/// 2. **Not constructible** — deserializes as a partial `dyn Reflect`, but
///    `FromReflect` can't build the concrete type (a field with no `Default`
///    was omitted). `ReflectEvent::trigger` PANICS on that, so the dispatcher
///    guards it and drops the command.
///
/// This mirrors exactly what `api_command_dispatcher` does, deliberately: the
/// dispatcher stays authoritative (it also serves in-process triggers), and this
/// is the synchronous gate in front of it.
pub fn validate_command_params(
    command: &str,
    params: &serde_json::Value,
    registration: &bevy::reflect::TypeRegistration,
    type_reg: &TypeRegistry,
    entities: &ApiEntityRegistry,
) -> Result<(), String> {
    use serde::de::DeserializeSeed;

    let mut resolved = params.clone();
    // Unit-struct commands (`Exit`, `Ping`) arrive with no `params` at all.
    if resolved.is_null() {
        resolved = serde_json::Value::Object(serde_json::Map::new());
    }
    resolve_command_ids(&mut resolved, registration.type_id(), type_reg, entities);

    let de = bevy::reflect::serde::TypedReflectDeserializer::new(registration, type_reg);
    let reflected = de
        .deserialize(resolved)
        .map_err(|e| format!("Command '{command}': invalid params: {e}"))?;

    let constructible = registration
        .data::<bevy::reflect::ReflectFromReflect>()
        .map(|fr| fr.from_reflect(reflected.as_ref()).is_some())
        .unwrap_or(true);
    if !constructible {
        return Err(format!(
            "Command '{command}': params are not constructible into the command type (a required field is missing or invalid)"
        ));
    }
    Ok(())
}

/// Dynamic dispatcher: converts generic [ApiCommandEvent] into pure simulation events.
///
/// This system listens for all API-triggered commands and uses reflection to
/// fire the specific [Event] types (e.g. `SetPorts`).
pub fn api_command_dispatcher(
    trigger: On<ApiCommandEvent>,
    mut commands: Commands,
    type_registry: Res<AppTypeRegistry>,
    registry: Res<ApiEntityRegistry>,
) {
    let event = trigger.event();
    let type_reg = type_registry.read();

    // 1. Find type registration by short name (e.g. "SetPorts")
    let Some(registration) = type_reg.get_with_short_type_path(&event.command) else {
        warn!("[lunco-api] Command '{}' not found in type registry", event.command);
        return;
    };

    // 2. Resolve IDs: recursively find fields that should be Entities and look them up in the registry
    let mut resolved_params = event.params.clone();
    // Coerce absent/null params to an empty object. Unit-struct commands
    // (e.g. `Exit`, `Ping`) and commands whose fields are all defaulted are
    // sent as `{"command":"X"}` with no `params`; TypedReflectDeserializer
    // rejects a bare `null` ("invalid type: null, expected reflected struct
    // value") and the command silently never fires (the HTTP layer still
    // returns a command_id, so it *looks* accepted). An empty map deserializes
    // fine — missing fields fall back to their reflect/serde defaults.
    if resolved_params.is_null() {
        resolved_params = serde_json::Value::Object(serde_json::Map::new());
    }
    resolve_command_ids(&mut resolved_params, registration.type_id(), &type_reg, &registry);

    // 3. Deserialize JSON into reflected struct
    let reflect_deserializer = bevy::reflect::serde::TypedReflectDeserializer::new(registration, &type_reg);
    
    use serde::de::DeserializeSeed;
    match reflect_deserializer.deserialize(resolved_params.clone()) {
        Ok(_reflected) => {
            // 4. Trigger the event dynamically via commands.queue to access World
            let cmd_name = event.command.clone();
            let cmd_id = event.id;

            commands.queue(move |world: &mut World| {
                let registry = world.resource::<AppTypeRegistry>().clone();
                let type_reg = registry.read();

                let Some(registration) = type_reg.get_with_short_type_path(&cmd_name) else { return };
                let Some(reflect_event) = registration.data::<bevy::ecs::reflect::ReflectEvent>() else { return };

                // Re-deserialize inside the world queue where we have access to everything
                let reflect_deserializer = bevy::reflect::serde::TypedReflectDeserializer::new(registration, &type_reg);
                let reflected = match reflect_deserializer.deserialize(resolved_params) {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = format!("command '{cmd_name}': invalid params: {e}");
                        warn!("[lunco-api] {msg}; dropped");
                        world.resource_mut::<lunco_core::CommandResults>().insert(
                            cmd_id,
                            lunco_core::CommandOutcome::Rejected(lunco_core::Reject::InvalidOp(msg)),
                        );
                        return;
                    }
                };
                {
                    // Guard against a panic in `ReflectEvent::trigger`: it builds
                    // the concrete type via `FromReflect`, falling back to
                    // `Default`/`FromWorld` and PANICKING when none apply (e.g. a
                    // struct still missing a no-`Default` field). Verify the value
                    // is fully constructible first so a malformed command logs and
                    // is dropped instead of killing the process. (Types without a
                    // registered `ReflectFromReflect` keep the legacy path.)
                    let constructible = registration
                        .data::<bevy::reflect::ReflectFromReflect>()
                        .map(|fr| fr.from_reflect(reflected.as_ref()).is_some())
                        .unwrap_or(true);
                    if !constructible {
                        let msg = format!(
                            "command '{cmd_name}' not constructible from params (missing/invalid fields)"
                        );
                        warn!("[lunco-api] {msg}; dropped");
                        // Record a TERMINAL outcome. A dropped command used to
                        // leave `CommandResults` empty, so `QueryCommandResult`
                        // answered `outcome: null` — the same answer a healthy
                        // fire-and-forget command gives. A poller could not tell
                        // "never ran" from "ran fine".
                        world.resource_mut::<lunco_core::CommandResults>().insert(
                            cmd_id,
                            lunco_core::CommandOutcome::Rejected(lunco_core::Reject::InvalidOp(msg)),
                        );
                        return;
                    }
                    // Scope the active request id around the trigger so a
                    // result-reporting `#[on_command]` wrapper records its
                    // outcome under this id. Observers run synchronously
                    // inside `trigger`, so set-before / clear-after is sound.
                    world.resource_mut::<lunco_core::ActiveCommandId>().set(Some(cmd_id));
                    reflect_event.trigger(world, reflected.as_ref(), &type_reg);
                    world.resource_mut::<lunco_core::ActiveCommandId>().set(None);
                }
            });
        },
        Err(e) => {
            // Terminal, and RECORDED — see the `!constructible` branch above.
            // An external caller sees this synchronously as a 422 from
            // `execute_request`'s pre-flight validation; an in-process trigger
            // learns about it by polling `QueryCommandResult`.
            let msg = format!("command '{}': invalid params: {e}", event.command);
            warn!("[lunco-api] {msg}; dropped");
            let cmd_id = event.id;
            commands.queue(move |world: &mut World| {
                world.resource_mut::<lunco_core::CommandResults>().insert(
                    cmd_id,
                    lunco_core::CommandOutcome::Rejected(lunco_core::Reject::InvalidOp(msg)),
                );
            });
        }
    }
}

// ── Entity-id conversion (schema-driven) ──────────────────────────────────
//
// Replaces an older heuristic that rewrote fields by NAME
// (`target`/`entity`/`body`/`parent`/`avatar`). We now walk the command's
// reflect `TypeInfo` alongside its JSON and convert every leaf whose declared
// type is `Entity` — name-independent, so renamed/new entity fields,
// `Vec<Entity>`, `Option<Entity>`, and nested structs/enums all convert, while
// a same-named non-entity field (`parent: String`, `target: f64`) is left
// alone. See `crates/lunco-networking/PH2_ID_CODEC.md`.

/// Incoming: wire `GlobalEntityId` (u64 or numeric string) → local
/// `Entity::to_bits()` (generation-preserving), in place, before deserialize.
/// `type_id` is the command struct's type id (`registration.type_id()`).
pub fn resolve_command_ids(
    value: &mut serde_json::Value,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
    entities: &ApiEntityRegistry,
) {
    convert_node(value, type_id, reg, IdDir::Resolve, entities, false);
}

/// Outgoing/capture: local `Entity::to_bits()` → wire `GlobalEntityId` u64. A
/// field tagged `#[sync_local]` (the `SyncLocal` reflect attribute) is replaced
/// with `Entity::PLACEHOLDER` instead, so a peer's local-only references (camera
/// avatar) never leak onto the wire.
pub fn globalize_command_ids(
    value: &mut serde_json::Value,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
    entities: &ApiEntityRegistry,
) {
    convert_node(value, type_id, reg, IdDir::Globalize, entities, false);
}

/// The global entity id a networked command authorizes against: the u64 value
/// of the top-level field tagged `#[authz_target]` (`AuthzTarget` reflect
/// attribute) in the command's schema. Runs on RAW wire params (global gids,
/// pre-resolve); `None` when the command has no such field (the host then
/// treats it as target-less). Replaces a hardcoded `params["target"]` lookup —
/// authorization no longer depends on a field being literally named `target`.
pub fn authz_target_gid(
    params: &serde_json::Value,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
) -> Option<u64> {
    use bevy::reflect::TypeInfo;
    let TypeInfo::Struct(s) = reg.get_type_info(type_id)? else {
        return None;
    };
    for i in 0..s.field_len() {
        let f = s.field_at(i)?;
        if f.has_attribute::<lunco_core::AuthzTarget>() {
            return params.get(f.name()).and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|x| x.parse::<u64>().ok()))
            });
        }
    }
    None
}

#[derive(Clone, Copy)]
enum IdDir {
    Resolve,
    Globalize,
}

/// Recursively convert `Entity` leaves in `value`, using `type_id`'s reflect
/// schema to find them. `sync_local` is set when the parent struct field
/// carried the `SyncLocal` attribute (only acted on for a direct `Entity` leaf
/// on the `Globalize` path).
fn convert_node(
    value: &mut serde_json::Value,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
    dir: IdDir,
    entities: &ApiEntityRegistry,
    sync_local: bool,
) {
    use bevy::reflect::{enums::VariantInfo, TypeInfo};
    use std::any::TypeId;

    // Leaf: the declared type IS Entity → convert the scalar.
    if type_id == TypeId::of::<Entity>() {
        convert_leaf(value, dir, entities, sync_local);
        return;
    }

    // Need the field type's schema to recurse. Unregistered (primitive like
    // f64/String, or simply not in the registry) → cannot contain an Entity
    // we can locate; leave it untouched.
    let Some(info) = reg.get_type_info(type_id) else { return };

    match info {
        TypeInfo::Struct(s) => {
            let Some(map) = value.as_object_mut() else { return };
            for i in 0..s.field_len() {
                let Some(f) = s.field_at(i) else { continue };
                if let Some(child) = map.get_mut(f.name()) {
                    let wl = f.has_attribute::<lunco_core::SyncLocal>();
                    convert_node(child, f.type_id(), reg, dir, entities, wl);
                }
            }
        }
        TypeInfo::TupleStruct(ts) => match value {
            serde_json::Value::Array(arr) => {
                for i in 0..ts.field_len() {
                    if let (Some(f), Some(child)) = (ts.field_at(i), arr.get_mut(i)) {
                        convert_node(child, f.type_id(), reg, dir, entities, false);
                    }
                }
            }
            // A 1-field tuple struct serializes as the bare inner value.
            other if ts.field_len() == 1 => {
                if let Some(f) = ts.field_at(0) {
                    convert_node(other, f.type_id(), reg, dir, entities, false);
                }
            }
            _ => {}
        },
        TypeInfo::List(l) => {
            if let Some(arr) = value.as_array_mut() {
                let item = l.item_ty().id();
                for child in arr.iter_mut() {
                    convert_node(child, item, reg, dir, entities, false);
                }
            }
        }
        TypeInfo::Array(a) => {
            if let Some(arr) = value.as_array_mut() {
                let item = a.item_ty().id();
                for child in arr.iter_mut() {
                    convert_node(child, item, reg, dir, entities, false);
                }
            }
        }
        TypeInfo::Map(m) => {
            // bevy reflect serializes maps as a JSON array of [k, v] pairs
            // (some paths emit an object); convert values only.
            let vty = m.value_ty().id();
            if let Some(arr) = value.as_array_mut() {
                for pair in arr.iter_mut() {
                    if let Some(child) = pair.as_array_mut().and_then(|p| p.get_mut(1)) {
                        convert_node(child, vty, reg, dir, entities, false);
                    }
                }
            } else if let Some(obj) = value.as_object_mut() {
                for (_, child) in obj.iter_mut() {
                    convert_node(child, vty, reg, dir, entities, false);
                }
            }
        }
        TypeInfo::Enum(e) => {
            // unit variant → bare string (no payload); data variant →
            // single-key object `{"Variant": payload}`.
            let serde_json::Value::Object(map) = value else { return };
            let Some((vname, payload)) = map.iter_mut().next() else { return };
            let Some(var) = e.variant(vname) else { return };
            match var {
                VariantInfo::Struct(sv) => {
                    if let Some(pobj) = payload.as_object_mut() {
                        for i in 0..sv.field_len() {
                            if let Some(f) = sv.field_at(i) {
                                if let Some(child) = pobj.get_mut(f.name()) {
                                    convert_node(child, f.type_id(), reg, dir, entities, false);
                                }
                            }
                        }
                    }
                }
                VariantInfo::Tuple(tv) if tv.field_len() == 1 => {
                    if let Some(f) = tv.field_at(0) {
                        // Propagate `sync_local` into the single-field payload so
                        // an `Option<Entity>` (the `Some` variant) tagged
                        // `#[sync_local]` — e.g. `PossessVessel::avatar` — still
                        // nulls its inner local bits on the wire.
                        convert_node(payload, f.type_id(), reg, dir, entities, sync_local);
                    }
                }
                VariantInfo::Tuple(tv) => {
                    if let Some(arr) = payload.as_array_mut() {
                        for i in 0..tv.field_len() {
                            if let (Some(f), Some(child)) = (tv.field_at(i), arr.get_mut(i)) {
                                convert_node(child, f.type_id(), reg, dir, entities, false);
                            }
                        }
                    }
                }
                VariantInfo::Unit(_) => {}
            }
        }
        _ => {} // Tuple, Set, Opaque — no Entity leaves in commands.
    }
}

fn convert_leaf(
    value: &mut serde_json::Value,
    dir: IdDir,
    entities: &ApiEntityRegistry,
    sync_local: bool,
) {
    use lunco_core::GlobalEntityId;
    match dir {
        IdDir::Resolve => {
            // wire gid (u64 or numeric string) → local Entity::to_bits().
            let gid = value
                .as_u64()
                .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()));
            if let Some(g) = gid {
                if let Some(entity) = entities.resolve(&GlobalEntityId::from_raw(g)) {
                    // to_bits() keeps index+generation so the deserialized
                    // Entity matches a live query (index() alone would not).
                    *value = serde_json::json!(entity.to_bits());
                }
            }
        }
        IdDir::Globalize => {
            // Local-only field (e.g. avatar): never put local bits on the wire.
            if sync_local {
                *value = serde_json::json!(Entity::PLACEHOLDER.to_bits());
                return;
            }
            if let Some(bits) = value.as_u64() {
                if let Some(entity) = Entity::try_from_bits(bits) {
                    if let Some(gid) = entities.api_id_for(entity) {
                        *value = serde_json::json!(gid.get());
                    }
                }
            }
        }
    }
}

/// Execute a single API request against the ECS world.
/// Returns `None` when the response is deferred — see [`DeferredCommands`].
fn execute_request(
    request: &ApiRequest,
    commands: &mut Commands,
    id_counter: &mut ApiIdCounter,
    registry: &ApiEntityRegistry,
    query_registry: &ApiQueryRegistry,
    visibility: &ApiVisibility,
    type_registry: &TypeRegistry,
    cmd_results: &lunco_core::CommandResults,
    subscriptions: &mut TelemetrySubscriptions,
    q_meta: &Query<(Option<&Name>, Has<lunco_fsw::FlightSoftware>, Option<&lunco_core::CelestialBody>)>,
    deferred_commands: Option<&DeferredCommands>,
    correlation_id: u64,
) -> Option<ApiResponse> {
    match request {
        ApiRequest::ExecuteCommand { command, params } => {
            // A DEFERRED command answers on this request's correlation id, later. The
            // executor does not know (and must not know) which commands those are — a crate
            // that owns one calls `register_deferred_command::<T>()`. See `DeferredCommands`.
            //
            // Note the ordering: we only defer for a command that is ALSO registered as a
            // type below. A binary without the owning plugin therefore falls through to the
            // ordinary `CommandNotFound` path instead of deferring into silence and hanging
            // the caller forever.
            if deferred_commands.is_some_and(|d| d.contains(command))
                && type_registry.get_with_short_type_path(command).is_some()
            {
                commands.insert_resource(PendingApiRequest { correlation_id });
                // Arm the watchdog BEFORE dispatching: if the handler forgets to answer (or
                // dies trying), the caller gets a clear error instead of hanging forever.
                commands.queue(move |world: &mut World| {
                    let now = world
                        .get_resource::<Time<bevy::time::Real>>()
                        .map(|t| t.elapsed_secs_f64())
                        .unwrap_or(0.0);
                    if let Some(mut deferred) = world.get_resource_mut::<DeferredRequests>() {
                        let deadline = now + deferred.timeout_secs;
                        deferred.outstanding.insert(correlation_id, deadline);
                    }
                });
                commands.trigger(ApiCommandEvent {
                    command: command.clone(),
                    params: params.clone(),
                    id: id_counter.next(),
                });
                return None; // the handler answers on `correlation_id`
            }

            // Visibility gate — commands marked hidden in `ApiVisibility`
            // are reachable inside the app (GUI, observers, tests) but
            // invisible to external callers. Reject with the same
            // `CommandNotFound` an unknown name produces, so the
            // external surface looks identical to "this command does
            // not exist on this binary."
            if visibility.is_hidden(command) {
                return Some(ApiResponse::error(
                    ApiErrorCode::CommandNotFound,
                    format!("Command '{}' not found or not API-accessible", command),
                ));
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

            // Validate the PARAMS synchronously, here, while the registry is in
            // hand. Previously this returned `command_accepted` immediately and
            // the dispatcher, running later, just `warn!`d and dropped anything
            // that failed to deserialize — so a typo'd param returned 200 OK and
            // `QueryCommandResult` came back `outcome: null`, which is also what
            // a fire-and-forget success looks like. A bad command was
            // INDISTINGUISHABLE from a good one. Now it is a synchronous 422.
            //
            // The dispatcher still re-validates (it must: `ApiCommandEvent` can
            // be triggered in-process too), so this is a gate, not the only
            // check.
            if let Some(registration) = registration {
                if let Err(msg) = validate_command_params(command, params, registration, type_registry, registry) {
                    return Some(ApiResponse::error(ApiErrorCode::DeserializationError, msg));
                }
            }

            // Trigger as ApiCommandEvent — handled by api_command_dispatcher
            let command_id = id_counter.next();
            commands.trigger(ApiCommandEvent {
                command: command.clone(),
                params: params.clone(),
                id: command_id,
            });

            Some(ApiResponse::command_accepted(command_id))
        }
        ApiRequest::ListEntities => {
            let entities: Vec<serde_json::Value> = registry.entities()
                .into_iter()
                .map(|(api_id, entity)| {
                    let (name, is_vehicle, body) = q_meta.get(entity).unwrap_or((None, false, None));
                    let kind = if is_vehicle { "rover" } else if body.is_some() { "planet" } else { "unknown" };
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
            let cmds = discover_commands(type_registry, Some(visibility));
            Some(ApiResponse::ok(serde_json::to_value(&ApiSchema { commands: cmds }).unwrap_or_default()))
        }
        ApiRequest::SubscribeTelemetry { filter } => {
            // Register the subscription so the telemetry observers actually
            // stream matching events (incl. script `emit()`s) back to this
            // client. Previously a no-op that lied "Subscription created".
            let id = subscriptions.subscribe(filter.clone());
            Some(ApiResponse::ok(serde_json::json!({ "subscription_id": id })))
        }
        ApiRequest::UnsubscribeTelemetry { id } => {
            // `unsubscribe` has existed since the beginning with NOTHING able to call
            // it — subscriptions leaked for the life of the process, and a client that
            // reconnected piled up a new one every time.
            subscriptions.unsubscribe(*id);
            Some(ApiResponse::ok(serde_json::json!({ "unsubscribed": id })))
        }
        ApiRequest::QueryCommandResult { id } => {
            // `outcome: null` = STILL PENDING (or an unknown id): the command was
            // accepted and no terminal outcome has been recorded yet. It no
            // longer doubles as "was silently dropped" — invalid params are now
            // rejected synchronously (422) by `execute_request`, and the
            // dispatcher records `Rejected` for anything it drops. A
            // fire-and-forget command whose handler reports nothing also stays
            // `null`; that's the one remaining ambiguity, and it is bounded to
            // handlers that never report.
            let outcome = cmd_results.get(*id);
            Some(ApiResponse::ok(serde_json::json!({
                "id": id,
                "outcome": outcome,
            })))
        }
    }
}

/// **Commands that answer LATER, on the request's correlation id.**
///
/// Most commands are fire-and-report: the executor validates them, dispatches an
/// `ApiCommandEvent`, and answers `command_accepted` immediately. A few cannot — their
/// result only exists after something asynchronous happens (a GPU frame is captured, a bake
/// finishes, a file is written). Those want to put the *actual payload* in the HTTP response
/// rather than make the caller poll `QueryCommandResult`.
///
/// The mechanism already existed for query providers (`return None; // response deferred`,
/// then answer with an [`ApiResponseEvent`] carrying the same `correlation_id`). This makes
/// it available to COMMANDS too, and — crucially — **without `lunco-api` knowing what any of
/// them are.**
///
/// It used to know. The executor special-cased the literal string `"CaptureScreenshot"`, and
/// carried a `PendingScreenshotRequest` resource and a `ScreenshotBackend` marker to go with
/// it — a domain capability named inside the substrate, plus a hand-rolled second answer to
/// "does this binary have that command?" that the type registry already gives you for free.
///
/// A crate that owns such a command registers it:
///
/// ```ignore
/// app.register_deferred_command::<CaptureScreenshot>();
/// ```
///
/// and its `#[on_command]` handler answers when ready:
///
/// ```ignore
/// let cid = pending.correlation_id;              // Res<PendingApiRequest>
/// commands.trigger(ApiResponseEvent { correlation_id: cid, response });
/// ```
///
/// **Contract:** a deferred command MUST eventually send exactly one `ApiResponseEvent` on
/// that id. If it never does, the caller hangs — which is precisely why a command that is
/// not registered here (because its plugin isn't in this binary) must fall through to the
/// ordinary `CommandNotFound` path rather than defer into silence.
#[derive(Resource, Default, Debug)]
pub struct DeferredCommands(std::collections::HashSet<String>);

impl DeferredCommands {
    pub fn contains(&self, command: &str) -> bool {
        self.0.contains(command)
    }
}

/// **The watchdog.** Every deferred response the executor is still waiting for, and when it
/// gives up.
///
/// A deferred command owes the caller exactly one [`ApiResponseEvent`] on its correlation id.
/// If it never sends one — a handler that forgot, a capture that never landed, a task that
/// panicked — the caller does not get an error. It gets **nothing**: the HTTP oneshot is
/// never resolved and the request hangs until some client-side timeout, with no trace in the
/// log. That is the worst failure mode available, and "the handler must remember" is not a
/// mechanism — it is a hope.
///
/// So the executor holds itself accountable: it records what it deferred, and if no answer
/// arrives inside [`timeout_secs`](Self::timeout_secs) it sends the error itself. A slow
/// handler degrades to a clear 500 instead of a silent hang.
///
/// This bit *immediately*: making deferral generic moved `save_to_file` screenshots onto this
/// path, and that branch answers early and never sends a second response — so the first thing
/// the watchdog caught was a real hang, in a command that had worked for months.
#[derive(Resource, Debug)]
pub struct DeferredRequests {
    /// correlation_id → deadline, in `Time<Real>` seconds. REAL time, deliberately: a paused
    /// or warped simulation must not change when an HTTP caller gives up.
    outstanding: std::collections::HashMap<u64, f64>,
    /// How long a deferred command may take. Generous — a GPU readback is a frame, but a bake
    /// or an export could be seconds — while still bounded.
    pub timeout_secs: f64,
}

impl Default for DeferredRequests {
    fn default() -> Self {
        Self { outstanding: std::collections::HashMap::new(), timeout_secs: 15.0 }
    }
}

impl DeferredRequests {
    /// Number of responses still owed (tests / diagnostics).
    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }
}

/// Any response — from a deferred command, a query provider, or an ordinary reply — settles
/// the debt for its correlation id.
fn clear_answered_request(
    trigger: On<ApiResponseEvent>,
    deferred: Option<ResMut<DeferredRequests>>,
) {
    if let Some(mut deferred) = deferred {
        deferred.outstanding.remove(&trigger.event().correlation_id);
    }
}

/// Nobody answered in time — answer for them, so the caller gets an error instead of silence.
fn expire_deferred_requests(
    time: Res<Time<bevy::time::Real>>,
    mut deferred: ResMut<DeferredRequests>,
    mut commands: Commands,
) {
    if deferred.outstanding.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let expired: Vec<u64> = deferred
        .outstanding
        .iter()
        .filter(|(_, &deadline)| now >= deadline)
        .map(|(&id, _)| id)
        .collect();

    for correlation_id in expired {
        deferred.outstanding.remove(&correlation_id);
        error!(
            "[lunco-api] a deferred command never answered request {correlation_id} within {}s \
             — returning an error. The handler must send exactly one ApiResponseEvent on its \
             correlation id; see DeferredRequests.",
            deferred.timeout_secs
        );
        commands.trigger(ApiResponseEvent {
            correlation_id,
            response: ApiResponse::error(
                ApiErrorCode::InternalError,
                "the command accepted this request but never produced a response",
            ),
        });
    }
}

/// The correlation id of the request a deferred command is currently answering.
///
/// Set by the executor immediately before it dispatches the command; read by the handler
/// (and by whatever async completion the handler arms) to address the response.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct PendingApiRequest {
    pub correlation_id: u64,
}

/// `app.register_deferred_command::<T>()` — declare that `T` answers on the correlation id.
pub trait DeferredCommandAppExt {
    fn register_deferred_command<T: bevy::prelude::Event + bevy::reflect::GetTypeRegistration>(
        &mut self,
    ) -> &mut Self;
}

impl DeferredCommandAppExt for App {
    fn register_deferred_command<T: bevy::prelude::Event + bevy::reflect::GetTypeRegistration>(
        &mut self,
    ) -> &mut Self {
        self.init_resource::<DeferredCommands>();
        self.init_resource::<PendingApiRequest>();
        // Registering the TYPE is what makes the command exist for this binary at all — it
        // is the same signal `DiscoverSchema` and the not-found check already read. No
        // separate "backend installed" marker.
        self.register_type::<T>();
        let short = std::any::type_name::<T>()
            .rsplit("::")
            .next()
            .unwrap_or_default()
            .to_string();
        self.world_mut().resource_mut::<DeferredCommands>().0.insert(short);
        self
    }
}

/// Plugin that registers the API executor observer.
pub struct ApiExecutorPlugin;

impl Plugin for ApiExecutorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiIdCounter>()
            // Command-result store + active-id scope. Also init'd by
            // lunco-core; idempotent, kept here so the API plugin is
            // self-contained (the executor reads CommandResults as a Res).
            .init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>()
            // The request observer takes `ResMut<TelemetrySubscriptions>` to
            // wire `SubscribeTelemetry`. Init here too (idempotent with
            // ApiTelemetryPlugin) so the executor is self-contained even when
            // the telemetry plugin isn't added.
            .init_resource::<TelemetrySubscriptions>()
            .add_observer(api_request_observer)
            .add_observer(api_command_dispatcher)
            .add_observer(project_command_events);

        // Deferred-command plumbing. WHICH commands are deferred is not decided here — a
        // crate that owns one calls `register_deferred_command::<T>()`.
        app.init_resource::<DeferredCommands>()
            .init_resource::<PendingApiRequest>()
            // The watchdog: a deferred response that never arrives becomes an error, not a
            // hang. See `DeferredRequests`.
            .init_resource::<DeferredRequests>()
            .add_observer(clear_answered_request)
            .add_systems(Update, expire_deferred_requests);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::{
        on_command, ActiveCommandId, Ack, Command, CommandOutcome, CommandResults, OpId,
    };

    /// THE WATCHDOG. A deferred command that never answers must produce an ERROR, not
    /// silence. Before this, a handler that forgot to send its `ApiResponseEvent` left the
    /// HTTP oneshot unresolved: the caller hung until a client-side timeout, and nothing was
    /// logged anywhere. "The handler must remember" is a hope, not a mechanism.
    #[test]
    fn a_deferred_command_that_never_answers_becomes_an_error() {
        use bevy::time::TimeUpdateStrategy;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin);
        app.init_resource::<DeferredRequests>();
        app.add_observer(clear_answered_request);
        app.add_systems(Update, expire_deferred_requests);
        // 5 s of real time per update — well past the default 15 s deadline in a few ticks.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(5)));

        // (correlation_id, is_error) — `ApiResponseEvent` isn't `Clone`.
        let seen: Arc<Mutex<Vec<(u64, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        app.add_observer(move |t: On<ApiResponseEvent>| {
            let e = t.event();
            sink.lock().unwrap().push((e.correlation_id, matches!(e.response, ApiResponse::Error { .. })));
        });

        // A command deferred at t=0 that nobody ever answers.
        app.world_mut().resource_mut::<DeferredRequests>().outstanding.insert(7, 15.0);
        assert_eq!(app.world().resource::<DeferredRequests>().outstanding(), 1);

        for _ in 0..5 {
            app.update();
        }

        let seen = seen.lock().unwrap();
        let (_, is_error) = *seen.iter().find(|(id, _)| *id == 7).expect("the caller must get AN answer");
        assert!(is_error, "an unanswered deferred command must surface as an error, not a hang");
        assert_eq!(
            app.world().resource::<DeferredRequests>().outstanding(),
            0,
            "the expired request must be dropped, not re-fired every frame"
        );
    }

    /// …and a command that DOES answer must not then be second-guessed by the watchdog.
    #[test]
    fn an_answered_request_is_not_timed_out() {
        use bevy::time::TimeUpdateStrategy;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin);
        app.init_resource::<DeferredRequests>();
        app.add_observer(clear_answered_request);
        app.add_systems(Update, expire_deferred_requests);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(5)));

        let count: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let sink = Arc::clone(&count);
        app.add_observer(move |t: On<ApiResponseEvent>| {
            if t.event().correlation_id == 9 {
                *sink.lock().unwrap() += 1;
            }
        });

        app.world_mut().resource_mut::<DeferredRequests>().outstanding.insert(9, 15.0);

        // The handler answers, promptly.
        app.world_mut().trigger(ApiResponseEvent {
            correlation_id: 9,
            response: ApiResponse::ok(serde_json::json!({ "ok": true })),
        });
        app.update();
        assert_eq!(app.world().resource::<DeferredRequests>().outstanding(), 0);

        // Long past the deadline — the watchdog must stay quiet. A SECOND response on the same
        // id would resolve the caller's oneshot twice.
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(*count.lock().unwrap(), 1, "exactly one response per request");
    }

    #[test]
    fn test_command_id_generation() {
        let mut counter = ApiIdCounter::default();
        assert_eq!(counter.next(), 0);
        assert_eq!(counter.next(), 1);
        assert_eq!(counter.next(), 2);
    }

    // A result-reporting command fixture: `Ok` → Succeeded, `Err` → Failed. It is a
    // REAL command (same `#[Command]` + `#[on_command]` + `register_commands!` path
    // as production verbs) rather than a mock, because what it exercises is the
    // executor's own reflection dispatch — a hand-rolled stand-in would prove the
    // stand-in works, not the dispatcher. Confined to `#[cfg(test)]`, so it never
    // reaches a real App's registry.
    #[Command(default)]
    struct TestEcho {
        pub fail: bool,
    }

    #[on_command(TestEcho)]
    fn on_test_echo(trigger: On<TestEcho>) -> Result<Ack, String> {
        if cmd.fail {
            Err("boom".into())
        } else {
            Ok(Ack::new(OpId::new()))
        }
    }

    #[test]
    fn result_handler_records_outcome_under_active_id() {
        let mut app = App::new();
        app.init_resource::<CommandResults>()
            .init_resource::<ActiveCommandId>();
        __register_on_test_echo(&mut app);

        // Success path, id scoped → recorded as Succeeded.
        app.world_mut().resource_mut::<ActiveCommandId>().set(Some(7));
        app.world_mut().trigger(TestEcho { fail: false });
        app.world_mut().resource_mut::<ActiveCommandId>().set(None);
        assert!(matches!(
            app.world().resource::<CommandResults>().get(7),
            Some(CommandOutcome::Succeeded(_))
        ));

        // Failure path → recorded as Failed (ran-and-errored, not Rejected).
        app.world_mut().resource_mut::<ActiveCommandId>().set(Some(8));
        app.world_mut().trigger(TestEcho { fail: true });
        app.world_mut().resource_mut::<ActiveCommandId>().set(None);
        assert!(matches!(
            app.world().resource::<CommandResults>().get(8),
            Some(CommandOutcome::Failed(_))
        ));

        // No active id (in-process trigger) → nothing recorded.
        app.world_mut().trigger(TestEcho { fail: false });
        assert!(app.world().resource::<CommandResults>().get(99).is_none());
    }

    // ── Params validation (a failed command must NOT report success) ──────
    //
    // The bug: `execute_request` minted a `command_id` and returned
    // `command_accepted` BEFORE anything looked at the params; the dispatcher
    // later dropped an undeserializable command with a `warn!`, and
    // `QueryCommandResult` answered `outcome: null` — the same thing it says
    // for a healthy fire-and-forget command. `{"command":"X","params":{bad}}`
    // was a 200 OK. These pin the synchronous gate that replaced it.

    fn test_registry() -> bevy::reflect::TypeRegistry {
        let mut reg = bevy::reflect::TypeRegistry::new();
        reg.register::<TestEcho>();
        reg
    }

    #[test]
    fn valid_params_pass_validation() {
        let reg = test_registry();
        let registration = reg.get_with_short_type_path("TestEcho").unwrap();
        assert!(validate_command_params(
            "TestEcho",
            &serde_json::json!({ "fail": true }),
            registration,
            &reg,
            &ApiEntityRegistry::default(),
        )
        .is_ok());
    }

    #[test]
    fn absent_params_pass_validation() {
        // Unit-ish command sent as `{"command":"TestEcho"}` — no params at all.
        // All fields default, so this is legitimately valid.
        let reg = test_registry();
        let registration = reg.get_with_short_type_path("TestEcho").unwrap();
        assert!(validate_command_params(
            "TestEcho",
            &serde_json::Value::Null,
            registration,
            &reg,
            &ApiEntityRegistry::default(),
        )
        .is_ok());
    }

    #[test]
    fn wrong_field_type_fails_validation() {
        let reg = test_registry();
        let registration = reg.get_with_short_type_path("TestEcho").unwrap();
        let err = validate_command_params(
            "TestEcho",
            &serde_json::json!({ "fail": "not-a-bool" }),
            registration,
            &reg,
            &ApiEntityRegistry::default(),
        )
        .unwrap_err();
        assert!(err.contains("TestEcho"), "error names the command: {err}");
    }

    #[test]
    fn unknown_field_fails_validation() {
        // The headline case: a typo'd param name. This used to return 200 OK.
        let reg = test_registry();
        let registration = reg.get_with_short_type_path("TestEcho").unwrap();
        assert!(validate_command_params(
            "TestEcho",
            &serde_json::json!({ "nope": true }),
            registration,
            &reg,
            &ApiEntityRegistry::default(),
        )
        .is_err());
    }
}

#[cfg(test)]
mod id_codec_tests {
    use super::{authz_target_gid, globalize_command_ids, resolve_command_ids};
    use crate::registry::ApiEntityRegistry;
    use bevy::prelude::*;
    use bevy::reflect::TypeRegistry;
    use lunco_core::GlobalEntityId;
    use serde_json::json;
    use std::any::TypeId;

    // Test command shapes. `#[reflect(@..)]` is exactly what the `#[Command]`
    // macro emits for `#[sync_local]` / `#[authz_target]`, so this exercises
    // the same runtime read path without pulling the whole command machinery.
    #[derive(Reflect)]
    struct TDrive {
        target: Entity,
        forward: f64,
    }
    #[derive(Reflect)]
    struct TVessel {
        // Name is OFF the old `[target,entity,body,parent,avatar]` allowlist —
        // the heuristic would have silently missed it.
        vessel: Entity,
    }
    #[derive(Reflect)]
    struct TNonEntity {
        // Heuristic field NAMES, but not entity TYPES — must be left alone.
        parent: String,
        target: f64,
    }
    #[derive(Reflect)]
    struct TInner {
        body: Entity,
    }
    #[derive(Reflect)]
    struct TColl {
        many: Vec<Entity>,
        maybe: Option<Entity>,
        inner: TInner,
    }
    #[derive(Reflect)]
    struct TPossess {
        #[reflect(@lunco_core::SyncLocal)]
        avatar: Entity,
        #[reflect(@lunco_core::AuthzTarget)]
        target: Entity,
    }
    #[derive(Reflect)]
    struct TPossessOpt {
        #[reflect(@lunco_core::SyncLocal)]
        avatar: Option<Entity>,
        #[reflect(@lunco_core::AuthzTarget)]
        target: Entity,
    }

    fn setup() -> (TypeRegistry, ApiEntityRegistry, Entity, GlobalEntityId) {
        // A real Entity (valid index+generation bits) we control the mapping of.
        let mut world = World::new();
        let e = world.spawn_empty().id();
        let gid = GlobalEntityId::from_raw(7000);
        let mut entities = ApiEntityRegistry::default();
        entities.assign(e, gid);

        let mut reg = TypeRegistry::new();
        reg.register::<TDrive>();
        reg.register::<TVessel>();
        reg.register::<TNonEntity>();
        reg.register::<TColl>();
        reg.register::<TInner>();
        reg.register::<TPossess>();
        reg.register::<TPossessOpt>();
        reg.register::<Entity>();
        reg.register::<Vec<Entity>>();
        reg.register::<Option<Entity>>();
        (reg, entities, e, gid)
    }

    #[test]
    fn resolve_converts_entity_field_by_type_not_name() {
        let (reg, ent, e, gid) = setup();
        let mut v = json!({ "target": gid.get(), "forward": 1.5 });
        resolve_command_ids(&mut v, TypeId::of::<TDrive>(), &reg, &ent);
        assert_eq!(v["target"], json!(e.to_bits())); // gid → local bits
        assert_eq!(v["forward"], json!(1.5)); // non-entity untouched
    }

    #[test]
    fn resolve_handles_field_off_the_old_heuristic_list() {
        let (reg, ent, e, gid) = setup();
        let mut v = json!({ "vessel": gid.get() });
        resolve_command_ids(&mut v, TypeId::of::<TVessel>(), &reg, &ent);
        assert_eq!(v["vessel"], json!(e.to_bits()));
    }

    #[test]
    fn resolve_skips_same_named_non_entity_fields() {
        let (reg, ent, _e, _gid) = setup();
        let mut v = json!({ "parent": "123", "target": 99 });
        let before = v.clone();
        resolve_command_ids(&mut v, TypeId::of::<TNonEntity>(), &reg, &ent);
        assert_eq!(v, before); // String/f64 left alone despite the names
    }

    #[test]
    fn resolve_descends_into_vec_option_and_nested_struct() {
        let (reg, ent, e, gid) = setup();
        let mut v = json!({
            "many": [gid.get(), gid.get()],
            "maybe": { "Some": gid.get() },
            "inner": { "body": gid.get() }
        });
        resolve_command_ids(&mut v, TypeId::of::<TColl>(), &reg, &ent);
        assert_eq!(v["many"], json!([e.to_bits(), e.to_bits()]));
        assert_eq!(v["maybe"], json!({ "Some": e.to_bits() }));
        assert_eq!(v["inner"]["body"], json!(e.to_bits()));
    }

    #[test]
    fn resolve_leaves_unmapped_gid_untouched() {
        let (reg, ent, _e, _gid) = setup();
        let mut v = json!({ "target": 999_999, "forward": 0.0 });
        resolve_command_ids(&mut v, TypeId::of::<TDrive>(), &reg, &ent);
        assert_eq!(v["target"], json!(999_999u64)); // no mapping → unchanged
    }

    #[test]
    fn globalize_inverts_resolve_and_strips_wire_local() {
        let (reg, ent, e, gid) = setup();
        let mut v = json!({ "avatar": e.to_bits(), "target": e.to_bits() });
        globalize_command_ids(&mut v, TypeId::of::<TPossess>(), &reg, &ent);
        assert_eq!(v["target"], json!(gid.get())); // local bits → gid
        // sync_local field never carries real local bits onto the wire.
        assert_eq!(v["avatar"], json!(Entity::PLACEHOLDER.to_bits()));
    }

    #[test]
    fn globalize_strips_wire_local_inside_option() {
        // `PossessVessel::avatar` is `Option<Entity>` + `#[sync_local]`. The
        // strip must reach the inner `Entity` of the `Some` payload, not just a
        // bare-`Entity` field — otherwise a possessing client leaks its local
        // camera bits onto the wire.
        let (reg, ent, e, _gid) = setup();
        let mut v = json!({ "avatar": { "Some": e.to_bits() }, "target": e.to_bits() });
        globalize_command_ids(&mut v, TypeId::of::<TPossessOpt>(), &reg, &ent);
        assert_eq!(v["avatar"], json!({ "Some": Entity::PLACEHOLDER.to_bits() }));
    }

    #[test]
    fn authz_target_reads_tagged_field_by_type() {
        let (reg, _ent, _e, gid) = setup();
        // Raw wire params carry the GLOBAL gid in the #[authz_target] field.
        let tagged = json!({ "avatar": 5, "target": gid.get() });
        assert_eq!(
            authz_target_gid(&tagged, TypeId::of::<TPossess>(), &reg),
            Some(gid.get())
        );
        // A command with no #[authz_target] field → None (target-less).
        let untagged = json!({ "target": 5, "forward": 1.0 });
        assert_eq!(
            authz_target_gid(&untagged, TypeId::of::<TDrive>(), &reg),
            None
        );
    }
}
