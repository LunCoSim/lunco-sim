//! HTTP transport adapter — maps axum routes to ApiRequest/ApiResponse.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};
use crate::schema::{ApiEntityId, ApiRequest, ApiResponse};

#[derive(Debug, Clone)]
pub struct HttpServerConfig { pub port: u16 }
impl Default for HttpServerConfig { fn default() -> Self { Self { port: 3000 } } }

pub struct BridgeMessage {
    pub request: ApiRequest,
    pub reply: oneshot::Sender<ApiResponse>,
}

#[derive(Clone)]
pub struct HttpBridge { tx: mpsc::UnboundedSender<BridgeMessage> }

impl HttpBridge {
    pub fn new(tx: mpsc::UnboundedSender<BridgeMessage>) -> Self { Self { tx } }
    pub async fn execute(&self, request: ApiRequest) -> Result<ApiResponse, StatusCode> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(BridgeMessage { request, reply: reply_tx })
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        reply_rx.await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// Request body for the command endpoint.
/// 
/// Example:
/// ```json
/// {
///   "command": "DriveRover",
///   "params": { "target": "01ARZ7NDEKTSV4M9", "forward": 0.8, "steer": 0.0 }
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct CommandRequest {
    /// Command type name (e.g. "DriveRover", "SpawnEntity").
    pub command: String,
    /// JSON object with field values. Entity fields accept ULID strings.
    #[serde(default = "default_params")]
    pub params: serde_json::Value,
}

fn default_params() -> serde_json::Value { serde_json::json!({}) }

#[derive(Debug, serde::Serialize)]
pub struct ApiResponseEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<ApiResponse> for (StatusCode, Json<ApiResponseEnvelope>) {
    fn from(response: ApiResponse) -> Self {
        let envelope = match response {
            ApiResponse::Ok { command_id, data } => ApiResponseEnvelope { command_id, data, error: None },
            ApiResponse::Error { code, message } => ApiResponseEnvelope { command_id: None, data: None, error: Some(message) },
            ApiResponse::TelemetryEvent(event) => ApiResponseEnvelope { command_id: None, data: Some(serde_json::json!(event)), error: None },
        };
        
        let status = if envelope.error.is_some() { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::OK };
        (status, Json(envelope))
    }
}

pub async fn execute_command(
    State(bridge): State<HttpBridge>,
    Json(req): Json<CommandRequest>,
) -> impl IntoResponse {
    eprintln!("[lunco-api] HTTP handler received: command='{}', params={:?}", req.command, req.params);
    let api_req = ApiRequest::ExecuteCommand {
        command: req.command.clone(),
        params: req.params.clone(),
    };
    let response = bridge.execute(api_req).await.unwrap_or_else(|status| {
        ApiResponse::Error { code: status.as_u16(), message: "Failed to process request".to_string() }
    });
    eprintln!("[lunco-api] HTTP handler returning: {:?}", response);
    let result: (StatusCode, Json<ApiResponseEnvelope>) = response.into();
    result
}

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok", "service": "lunco-api" })))
}

pub async fn discover_schema(
    State(bridge): State<HttpBridge>,
) -> impl IntoResponse {
    let api_req = ApiRequest::DiscoverSchema;
    let response = bridge.execute(api_req).await.unwrap_or_else(|status| {
        ApiResponse::Error { code: status.as_u16(), message: "Failed to process request".to_string() }
    });
    let result: (StatusCode, Json<ApiResponseEnvelope>) = response.into();
    result
}

pub async fn list_entities(
    State(bridge): State<HttpBridge>,
) -> impl IntoResponse {
    let api_req = ApiRequest::ListEntities;
    let response = bridge.execute(api_req).await.unwrap_or_else(|status| {
        ApiResponse::Error { code: status.as_u16(), message: "Failed to process request".to_string() }
    });
    let result: (StatusCode, Json<ApiResponseEnvelope>) = response.into();
    result
}

pub async fn get_entity(
    State(bridge): State<HttpBridge>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let entity_id: ApiEntityId = match id.parse() {
        Ok(id) => id,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponseEnvelope {
            command_id: None, data: None, error: Some(format!("Invalid entity ID: {}", e)),
        })),
    };
    let api_req = ApiRequest::QueryEntity { id: entity_id };
    let response = bridge.execute(api_req).await.unwrap_or_else(|status| {
        ApiResponse::Error { code: status.as_u16(), message: "Failed to process request".to_string() }
    });
    let result: (StatusCode, Json<ApiResponseEnvelope>) = response.into();
    result
}

pub fn build_router(bridge: HttpBridge) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/commands", post(execute_command))
        .route("/api/commands/schema", get(discover_schema))
        .route("/api/entities", get(list_entities))
        .route("/api/entities/{id}", get(get_entity))
        .with_state(bridge)
}

pub fn spawn_server(config: HttpServerConfig, bridge: HttpBridge) {
    let addr = format!("127.0.0.1:{}", config.port);
    let router = build_router(bridge);

    std::thread::Builder::new()
        .name("lunco-api-http".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => { eprintln!("[lunco-api] Failed to create tokio runtime: {}", e); return; }
            };
            rt.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(&addr).await {
                    Ok(l) => l,
                    Err(e) => { eprintln!("[lunco-api] Failed to bind HTTP on {}: {}", addr, e); return; }
                };
                eprintln!("[lunco-api] HTTP server listening on http://{}", addr);
                if let Err(e) = axum::serve(listener, router).await {
                    eprintln!("[lunco-api] HTTP server error: {}", e);
                }
            });
        })
        .expect("Failed to spawn API server thread");
}
