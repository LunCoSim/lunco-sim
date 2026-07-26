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
use std::time::Duration;

/// How long the synchronous path waits for a fire-and-forget command's terminal
/// outcome before giving up and returning `status: "pending"` (the caller can
/// still poll `QueryCommandResult`). Generous: covers a normal command flush plus
/// any observer that reports a frame or two later.
const SYNC_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll cadence while waiting for the outcome. Small enough to feel synchronous,
/// large enough not to hammer the command funnel.
const SYNC_POLL_INTERVAL: Duration = Duration::from_millis(15);

pub async fn handle_api_commands(
    State(bridge): State<HttpBridge>,
    Json(req): Json<ApiRequestUnified>,
) -> impl IntoResponse {
    // Read the transport-level `async` flag off the envelope BEFORE conversion —
    // `ApiRequest` has no notion of it. Default (false) ⇒ wait for the outcome.
    let want_async = req.is_async;
    let api_req: ApiRequest = req.into();
    execute_api_request(bridge, api_req, want_async).await
}

/// Map a serialized [`lunco_core::CommandOutcome`] to a terminal status string.
/// `null` (unknown id) and `"Pending"` are NOT terminal → keep waiting.
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

/// Synchronous path: a fire-and-forget command was accepted (its `command_id` is
/// known but no data came back). Poll `QueryCommandResult` until the observer
/// records a terminal outcome, then return it inline so the HTTP reply is
/// authoritative. On timeout, return `status: "pending"` — the command was
/// accepted; a handler that never reports (the one bounded ambiguity) shouldn't
/// hang the request forever.
async fn await_command_outcome(bridge: &HttpBridge, id: u64) -> ApiResponse {
    let max_polls =
        (SYNC_WAIT_TIMEOUT.as_millis() / SYNC_POLL_INTERVAL.as_millis().max(1)).max(1);
    for _ in 0..max_polls {
        if let Ok(ApiResponse::Ok {
            data: Some(data), ..
        }) = bridge.execute(ApiRequest::QueryCommandResult { id }).await
        {
            let outcome = data.get("outcome").cloned().unwrap_or(serde_json::Value::Null);
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
            "note": "no terminal outcome recorded within the sync-wait window; \
                     poll QueryCommandResult, or resend with \"async\": true",
        })),
    }
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
    // A query provider — returns data inline, never `command_accepted` — so the
    // sync-wait path is a no-op; pass `true` to skip it.
    execute_api_request(
        bridge,
        ApiRequest::ExecuteCommand {
            command: "GetReadiness".to_string(),
            params: serde_json::json!({}),
        },
        true,
    )
    .await
}

/// `GET /api/diagnostics` — co-sim connection health. Reaches into the world for
/// the `GetBrokenConnections` query: the wiring targets that dropped their write
/// on the last propagation tick, each tagged `fault` (genuine dangling wire) vs
/// structural/still-loading. `200` with the report either way — "some broken" is
/// a valid state to report, not a request error.
pub async fn handle_diagnostics(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    // A query provider — returns data inline, never `command_accepted` — so the
    // sync-wait path is a no-op; pass `true` to skip it.
    execute_api_request(
        bridge,
        ApiRequest::ExecuteCommand {
            command: "GetBrokenConnections".to_string(),
            params: serde_json::json!({}),
        },
        true,
    )
    .await
}

/// `GET /api/commands/schema` — the derived command schema (`DiscoverSchema`).
/// Same data the MCP tool list is built from; a GET so it is trivially
/// browsable and scriptable.
pub async fn handle_schema(State(bridge): State<HttpBridge>) -> impl IntoResponse {
    // A query — it returns data directly, never `command_accepted` — so the
    // sync-wait path is a no-op here; pass `true` to skip it explicitly.
    execute_api_request(bridge, ApiRequest::DiscoverSchema, true).await
}

pub async fn execute_api_request(
    bridge: HttpBridge,
    api_req: ApiRequest,
    want_async: bool,
) -> impl IntoResponse {
    let response = match bridge.execute(api_req).await {
        Ok(resp) => resp,
        Err(_) => ApiResponse::Error {
            code: 500,
            message: "Failed to process request".to_string(),
        },
    };

    // Synchronous by default: a fire-and-forget command answers with
    // `command_accepted` (a `command_id`, no data). Unless the caller opted into
    // `async`, wait for its terminal outcome so the reply is authoritative
    // instead of merely "queued". Query providers and deferred commands already
    // return their real payload inline, so they never enter this branch.
    let response = match response {
        ApiResponse::Ok {
            command_id: Some(id),
            data: None,
        } if !want_async => await_command_outcome(&bridge, id).await,
        other => other,
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

#[cfg(test)]
mod tests {
    use super::terminal_status;
    use serde_json::{json, Value};

    #[test]
    fn terminal_status_maps_outcome_variants() {
        // Externally-tagged `CommandOutcome`: terminal variants are objects.
        assert_eq!(terminal_status(&json!({"Succeeded": {}})), Some("succeeded"));
        assert_eq!(terminal_status(&json!({"Failed": "boom"})), Some("failed"));
        assert_eq!(terminal_status(&json!({"Rejected": {}})), Some("rejected"));
        // Not terminal — keep waiting.
        assert_eq!(terminal_status(&json!("Pending")), None);
        assert_eq!(terminal_status(&Value::Null), None);
    }
}
