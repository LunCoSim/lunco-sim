use crate::{
    schema::{ApiRequest, ApiResponse},
    transports::envelope::{ApiRequestUnified, ApiResponseEnvelope},
    transports::HttpBridge,
};
use axum::{
    extract::{Json, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

pub async fn handle_api_commands(
    State(bridge): State<HttpBridge>,
    Json(req): Json<ApiRequestUnified>,
) -> Response {
    let api_req: ApiRequest = match req.try_into() {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ApiResponseEnvelope::from(ApiResponse::error(
                    crate::schema::ApiErrorCode::DeserializationError,
                    error,
                ))),
            )
                .into_response();
        }
    };
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

/// `GET /api/ready` — readiness. Unlike `/api/health` (liveness, no world
/// access), this reaches into the world for the `ReadinessRegistry` state via the
/// `GetReadiness` query provider: `ready` is true only when nothing is holding on
/// a scene load / program compile / participant init. Answers `200` with the
/// structured status either way — "not ready" is a valid state, not an error.
pub async fn handle_ready(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    // A query provider returns data inline through the same command channel.
    execute_api_request(
        bridge,
        ApiRequest::ExecuteCommand {
            command: "GetReadiness".to_string(),
            params: serde_json::json!({}),
        },
    )
    .await
}

/// `GET /api/diagnostics` — co-sim connection health. Reaches into the world for
/// the `GetBrokenConnections` query: the wiring targets that dropped their write
/// on the last propagation tick, each tagged `fault` (genuine dangling wire) vs
/// structural/still-loading. `200` with the report either way — "some broken" is
/// a valid state to report, not a request error.
pub async fn handle_diagnostics(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    // A query provider returns data inline through the same command channel.
    execute_api_request(
        bridge,
        ApiRequest::ExecuteCommand {
            command: "GetBrokenConnections".to_string(),
            params: serde_json::json!({}),
        },
    )
    .await
}

/// `GET /api/commands/schema` — the derived command schema (`DiscoverSchema`).
/// Same data the MCP tool list is built from; a GET so it is trivially
/// browsable and scriptable.
pub async fn handle_schema(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    // Schema discovery returns data directly through the same response channel.
    execute_api_request(bridge, ApiRequest::DiscoverSchema).await
}

pub async fn execute_api_request(bridge: HttpBridge, api_req: ApiRequest) -> Response {
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
