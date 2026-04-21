use axum::{
    extract::{Json, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use crate::{
    transports::HttpBridge,
    schema::{ApiRequest, ApiResponse},
};

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
                id: env.id.unwrap_or_default().parse().unwrap_or_default(),
            },
            Some("ExecuteScript") => ApiRequest::ExecuteScript {
                language: env.language.unwrap_or_default(),
                code: env.code.unwrap_or_default(),
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
            _ => ApiRequest::ExecuteCommand {
                command: env.type_field.unwrap_or_default(),
                params: serde_json::json!({}),
            },
        }
    }
}

pub async fn handle_api_commands(
    State(bridge): State<HttpBridge>,
    Json(req): Json<ApiRequestUnified>,
) -> impl IntoResponse {
    let api_req: ApiRequest = req.into();
    execute_api_request(bridge, api_req).await
}

pub async fn execute_api_request(
    bridge: HttpBridge,
    api_req: ApiRequest,
) -> impl IntoResponse {
    let response = match bridge.execute(api_req).await {
        Ok(resp) => resp,
        Err(_) => ApiResponse::Error { code: 500, message: "Failed to process request".to_string() },
    };

    // Screenshot responses return raw PNG bytes directly.
    if let ApiResponse::Screenshot { png_bytes } = response {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "image/png")],
            png_bytes,
        ).into_response();
    }

    let envelope = ApiResponseEnvelope::from(response);
    let status = if envelope.error.is_some() { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::OK };
    (status, Json(envelope)).into_response()
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
