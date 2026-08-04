//! Transport-agnostic request/response envelopes.
//!
//! These wire types and their `ApiRequest`/`ApiResponse` conversions are pure
//! serde — no axum, no tokio — so they are shared by the native HTTP transport
//! and the wasm JS bridge alike. Only the axum *handlers* live in `http.rs`.

use crate::schema::{ApiRequest, ApiResponse};
use serde::{Deserialize, Serialize};

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

/// Canonical `/api/commands` request envelope.
///
/// Every command and query uses the same shape:
/// `{"command":"Name","params":{...}}`. Query-specific arguments live
/// inside `params` as well, so transports do not carry parallel wire formats.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiRequestUnified {
    pub command: String,
    #[serde(default = "default_params")]
    pub params: serde_json::Value,
}

impl TryFrom<ApiRequestUnified> for ApiRequest {
    type Error = String;

    fn try_from(env: ApiRequestUnified) -> Result<Self, Self::Error> {
        match env.command.as_str() {
            "DiscoverSchema" => Ok(ApiRequest::DiscoverSchema),
            "ListEntities" => Ok(ApiRequest::ListEntities),
            // `QueryEntity` is a PROVIDER (owned by `lunco-scene-commands`, beside
            // `MoveEntity` — same entities, same frame), not a built-in variant.
            "QueryEntity" => {
                require_numeric_param(&env.params, "id")?;
                Ok(ApiRequest::ExecuteCommand {
                    command: "QueryEntity".to_string(),
                    params: env.params,
                })
            }
            "QueryCommandResult" => Ok(ApiRequest::QueryCommandResult {
                id: require_numeric_param(&env.params, "id")?,
            }),
            "SubscribeTelemetry" => {
                let filter = match env.params.get("filter") {
                    None => None,
                    Some(value) => Some(
                        serde_json::from_value(value.clone())
                            .map_err(|e| format!("invalid telemetry filter: {e}"))?,
                    ),
                };
                Ok(ApiRequest::SubscribeTelemetry { filter })
            }
            "UnsubscribeTelemetry" => Ok(ApiRequest::UnsubscribeTelemetry {
                id: require_numeric_param(&env.params, "id")?,
            }),
            _ => Ok(ApiRequest::ExecuteCommand {
                command: env.command,
                params: env.params,
            }),
        }
    }
}

fn require_numeric_param(params: &serde_json::Value, name: &str) -> Result<u64, String> {
    params
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("parameter `{name}` must be a JSON number"))
}

fn default_params() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ApiRequest {
        serde_json::from_str::<ApiRequestUnified>(json)
            .unwrap()
            .try_into()
            .unwrap()
    }

    #[test]
    fn query_entity_uses_the_canonical_command_envelope() {
        match parse(r#"{"command":"QueryEntity","params":{"id":98466552102768}}"#) {
            ApiRequest::ExecuteCommand { command, params } => {
                assert_eq!(command, "QueryEntity");
                assert_eq!(params["id"], 98466552102768_u64);
            }
            other => panic!("expected the QueryEntity provider call, got {other:?}"),
        }
    }

    #[test]
    fn query_entity_string_id_is_rejected() {
        let env = serde_json::from_str::<ApiRequestUnified>(
            r#"{"command":"QueryEntity","params":{"id":"98466552102768"}}"#,
        )
        .unwrap();
        assert!(ApiRequest::try_from(env).is_err());
    }

    #[test]
    fn command_params_are_forwarded_verbatim() {
        match parse(r#"{"command":"SetCamera","params":{"eye":[1.0,2.0,3.0]}}"#) {
            ApiRequest::ExecuteCommand { command, params } => {
                assert_eq!(command, "SetCamera");
                assert_eq!(params["eye"], serde_json::json!([1.0, 2.0, 3.0]));
            }
            other => panic!("expected ExecuteCommand, got {other:?}"),
        }
    }

    #[test]
    fn missing_params_use_an_empty_object() {
        match parse(r#"{"command":"SetCamera"}"#) {
            ApiRequest::ExecuteCommand { command, params } => {
                assert_eq!(command, "SetCamera");
                assert_eq!(params, serde_json::json!({}));
            }
            other => panic!("expected ExecuteCommand, got {other:?}"),
        }
    }

    #[test]
    fn query_command_result_id_is_a_number() {
        match parse(r#"{"command":"QueryCommandResult","params":{"id":42}}"#) {
            ApiRequest::QueryCommandResult { id } => assert_eq!(id, 42),
            other => panic!("expected QueryCommandResult, got {other:?}"),
        }
    }
}

impl From<ApiResponse> for ApiResponseEnvelope {
    fn from(response: ApiResponse) -> Self {
        match response {
            ApiResponse::Ok { command_id, data } => ApiResponseEnvelope {
                command_id,
                data,
                error: None,
                error_code: None,
            },
            ApiResponse::Error { code, message } => ApiResponseEnvelope {
                command_id: None,
                data: None,
                error: Some(message),
                error_code: Some(code),
            },
            ApiResponse::TelemetryEvent(event) => ApiResponseEnvelope {
                command_id: None,
                data: Some(serde_json::json!(event)),
                error: None,
                error_code: None,
            },
            ApiResponse::Screenshot { .. } => ApiResponseEnvelope {
                command_id: None,
                data: None,
                error: Some("unexpected screenshot response".into()),
                error_code: Some(crate::schema::ApiErrorCode::InternalError as u16),
            },
        }
    }
}
