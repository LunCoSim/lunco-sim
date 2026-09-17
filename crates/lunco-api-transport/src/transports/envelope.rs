//! Conversion between the pure API wire envelope and the ECS API runtime.

use lunco_api::schema::{ApiRequest, ApiResponse};
use lunco_api_contracts::{ApiRequestEnvelope, ApiResponseEnvelope};

/// Decode a transport request into the API runtime's semantic request.
pub(crate) fn decode_request(envelope: ApiRequestEnvelope) -> Result<ApiRequest, String> {
    match envelope {
        ApiRequestEnvelope::ExecuteCommand { command, params } => {
            Ok(ApiRequest::ExecuteCommand { command, params })
        }
        ApiRequestEnvelope::DiscoverSchema => Ok(ApiRequest::DiscoverSchema),
        ApiRequestEnvelope::ListEntities => Ok(ApiRequest::ListEntities),
        ApiRequestEnvelope::SubscribeTelemetry { filter } => {
            let filter = filter
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| format!("invalid telemetry filter: {error}"))?;
            Ok(ApiRequest::SubscribeTelemetry { filter })
        }
        ApiRequestEnvelope::UnsubscribeTelemetry { id } => {
            Ok(ApiRequest::UnsubscribeTelemetry { id })
        }
    }
}

/// Convert an internal API response to the response envelope shared by all
/// outward transports.
pub(crate) fn encode_response(response: ApiResponse) -> ApiResponseEnvelope {
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
            error_code: Some(lunco_api::schema::ApiErrorCode::InternalError as u16),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_envelope_decodes_without_transport_details() {
        let envelope = ApiRequestEnvelope::execute_command(
            "RunRhai",
            serde_json::json!({ "code": "print(1)" }),
        );
        let ApiRequest::ExecuteCommand { command, params } = decode_request(envelope).unwrap()
        else {
            panic!("expected an execute-command request");
        };
        assert_eq!(command, "RunRhai");
        assert_eq!(params["code"], "print(1)");
    }
}
