//! API request executor — processes `ApiRequest` and produces `ApiResponse`.
//!
//! Uses Bevy's `AppTypeRegistry` to discover all marked typed commands
//! for schema discovery. Commands are triggered as `ApiCommandEvent` which carries
//! the command name and typed in-process parameters.
//!
//! Domain observers can observe both:
//! - `On<SetPorts>` for internal triggers
//! - `On<ApiCommandEvent>` for API triggers (downcast the command)

use crate::queries::{ApiQueryRegistry, execute_query_response};
use crate::{
    discovery::{
        ApiCommandLookupError, discover_commands, discover_hooks, discover_queries,
        find_api_command,
    },
    queries::ApiVisibility,
    registry::ApiEntityRegistry,
    subscription::TelemetrySubscriptions,
};
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use lunco_api_core::{
    ApiErrorCode, ApiRequest, ApiResponse, ApiSchema, ApiValue, ApiValueDeserializer, api_value,
    api_value_from_serializable, api_value_from_u64, validate_reflection_value,
};
use lunco_celestial::CelestialBody;

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

/// A command ready for in-process dispatch, carrying typed parameters.
///
/// This is used internally by the API layer to bridge requests into simulation events.
#[derive(Event, Debug, Clone, Reflect)]
pub struct ApiCommandEvent {
    pub command: String,
    #[reflect(ignore)]
    pub params: ApiValue,
    /// Internal outcome key used by result-reporting handlers.
    pub id: u64,
    /// The transport request waiting for this command's result, if any.
    /// `None` is used by in-process callers and by commands whose owner sends a
    /// deferred response through its own completion path.
    #[reflect(ignore)]
    pub correlation_id: Option<u64>,
}

/// System counter for generating unique IDs.
#[derive(Resource, Default)]
pub struct ApiIdCounter {
    next: u64,
}
impl ApiIdCounter {
    /// Mint the next correlation id.
    ///
    /// Named `next_id`, not `next`: a bare `next(&mut self) -> u64` shadows
    /// `Iterator::next` at every call site. The lint's alternative — actually
    /// implementing `Iterator` — would be wrong here: this is an unbounded id
    /// source that never yields `None`, so advertising it as an iterator makes
    /// it `.collect()`-able into a hang.
    pub fn next_id(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }
}

/// Observer that processes API requests and produces responses.
pub fn api_request_observer(
    trigger: On<ApiRequestEvent>,
    mut commands: Commands,
    mut id_counter: ResMut<ApiIdCounter>,
    registry: Res<ApiEntityRegistry>,
    query_registry: Res<ApiQueryRegistry>,
    visibility: Res<ApiVisibility>,
    type_registry: Res<AppTypeRegistry>,
    mut subscriptions: ResMut<TelemetrySubscriptions>,
    q_meta: Query<(
        Option<&Name>,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        Has<lunco_control_core::ControlBinding>,
        Option<&CelestialBody>,
        Option<&lunco_core::UsdPrimKind>,
    )>,
    // Which commands answer later, on the correlation id. Populated by whichever crate owns
    // them (`register_deferred_command`), never by name here.
    deferred_commands: Option<Res<DeferredCommands>>,
) {
    let req = trigger.event();
    let correlation_id = req.correlation_id;

    let maybe_response = {
        let type_reg = type_registry.read();
        execute_request(
            &req.request,
            &mut commands,
            &mut id_counter,
            &registry,
            &query_registry,
            &visibility,
            &type_reg,
            &mut subscriptions,
            &q_meta,
            deferred_commands.as_deref(),
            correlation_id,
        )
    };

    // None means the response is produced by queued work — a command handler,
    // deferred command, or query provider answers on this correlation id later.
    if let Some(response) = maybe_response {
        commands.trigger(ApiResponseEvent {
            response,
            correlation_id,
        });
    }
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
/// Validate typed in-process command parameters at the API reflection edge.
///
/// A typed-value deserializer feeds Bevy reflection directly, keeping JSON out
/// of in-process command validation and dispatch.
pub fn validate_command_params_value(
    command: &str,
    params: &ApiValue,
    registration: &bevy::reflect::TypeRegistration,
    type_reg: &TypeRegistry,
    entities: &ApiEntityRegistry,
) -> Result<(), String> {
    use serde::de::DeserializeSeed;

    let mut resolved = params.clone();
    if matches!(resolved, ApiValue::Unit) {
        resolved = ApiValue::Map(Vec::new());
    }
    validate_reflection_value(&resolved)?;
    resolve_command_ids_value(&mut resolved, registration.type_id(), type_reg, entities)?;

    let deserializer = bevy::reflect::serde::TypedReflectDeserializer::new(registration, type_reg);
    let reflected = deserializer
        .deserialize(ApiValueDeserializer::new(resolved))
        .map_err(|error| format!("Command '{command}': invalid params: {error}"))?;
    let constructible = registration
        .data::<bevy::reflect::ReflectFromReflect>()
        .is_some_and(|from_reflect| from_reflect.from_reflect(reflected.as_ref()).is_some());
    if !constructible {
        return Err(format!(
            "Command '{command}': params are not constructible into the command type (a required field is missing or invalid)"
        ));
    }
    Ok(())
}

fn record_rejected_command(
    world: &mut World,
    id: u64,
    message: String,
    correlation_id: Option<u64>,
) {
    world.resource_mut::<lunco_core::CommandResults>().insert(
        id,
        lunco_core::CommandOutcome::Rejected(lunco_command_contracts::Reject::InvalidOp(message)),
    );
    emit_command_response(world, correlation_id, id);
}

struct NormalizedCommandResult {
    data: Option<ApiValue>,
    error: Option<(ApiErrorCode, String)>,
}

fn normalize_command_result(
    outcome: Option<&lunco_core::CommandOutcome>,
) -> NormalizedCommandResult {
    match outcome {
        Some(lunco_core::CommandOutcome::Succeeded(ack)) => NormalizedCommandResult {
            data: ack.data.clone(),
            error: None,
        },
        Some(lunco_core::CommandOutcome::Rejected(reject)) => NormalizedCommandResult {
            data: None,
            error: Some((ApiErrorCode::CommandRejected, reject.to_string())),
        },
        Some(lunco_core::CommandOutcome::Failed(message)) => NormalizedCommandResult {
            data: None,
            error: Some((ApiErrorCode::InternalError, message.clone())),
        },
        Some(lunco_core::CommandOutcome::Pending) | None => NormalizedCommandResult {
            data: None,
            error: None,
        },
    }
}

/// Convert the command-result contract to the typed in-process value used by
/// scripting bridges.
pub fn command_result_value(id: u64, outcome: Option<&lunco_core::CommandOutcome>) -> ApiValue {
    let result = normalize_command_result(outcome);
    let status = match outcome {
        Some(lunco_core::CommandOutcome::Succeeded(_)) => "applied",
        Some(lunco_core::CommandOutcome::Rejected(_)) => "rejected",
        Some(lunco_core::CommandOutcome::Failed(_)) => "failed",
        Some(lunco_core::CommandOutcome::Pending) | None => "pending",
    };
    let mut entries = vec![
        ("id".into(), api_value_from_u64(id)),
        ("ok".into(), ApiValue::Bool(result.error.is_none())),
        ("status".into(), ApiValue::Str(status.into())),
    ];
    if let Some(data) = result.data {
        entries.push(("data".into(), data));
    }
    if let Some((_, error)) = result.error {
        entries.push(("error".into(), ApiValue::Str(error)));
    }
    ApiValue::Map(entries)
}

/// Convert the same normalized result into the transport response envelope.
/// Keeping this beside [`command_result_value`] prevents HTTP and scripting
/// from inventing separate Ack/data handling.
fn command_response(outcome: Option<&lunco_core::CommandOutcome>) -> ApiResponse {
    command_response_from_normalized(normalize_command_result(outcome))
}

fn command_response_from_normalized(result: NormalizedCommandResult) -> ApiResponse {
    match result.error {
        Some((code, message)) => ApiResponse::error(code, message),
        None => match result.data {
            Some(data) => ApiResponse::ok(data),
            None => ApiResponse::accepted(),
        },
    }
}

fn normalize_command_handler_result(
    result: &Result<lunco_command_contracts::Ack, String>,
    error_code: ApiErrorCode,
) -> NormalizedCommandResult {
    match result {
        Ok(ack) => NormalizedCommandResult {
            data: ack.data.clone(),
            error: None,
        },
        Err(message) => NormalizedCommandResult {
            data: None,
            error: Some((error_code, message.clone())),
        },
    }
}

/// Variant for a deferred owner whose failed result is a valid command that
/// the current simulation state rejected rather than an internal failure.
fn command_response_from_result_with_error_code(
    result: &Result<lunco_command_contracts::Ack, String>,
    error_code: ApiErrorCode,
) -> ApiResponse {
    command_response_from_normalized(normalize_command_handler_result(result, error_code))
}

/// Record a deferred command's result and, when a transport is waiting, emit
/// its response through the same mapper used by ordinary command dispatch.
/// `error_code` is supplied by the owning domain because only that owner knows
/// whether an error means a command rejection or an internal failure. A
/// `CommandRejected` error is retained as `CommandOutcome::Rejected` for
/// in-process callers as well as mapped to the transport error response.
pub fn finish_command_result(
    world: &mut World,
    command_id: Option<u64>,
    correlation_id: Option<u64>,
    result: Result<lunco_command_contracts::Ack, String>,
    error_code: ApiErrorCode,
) {
    let response = correlation_id.map(|correlation_id| ApiResponseEvent {
        response: command_response_from_result_with_error_code(&result, error_code),
        correlation_id,
    });
    if let Some(command_id) = command_id {
        let outcome = match result {
            Ok(ack) => lunco_core::CommandOutcome::Succeeded(ack),
            Err(message) if matches!(error_code, ApiErrorCode::CommandRejected) => {
                lunco_core::CommandOutcome::Rejected(lunco_command_contracts::Reject::InvalidOp(
                    message,
                ))
            }
            Err(message) => lunco_core::CommandOutcome::Failed(message),
        };
        world
            .resource_mut::<lunco_core::CommandResults>()
            .insert(command_id, outcome);
    }
    if let Some(event) = response {
        world.commands().trigger(event);
    }
}

/// Build a successful acknowledgement with a typed string field.
pub fn ack_with_string_field(
    op_id: lunco_command_contracts::OpId,
    field: &str,
    value: impl Into<String>,
) -> lunco_command_contracts::Ack {
    lunco_command_contracts::Ack::with_data(
        op_id,
        ApiValue::map([(field, ApiValue::Str(value.into()))]),
    )
}

fn emit_command_response(world: &mut World, correlation_id: Option<u64>, id: u64) {
    let Some(correlation_id) = correlation_id else {
        return;
    };
    let response = world
        .get_resource::<lunco_core::CommandResults>()
        .and_then(|results| results.get(id))
        .map(|outcome| command_response(Some(outcome)))
        .unwrap_or_else(ApiResponse::accepted);
    world.commands().trigger(ApiResponseEvent {
        response,
        correlation_id,
    });
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

    // 1. Resolve through the marker boundary. This observer is also reachable
    // in-process, so it must not turn an arbitrary reflected event into a
    // command merely because a caller constructed `ApiCommandEvent` by hand.
    let registration = match find_api_command(&type_reg, &event.command, None) {
        Ok(registration) => registration,
        Err(error) => {
            let message = error.message(&event.command);
            warn!("[lunco-api] {message}");
            let id = event.id;
            let correlation_id = event.correlation_id;
            commands.queue(move |world: &mut World| {
                record_rejected_command(world, id, message, correlation_id);
            });
            return;
        }
    };

    // 2. Resolve typed entity identities, then deserialize the typed value into
    // the reflected command. Internal command events do not use JSON.
    let mut resolved_params = event.params.clone();
    if matches!(resolved_params, ApiValue::Unit) {
        resolved_params = ApiValue::Map(Vec::new());
    }
    if let Err(error) = validate_reflection_value(&resolved_params) {
        let id = event.id;
        let correlation_id = event.correlation_id;
        let message = format!("Command '{}': invalid typed params: {error}", event.command);
        commands.queue(move |world: &mut World| {
            record_rejected_command(world, id, message, correlation_id);
        });
        return;
    }
    if let Err(error) = resolve_command_ids_value(
        &mut resolved_params,
        registration.type_id(),
        &type_reg,
        &registry,
    ) {
        let id = event.id;
        let correlation_id = event.correlation_id;
        let message = format!("Command '{}': invalid typed params: {error}", event.command);
        commands.queue(move |world: &mut World| {
            record_rejected_command(world, id, message, correlation_id);
        });
        return;
    }

    // 3. Deserialize the typed value into the reflected struct.
    let reflect_deserializer =
        bevy::reflect::serde::TypedReflectDeserializer::new(registration, &type_reg);

    use serde::de::DeserializeSeed;
    match reflect_deserializer.deserialize(ApiValueDeserializer::new(resolved_params.clone())) {
        Ok(_reflected) => {
            // 4. Trigger the event dynamically via commands.queue to access World
            let cmd_name = event.command.clone();
            let cmd_id = event.id;
            let correlation_id = event.correlation_id;

            commands.queue(move |world: &mut World| {
                let registry = world.resource::<AppTypeRegistry>().clone();
                let type_reg = registry.read();

                let registration = match find_api_command(&type_reg, &cmd_name, None) {
                    Ok(registration) => registration,
                    Err(error) => {
                        let message = error.message(&cmd_name);
                        warn!("[lunco-api] {message}");
                        record_rejected_command(world, cmd_id, message, correlation_id);
                        return;
                    }
                };
                let Some(reflect_event) = registration.data::<bevy::ecs::reflect::ReflectEvent>() else {
                    let message = format!("Command '{cmd_name}' has no reflected event registration");
                    warn!("[lunco-api] {message}");
                    record_rejected_command(world, cmd_id, message, correlation_id);
                    return;
                };

                // Re-deserialize inside the world queue where we have access to everything
                let reflect_deserializer = bevy::reflect::serde::TypedReflectDeserializer::new(registration, &type_reg);
                let reflected = match reflect_deserializer
                    .deserialize(ApiValueDeserializer::new(resolved_params))
                {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = format!("command '{cmd_name}': invalid params: {e}");
                        warn!("[lunco-api] {msg}; dropped");
                        record_rejected_command(world, cmd_id, msg, correlation_id);
                        return;
                    }
                };
                {
                    // Guard against a panic in `ReflectEvent::trigger`: it builds
                    // the concrete type via `FromReflect`, falling back to
                    // `Default`/`FromWorld` and panicking when none apply. Verify
                    // the value is fully constructible first so malformed
                    // commands are logged and dropped instead of killing the
                    // process. Types without a registered `ReflectFromReflect`
                    // use Bevy's normal reflected-event path.
                    let constructible = registration
                        .data::<bevy::reflect::ReflectFromReflect>()
                        .is_some_and(|fr| fr.from_reflect(reflected.as_ref()).is_some());
                    if !constructible {
                        let msg = format!(
                            "command '{cmd_name}' not constructible from params (missing/invalid fields)"
                        );
                        warn!("[lunco-api] {msg}; dropped");
                        // Record a terminal internal outcome for scripts and
                        // in-process result handlers.
                        record_rejected_command(world, cmd_id, msg, correlation_id);
                        return;
                    }
                    // Scope the active request id around the trigger so a
                    // result-reporting `#[on_command]` wrapper records its
                    // outcome under this id. Observers run synchronously
                    // inside `trigger`, so set-before / clear-after is sound.
                    world.resource_mut::<lunco_core::ActiveCommandId>().set(Some(cmd_id));
                    reflect_event.trigger(world, reflected.as_ref(), &type_reg);
                    world.resource_mut::<lunco_core::ActiveCommandId>().set(None);
                    // The pending correlation is a per-dispatch handoff to a
                    // deferred command handler. Clear it immediately after
                    // the reflected event so a later in-process trigger cannot
                    // inherit an old transport request.
                    if let Some(mut pending) = world.get_resource_mut::<PendingApiRequest>() {
                        pending.correlation_id = 0;
                    }
                    emit_command_response(world, correlation_id, cmd_id);
                }
            });
        }
        Err(e) => {
            // Terminal, and RECORDED — see the `!constructible` branch above.
            // An external caller sees this synchronously as a 422 from
            // `execute_request`'s pre-flight validation; an in-process trigger
            // learns about it through the internal command-result substrate.
            let msg = format!("command '{}': invalid params: {e}", event.command);
            warn!("[lunco-api] {msg}; dropped");
            let cmd_id = event.id;
            let correlation_id = event.correlation_id;
            commands.queue(move |world: &mut World| {
                record_rejected_command(world, cmd_id, msg, correlation_id);
            });
        }
    }
}

// ── Entity-id conversion (schema-driven) ──────────────────────────────────
//
// One reflection walk handles local/global conversion in the typed API ABI.
// Network wire encoding is owned by lunco-api-codec at the network boundary.

/// Resolve global entity IDs in typed command parameters before reflection.
pub fn resolve_command_ids_value(
    value: &mut ApiValue,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
    entities: &ApiEntityRegistry,
) -> Result<(), String> {
    convert_value_node(
        value,
        type_id,
        reg,
        entities,
        EntityIdDirection::Resolve,
        false,
    )
}

/// Convert local entity identities to global IDs before a command is sent to peers.
pub fn globalize_command_ids_value(
    value: &mut ApiValue,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
    entities: &ApiEntityRegistry,
) -> Result<(), String> {
    convert_value_node(
        value,
        type_id,
        reg,
        entities,
        EntityIdDirection::Globalize,
        false,
    )
}

/// Read the global ID of the field marked `#[authz_target]`.
pub fn authz_target_gid_value(
    params: &ApiValue,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
) -> Result<Option<u64>, String> {
    use bevy::reflect::TypeInfo;

    let Some(type_info) = reg.get_type_info(type_id) else {
        return Ok(None);
    };
    let TypeInfo::Struct(struct_info) = type_info else {
        return Ok(None);
    };
    let Some(field) = (0..struct_info.field_len())
        .filter_map(|index| struct_info.field_at(index))
        .find(|field| field.has_attribute::<lunco_core::AuthzTarget>())
    else {
        return Ok(None);
    };
    let Some(value) = params.get(field.name()) else {
        // An optional authorization target is an intentionally unscoped command
        // request. The command handler remains responsible for resolving its
        // semantic default (for example, a WorldRoot host).
        if matches!(reg.get_type_info(field.type_id()), Some(TypeInfo::Enum(info)) if info.variant("None").is_some())
        {
            return Ok(None);
        }
        return Err(format!(
            "authorization target field '{}' is missing",
            field.name()
        ));
    };
    if matches!(value, ApiValue::Unit)
        && matches!(reg.get_type_info(field.type_id()), Some(TypeInfo::Enum(info)) if info.variant("None").is_some())
    {
        return Ok(None);
    }
    api_value_u64(value).map(Some).ok_or_else(|| {
        format!(
            "authorization target field '{}' must be an unsigned ID",
            field.name()
        )
    })
}

#[derive(Clone, Copy)]
enum EntityIdDirection {
    Resolve,
    Globalize,
}

fn api_value_u64(value: &ApiValue) -> Option<u64> {
    match value {
        ApiValue::Int(value) => u64::try_from(*value).ok(),
        ApiValue::UInt(value) => Some(*value),
        _ => None,
    }
}

fn convert_value_node(
    value: &mut ApiValue,
    type_id: std::any::TypeId,
    reg: &bevy::reflect::TypeRegistry,
    entities: &ApiEntityRegistry,
    direction: EntityIdDirection,
    sync_local: bool,
) -> Result<(), String> {
    use bevy::reflect::{TypeInfo, enums::VariantInfo};
    use std::any::TypeId;

    if type_id == TypeId::of::<Entity>() {
        match direction {
            EntityIdDirection::Resolve => {
                if let Some(gid) = api_value_u64(value) {
                    if let Some(entity) =
                        entities.resolve(&lunco_core::GlobalEntityId::from_raw(gid))
                    {
                        *value = api_value_from_u64(entity.to_bits());
                    }
                }
            }
            EntityIdDirection::Globalize if sync_local => {
                *value = api_value_from_u64(Entity::PLACEHOLDER.to_bits());
            }
            EntityIdDirection::Globalize => {
                if let Some(bits) = api_value_u64(value) {
                    if let Some(entity) = Entity::try_from_bits(bits) {
                        if let Some(gid) = entities.api_id_for(entity) {
                            *value = api_value_from_u64(gid.get());
                        }
                    }
                }
            }
        }
        return Ok(());
    }

    let Some(info) = reg.get_type_info(type_id) else {
        return Ok(());
    };
    match info {
        TypeInfo::Struct(struct_info) => {
            let ApiValue::Map(entries) = value else {
                return Ok(());
            };
            for index in 0..struct_info.field_len() {
                let Some(field) = struct_info.field_at(index) else {
                    continue;
                };
                if let Some((_, child)) = entries.iter_mut().find(|(name, _)| name == field.name())
                {
                    convert_value_node(
                        child,
                        field.type_id(),
                        reg,
                        entities,
                        direction,
                        field.has_attribute::<lunco_core::SyncLocal>(),
                    )?;
                }
            }
        }
        TypeInfo::TupleStruct(tuple_info) => match value {
            ApiValue::Array(values) => {
                for index in 0..tuple_info.field_len() {
                    if let (Some(field), Some(child)) =
                        (tuple_info.field_at(index), values.get_mut(index))
                    {
                        convert_value_node(
                            child,
                            field.type_id(),
                            reg,
                            entities,
                            direction,
                            false,
                        )?;
                    }
                }
            }
            child if tuple_info.field_len() == 1 => {
                if let Some(field) = tuple_info.field_at(0) {
                    convert_value_node(child, field.type_id(), reg, entities, direction, false)?;
                }
            }
            _ => {}
        },
        TypeInfo::Tuple(tuple_info) => {
            if let ApiValue::Array(values) = value {
                for index in 0..tuple_info.field_len() {
                    if let (Some(field), Some(child)) =
                        (tuple_info.field_at(index), values.get_mut(index))
                    {
                        convert_value_node(
                            child,
                            field.type_id(),
                            reg,
                            entities,
                            direction,
                            false,
                        )?;
                    }
                }
            }
        }
        TypeInfo::List(list_info) => {
            if let ApiValue::Array(values) = value {
                for child in values {
                    convert_value_node(
                        child,
                        list_info.item_ty().id(),
                        reg,
                        entities,
                        direction,
                        false,
                    )?;
                }
            }
        }
        TypeInfo::Array(array_info) => {
            if let ApiValue::Array(values) = value {
                for child in values {
                    convert_value_node(
                        child,
                        array_info.item_ty().id(),
                        reg,
                        entities,
                        direction,
                        false,
                    )?;
                }
            }
        }
        TypeInfo::Map(map_info) => match value {
            ApiValue::Map(entries) => {
                for (_, child) in entries {
                    convert_value_node(
                        child,
                        map_info.value_ty().id(),
                        reg,
                        entities,
                        direction,
                        false,
                    )?;
                }
            }
            ApiValue::Array(entries) => {
                for pair in entries {
                    if let ApiValue::Array(pair) = pair {
                        if let Some(child) = pair.get_mut(1) {
                            convert_value_node(
                                child,
                                map_info.value_ty().id(),
                                reg,
                                entities,
                                direction,
                                false,
                            )?;
                        }
                    }
                }
            }
            _ => {}
        },
        TypeInfo::Enum(enum_info) => match value {
            ApiValue::Map(entries) if entries.len() == 1 => {
                let (variant_name, payload) = &mut entries[0];
                let Some(variant) = enum_info.variant(variant_name) else {
                    return Ok(());
                };
                match variant {
                    VariantInfo::Struct(struct_info) => {
                        if let ApiValue::Map(fields) = payload {
                            for index in 0..struct_info.field_len() {
                                if let Some(field) = struct_info.field_at(index) {
                                    if let Some((_, child)) =
                                        fields.iter_mut().find(|(name, _)| name == field.name())
                                    {
                                        convert_value_node(
                                            child,
                                            field.type_id(),
                                            reg,
                                            entities,
                                            direction,
                                            field.has_attribute::<lunco_core::SyncLocal>(),
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                    VariantInfo::Tuple(tuple_info) if tuple_info.field_len() == 1 => {
                        if let Some(field) = tuple_info.field_at(0) {
                            convert_value_node(
                                payload,
                                field.type_id(),
                                reg,
                                entities,
                                direction,
                                sync_local,
                            )?;
                        }
                    }
                    VariantInfo::Tuple(tuple_info) => {
                        if let ApiValue::Array(values) = payload {
                            for index in 0..tuple_info.field_len() {
                                if let (Some(field), Some(child)) =
                                    (tuple_info.field_at(index), values.get_mut(index))
                                {
                                    convert_value_node(
                                        child,
                                        field.type_id(),
                                        reg,
                                        entities,
                                        direction,
                                        field.has_attribute::<lunco_core::SyncLocal>(),
                                    )?;
                                }
                            }
                        }
                    }
                    VariantInfo::Unit(_) => {}
                }
            }
            ApiValue::Unit | ApiValue::Str(_) => {}
            child => {
                if let Some(VariantInfo::Tuple(tuple_info)) = enum_info.variant("Some") {
                    if tuple_info.field_len() == 1 {
                        if let Some(field) = tuple_info.field_at(0) {
                            convert_value_node(
                                child,
                                field.type_id(),
                                reg,
                                entities,
                                direction,
                                sync_local,
                            )?;
                        }
                    }
                }
            }
        },
        _ => {}
    }
    Ok(())
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
    subscriptions: &mut TelemetrySubscriptions,
    q_meta: &Query<(
        Option<&Name>,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        Has<lunco_control_core::ControlBinding>,
        Option<&CelestialBody>,
        Option<&lunco_core::UsdPrimKind>,
    )>,
    deferred_commands: Option<&DeferredCommands>,
    correlation_id: u64,
) -> Option<ApiResponse> {
    match request {
        ApiRequest::ExecuteCommand { command, params } => {
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

            // Reflected commands own the public mutation namespace. The read
            // provider namespace is consulted only when no reflected command
            // owns the name, so a future command cannot be silently shadowed
            // by a provider registered earlier. Ambiguous reflected names are
            // hard failures, never query fallbacks.
            let registration = match find_api_command(type_registry, command, Some(visibility)) {
                Ok(registration) => Some(registration),
                Err(ApiCommandLookupError::NotFound)
                | Err(ApiCommandLookupError::NotApiCommand) => None,
                Err(error @ ApiCommandLookupError::Ambiguous)
                | Err(error @ ApiCommandLookupError::Hidden) => {
                    return Some(ApiResponse::error(
                        ApiErrorCode::CommandNotFound,
                        error.message(command),
                    ));
                }
            };
            let is_public_command = registration.is_some();

            // Query registry — read-only endpoints that return data. Domain
            // crates register providers via `ApiQueryRegistry::register`. The
            // provider runs deferred via `commands.queue`; the response is
            // fired back via `ApiResponseEvent`
            // when the queue flushes.
            if !is_public_command {
                if let Some(provider) = query_registry.get(command) {
                    let typed_params = params.clone();
                    commands.queue(move |world: &mut World| {
                        let response =
                            execute_query_response(provider.as_ref(), world, &typed_params);
                        world.commands().trigger(ApiResponseEvent {
                            response,
                            correlation_id,
                        });
                    });
                    return None; // response deferred
                }
            }

            if !is_public_command {
                return Some(ApiResponse::error(
                    ApiErrorCode::CommandNotFound,
                    format!("Command '{}' not found or not API-accessible", command),
                ));
            }

            let typed_params = params.clone();

            // Validate the params synchronously, here, while the registry is in
            // hand. A typo'd param must be rejected at the request boundary,
            // rather than being dropped later by the reflected dispatcher. This
            // gate also applies to deferred commands: deferral is a response
            // timing policy, not a different command contract.
            //
            // The dispatcher still re-validates (it must: `ApiCommandEvent` can
            // be triggered in-process too), so this is a gate, not the only
            // check.
            if let Some(registration) = registration {
                if let Err(msg) = validate_command_params_value(
                    command,
                    &typed_params,
                    registration,
                    type_registry,
                    registry,
                ) {
                    return Some(ApiResponse::error(ApiErrorCode::DeserializationError, msg));
                }
            }

            // A DEFERRED command answers on this request's correlation id, later. The
            // executor does not know (and must not know) which commands those are — a crate
            // that owns one calls `register_deferred_command::<T>()`. See `DeferredCommands`.
            //
            // The visibility, type, and validation gates above deliberately run first. A
            // hidden command cannot bypass its visibility policy, malformed input is rejected
            // synchronously, and an absent event registration cannot be parked as deferred work.
            if deferred_commands.is_some_and(|d| d.contains(command)) {
                commands.insert_resource(PendingApiRequest { correlation_id });
                commands.trigger(ApiCommandEvent {
                    command: command.clone(),
                    params: typed_params.clone(),
                    id: id_counter.next_id(),
                    correlation_id: None,
                });
                return None; // the handler answers on `correlation_id`
            }

            // Trigger as ApiCommandEvent — handled by api_command_dispatcher.
            // The dispatcher sends the command's Ack.data after the typed
            // handler runs, so the transport and in-process callers use the
            // same result-producing path.
            let command_id = id_counter.next_id();
            commands.trigger(ApiCommandEvent {
                command: command.clone(),
                params: typed_params,
                id: command_id,
                correlation_id: Some(correlation_id),
            });

            None
        }
        ApiRequest::ListEntities => {
            let entities: Vec<ApiValue> = registry
                .entities()
                .into_iter()
                .map(|(api_id, entity)| {
                    let (name, callsign, catalog_id, accepts_commands, body, usd_kind) = q_meta
                        .get(entity)
                        .unwrap_or((None, None, None, false, None, None));
                    let kind = usd_kind.map(|kind| kind.0.as_str()).unwrap_or("untyped");
                    ApiValue::map([
                        ("api_id", api_value_from_u64(api_id.get())),
                        (
                            "name",
                            ApiValue::str(lunco_core::entity_display_name(
                                name, callsign, catalog_id,
                            )),
                        ),
                        ("type", ApiValue::str(kind)),
                        ("control_bound", ApiValue::Bool(accepts_commands)),
                        ("celestial_body", ApiValue::Bool(body.is_some())),
                    ])
                })
                .collect();
            let count = api_value_from_u64(entities.len() as u64);
            Some(ApiResponse::ok(ApiValue::map([
                ("entities", ApiValue::Array(entities)),
                ("count", count),
            ])))
        }
        ApiRequest::DiscoverSchema => {
            let cmds = discover_commands(type_registry, Some(visibility));
            let queries = discover_queries(Some(query_registry));
            let hooks = discover_hooks();
            match api_value_from_serializable(&ApiSchema {
                commands: cmds,
                queries,
                hooks,
            }) {
                Ok(schema) => Some(ApiResponse::ok(schema)),
                Err(error) => Some(ApiResponse::error(
                    ApiErrorCode::InternalError,
                    format!("API schema could not be represented as typed values: {error}"),
                )),
            }
        }
        ApiRequest::SubscribeTelemetry { filter } => {
            // Register the subscription so the telemetry observers actually
            // stream matching events (incl. script `emit()`s) back to this
            // client. Previously a no-op that lied "Subscription created".
            let id = subscriptions.subscribe(filter.clone());
            Some(ApiResponse::ok(api_value!({
                "subscription_id": api_value_from_u64(id)
            })))
        }
        ApiRequest::UnsubscribeTelemetry { id } => {
            // `unsubscribe` has existed since the beginning with NOTHING able to call
            // it — subscriptions leaked for the life of the process, and a client that
            // reconnected piled up a new one every time.
            subscriptions.unsubscribe(*id);
            Some(ApiResponse::ok(api_value!({
                "unsubscribed": api_value_from_u64(*id)
            })))
        }
    }
}

/// **Commands that answer LATER, on the request's correlation id.**
///
/// Commands are validated here, then their typed handler produces the response
/// through the same `ApiResponseEvent` path as deferred work. A few cannot
/// finish in the handler — a GPU frame is captured, a bake finishes, or a file
/// is written. Those keep ownership of completion and send their actual payload
/// later rather than making the caller poll a second endpoint.
///
/// The mechanism already existed for query providers (`return None; // response deferred`,
/// then answer with an [`ApiResponseEvent`] carrying the same `correlation_id`). This makes
/// it available to COMMANDS too, and — crucially — **without `lunco-api` knowing what any of
/// them are.**
///
/// The executor remains capability-agnostic: render-bound commands register their own deferred
/// types, while binaries without the owning plugin follow the ordinary `CommandNotFound` path.
///
/// A crate that owns such a command registers it:
///
/// ```ignore
/// app.register_deferred_command::<CaptureScreenshot>();
/// ```
///
/// and its `#[on_command]` handler answers when ready. An Ack-based deferred
/// command completes through [`finish_command_result`]:
///
/// ```ignore
/// let cid = pending.correlation_id;              // Res<PendingApiRequest>
/// finish_command_result(
///     world,
///     command_id,
///     Some(cid),
///     result,
///     ApiErrorCode::InternalError,
/// );
/// ```
/// Specialized payloads such as raw screenshot bytes may emit
/// [`ApiResponseEvent`] directly because they are not represented by an `Ack`.
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
        self.world_mut()
            .resource_mut::<DeferredCommands>()
            .0
            .insert(short);
        self
    }
}

/// Plugin that registers the API executor observer.
pub struct ApiExecutorPlugin;

impl Plugin for ApiExecutorPlugin {
    fn build(&self, app: &mut App) {
        // Session commands (`Ping`) — registered with the command CORE, so any
        // host that can receive a command can answer a readiness probe.
        crate::session::register_all_commands(app);
        app.init_resource::<crate::session::InteractiveExitHandler>();
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
            .add_observer(api_command_dispatcher);

        // Deferred-command plumbing. Which commands are deferred is not decided here — a crate
        // that owns one calls `register_deferred_command::<T>()`.
        app.init_resource::<DeferredCommands>()
            .init_resource::<PendingApiRequest>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_command_contracts::{Ack, OpId};
    use lunco_core::{ActiveCommandId, Command, CommandOutcome, CommandResults, on_command};

    #[test]
    fn internal_command_id_generation() {
        let mut counter = ApiIdCounter::default();
        assert_eq!(counter.next_id(), 0);
        assert_eq!(counter.next_id(), 1);
        assert_eq!(counter.next_id(), 2);
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

    #[derive(Reflect)]
    #[reflect(from_reflect = false)]
    struct NoConstructor {
        pub fail: bool,
    }

    #[on_command(TestEcho)]
    fn on_test_echo(trigger: On<TestEcho>) -> Result<Ack, String> {
        if cmd.fail {
            Err("boom".into())
        } else {
            Ok(Ack::with_data(
                OpId::new(),
                lunco_api_core::api_value!({ "answer": 42 }),
            ))
        }
    }

    #[test]
    fn synchronous_command_data_uses_the_api_response_path() {
        use std::sync::{Arc, Mutex};

        let mut app = App::new();
        app.add_plugins(ApiExecutorPlugin)
            .init_resource::<ApiEntityRegistry>()
            .init_resource::<ApiQueryRegistry>()
            .init_resource::<ApiVisibility>();
        __register_on_test_echo(&mut app);

        let responses = Arc::new(Mutex::new(Vec::<ApiResponse>::new()));
        let sink = Arc::clone(&responses);
        app.add_observer(move |trigger: On<ApiResponseEvent>| {
            sink.lock().unwrap().push(trigger.event().response.clone());
        });

        app.world_mut().trigger(ApiRequestEvent {
            request: ApiRequest::ExecuteCommand {
                command: "TestEcho".into(),
                params: api_value!({ "fail": false }),
            },
            correlation_id: 42,
        });
        app.world_mut().flush();

        let responses = responses.lock().unwrap();
        assert!(matches!(
            responses.as_slice(),
            [ApiResponse::Ok {
                data: Some(data)
            }] if data.get("answer").and_then(ApiValue::as_i64) == Some(42)
        ));
    }

    #[test]
    fn deferred_command_result_records_and_emits_once() {
        use std::sync::{Arc, Mutex};

        let mut app = App::new();
        app.init_resource::<CommandResults>();

        let responses = Arc::new(Mutex::new(Vec::<ApiResponse>::new()));
        let sink = Arc::clone(&responses);
        app.add_observer(move |trigger: On<ApiResponseEvent>| {
            sink.lock().unwrap().push(trigger.event().response.clone());
        });

        finish_command_result(
            app.world_mut(),
            Some(7),
            Some(42),
            Ok(Ack::with_data(
                OpId::new(),
                lunco_api_core::api_value!({ "answer": 42 }),
            )),
            ApiErrorCode::InternalError,
        );
        app.world_mut().flush();

        assert!(matches!(
            app.world().resource::<CommandResults>().get(7),
            Some(CommandOutcome::Succeeded(ack))
                if ack.data.as_ref().and_then(|data| data.get("answer")).and_then(ApiValue::as_i64) == Some(42)
        ));
        let responses = responses.lock().unwrap();
        assert!(matches!(
            responses.as_slice(),
            [ApiResponse::Ok {
                data: Some(data)
            }] if data.get("answer").and_then(ApiValue::as_i64) == Some(42)
        ));
    }

    #[test]
    fn result_handler_records_outcome_under_active_id() {
        let mut app = App::new();
        app.init_resource::<CommandResults>()
            .init_resource::<ActiveCommandId>();
        __register_on_test_echo(&mut app);

        // Success path, id scoped → recorded as Succeeded.
        app.world_mut()
            .resource_mut::<ActiveCommandId>()
            .set(Some(7));
        app.world_mut().trigger(TestEcho { fail: false });
        app.world_mut().resource_mut::<ActiveCommandId>().set(None);
        assert!(matches!(
            app.world().resource::<CommandResults>().get(7),
            Some(CommandOutcome::Succeeded(_))
        ));

        // Failure path → recorded as Failed (ran-and-errored, not Rejected).
        app.world_mut()
            .resource_mut::<ActiveCommandId>()
            .set(Some(8));
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
    // The bug: the request path used to acknowledge before validating params;
    // the dispatcher then dropped malformed commands. These pin the
    // synchronous validation gate that replaced that ambiguity.

    fn test_registry() -> bevy::reflect::TypeRegistry {
        let mut reg = bevy::reflect::TypeRegistry::new();
        reg.register::<TestEcho>();
        reg
    }

    #[test]
    fn missing_constructor_fails_validation() {
        let mut reg = bevy::reflect::TypeRegistry::new();
        reg.register::<NoConstructor>();
        let registration = reg.get_with_short_type_path("NoConstructor").unwrap();
        let err = validate_command_params_value(
            "NoConstructor",
            &api_value!({ "fail": true }),
            registration,
            &reg,
            &ApiEntityRegistry::default(),
        )
        .expect_err("a reflected command without a constructor must be rejected");
        assert!(err.contains("not constructible"), "unexpected error: {err}");
    }

    #[test]
    fn valid_params_pass_validation() {
        let reg = test_registry();
        let registration = reg.get_with_short_type_path("TestEcho").unwrap();
        assert!(
            validate_command_params_value(
                "TestEcho",
                &api_value!({ "fail": true }),
                registration,
                &reg,
                &ApiEntityRegistry::default(),
            )
            .is_ok()
        );
    }

    #[test]
    fn absent_params_pass_validation() {
        // Unit-ish command with omitted params — no fields at all.
        // All fields default, so this is legitimately valid.
        let reg = test_registry();
        let registration = reg.get_with_short_type_path("TestEcho").unwrap();
        assert!(
            validate_command_params_value(
                "TestEcho",
                &ApiValue::Unit,
                registration,
                &reg,
                &ApiEntityRegistry::default(),
            )
            .is_ok()
        );
    }

    #[test]
    fn wrong_field_type_fails_validation() {
        let reg = test_registry();
        let registration = reg.get_with_short_type_path("TestEcho").unwrap();
        let err = validate_command_params_value(
            "TestEcho",
            &api_value!({ "fail": "not-a-bool" }),
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
        assert!(
            validate_command_params_value(
                "TestEcho",
                &api_value!({ "nope": true }),
                registration,
                &reg,
                &ApiEntityRegistry::default(),
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod id_codec_tests {
    use super::{authz_target_gid_value, globalize_command_ids_value, resolve_command_ids_value};
    use crate::registry::ApiEntityRegistry;
    use bevy::prelude::*;
    use bevy::reflect::TypeRegistry;
    use lunco_api_core::{ApiValue, api_value_from_u64};
    use lunco_core::GlobalEntityId;
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
    struct TControl {
        #[reflect(@lunco_core::SyncLocal)]
        source: Entity,
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
        reg.register::<TColl>();
        reg.register::<TInner>();
        reg.register::<TControl>();
        reg.register::<Entity>();
        reg.register::<Vec<Entity>>();
        reg.register::<Option<Entity>>();
        (reg, entities, e, gid)
    }

    #[test]
    fn typed_resolver_descends_into_vec_option_and_nested_struct() {
        let (reg, ent, e, gid) = setup();
        let gid = api_value_from_u64(gid.get());
        let mut value = ApiValue::map([
            ("many", ApiValue::Array(vec![gid.clone(), gid.clone()])),
            ("maybe", ApiValue::map([("Some", gid.clone())])),
            ("inner", ApiValue::map([("body", gid)])),
        ]);

        resolve_command_ids_value(&mut value, TypeId::of::<TColl>(), &reg, &ent)
            .expect("typed entity identities resolve");

        let local = api_value_from_u64(e.to_bits());
        assert_eq!(
            value.get("many"),
            Some(&ApiValue::Array(vec![local.clone(), local.clone()]))
        );
        assert_eq!(
            value.get("maybe"),
            Some(&ApiValue::map([("Some", local.clone())]))
        );
        assert_eq!(value.get("inner"), Some(&ApiValue::map([("body", local)])));
    }

    #[test]
    fn globalize_and_resolve_use_the_same_typed_contract() {
        let (reg, ent, e, gid) = setup();
        let mut value = ApiValue::map([
            ("source", api_value_from_u64(e.to_bits())),
            ("target", api_value_from_u64(e.to_bits())),
        ]);
        globalize_command_ids_value(&mut value, TypeId::of::<TControl>(), &reg, &ent)
            .expect("entity ids globalize");
        assert_eq!(value.get("target"), Some(&api_value_from_u64(gid.get())));
        assert_eq!(
            value.get("source"),
            Some(&api_value_from_u64(Entity::PLACEHOLDER.to_bits()))
        );
    }

    #[test]
    fn authz_target_is_reflected_from_typed_parameters() {
        let (reg, _ent, _e, gid) = setup();
        let params = ApiValue::map([("target", api_value_from_u64(gid.get()))]);
        assert_eq!(
            authz_target_gid_value(&params, TypeId::of::<TControl>(), &reg),
            Ok(Some(gid.get()))
        );
        assert_eq!(
            authz_target_gid_value(&params, TypeId::of::<TDrive>(), &reg),
            Ok(None)
        );
    }

    #[test]
    fn malformed_authorization_target_is_reported() {
        let (reg, _entities, _entity, _gid) = setup();
        let missing = ApiValue::Map(Vec::new());
        assert!(
            authz_target_gid_value(&missing, TypeId::of::<TControl>(), &reg)
                .expect_err("required target must not be treated as targetless")
                .contains("target field 'target' is missing")
        );

        let invalid = ApiValue::map([("target", ApiValue::str("not-an-id"))]);
        assert!(
            authz_target_gid_value(&invalid, TypeId::of::<TControl>(), &reg)
                .expect_err("invalid target must not be treated as targetless")
                .contains("target field 'target' must be an unsigned ID")
        );
    }
}
