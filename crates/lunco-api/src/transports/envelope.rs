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
    /// The `ApiErrorCode` behind `error` (400 CommandNotFound, 404
    /// EntityNotFound, 422 DeserializationError, 500 InternalError). The HTTP
    /// transport also maps it to the status line; the wasm/JS bridge has no
    /// status line, so it reads the code from here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u16>,
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
    /// Entity / command id. Always a JSON **number** — `GlobalEntityId` is
    /// ≤53-bit so it round-trips through a JSON number without precision loss
    /// (the one and only id form; `ListEntities` emits the same).
    pub id: Option<u64>,
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
            // `QueryEntity` is a PROVIDER (owned by `lunco-scene-commands`, beside
            // `MoveEntity` — same entities, same frame), not a built-in variant.
            // The wire shape predates that and stays supported: this maps it onto
            // the provider call, so `{"type":"QueryEntity","id":…}` keeps working
            // for every existing client. No id → a non-resolving sentinel (id 0 is
            // never in the registry), as before.
            Some("QueryEntity") => ApiRequest::ExecuteCommand {
                command: "QueryEntity".to_string(),
                params: serde_json::json!({ "id": env.id.unwrap_or(0) }),
            },
            Some("QueryCommandResult") => ApiRequest::QueryCommandResult {
                id: env.id.unwrap_or(0),
            },
            Some("SubscribeTelemetry") => ApiRequest::SubscribeTelemetry {
                filter: env.filter.and_then(|v| serde_json::from_value(v).ok()),
            },
            Some("UnsubscribeTelemetry") => ApiRequest::UnsubscribeTelemetry {
                id: env.id.unwrap_or(0),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ApiRequest {
        serde_json::from_str::<ApiRequestUnified>(json).unwrap().into()
    }

    /// The legacy `{"type":"QueryEntity"}` wire shape still reaches the provider
    /// (which now lives in `lunco-scene-commands`), with the id carried through as
    /// a param. Existing clients — including the `query_entity` MCP tool — see no
    /// change.
    #[test]
    fn query_entity_maps_onto_the_provider_with_a_number_id() {
        // One id form on the wire: a JSON number (GlobalEntityId is ≤53-bit).
        match parse(r#"{"type":"QueryEntity","id":98466552102768}"#) {
            ApiRequest::ExecuteCommand { command, params } => {
                assert_eq!(command, "QueryEntity");
                assert_eq!(params["id"], 98466552102768_u64);
            }
            other => panic!("expected the QueryEntity provider call, got {other:?}"),
        }
    }

    #[test]
    fn query_entity_string_id_is_rejected() {
        // No legacy string-id form — a stringified id must NOT silently parse.
        assert!(
            serde_json::from_str::<ApiRequestUnified>(r#"{"type":"QueryEntity","id":"98466552102768"}"#)
                .is_err(),
            "string ids should be rejected; ids are numbers"
        );
    }

    #[test]
    fn query_command_result_id_is_a_number() {
        match parse(r#"{"type":"QueryCommandResult","id":42}"#) {
            ApiRequest::QueryCommandResult { id } => assert_eq!(id, 42),
            other => panic!("expected QueryCommandResult, got {other:?}"),
        }
    }
}

impl From<ApiResponse> for ApiResponseEnvelope {
    fn from(response: ApiResponse) -> Self {
        match response {
            ApiResponse::Ok { command_id, data } => ApiResponseEnvelope { command_id, data, error: None, error_code: None },
            ApiResponse::Error { code, message } => ApiResponseEnvelope { command_id: None, data: None, error: Some(message), error_code: Some(code) },
            ApiResponse::TelemetryEvent(event) => ApiResponseEnvelope { command_id: None, data: Some(serde_json::json!(event)), error: None, error_code: None },
            ApiResponse::Screenshot { .. } => ApiResponseEnvelope { command_id: None, data: None, error: Some("unexpected screenshot response".into()), error_code: Some(crate::schema::ApiErrorCode::InternalError as u16) },
        }
    }
}
