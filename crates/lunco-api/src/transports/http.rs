use crate::{
    schema::{ApiErrorCode, ApiRequest, ApiResponse},
    transports::envelope::{ApiRequestUnified, ApiResponseEnvelope},
    transports::HttpBridge,
};
use axum::{
    extract::{Json, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use std::time::Duration;

/// How long the synchronous path waits for a fire-and-forget command's
/// terminal outcome before returning a pending response.
const SYNC_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const SYNC_POLL_INTERVAL: Duration = Duration::from_millis(15);

pub async fn handle_api_commands(
    State(bridge): State<HttpBridge>,
    Json(req): Json<ApiRequestUnified>,
) -> Response {
    let api_req: ApiRequest = match req.try_into() {
        Ok(req) => req,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ApiResponseEnvelope::from(ApiResponse::error(
                    ApiErrorCode::DeserializationError,
                    error,
                ))),
            )
                .into_response();
        }
    };
    execute_api_request(bridge, api_req).await.into_response()
}

/// Map a serialized [`lunco_core::CommandOutcome`] to a terminal status.
fn terminal_status(outcome: &serde_json::Value) -> Option<&'static str> {
    if outcome.get("Succeeded").is_some() {
        Some("succeeded")
    } else if outcome.get("Failed").is_some() {
        Some("failed")
    } else if outcome.get("Rejected").is_some() {
        Some("rejected")
    } else {
        None
    }
}

async fn await_command_outcome(bridge: &HttpBridge, id: u64) -> ApiResponse {
    let max_polls = (SYNC_WAIT_TIMEOUT.as_millis() / SYNC_POLL_INTERVAL.as_millis().max(1)).max(1);
    for _ in 0..max_polls {
        if let Ok(ApiResponse::Ok {
            data: Some(data), ..
        }) = bridge.execute(ApiRequest::QueryCommandResult { id }).await
        {
            let outcome = data
                .get("outcome")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            if let Some(status) = terminal_status(&outcome) {
                return ApiResponse::Ok {
                    command_id: Some(id),
                    data: Some(serde_json::json!({
                        "command_id": id,
                        "status": status,
                        "outcome": outcome,
                    })),
                };
            }
        }
        tokio::time::sleep(SYNC_POLL_INTERVAL).await;
    }
    ApiResponse::Ok {
        command_id: Some(id),
        data: Some(serde_json::json!({
            "command_id": id,
            "status": "pending",
            "note": "no terminal outcome recorded within the sync-wait window; the command was accepted — poll QueryCommandResult for its outcome",
        })),
    }
}

/// `GET /api/health` — liveness.
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

/// `GET /api/ready` — readiness.
pub async fn handle_ready(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    execute_api_request(
        bridge,
        ApiRequest::ExecuteCommand {
            command: "GetReadiness".to_string(),
            params: serde_json::json!({}),
        },
    )
    .await
}

/// `GET /api/diagnostics` — co-sim connection health.
pub async fn handle_diagnostics(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    execute_api_request(
        bridge,
        ApiRequest::ExecuteCommand {
            command: "GetBrokenConnections".to_string(),
            params: serde_json::json!({}),
        },
    )
    .await
}

/// `GET /api/commands/schema` — the derived command schema.
pub async fn handle_schema(State(bridge): State<HttpBridge>) -> impl IntoResponse {
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

    // Fire-and-forget commands are acknowledged with an id, then waited on for
    // a bounded interval so callers receive a useful terminal status whenever
    // the handler reports one promptly.
    let response = match response {
        ApiResponse::Ok {
            command_id: Some(id),
            data: None,
        } => await_command_outcome(&bridge, id).await,
        other => other,
    };

    if let ApiResponse::Screenshot { png_bytes } = response {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "image/png")],
            png_bytes,
        )
            .into_response();
    }

    let envelope = ApiResponseEnvelope::from(response);
    let status = match envelope.error_code {
        Some(code) => StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        None => StatusCode::OK,
    };
    (status, Json(envelope)).into_response()
}

#[cfg(test)]
mod tests {
    use super::terminal_status;
    use serde_json::{json, Value};

    #[test]
    fn terminal_status_maps_outcome_variants() {
        assert_eq!(
            terminal_status(&json!({"Succeeded": {}})),
            Some("succeeded")
        );
        assert_eq!(terminal_status(&json!({"Failed": "boom"})), Some("failed"));
        assert_eq!(terminal_status(&json!({"Rejected": {}})), Some("rejected"));
        assert_eq!(terminal_status(&json!("Pending")), None);
        assert_eq!(terminal_status(&Value::Null), None);
    }
}
