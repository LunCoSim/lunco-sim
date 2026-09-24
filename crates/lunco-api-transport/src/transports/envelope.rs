//! Conversion between the pure API wire envelope and the ECS API runtime.

use lunco_api_codec::{value_from_json, value_to_json};
use lunco_api_contracts::{ApiRequestEnvelope, ApiResponseEnvelope};
use lunco_api_core::{ApiErrorCode, ApiRequest, ApiResponse, api_value};

/// Decode a transport request into the API runtime's semantic request.
pub(crate) fn decode_request(envelope: ApiRequestEnvelope) -> Result<ApiRequest, String> {
    match envelope {
        ApiRequestEnvelope::ExecuteCommand { command, params } => {
            let params = value_from_json(&params)?;
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
    let result = match response {
        ApiResponse::Ok { data } => {
            data.as_ref()
                .map(value_to_json)
                .transpose()
                .map(|data| ApiResponseEnvelope {
                    data,
                    error: None,
                    error_code: None,
                })
        }
        ApiResponse::Error { code, message } => Ok(ApiResponseEnvelope {
            data: None,
            error: Some(message),
            error_code: Some(code),
        }),
        ApiResponse::TelemetryEvent(event) => value_to_json(&api_value!({
            "name": event.name,
            "value": event.value,
            "unit": event.unit,
            "timestamp": event.timestamp,
            "sim_secs": event.sim_secs,
            "sim_tick": event.sim_tick,
            "source": event.source,
        }))
        .map(|data| ApiResponseEnvelope {
            data: Some(data),
            error: None,
            error_code: None,
        }),
        ApiResponse::Screenshot { .. } => Err("unexpected screenshot response".to_string()),
    };

    result.unwrap_or_else(|message| ApiResponseEnvelope {
        data: None,
        error: Some(format!("API response cannot be encoded: {message}")),
        error_code: Some(ApiErrorCode::InternalError as u16),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_api_core::ApiValue;

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
        assert_eq!(
            params.get("code").and_then(ApiValue::as_str),
            Some("print(1)")
        );
    }
}
