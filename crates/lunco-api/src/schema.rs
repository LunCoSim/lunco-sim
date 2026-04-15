//! Transport-agnostic API request/response types.
//!
//! This module defines the core API contract that all transports (HTTP, ROS2, IPC, DDS, etc.)
//! must map to. The API layer knows nothing about HTTP — it only understands `ApiRequest`
//! and produces `ApiResponse`.
//!
//! Commands are discovered via reflection. The API scans `AppTypeRegistry` for types
//! that implement `Event + Reflect`. This means any `#[Command]` struct is automatically
//! available as an API endpoint — zero hardcoding.

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Stable entity identifier for external API clients.
///
/// Bevy `Entity` IDs are process-local and recycled. This ULID-based ID
/// provides stable, cross-process identity for API consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApiEntityId(pub Ulid);

impl ApiEntityId {
    pub fn new() -> Self { Self(Ulid::new()) }
}

impl Default for ApiEntityId {
    fn default() -> Self { Self::new() }
}

impl std::fmt::Display for ApiEntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for ApiEntityId {
    type Err = ulid::DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_str(s).map(ApiEntityId)
    }
}

/// Telemetry subscription filter.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelemetryFilter {
    pub names: Vec<String>,
    pub min_severity: Option<String>,
}

/// Transport-agnostic API request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApiRequest {
    /// Execute a typed command by name.
    /// The `command` field matches the short type name (e.g. "DriveRover").
    /// The `params` field is a JSON object with field values for the command struct.
    /// Entity fields (like `target`) accept ULID strings and are resolved automatically.
    ExecuteCommand {
        command: String,
        params: serde_json::Value,
    },
    QueryEntity { id: ApiEntityId },
    ListEntities,
    DiscoverSchema,
    SubscribeTelemetry { filter: Option<TelemetryFilter> },
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
    Ok { command_id: Option<u64>, data: Option<serde_json::Value> },
    Error { code: u16, message: String },
    TelemetryEvent(TelemetryResponse),
}

impl ApiResponse {
    pub fn ok(data: serde_json::Value) -> Self {
        Self::Ok { command_id: None, data: Some(data) }
    }
    pub fn command_accepted(command_id: u64) -> Self {
        Self::Ok { command_id: Some(command_id), data: None }
    }
    pub fn error(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self::Error { code: code as u16, message: message.into() }
    }
}

/// A telemetry event pushed to a subscriber.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryResponse {
    pub name: String,
    pub value: serde_json::Value,
    pub unit: String,
    pub timestamp: f64,
}

impl TelemetryResponse {
    pub fn from_sampled(param: &lunco_core::telemetry::SampledParameter) -> Self {
        Self {
            name: param.name.clone(),
            value: telemetry_value_to_json(&param.value),
            unit: param.unit.clone(),
            timestamp: param.timestamp,
        }
    }
    pub fn from_event(event: &lunco_core::telemetry::TelemetryEvent) -> Self {
        Self {
            name: event.name.clone(),
            value: telemetry_value_to_json(&event.data),
            unit: String::new(),
            timestamp: event.timestamp,
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
