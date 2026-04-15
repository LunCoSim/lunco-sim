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

#[derive(Debug, Deserialize)]
pub struct CommandRequest {
    pub command: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct ApiResponseEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn execute_command(
    State(bridge): State<HttpBridge>,
    Json(req): Json<CommandRequest>,
) -> impl IntoResponse {
    let api_req = ApiRequest::ExecuteCommand {
        command: req.command.clone(),
        params: req.params.clone().unwrap_or_else(|| serde_json::json!({})),
    };

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
