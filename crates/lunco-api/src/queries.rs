use crate::registry::ApiEntityRegistry;
use bevy::prelude::*;
use lunco_api_core::{
    api_value_from_serializable, api_value_from_u64, ApiErrorCode, ApiResponse, ApiValue,
    ApiValueError, IntoApiValue,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Typed result from a read-only API query provider.
pub type ApiQueryResult = Result<Option<ApiValue>, ApiQueryError>;

/// A provider rejection or failure before external serialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiQueryError {
    pub code: ApiErrorCode,
    pub message: String,
}

impl ApiQueryError {
    /// Create a typed provider error.
    pub fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<ApiValueError> for ApiQueryError {
    fn from(error: ApiValueError) -> Self {
        Self::new(ApiErrorCode::InternalError, error.to_string())
    }
}

/// Read-only structured provider for one named API query.
pub trait ApiQueryProvider: Send + Sync + 'static {
    /// Stable name matched against the command field of `ExecuteCommand`.
    fn name(&self) -> &'static str;

    /// Execute against an immutable ECS world using typed parameters.
    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult;
}

/// Registry of named read-only providers.
#[derive(Resource, Default)]
pub struct ApiQueryRegistry {
    providers: HashMap<String, Arc<dyn ApiQueryProvider>>,
}

impl ApiQueryRegistry {
    /// Register a provider; duplicate public names are startup errors.
    pub fn register<P: ApiQueryProvider>(&mut self, provider: P) {
        let name = provider.name();
        assert!(
            !self.providers.contains_key(name),
            "duplicate API query provider registration: {name}"
        );
        self.providers.insert(name.to_owned(), Arc::new(provider));
    }

    /// Look up a provider by its public name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ApiQueryProvider>> {
        self.providers.get(name).cloned()
    }

    /// Iterate the registered public names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }
}

/// Execute a provider for an in-process caller.
pub fn execute_query_value(world: &World, name: &str, params: &ApiValue) -> ApiQueryResult {
    let provider = world
        .get_resource::<ApiQueryRegistry>()
        .and_then(|registry| registry.get(name))
        .ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::CommandNotFound,
                format!("query '{name}' is not registered"),
            )
        })?;
    provider.execute(world, params)
}

/// Adapt a provider result to the typed API response channel.
pub fn execute_query_response(
    provider: &dyn ApiQueryProvider,
    world: &World,
    params: &ApiValue,
) -> ApiResponse {
    match provider.execute(world, params) {
        Ok(value) => ApiResponse::Ok { data: value },
        Err(error) => ApiResponse::error(error.code, error.message),
    }
}

/// Read a required unsigned integer parameter.
pub fn api_param_u64(params: &ApiValue, name: &str) -> Option<u64> {
    params
        .get(name)
        .and_then(ApiValue::as_i64)
        .and_then(|value| u64::try_from(value).ok())
}

/// Read an unsigned integer parameter that also accepts decimal text.
pub fn api_param_u64_or_string(params: &ApiValue, name: &str) -> Option<u64> {
    match params.get(name)? {
        ApiValue::Int(value) => u64::try_from(*value).ok(),
        ApiValue::Str(value) => value.parse().ok(),
        _ => None,
    }
}

/// Read a floating-point parameter from either signed integers or floats.
pub fn api_param_f64(params: &ApiValue, name: &str) -> Option<f64> {
    params.get(name).and_then(ApiValue::as_f64)
}

/// Read a string parameter without coercing another value type.
pub fn api_param_str<'a>(params: &'a ApiValue, name: &str) -> Option<&'a str> {
    params.get(name).and_then(ApiValue::as_str)
}

/// Read a boolean parameter without coercing an integer.
pub fn api_param_bool(params: &ApiValue, name: &str) -> Option<bool> {
    match params.get(name)? {
        ApiValue::Bool(value) => Some(*value),
        _ => None,
    }
}

/// Read an array parameter.
pub fn api_param_array<'a>(params: &'a ApiValue, name: &str) -> Option<&'a [ApiValue]> {
    match params.get(name)? {
        ApiValue::Array(values) => Some(values),
        _ => None,
    }
}

/// Plugin that adds the [`ApiQueryRegistry`] resource. The API transport
/// plugin installs it; domain crates do not need to add this plugin themselves
/// — they just mutate the registry.
pub struct ApiQueryRegistryPlugin;

impl Plugin for ApiQueryRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiQueryRegistry>();
    }
}

/// `ReadPorts` — every exposed port on an entity (model I/O, physics velocity,
/// sensors, joints), by `api_id`. A one-shot read of the same `PortRegistry`
/// backends the telemetry stream samples — the direct alternative to subscribing.
/// params: `{ api_id: u64 }` · returns:
/// `{ api_id, ports: [{ name, value, direction, metadata }] }`
pub struct ReadPortsProvider;

fn port_info_to_api_value(
    port: &lunco_port_core::ports::PortInfo,
) -> Result<ApiValue, ApiValueError> {
    let range = match (port.metadata.min, port.metadata.max) {
        (Some(min), Some(max)) => {
            ApiValue::map([("min", ApiValue::Float(min)), ("max", ApiValue::Float(max))])
        }
        (Some(min), None) => ApiValue::map([("min", ApiValue::Float(min))]),
        (None, Some(max)) => ApiValue::map([("max", ApiValue::Float(max))]),
        (None, None) => ApiValue::Unit,
    };
    Ok(ApiValue::map([
        ("name", ApiValue::str(port.name.clone())),
        ("value", api_value_from_serializable(&port.value)?),
        (
            "direction",
            ApiValue::str(match port.direction {
                lunco_port_core::ports::PortDirection::In => "in",
                lunco_port_core::ports::PortDirection::Out => "out",
                lunco_port_core::ports::PortDirection::InOut => "inout",
            }),
        ),
        (
            "metadata",
            ApiValue::map([
                ("type", ApiValue::str(port.metadata.value_type)),
                ("unit", port.metadata.unit.clone().into_api_value()),
                ("range", range),
                ("source", ApiValue::str(port.metadata.source.clone())),
                ("authority", ApiValue::str(port.metadata.authority.clone())),
                ("writable", ApiValue::Bool(port.metadata.writable)),
            ]),
        ),
    ]))
}

impl ApiQueryProvider for ReadPortsProvider {
    fn name(&self) -> &'static str {
        "ReadPorts"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(api_id) = api_param_u64_or_string(params, "api_id") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ReadPorts: `api_id` (u64) required",
            ));
        };
        let gid = lunco_core::GlobalEntityId::from_raw(api_id);
        let Some(entity) = world.resource::<ApiEntityRegistry>().resolve(&gid) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("ReadPorts: no entity for api_id {api_id}"),
            ));
        };
        // `PortRegistry` is `Clone` (a Vec of `'static` backends), so clone it out
        // to release the immutable world borrow before `entity_ports` reborrows
        // `&World` to read component values.
        let Some(registry) = world
            .get_resource::<lunco_port_core::ports::PortRegistry>()
            .cloned()
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ReadPorts: PortRegistry not present (no cosim plugin)".to_string(),
            ));
        };
        let ports = registry.entity_port_infos(world, entity);
        let ports = ports
            .into_iter()
            .map(|port| port_info_to_api_value(&port))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(ApiValue::map([
            ("api_id", api_value_from_u64(api_id)),
            ("ports", ApiValue::Array(ports)),
        ])))
    }
}

/// `GetReadiness` — backs `GET /api/ready`. Reports whether the world is holding
/// on any not-yet-satisfied readiness wait (scene load, program compile,
/// participant init) and enumerates what is still pending.
///
/// Truthful by construction: it reports exactly what the [`ReadinessRegistry`]
/// tracks and nothing it doesn't. It does NOT invent asset/camera/port readiness
/// signals the substrate can't vouch for — a false "ready" is the failure mode
/// the interaction report calls out, so an untracked host reports
/// `ready: false, readiness_tracked: false` rather than a hopeful `true`.
///
/// params: none · returns:
/// `{ ready, world_hold, faulted, fault: {kind, subject, detail} | null,
///    readiness_tracked, pending_count, pending: [{kind, subject, label, elapsed_s, action}] }`
pub struct ReadinessProvider;
impl ApiQueryProvider for ReadinessProvider {
    fn name(&self) -> &'static str {
        "GetReadiness"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        use lunco_readiness::{ReadinessRegistry, ReadinessState, Subject};
        let registry = world.get_resource::<ReadinessRegistry>();
        let fault = world
            .get_resource::<lunco_core::RuntimeFaults>()
            .and_then(|faults| faults.first.as_ref());
        let world_hold = world
            .get_resource::<ReadinessState>()
            .is_some_and(|s| s.world_hold);
        let pending: Vec<ApiValue> = registry
            .map(|r| {
                r.pending()
                    .map(|item| {
                        let subject = match item.subject {
                            Subject::World => ApiValue::str("world"),
                            // Entity bits are stable within the session; the
                            // richer `api_id` isn't worth a second registry lookup
                            // for a transient wait.
                            Subject::Entity(e) => {
                                ApiValue::map([("entity_bits", api_value_from_u64(e.to_bits()))])
                            }
                        };
                        ApiValue::map([
                            ("kind", ApiValue::str(item.kind)),
                            ("subject", subject),
                            ("label", ApiValue::str(item.label.clone())),
                            ("elapsed_s", ApiValue::Float(item.elapsed_s)),
                            ("action", ApiValue::str(item.action.name())),
                        ])
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ready = registry.is_some() && pending.is_empty() && !world_hold && fault.is_none();
        let fault = fault.map(|fault| {
            ApiValue::map([
                ("kind", ApiValue::str(fault.kind)),
                ("subject", ApiValue::str(fault.subject.clone())),
                ("detail", ApiValue::str(fault.detail.clone())),
            ])
        });
        Ok(Some(ApiValue::map([
            ("ready", ApiValue::Bool(ready)),
            ("world_hold", ApiValue::Bool(world_hold)),
            ("faulted", ApiValue::Bool(fault.is_some())),
            ("fault", fault.into_api_value()),
            ("readiness_tracked", ApiValue::Bool(registry.is_some())),
            ("pending_count", ApiValue::Int(pending.len() as i64)),
            ("pending", ApiValue::Array(pending)),
        ])))
    }
}

/// `ReadExposures` — reads the generic engine capability snapshot consumed by
/// runtime UI surfaces, egui, telemetry tools, and remote clients.
///
/// params: `{ surface?: string }` · returns:
/// `{ revision, surfaces: { <name>: { visible, properties: { <key>: value } } } }`
///
/// `revision` is the change-detection boundary owned by `EngineExposures`. A
/// client can poll this query and skip rebuilding its view when the revision is
/// unchanged. The optional filter avoids serializing unrelated surfaces for a
/// narrow consumer while retaining one generic API contract.
pub struct ReadExposuresProvider;
impl ApiQueryProvider for ReadExposuresProvider {
    fn name(&self) -> &'static str {
        "ReadExposures"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let surface_filter = match params.get("surface") {
            None | Some(ApiValue::Unit) => None,
            Some(ApiValue::Str(name)) => Some(name.as_str()),
            Some(_) => {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "ReadExposures: `surface` must be a string",
                ));
            }
        };

        let Some(exposures) = world.get_resource::<lunco_exposure_core::EngineExposures>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ReadExposures: EngineExposures resource is not present",
            ));
        };

        let surfaces = exposures
            .surfaces
            .iter()
            .filter(|(name, _)| surface_filter.is_none_or(|filter| (*name).as_str() == filter))
            .map(|(name, surface)| {
                let properties = surface
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), exposure_value_to_api_value(value)))
                    .collect::<Vec<_>>();
                (
                    name.clone(),
                    ApiValue::map([
                        ("visible", ApiValue::Bool(surface.visible)),
                        ("properties", ApiValue::Map(properties)),
                    ]),
                )
            })
            .collect::<Vec<_>>();

        Ok(Some(ApiValue::map([
            ("revision", api_value_from_u64(exposures.revision)),
            ("surfaces", ApiValue::Map(surfaces)),
        ])))
    }
}

fn exposure_value_to_api_value(value: &lunco_exposure_core::ExposureValue) -> ApiValue {
    match value {
        lunco_exposure_core::ExposureValue::Text(value) => ApiValue::str(value.clone()),
        lunco_exposure_core::ExposureValue::Bool(value) => ApiValue::Bool(*value),
        lunco_exposure_core::ExposureValue::Number(value) if value.is_finite() => {
            ApiValue::Float(*value)
        }
        lunco_exposure_core::ExposureValue::Number(_) => ApiValue::Unit,
        lunco_exposure_core::ExposureValue::Array(values) => {
            ApiValue::Array(values.iter().map(exposure_value_to_api_value).collect())
        }
        lunco_exposure_core::ExposureValue::Map(values) => ApiValue::Map(
            values
                .iter()
                .map(|(key, value)| (key.clone(), exposure_value_to_api_value(value)))
                .collect(),
        ),
    }
}

pub fn register_builtin_queries(registry: &mut ApiQueryRegistry) {
    // Not spatial, but built-in and transform/physics-agnostic (it only reads the
    // `PortRegistry`), so it registers here with the other always-available queries.
    registry.register(ReadPortsProvider);
    // Readiness status — backs `GET /api/ready`. Always available; degrades to
    // `readiness_tracked: false` when the readiness substrate isn't installed.
    registry.register(ReadinessProvider);
    registry.register(ReadExposuresProvider);
}

// ─── ApiVisibility ─────────────────────────────────────────────────────

/// Filter for which Reflect-registered commands are exposed via the
/// external API surface (HTTP transport, MCP `discover_schema`, etc.)
/// while keeping them fully reflectable, observable, and dispatchable
/// **within the app**.
///
/// ## Why a separate filter
///
/// The Bevy `AppTypeRegistry` is the single source of truth for
/// reflected types — every domain plugin's GUI panel, observer, and
/// (per AGENTS.md §4.1) UI command bindings rely on registration. We
/// can't gate sensitive surfaces by *not registering* them: that breaks
/// the in-app dispatch path the GUI itself uses.
///
/// Instead, registration stays unconditional and domain crates push
/// command names that should be hidden from external callers into
/// [`hidden_commands`]. The discovery and executor layers consult this
/// set:
///
/// - [`crate::discover_commands`] omits hidden names from
///   [`crate::ApiSchema`].
/// - The executor rejects hidden commands with
///   [`crate::ApiErrorCode::CommandNotFound`] — the same error a
///   typo'd command name produces, so the surface looks identical to
///   "the command does not exist" from outside.
///
/// ## Default policy
///
/// Empty by default — every Reflect-registered command is visible.
/// Domain crates that ship internal-by-default mutation surfaces add
/// their command names in their plugin `build`. CLI flags or other
/// runtime knobs can clear entries to opt those surfaces in.
///
/// Mutating this resource **after** the API server has started works —
/// future calls observe the new visibility — so a future
/// "live toggle from a privileged channel" feature is reachable
/// without re-architecting the gate.
#[derive(Resource, Default, Debug)]
pub struct ApiVisibility {
    /// Set of Reflect command short names that should be invisible to
    /// external API consumers. The name is the short type path
    /// (`"SetDocumentSource"`), matching what
    /// [`crate::ApiRequest::ExecuteCommand`]'s `command` field carries.
    pub hidden_commands: std::collections::HashSet<String>,
}

impl ApiVisibility {
    /// Hide a command from external API surface. Idempotent.
    pub fn hide(&mut self, name: impl Into<String>) {
        self.hidden_commands.insert(name.into());
    }

    /// Reveal a previously-hidden command. Idempotent — no-op if the
    /// name was never hidden.
    pub fn reveal(&mut self, name: &str) {
        self.hidden_commands.remove(name);
    }

    /// True when the command is hidden from external callers.
    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden_commands.contains(name)
    }
}

/// Plugin that adds the [`ApiVisibility`] resource. The API transport plugin
/// installs it.
pub struct ApiVisibilityPlugin;

impl Plugin for ApiVisibilityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiVisibility>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_exposure_core::EngineExposures;

    #[test]
    fn read_exposures_returns_revision_and_typed_properties() {
        let mut world = World::new();
        let mut exposures = EngineExposures::default();
        {
            let mut surface = exposures.writer("hud");
            surface.visible(true);
            surface.property("label", "Rover");
            surface.property("speed", 1.5_f64);
            surface.property("active", true);
        }
        let revision = exposures.revision;
        world.insert_resource(exposures);

        let data = ReadExposuresProvider
            .execute(&world, &ApiValue::Unit)
            .expect("ReadExposures query succeeds")
            .expect("ReadExposures returns data");
        assert_eq!(data.get("revision"), Some(&api_value_from_u64(revision)));
        let surface = data.get("surfaces").and_then(|value| value.get("hud"));
        assert_eq!(
            surface.and_then(|value| value.get("visible")),
            Some(&ApiValue::Bool(true))
        );
        let properties = surface.and_then(|value| value.get("properties"));
        assert_eq!(
            properties.and_then(|value| value.get("label")),
            Some(&ApiValue::str("Rover"))
        );
        assert_eq!(
            properties.and_then(|value| value.get("speed")),
            Some(&ApiValue::Float(1.5))
        );
        assert_eq!(
            properties.and_then(|value| value.get("active")),
            Some(&ApiValue::Bool(true))
        );
    }

    #[test]
    fn read_exposures_can_filter_one_surface() {
        let mut world = World::new();
        let mut exposures = EngineExposures::default();
        exposures.writer("hud").visible(true);
        exposures.writer("telemetry").visible(true);
        world.insert_resource(exposures);

        let params = ApiValue::map([("surface", ApiValue::str("hud"))]);
        let data = ReadExposuresProvider
            .execute(&world, &params)
            .expect("ReadExposures query succeeds")
            .expect("ReadExposures returns data");
        let surfaces = data.get("surfaces").expect("surfaces map is present");
        assert!(surfaces.get("hud").is_some());
        assert!(surfaces.get("telemetry").is_none());
    }

    #[test]
    fn builtin_queries_register_the_exposure_reader() {
        let mut registry = ApiQueryRegistry::default();
        register_builtin_queries(&mut registry);
        assert!(registry.get("ReadExposures").is_some());
    }

    #[test]
    fn port_projection_preserves_owner_metadata_as_typed_values() {
        let port = lunco_port_core::ports::PortInfo {
            name: "throttle".into(),
            direction: lunco_port_core::ports::PortDirection::In,
            value: 0.5,
            metadata: lunco_port_core::ports::PortMetadata::scalar(
                lunco_port_core::ports::PortDirection::In,
                Some("m/s"),
                Some(-1.0),
                Some(1.0),
                "control surface",
                "operator",
                true,
            ),
        };

        let value = port_info_to_api_value(&port).expect("port projects to an API value");
        let metadata = value.get("metadata").expect("metadata is present");
        let range = metadata.get("range").expect("range is present");
        assert_eq!(
            value.get("direction").and_then(ApiValue::as_str),
            Some("in")
        );
        assert_eq!(
            metadata.get("type").and_then(ApiValue::as_str),
            Some("scalar")
        );
        assert_eq!(metadata.get("unit").and_then(ApiValue::as_str), Some("m/s"));
        assert_eq!(range.get("min").and_then(ApiValue::as_f64), Some(-1.0));
        assert_eq!(range.get("max").and_then(ApiValue::as_f64), Some(1.0));
        assert_eq!(
            metadata.get("source").and_then(ApiValue::as_str),
            Some("control surface")
        );
        assert_eq!(
            metadata.get("authority").and_then(ApiValue::as_str),
            Some("operator")
        );
        assert_eq!(
            metadata.get("writable").and_then(ApiValue::as_bool),
            Some(true)
        );
    }
}
