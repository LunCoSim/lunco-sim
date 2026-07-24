use crate::{
    schema::{ApiRequest, ApiResponse},
    transports::envelope::{ApiRequestUnified, ApiResponseEnvelope},
    transports::HttpBridge,
};
use axum::{
    extract::{Json, State},
    http::{header, StatusCode},
    response::IntoResponse,
};

pub async fn handle_api_commands(
    State(bridge): State<HttpBridge>,
    Json(req): Json<ApiRequestUnified>,
) -> impl IntoResponse {
    let api_req: ApiRequest = req.into();
    execute_api_request(bridge, api_req).await
}

/// `GET /api/health` — liveness. Answers from the transport thread without
/// touching the world, so it stays truthful even while the app is busy: a reply
/// means the process is up and the API port is served.
pub async fn handle_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
        .into_response()
}

/// `GET /api/commands/schema` — the derived command schema (`DiscoverSchema`).
/// Same data the MCP tool list is built from; a GET so it is trivially
/// browsable and scriptable.
pub async fn handle_schema(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    execute_api_request(bridge, ApiRequest::DiscoverSchema).await
}

pub async fn execute_api_request(bridge: HttpBridge, api_req: ApiRequest) -> impl IntoResponse {
    let response = match bridge.execute(api_req).await {
        Ok(resp) => resp,
        Err(_) => ApiResponse::Error {
            code: 500,
            message: "Failed to process request".to_string(),
        },
    };

    // Screenshot responses return raw PNG bytes directly.
    if let ApiResponse::Screenshot { png_bytes } = response {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "image/png")],
            png_bytes,
        )
            .into_response();
    }

    let envelope = ApiResponseEnvelope::from(response);
    // Honour the TYPED error code. Every error used to be a 500, which threw
    // away `CommandNotFound` (400), `EntityNotFound` (404) and
    // `DeserializationError` (422) — codes `ApiErrorCode` has always carried.
    let status = match envelope.error_code {
        Some(code) => StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        None => StatusCode::OK,
    };
    (status, Json(envelope)).into_response()
}
