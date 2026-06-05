//! Transport-agnostic request/response envelopes.
//!
//! These wire types and their `ApiRequest`/`ApiResponse` conversions are pure
//! serde — no axum, no tokio — so they are shared by the native HTTP transport
//! and the wasm JS bridge alike. Only the axum *handlers* live in `http.rs`.

use serde::{Deserialize, Serialize};
use crate::schema::{ApiRequest, ApiResponse};

#[derive(Debug, Serialize)]
pub struct ApiResponseEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Legacy request format:
/// {"command": "...", "params": {...}}
#[derive(Debug, Deserialize)]
pub struct LegacyCommandRequest {
    pub command: String,
    pub params: Option<serde_json::Value>,
}

impl From<LegacyCommandRequest> for ApiRequest {
    fn from(req: LegacyCommandRequest) -> Self {
        ApiRequest::ExecuteCommand { command: req.command, params: req.params.unwrap_or_default() }
    }
}

/// Unified input that handles both tagged and legacy formats.
///
/// `extra` captures any fields not consumed by the named slots. When
/// the request looks like a typed command (`{"type":"OpenTwin","path":"..."}`)
/// — i.e. `type_field` names a domain command and the caller hasn't
/// supplied a `params` envelope — we promote `extra` into `params`
/// so observers see the field. Without this promotion the field is
/// silently dropped, the typed event fires with `Default::default()`,
/// and `path`-empty observers (like `OpenTwin`) silently open the
/// file picker instead of acting on the supplied path.
#[derive(Debug, Deserialize)]
pub struct ApiRequestUnified {
    #[serde(rename = "type", default)]
    pub type_field: Option<String>,
    pub command: Option<String>,
    pub params: Option<serde_json::Value>,
    pub id: Option<String>,
    pub language: Option<String>,
    pub code: Option<String>,
    pub filter: Option<serde_json::Value>,
    /// Catches every other top-level field. Used by the typed-command
    /// fallback to forward the caller's payload as `params`.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl From<ApiRequestUnified> for ApiRequest {
    fn from(env: ApiRequestUnified) -> Self {
        match env.type_field.as_deref() {
            Some("ExecuteCommand") => ApiRequest::ExecuteCommand {
                command: env.command.unwrap_or_default(),
                params: env.params.unwrap_or_default(),
            },
            Some("DiscoverSchema") => ApiRequest::DiscoverSchema,
            Some("ListEntities") => ApiRequest::ListEntities,
            Some("QueryEntity") => ApiRequest::QueryEntity {
                // No/invalid id → a non-resolving sentinel (id 0 is never in the
                // registry). Mirrors the prior behavior, where the removed
                // `Default` minted a fresh — equally non-matching — id.
                id: env
                    .id
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or(lunco_core::GlobalEntityId::from_raw(0)),
            },
            Some("QueryCommandResult") => ApiRequest::QueryCommandResult {
                id: env.id.unwrap_or_default().parse().unwrap_or(0),
            },
            Some("SubscribeTelemetry") => ApiRequest::SubscribeTelemetry {
                filter: env.filter.and_then(|v| serde_json::from_value(v).ok()),
            },
            None if env.command.is_some() => {
                // Legacy format: {"command": "...", "params": {...}}
                ApiRequest::ExecuteCommand {
                    command: env.command.unwrap_or_default(),
                    params: env.params.unwrap_or_default(),
                }
            }
            _ => {
                // Typed-command shape: `{"type":"OpenTwin","path":"..."}`.
                // The caller didn't supply a `params` envelope, so
                // promote whatever extra top-level fields they sent
                // into `params` — this is what makes the intuitive
                // shape work without forcing `{"command":"X","params":{...}}`.
                let params = if let Some(explicit) = env.params {
                    explicit
                } else {
                    let mut map = serde_json::Map::new();
                    for (k, v) in env.extra {
                        map.insert(k, v);
                    }
                    serde_json::Value::Object(map)
                };
                ApiRequest::ExecuteCommand {
                    command: env.type_field.unwrap_or_default(),
                    params,
                }
            }
        }
    }
}

impl From<ApiResponse> for ApiResponseEnvelope {
    fn from(response: ApiResponse) -> Self {
        match response {
            ApiResponse::Ok { command_id, data } => ApiResponseEnvelope { command_id, data, error: None },
            ApiResponse::Error { code: _, message } => ApiResponseEnvelope { command_id: None, data: None, error: Some(message) },
            ApiResponse::TelemetryEvent(event) => ApiResponseEnvelope { command_id: None, data: Some(serde_json::json!(event)), error: None },
            ApiResponse::Screenshot { .. } => ApiResponseEnvelope { command_id: None, data: None, error: Some("unexpected screenshot response".into()) },
        }
    }
}
