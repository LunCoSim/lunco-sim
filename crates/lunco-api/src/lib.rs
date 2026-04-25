//! # lunco-api — Transport-Agnostic API Layer
//!
//! Exposes LunCoSim's simulation state and command system via a unified API contract.
//! All transports (HTTP, ROS2, IPC, DDS, WebSocket) map to the same `ApiRequest`/`ApiResponse`
//! types, so adding a new transport is just serialization — no simulation logic changes.
//!
//! ## Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────┐
//! │  Transports                                                    │
//! │  HTTP (axum) │ ROS2 (topics) │ IPC (pipes) │ DDS │ WebSocket  │
//! │  Each handles: wire format, connection management, auth        │
//! └────────────────────────┬───────────────────────────────────────┘
//!                          │
//!                          ▼
//! ┌────────────────────────────────────────────────────────────────┐
//! │  lunco-api-core (transport-agnostic)                           │
//! │                                                                │
//! │  ApiRegistry   — stable ULID ↔ Bevy Entity mapping             │
//! │  ApiExecutor   — ApiRequest → ECS (typed commands, Reflect)    │
//! │  ApiDiscovery  — schema introspection via TypeRegistry         │
//! │  ApiTelemetry  — telemetry subscription + broadcast            │
//! │                                                                │
//! │  ApiRequest    — ExecuteCommand, QueryEntity, MutateResource…  │
//! │  ApiResponse   — Ok, Error, TelemetryEvent                     │
//! └────────────────────────┬───────────────────────────────────────┘
//!                          │
//!                          ▼
//! ┌────────────────────────────────────────────────────────────────┐
//! │  ECS World                                                     │
//! │  Typed commands (#[derive(Command)]) · Resources               │
//! └────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Principles
//!
//! - **No hardcoded commands**: Commands are discovered via `AppTypeRegistry`
//!   reflection. Any `#[derive(Command)]` type is automatically available.
//! - **No hardcoded entity types**: Schema discovery via `AppTypeRegistry` tells
//!   clients what components and resources exist at runtime.
//! - **Transport-independent**: HTTP is one optional transport (feature-gated).
//!   The core types and executor know nothing about HTTP.
//! - **Headless-compatible**: No rendering dependencies. Runs on server-only builds.

use bevy::prelude::*;

pub mod discovery;
pub mod executor;
pub mod queries;
pub mod registry;
pub mod schema;
pub mod subscription;
pub mod transports;

// Re-export public types for convenience
pub use discovery::*;
pub use executor::*;
pub use queries::*;
pub use registry::*;
pub use schema::*;
pub use subscription::*;

/// Configuration for the API plugin.
#[derive(Debug, Clone)]
pub struct LunCoApiConfig {
    /// HTTP server configuration (None = no HTTP transport).
    #[cfg(feature = "transport-http")]
    pub http_config: Option<transports::HttpServerConfig>,
}

impl LunCoApiConfig {
    /// Create configuration by parsing CLI arguments (`--api [PORT]`).
    ///
    /// If `--api` is present without a port, it defaults to 3000.
    /// If `--api` is NOT present, returns configuration with HTTP disabled.
    pub fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut port = None;

        for i in 0..args.len() {
            if args[i] == "--api" {
                port = Some(3000);
                if i + 1 < args.len() {
                    if let Ok(p) = args[i + 1].parse::<u16>() {
                        port = Some(p);
                    }
                }
                break;
            }
        }

        Self {
            #[cfg(feature = "transport-http")]
            http_config: port.map(|p| transports::HttpServerConfig { port: p }),
        }
    }
}

impl Default for LunCoApiConfig {
    fn default() -> Self {
        Self::from_args()
    }
}

/// Main API plugin.
///
/// Registers:
/// - Entity registry (ULID ↔ Entity mapping)
/// - API executor (ApiRequest → ECS)
/// - Telemetry subscription system
/// - HTTP transport server (if enabled)
pub struct LunCoApiPlugin {
    config: LunCoApiConfig,
}

impl LunCoApiPlugin {
    /// Create a new API plugin with the given configuration.
    pub fn new(config: LunCoApiConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration.
    pub fn default() -> Self {
        Self {
            config: LunCoApiConfig::default(),
        }
    }
}

impl Plugin for LunCoApiPlugin {
    fn build(&self, app: &mut App) {
        // Core systems (always enabled)
        app.add_plugins((
            ApiEntityRegistryPlugin,
            ApiQueryRegistryPlugin,
            ApiVisibilityPlugin,
            ApiExecutorPlugin,
            ApiDiscoveryPlugin,
            ApiTelemetryPlugin,
        ));

        // HTTP transport (feature-gated)
        #[cfg(feature = "transport-http")]
        {
            use transports::{HttpBridge, spawn_server};
            use crate::{http_bridge_request_router, http_response_observer, ApiHttpResponsePending};

            let bridge = {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let bridge = HttpBridge::new(tx);
                app.insert_resource(ApiHttpBridgeReceiver(rx))
                    .init_resource::<ApiHttpResponsePending>()
                    .add_observer(http_response_observer)
                    .add_systems(Update, http_bridge_request_router);
                bridge
            };

            if let Some(config) = &self.config.http_config {
                spawn_server(config.clone(), bridge);
            }
        }
    }
}

// ── HTTP bridge (feature-gated) ───────────────────────────────────────────────

/// Receives HTTP bridge requests and injects them as ApiRequestEvent.
#[cfg(feature = "transport-http")]
#[derive(Resource)]
pub struct ApiHttpBridgeReceiver(
    tokio::sync::mpsc::UnboundedReceiver<transports::BridgeMessage>,
);

/// Pending HTTP response senders (correlation_id → oneshot).
#[cfg(feature = "transport-http")]
#[derive(Resource, Default)]
pub struct ApiHttpResponsePending(
    std::collections::HashMap<u64, tokio::sync::oneshot::Sender<schema::ApiResponse>>,
);

/// System that polls the HTTP bridge receiver and triggers API requests.
#[cfg(feature = "transport-http")]
pub fn http_bridge_request_router(
    mut receiver: ResMut<ApiHttpBridgeReceiver>,
    mut pending: ResMut<ApiHttpResponsePending>,
    mut id_counter: Local<u64>,
    mut commands: Commands,
) {
    while let Ok(msg) = receiver.0.try_recv() {
        *id_counter += 1;
        let correlation_id = *id_counter;
        pending.0.insert(correlation_id, msg.reply);
        commands.trigger(executor::ApiRequestEvent {
            request: msg.request,
            correlation_id,
        });
    }
}

/// Observer that catches ApiResponseEvent and sends HTTP responses back.
#[cfg(feature = "transport-http")]
pub fn http_response_observer(
    trigger: On<executor::ApiResponseEvent>,
    mut pending: ResMut<ApiHttpResponsePending>,
) {
    let event = trigger.event();
    if let Some(sender) = pending.0.remove(&event.correlation_id) {
        let _ = sender.send(event.response.clone());
    }
}
