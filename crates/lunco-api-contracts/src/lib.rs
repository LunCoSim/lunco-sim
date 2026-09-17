//! Pure LunCoSim API wire contracts.
//!
//! This package deliberately contains no ECS, Bevy, server, or scripting
//! runtime. It is the single owner of the JSON envelope accepted by every API
//! transport and consumed by every API client. The runtime API maps these
//! envelopes to its in-process request/response types; clients never need to
//! depend on that Bevy-backed runtime package.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// Default loopback port of the command API.
pub const DEFAULT_API_PORT: u16 = 4101;

/// Path of the command API endpoint relative to an API base URL.
pub const COMMANDS_PATH: &str = "/api/commands";

/// JSON request envelope accepted by the command API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ApiRequestEnvelope {
    /// Execute a reflected command or read-only provider by name.
    ExecuteCommand {
        /// Reflected command/provider identifier.
        command: String,
        /// Parameters validated by the command/provider owner.
        #[serde(default)]
        params: serde_json::Value,
    },
    /// Discover commands, queries, and hooks exposed by the running host.
    DiscoverSchema,
    /// List runtime entities visible through the API.
    ListEntities,
    /// Subscribe to telemetry matching a filter.
    SubscribeTelemetry {
        /// Optional telemetry filter owned by the API runtime.
        filter: Option<serde_json::Value>,
    },
    /// Cancel a telemetry subscription.
    UnsubscribeTelemetry {
        /// Subscription identifier.
        id: u64,
    },
}

impl ApiRequestEnvelope {
    /// Construct the common command/provider request envelope.
    pub fn execute_command(command: impl Into<String>, params: serde_json::Value) -> Self {
        Self::ExecuteCommand {
            command: command.into(),
            params,
        }
    }
}

/// JSON response envelope returned by the command API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiResponseEnvelope {
    /// Successful command/provider data, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Human-readable error returned by the owning API operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Numeric API error code when the runtime rejected the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u16>,
}

impl ApiResponseEnvelope {
    /// Returns whether the envelope represents a successful operation.
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_envelope_round_trips_as_the_public_wire_shape() {
        let request = ApiRequestEnvelope::execute_command(
            "RunRhai",
            serde_json::json!({ "code": "print(1)" }),
        );
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ApiRequestEnvelope>(&json).unwrap(),
            request
        );
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["params"]["code"], "print(1)");
    }

    #[test]
    fn malformed_or_untagged_requests_are_rejected_at_the_wire_boundary() {
        assert!(
            serde_json::from_str::<ApiRequestEnvelope>(
                r#"{"command":"RunRhai","params":{"code":"print(1)"}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<ApiRequestEnvelope>(
                r#"{"type":"ExecuteCommand","command":"RunRhai","code":"print(1)"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn response_envelope_distinguishes_success_and_failure() {
        let success = ApiResponseEnvelope {
            data: Some(serde_json::json!({ "stdout": "ok" })),
            error: None,
            error_code: None,
        };
        assert!(success.is_ok());
        let failure = ApiResponseEnvelope {
            data: None,
            error: Some("rejected".into()),
            error_code: Some(409),
        };
        assert!(!failure.is_ok());
    }
}
