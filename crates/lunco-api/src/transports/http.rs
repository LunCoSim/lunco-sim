use axum::{
    extract::{Json, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use crate::{
    transports::HttpBridge,
    transports::envelope::{ApiRequestUnified, ApiResponseEnvelope},
    schema::{ApiRequest, ApiResponse},
};

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
