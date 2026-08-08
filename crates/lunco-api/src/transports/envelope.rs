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

/// The single request envelope accepted by every API transport.
///
/// A command is always explicitly tagged as `ExecuteCommand`; domain queries
/// use the same command channel and are registered by the crate that owns the
/// queried data. Keeping the wire discriminator closed makes malformed or
/// stale envelopes fail at the transport boundary instead of being promoted
/// into a different command with default parameters.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ApiRequestUnified {
    ExecuteCommand {
        command: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    DiscoverSchema,
    ListEntities,
    SubscribeTelemetry {
        filter: Option<serde_json::Value>,
    },
    UnsubscribeTelemetry {
        /// Entity ids are JSON numbers on the wire. `GlobalEntityId` is
        /// bounded to the exact-integer range of JSON numbers.
        id: u64,
    },
}

impl TryFrom<ApiRequestUnified> for ApiRequest {
    type Error = String;

    fn try_from(env: ApiRequestUnified) -> Result<Self, Self::Error> {
        match env {
            ApiRequestUnified::ExecuteCommand { command, params } => {
                Ok(ApiRequest::ExecuteCommand { command, params })
            }
            ApiRequestUnified::DiscoverSchema => Ok(ApiRequest::DiscoverSchema),
            ApiRequestUnified::ListEntities => Ok(ApiRequest::ListEntities),
            ApiRequestUnified::SubscribeTelemetry { filter } => {
                let filter = filter
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|error| format!("invalid telemetry filter: {error}"))?;
                Ok(ApiRequest::SubscribeTelemetry { filter })
            }
            ApiRequestUnified::UnsubscribeTelemetry { id } => {
                Ok(ApiRequest::UnsubscribeTelemetry { id })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<ApiRequest, String> {
        let envelope =
            serde_json::from_str::<ApiRequestUnified>(json).map_err(|error| error.to_string())?;
        envelope.try_into()
    }

    #[test]
    fn canonical_provider_query_uses_the_execute_command_envelope() {
        match parse(
            r#"{"type":"ExecuteCommand","command":"ReadPorts","params":{"api_id":98466552102768}}"#,
        )
        .unwrap()
        {
            ApiRequest::ExecuteCommand { command, params } => {
                assert_eq!(command, "ReadPorts");
                assert_eq!(params["api_id"], 98466552102768_u64);
            }
            other => panic!("expected the provider query call, got {other:?}"),
        }
    }

    #[test]
    fn command_params_are_left_for_typed_validation() {
        assert!(parse(
            r#"{"type":"ExecuteCommand","command":"ReadPorts","params":{"api_id":"98466552102768"}}"#,
        )
        .is_ok());

        // The envelope carries JSON params; command/provider validation is
        // responsible for rejecting a string where the provider expects u64.
        let request = parse(
            r#"{"type":"ExecuteCommand","command":"UnsubscribeTelemetry","params":{"id":"42"}}"#,
        )
        .unwrap();
        assert!(matches!(request, ApiRequest::ExecuteCommand { .. }));
    }

    #[test]
    fn untagged_command_forms_are_rejected() {
        assert!(parse(r#"{"command":"SetCamera","params":{"eye":[1,2,3]}}"#).is_err());
        assert!(parse(r#"{"type":"Query","id":42}"#).is_err());
        assert!(parse(r#"{"type":"QueryCommandResult","id":42}"#).is_err());
        assert!(
            parse(r#"{"type":"ExecuteCommand","command":"SetCamera","eye":[1,2,3]}"#,).is_err()
        );
    }

    #[test]
    fn response_envelope_contains_ack_data_without_a_command_id() {
        let envelope = ApiResponseEnvelope::from(ApiResponse::accepted());
        assert_eq!(envelope.data.unwrap()["accepted"], true);
    }
}

impl From<ApiResponse> for ApiResponseEnvelope {
    fn from(response: ApiResponse) -> Self {
        match response {
            ApiResponse::Ok { data } => ApiResponseEnvelope {
                data,
                error: None,
                error_code: None,
            },
            ApiResponse::Error { code, message } => ApiResponseEnvelope {
                data: None,
                error: Some(message),
                error_code: Some(code),
            },
            ApiResponse::TelemetryEvent(event) => ApiResponseEnvelope {
                data: Some(serde_json::json!(event)),
                error: None,
                error_code: None,
            },
            ApiResponse::Screenshot { .. } => ApiResponseEnvelope {
                data: None,
                error: Some("unexpected screenshot response".into()),
                error_code: Some(crate::schema::ApiErrorCode::InternalError as u16),
            },
        }
    }
}
