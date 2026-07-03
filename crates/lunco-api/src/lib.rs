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
    /// If `--api` is present without a port, it defaults to
    /// [`DEFAULT_API_PORT`](lunco_core::session::DEFAULT_API_PORT).
    /// If `--api` is NOT present, returns configuration with HTTP disabled.
    pub fn from_args() -> Self {
        // The CLI port only matters when an outward HTTP transport is compiled
        // in; without `transport-http` the config carries no fields, so parsing
        // args here would be dead work (and an unused `port`).
        #[cfg(feature = "transport-http")]
        let http_config = {
            let args: Vec<String> = std::env::args().collect();
            let mut port = None;

            for i in 0..args.len() {
                if args[i] == "--api" {
                    port = Some(lunco_core::session::DEFAULT_API_PORT);
                    if i + 1 < args.len() {
                        if let Ok(p) = args[i + 1].parse::<u16>() {
                            port = Some(p);
                        }
                    }
                    break;
                }
            }

            port.map(|p| transports::HttpServerConfig { port: p })
        };

        Self {
            #[cfg(feature = "transport-http")]
            http_config,
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
    /// HTTP transport config — only present when `transport-http` is compiled
    /// in. Without it `LunCoApiConfig` has no fields and the plugin registers
    /// only ECS-side plumbing, so there is nothing to store.
    #[cfg(feature = "transport-http")]
    config: LunCoApiConfig,
}

impl LunCoApiPlugin {
    /// Create a new API plugin with the given configuration.
    pub fn new(config: LunCoApiConfig) -> Self {
        // Without an HTTP transport the config carries nothing actionable.
        #[cfg(not(feature = "transport-http"))]
        let _ = config;
        Self {
            #[cfg(feature = "transport-http")]
            config,
        }
    }

    /// Create with default configuration.
    pub fn default() -> Self {
        Self {
            #[cfg(feature = "transport-http")]
            config: LunCoApiConfig::default(),
        }
    }
}

/// Add `plugin` only if a plugin of the same type isn't already present.
/// `Plugin` is unique by default and a duplicate `add_plugins` panics, so this
/// keeps the transport-free core composable across `LunCoApiPlugin` and
/// `LunCoScriptingPlugin` (which both want it) regardless of add order.
pub fn add_plugin_once<P: Plugin>(app: &mut App, plugin: P) {
    if !app.is_plugin_added::<P>() {
        app.add_plugins(plugin);
    }
}

/// Ensure the **transport-free command core** — the reflect-based command
/// dispatcher ([`ApiExecutorPlugin`]) and entity-id registry
/// ([`ApiEntityRegistryPlugin`]) — is present, without pulling any transport
/// (no HTTP server, no `LunCoApiPlugin`). This is the seam that lets the
/// scripting substrate run `cmd()` **independently of the API**: an app can add
/// `LunCoScriptingPlugin` alone and scripts still dispatch every `#[Command]`.
/// Idempotent — safe to call from both plugins.
pub fn ensure_command_core(app: &mut App) {
    add_plugin_once::<ApiExecutorPlugin>(app, ApiExecutorPlugin);
    add_plugin_once::<ApiEntityRegistryPlugin>(app, ApiEntityRegistryPlugin);
}

impl Plugin for LunCoApiPlugin {
    fn build(&self, app: &mut App) {
        // Transport-free command core (always enabled). Added via guarded helpers
        // so it COMPOSES with `LunCoScriptingPlugin`, which now self-supplies the
        // same core (`ensure_command_core`) to stay independent of this HTTP-API
        // plugin — either may be added first, and neither double-adds. Plain
        // `add_plugins` panics on a duplicate, hence the `is_plugin_added` guards.
        ensure_command_core(app);
        add_plugin_once::<ApiQueryRegistryPlugin>(app, ApiQueryRegistryPlugin);
        add_plugin_once::<ApiVisibilityPlugin>(app, ApiVisibilityPlugin);
        add_plugin_once::<ApiDiscoveryPlugin>(app, ApiDiscoveryPlugin);
        add_plugin_once::<ApiTelemetryPlugin>(app, ApiTelemetryPlugin);

        // Built-in transform-only spatial query providers (Nearest,
        // EntitiesInRadius) — reachable over the API and via the scripting
        // `query()` verb. Physics-backed providers (Raycast) register the same
        // way from their owning crate.
        queries::register_builtin_spatial_queries(
            &mut app.world_mut().resource_mut::<queries::ApiQueryRegistry>(),
        );

        // The networking wire (codec, capture/apply, snapshots) lives in the
        // optional `lunco-networking` crate and is registered by its plugin only
        // when the `networking` feature is on. This crate stays transport- and
        // networking-agnostic.

        // Bridge transport (feature-gated). The ECS-side plumbing — receiver,
        // router system, response observer — is identical whether the outward
        // transport is the native HTTP server or the in-browser JS bridge.
        // Only the final hand-off differs: `spawn_server` (TcpListener) vs.
        // `set_wasm_bridge` (the `window.lunco_api` export).
        #[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
        {
            use transports::HttpBridge;
            use crate::{http_bridge_request_router, http_response_observer, ApiHttpResponsePending};

            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            #[allow(unused_mut)]
            let mut bridge = HttpBridge::new(tx);

            // Hook the winit event loop so requests wake the app immediately
            // instead of waiting for the next reactive tick. Native + windowed
            // only (the `winit` feature): the wasm build runs a continuous
            // requestAnimationFrame loop and a headless server ticks via
            // ScheduleRunnerPlugin, so neither needs (or has) the waker.
            #[cfg(all(feature = "transport-http", feature = "winit"))]
            if let Some(proxy) = app.world().get_resource::<bevy::winit::EventLoopProxyWrapper>() {
                let proxy = (**proxy).clone();
                bridge = bridge.with_waker(std::sync::Arc::new(move || {
                    // Ignored by design: a send error means the winit event loop
                    // has already exited (app shutting down), so there is nothing
                    // left to wake — warning here would just spam at teardown.
                    let _ = proxy.send_event(bevy::winit::WinitUserEvent::WakeUp);
                }));
            }

            app.insert_resource(ApiHttpBridgeReceiver(rx))
                .init_resource::<ApiHttpResponsePending>()
                .add_observer(http_response_observer)
                .add_systems(Update, http_bridge_request_router);

            // Native: spawn the blocking TcpListener HTTP server. wasm has no
            // `spawn_server` (axum/tokio-net are native-only) — the browser uses
            // the JS bridge below instead, so skip the call there even if
            // `transport-http` happens to be enabled in the feature set.
            #[cfg(all(feature = "transport-http", not(target_arch = "wasm32")))]
            if let Some(config) = &self.config.http_config {
                transports::spawn_server(config.clone(), bridge.clone());
            }

            // Wasm: register the bridge behind the `window.lunco_api` JS export.
            #[cfg(target_arch = "wasm32")]
            transports::set_wasm_bridge(bridge.clone());

            // Consumed by whichever transport is active above; the clones keep
            // this block valid across every feature combination.
            let _ = bridge;
        }
    }
}

// ── HTTP bridge (feature-gated) ───────────────────────────────────────────────

/// Receives bridge requests (HTTP or wasm) and injects them as ApiRequestEvent.
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
#[derive(Resource)]
pub struct ApiHttpBridgeReceiver(
    tokio::sync::mpsc::UnboundedReceiver<transports::BridgeMessage>,
);

/// Pending response senders (correlation_id → oneshot).
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
#[derive(Resource, Default)]
pub struct ApiHttpResponsePending(
    std::collections::HashMap<u64, tokio::sync::oneshot::Sender<schema::ApiResponse>>,
);

/// System that polls the bridge receiver and triggers API requests.
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
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

/// Observer that catches ApiResponseEvent and resolves the pending reply.
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
pub fn http_response_observer(
    trigger: On<executor::ApiResponseEvent>,
    mut pending: ResMut<ApiHttpResponsePending>,
) {
    let event = trigger.event();
    if let Some(sender) = pending.0.remove(&event.correlation_id) {
        // Ignored by design: a send error means the receiver was dropped — the
        // HTTP client disconnected or timed out before the response was ready.
        // Nothing to do; the pending entry is already removed.
        let _ = sender.send(event.response.clone());
    }
}
