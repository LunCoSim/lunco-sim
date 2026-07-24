//! Transport-agnostic API request/response types.
//!
//! This module defines the core API contract that all transports (HTTP, ROS2, IPC, DDS, etc.)
//! must map to. The API layer knows nothing about HTTP — it only understands `ApiRequest`
//! and produces `ApiResponse`.
//!
//! Commands are discovered via reflection. The API scans `AppTypeRegistry` for types
//! that implement `Event + Reflect`. This means any `#[Command]` struct is automatically
//! available as an API endpoint — zero hardcoding.

pub use lunco_core::GlobalEntityId as ApiEntityId;
use serde::{Deserialize, Serialize};

/// Telemetry subscription filter.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelemetryFilter {
    pub names: Vec<String>,
    pub min_severity: Option<String>,
    /// Cap the delivery rate for *this subscription*, in samples per second of simulation
    /// time. `None` ⇒ every sample the channel produces.
    ///
    /// Independent of the channel's own `rate_hz`: a 60 Hz channel can feed a 1 Hz
    /// dashboard without the dashboard drowning, and without slowing the channel down for
    /// everyone else.
    ///
    /// **Caveat, stated honestly:** telemetry is delivered as ONE shared stream to all
    /// connected clients, not fanned out per subscriber. So the effective decimation is
    /// the *fastest* rate any matching subscriber asked for — a slow subscriber does not
    /// throttle a fast one, and cannot. Per-subscriber fan-out would need a routed
    /// transport; until then this is a stream-level cap, not a private one.
    pub rate_hz: Option<f64>,
}

/// Transport-agnostic API request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApiRequest {
    /// Execute a typed command by name.
    /// The `command` field matches the short type name (e.g. "SetPorts").
    /// The `params` field is a JSON object with field values for the command struct.
    /// Entity fields (like `target`) take a numeric `api_id` (as returned by
    /// `ListEntities`) and are resolved to the live entity automatically.
    ExecuteCommand {
        command: String,
        params: serde_json::Value,
    },
    // NO `QueryEntity` variant. Reading an entity's pose means knowing which
    // coordinate frame it is in, and that belongs to the crate that owns the
    // scene verbs, not to the transport: it now lives beside `MoveEntity` in
    // `lunco-scene-commands` as an `ApiQueryProvider`, reporting the same
    // grid-absolute frame that command accepts. As a built-in it read
    // `GlobalTransform` — the render frame — and reported a position that shifted
    // with the floating origin and could not be fed back. The `{"type":
    // "QueryEntity"}` wire shape is unchanged; the envelope maps it to the
    // provider.
    ListEntities,
    DiscoverSchema,
    SubscribeTelemetry {
        filter: Option<TelemetryFilter>,
    },
    /// Cancel a subscription created by [`ApiRequest::SubscribeTelemetry`].
    ///
    /// `TelemetrySubscriptions::unsubscribe` existed from the start but **nothing could
    /// reach it** — every subscription leaked for the life of the process, and a client
    /// that reconnected accumulated a new one each time.
    UnsubscribeTelemetry {
        id: u64,
    },
    /// Poll the outcome of a previously-accepted command by its
    /// `command_id` (the request id returned in `command_accepted`).
    QueryCommandResult {
        id: u64,
    },
}

/// Response status codes for API errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiErrorCode {
    EntityNotFound = 404,
    CommandNotFound = 400,
    DeserializationError = 422,
    InternalError = 500,
}

/// Transport-agnostic API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApiResponse {
    Ok {
        command_id: Option<u64>,
        data: Option<serde_json::Value>,
    },
    Error {
        code: u16,
        message: String,
    },
    TelemetryEvent(TelemetryResponse),
    /// Raw screenshot PNG bytes — returned directly by the HTTP transport.
    #[serde(skip)]
    Screenshot {
        png_bytes: Vec<u8>,
    },
}

impl ApiResponse {
    pub fn ok(data: serde_json::Value) -> Self {
        Self::Ok {
            command_id: None,
            data: Some(data),
        }
    }
    pub fn command_accepted(command_id: u64) -> Self {
        Self::Ok {
            command_id: Some(command_id),
            data: None,
        }
    }
    pub fn error(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code: code as u16,
            message: message.into(),
        }
    }
}

/// A telemetry event pushed to a subscriber.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryResponse {
    pub name: String,
    pub value: serde_json::Value,
    pub unit: String,
    /// Absolute TDB epoch (Julian Date) — for wall-clock labelling and ephemeris
    /// correlation. **Not a Δt timebase**: at JD magnitudes an `f64` has ~86 µs of
    /// resolution left, so differencing two of these destroys the precision. Use
    /// [`sim_secs`](Self::sim_secs).
    pub timestamp: f64,
    /// Seconds on the sample's own time domain — starts near zero, keeps full `f64`
    /// precision. **This is the field to plot against and to difference.** `None` for
    /// discrete `TelemetryEvent`s, which are not sampled on a clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_secs: Option<f64>,
    /// The `api_id` of the entity that owns the channel. Parameter names are **not**
    /// unique — two rovers both report `"motor_current"` — so a subscriber needs this
    /// to tell them apart. `None` when the source entity has no global id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<u64>,
}

impl TelemetryResponse {
    /// `source` is the sampling entity's `GlobalEntityId`, resolved by the caller (the
    /// observer has the world; this type does not).
    pub fn from_sampled(
        param: &lunco_core::telemetry::SampledParameter,
        source: Option<u64>,
    ) -> Self {
        Self {
            name: param.name.clone(),
            value: telemetry_value_to_json(&param.value),
            unit: param.unit.clone(),
            timestamp: param.timestamp,
            sim_secs: Some(param.sim_secs),
            source,
        }
    }
    pub fn from_event(event: &lunco_core::telemetry::TelemetryEvent) -> Self {
        Self {
            name: event.name.clone(),
            value: telemetry_value_to_json(&event.data),
            unit: String::new(),
            timestamp: event.timestamp,
            // A discrete event isn't sampled on a clock and has no domain time.
            sim_secs: None,
            // `TelemetryEvent` already carries its emitter as a gid; 0 = "no entity".
            source: (event.source != 0).then_some(event.source),
        }
    }
}

fn telemetry_value_to_json(value: &lunco_core::TelemetryValue) -> serde_json::Value {
    match value {
        lunco_core::TelemetryValue::F64(v) => serde_json::json!(*v),
        lunco_core::TelemetryValue::I64(v) => serde_json::json!(*v),
        lunco_core::TelemetryValue::Bool(v) => serde_json::json!(*v),
        lunco_core::TelemetryValue::String(v) => serde_json::json!(v),
    }
}

/// API schema — discovered capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSchema {
    pub commands: Vec<CommandSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSchema {
    pub name: String,
    pub fields: Vec<FieldSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: String,
    pub type_name: String,
}

/// Metadata about a spawn catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntryInfo {
    pub id: String,
    pub name: String,
    pub category: String,
}
