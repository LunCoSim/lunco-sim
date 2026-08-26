//! API query providers — extension point for domain crates to expose
//! read endpoints without `lunco-api` taking direct dependencies on their
//! domains.
//!
//! ## Why
//!
//! `lunco-api` already has built-in query variants (`ListEntities` and
//! `DiscoverSchema`) that read ECS state and return JSON
//! synchronously. Adding bundled-model / Twin / MSL listing the same way
//! would require `lunco-api` to depend on `lunco-modelica` and
//! `lunco-workspace` — a layering inversion (those crates already depend
//! on `lunco-api` for the executor plugin).
//!
//! Instead, domain crates register an [`ApiQueryProvider`] at startup.
//! When an `ExecuteCommand` request arrives whose `command` matches a
//! registered provider name, the executor calls the provider with
//! `&mut World` access and returns its `ApiResponse` to the transport.
//! Reflect-registered commands are the mutation and operation channel. Query
//! providers are deliberately read-only; this keeps one authoritative command
//! contract for every state change.
//!
//! ## Provider semantics
//!
//! - **Returns data**, unlike ordinary Reflect Event commands which return an
//!   acknowledgement. Use this trait when the caller needs a structured
//!   response.
//! - **Has `&mut World` access** — providers can read any resource and
//!   run any query they need.
//! - **Runs deferred** via `Commands::queue`, so providers execute on a
//!   later command flush, not synchronously inside the observer. This
//!   matches how `CaptureScreenshot` already works.
//!
//! ## Example
//!
//! ```ignore
//! struct ListBundledProvider;
//! impl ApiQueryProvider for ListBundledProvider {
//!     fn name(&self) -> &'static str { "ListBundled" }
//!     fn execute(&self, _world: &mut World, _params: &serde_json::Value) -> ApiResponse {
//!         let bundled = lunco_modelica::bundled_models();
//!         ApiResponse::ok(serde_json::json!({ "bundled": bundled }))
//!     }
//! }
//!
//! // In a domain crate's plugin build:
//! app.world_mut()
//!     .resource_mut::<ApiQueryRegistry>()
//!     .register(ListBundledProvider);
//! ```

use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use crate::registry::ApiEntityRegistry;
use crate::schema::ApiResponse;

/// One read-only structured provider — answers a typed request with data.
///
/// See module docs for the design rationale.
pub trait ApiQueryProvider: Send + Sync + 'static {
    /// Stable name matched against the `command` field of incoming
    /// `ExecuteCommand` requests. Convention: PascalCase verb-prefixed,
    /// e.g. `"ListBundled"`, `"MslStatus"`, `"ListOpenDocuments"`.
    fn name(&self) -> &'static str;

    /// Run the query against the ECS world. Returning an
    /// [`ApiResponse::Error`] is the right move when params don't
    /// validate or required state is missing.
    ///
    /// Providers MUST NOT block for long — the caller is waiting on a
    /// deferred HTTP response. Cap any blocking work at a few hundred
    /// milliseconds and prefer returning a "not ready yet" response over
    /// blocking on a background task.
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse;
}

/// Registry of named read-only providers. Domain crates push impls here at
/// startup via [`Self::register`]; the executor consults it when an
/// `ExecuteCommand` request arrives.
///
/// Stored as Bevy `Resource` so domain plugins can mutate it during
/// `App::build`.
#[derive(Resource, Default)]
pub struct ApiQueryRegistry {
    providers: HashMap<String, Arc<dyn ApiQueryProvider>>,
}

impl ApiQueryRegistry {
    /// Register a read-only provider.
    ///
    /// Provider names are public API identifiers. A duplicate is a startup
    /// configuration error, not an override point, so registration fails
    /// visibly instead of depending on plugin order.
    pub fn register<P: ApiQueryProvider>(&mut self, provider: P) {
        let name = provider.name();
        if self.providers.contains_key(name) {
            panic!("duplicate API query provider registration: {name}");
        }
        self.providers.insert(name.to_string(), Arc::new(provider));
    }

    /// Look up a provider by name. Returns an `Arc` so the caller can
    /// drop the registry borrow before invoking `execute` (which needs
    /// `&mut World`).
    pub fn get(&self, name: &str) -> Option<Arc<dyn ApiQueryProvider>> {
        self.providers.get(name).cloned()
    }

    /// Names of every registered provider. Useful for debug-dumping the
    /// available query surface.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }
}

/// Plugin that adds the [`ApiQueryRegistry`] resource. Always installed
/// by [`crate::LunCoApiPlugin`]; domain crates do not need to add this
/// plugin themselves — they just mutate the registry.
pub struct ApiQueryRegistryPlugin;

impl Plugin for ApiQueryRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiQueryRegistry>();
    }
}

use crate::schema::ApiErrorCode;
/// `ReadPorts` — every exposed port on an entity (model I/O, physics velocity,
/// sensors, joints), by `api_id`. A one-shot read of the same `PortRegistry`
/// backends the telemetry stream samples — the direct alternative to subscribing.
/// params: `{ api_id: u64 }` · returns: `{ api_id, ports: [{ name, value, direction }] }`
pub struct ReadPortsProvider;
impl ApiQueryProvider for ReadPortsProvider {
    fn name(&self) -> &'static str {
        "ReadPorts"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(api_id) = params.get("api_id").and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        }) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ReadPorts: `api_id` (u64) required".to_string(),
            );
        };
        let gid = lunco_core::GlobalEntityId::from_raw(api_id);
        let Some(entity) = world.resource::<ApiEntityRegistry>().resolve(&gid) else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("ReadPorts: no entity for api_id {api_id}"),
            );
        };
        // `PortRegistry` is `Clone` (a Vec of `'static` backends), so clone it out
        // to release the immutable world borrow before `entity_ports` reborrows
        // `&World` to read component values.
        let Some(registry) = world
            .get_resource::<lunco_core::ports::PortRegistry>()
            .cloned()
        else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "ReadPorts: PortRegistry not present (no cosim plugin)".to_string(),
            );
        };
        let ports = registry.entity_ports(world, entity);
        let arr: Vec<_> = ports
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "value": p.value,
                    "direction": format!("{:?}", p.direction),
                })
            })
            .collect();
        ApiResponse::ok(serde_json::json!({ "api_id": api_id, "ports": arr }))
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

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
        use lunco_readiness::{ReadinessRegistry, ReadinessState, Subject};
        let registry = world.get_resource::<ReadinessRegistry>();
        let fault = world
            .get_resource::<lunco_core::RuntimeFaults>()
            .and_then(|faults| faults.first.as_ref());
        let world_hold = world
            .get_resource::<ReadinessState>()
            .is_some_and(|s| s.world_hold);
        let pending: Vec<serde_json::Value> = registry
            .map(|r| {
                r.pending()
                    .map(|item| {
                        let subject = match item.subject {
                            Subject::World => serde_json::json!("world"),
                            // Entity bits are stable within the session; the
                            // richer `api_id` isn't worth a second registry lookup
                            // for a transient wait.
                            Subject::Entity(e) => serde_json::json!({ "entity_bits": e.to_bits() }),
                        };
                        serde_json::json!({
                            "kind": item.kind,
                            "subject": subject,
                            "label": item.label,
                            "elapsed_s": item.elapsed_s,
                            "action": item.action.name(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ready = registry.is_some() && pending.is_empty() && !world_hold && fault.is_none();
        ApiResponse::ok(serde_json::json!({
            "ready": ready,
            "world_hold": world_hold,
            "faulted": fault.is_some(),
            "fault": fault.map(|fault| serde_json::json!({
                "kind": fault.kind,
                "subject": fault.subject,
                "detail": fault.detail,
            })),
            "readiness_tracked": registry.is_some(),
            "pending_count": pending.len(),
            "pending": pending,
        }))
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

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let surface_filter = match params.get("surface") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(name)) => Some(name.as_str()),
            Some(_) => {
                return ApiResponse::error(
                    ApiErrorCode::DeserializationError,
                    "ReadExposures: `surface` must be a string",
                )
            }
        };

        let Some(exposures) = world.get_resource::<lunco_core::exposure::EngineExposures>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "ReadExposures: EngineExposures resource is not present",
            );
        };

        let surfaces = exposures
            .surfaces
            .iter()
            .filter(|(name, _)| surface_filter.is_none_or(|filter| (*name).as_str() == filter))
            .map(|(name, surface)| {
                let properties = surface
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), exposure_value_to_json(value)))
                    .collect::<serde_json::Map<_, _>>();
                (
                    name.clone(),
                    serde_json::json!({
                        "visible": surface.visible,
                        "properties": properties,
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();

        ApiResponse::ok(serde_json::json!({
            "revision": exposures.revision,
            "surfaces": surfaces,
        }))
    }
}

fn exposure_value_to_json(value: &lunco_core::exposure::ExposureValue) -> serde_json::Value {
    match value {
        lunco_core::exposure::ExposureValue::Text(value) => serde_json::json!(value),
        lunco_core::exposure::ExposureValue::Bool(value) => serde_json::json!(*value),
        lunco_core::exposure::ExposureValue::Number(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
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

/// Plugin that adds the [`ApiVisibility`] resource. Always installed by
/// [`crate::LunCoApiPlugin`].
pub struct ApiVisibilityPlugin;

impl Plugin for ApiVisibilityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiVisibility>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::exposure::EngineExposures;

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

        let response = ReadExposuresProvider.execute(&mut world, &serde_json::json!({}));
        let ApiResponse::Ok {
            data: Some(data), ..
        } = response
        else {
            panic!("ReadExposures did not return data");
        };
        assert_eq!(data["revision"], revision);
        assert_eq!(data["surfaces"]["hud"]["visible"], true);
        assert_eq!(data["surfaces"]["hud"]["properties"]["label"], "Rover");
        assert_eq!(data["surfaces"]["hud"]["properties"]["speed"], 1.5);
        assert_eq!(data["surfaces"]["hud"]["properties"]["active"], true);
    }

    #[test]
    fn read_exposures_can_filter_one_surface() {
        let mut world = World::new();
        let mut exposures = EngineExposures::default();
        exposures.writer("hud").visible(true);
        exposures.writer("telemetry").visible(true);
        world.insert_resource(exposures);

        let response =
            ReadExposuresProvider.execute(&mut world, &serde_json::json!({ "surface": "hud" }));
        let ApiResponse::Ok {
            data: Some(data), ..
        } = response
        else {
            panic!("ReadExposures did not return data");
        };
        assert!(data["surfaces"].get("hud").is_some());
        assert!(data["surfaces"].get("telemetry").is_none());
    }

    #[test]
    fn builtin_queries_register_the_exposure_reader() {
        let mut registry = ApiQueryRegistry::default();
        register_builtin_queries(&mut registry);
        assert!(registry.get("ReadExposures").is_some());
    }
}
