//! Transport adapters.
//!
//! The bridge core (`HttpBridge`, `BridgeMessage`, the request/response
//! envelopes) is transport-agnostic — pure Bevy + `tokio::sync` channels +
//! serde — and is shared by the native HTTP server and the wasm JS bridge.
//! Only `spawn_server` (a real `TcpListener`) is native-only.

// The bridge core compiles whenever a transport is present: the native HTTP
// server (`transport-http`) or — automatically — the wasm JS bridge (any
// wasm32 build, since that's the only transport a browser can use).
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
mod envelope;
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
pub use envelope::*;

// The axum HTTP server is native-only: even when `transport-http` is enabled, it
// must not compile on wasm (axum + tokio/net are absent there by construction —
// see Cargo.toml). The bridge core above stays shared across both transports.
#[cfg(all(feature = "transport-http", not(target_arch = "wasm32")))]
mod http;
#[cfg(all(feature = "transport-http", not(target_arch = "wasm32")))]
pub use http::*;

/// Read-only content-addressed asset server (`GET /scenario-assets/<cid>`) — the
/// bytes plane of scenario distribution. Native-only, same reasoning as `http` above.
#[cfg(all(feature = "transport-http", not(target_arch = "wasm32")))]
pub mod assets;

/// In-browser JS bridge (`window.lunco_api`). Reuses the entire bridge core;
/// replaces the TcpListener transport with a `#[wasm_bindgen]` async export.
/// Always compiled on wasm32 — no feature gate.
#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(feature = "transport-http")]
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    pub port: u16,
}

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
pub struct BridgeMessage {
    pub request: crate::schema::ApiRequest,
    pub reply: tokio::sync::oneshot::Sender<crate::schema::ApiResponse>,
}

/// Wakes the host event loop after pushing a message into the
/// bridge's mpsc. Without this, an HTTP request handed to the bridge
/// only gets drained on the next Bevy tick — which, in reactive
/// `WinitSettings`, may not arrive for a full second. The waker is
/// optional so headless tests / non-winit hosts (and wasm, which runs
/// a continuous rAF loop) can still use the bridge without it.
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
pub type ApiWaker = std::sync::Arc<dyn Fn() + Send + Sync>;

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
#[derive(Clone)]
pub struct HttpBridge {
    pub tx: tokio::sync::mpsc::Sender<BridgeMessage>,
    pub waker: Option<ApiWaker>,
}

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
impl HttpBridge {
    pub fn new(tx: tokio::sync::mpsc::Sender<BridgeMessage>) -> Self {
        Self { tx, waker: None }
    }

    pub fn with_waker(mut self, waker: ApiWaker) -> Self {
        self.waker = Some(waker);
        self
    }

    pub async fn execute(&self, request: crate::schema::ApiRequest) -> Result<crate::schema::ApiResponse, ()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        // AWAIT a full queue rather than dropping. This is the command funnel: a
        // dropped request is an unattributable failure — the caller sees a timeout
        // with nothing in the log explaining it — which is exactly what this
        // codebase spends its diagnostics budget avoiding. `try_send` + warn would
        // still lose the command.
        //
        // Suspending costs nothing structurally: every caller is already async and
        // already awaits the `oneshot` below, and the HTTP client's own timeout is
        // the natural shed valve when the app genuinely cannot keep up. An `Err`
        // here means the ECS receiver is gone (app shutting down), which is the
        // existing contract for `Err(())`.
        self.tx.send(BridgeMessage { request, reply: tx }).await.map_err(|_| ())?;
        if let Some(waker) = &self.waker {
            waker();
        }
        rx.await.map_err(|_| ())
    }
}

// A long-lived OS thread hosting a blocking tokio HTTP-server runtime is
// the correct shape here — not an `AsyncComputeTaskPool` task (which is
// for short compute jobs and would occupy a pool slot forever). The
// `disallowed_methods` ban targets wasm + short tasks, neither of which
// applies to this native, `transport-http`-gated server, so it's locally
// allowed. The previous triple `.unwrap()` panicked this *detached*
// thread silently (e.g. on port-in-use → the API just never came up);
// failures are now logged and the thread returns.
#[cfg(all(feature = "transport-http", not(target_arch = "wasm32")))]
#[allow(clippy::disallowed_methods)]
pub fn spawn_server(config: HttpServerConfig, bridge: HttpBridge) {
    let port = config.port;
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                bevy::log::error!("[lunco-api] failed to start HTTP server runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            // Three routes, all of them real (the docs used to list four more
            // that were never registered — every curl example 404'd):
            //   POST /api/commands        — the one command funnel
            //   GET  /api/health          — liveness; no world access
            //   GET  /api/commands/schema — the `DiscoverSchema` result, i.e.
            //                               the same derived list the MCP tool
            //                               surface is built from
            let app = axum::Router::new()
                .route("/api/commands", axum::routing::post(http::handle_api_commands))
                .route("/api/commands/schema", axum::routing::get(http::handle_schema))
                .route("/api/health", axum::routing::get(http::handle_health))
                .with_state(bridge);

            // TODO(multiplayer): deferred — singleplayer focus for now, RBAC
            // disabled for ease of debugging. Loopback-only bind, but the command
            // API has zero local auth — any local process/user can drive the full
            // command surface. Revisit before multiplayer hardening
            // (REVIEW-2026-07-19.md API-1).
            let listener = match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                Ok(l) => l,
                Err(e) => {
                    bevy::log::error!(
                        "[lunco-api] HTTP server failed to bind 127.0.0.1:{port}: {e} \
                         (port already in use?) — API will be unavailable"
                    );
                    return;
                }
            };
            if let Err(e) = axum::serve(listener, app).await {
                bevy::log::error!("[lunco-api] HTTP server stopped with error: {e}");
            }
        });
    });
}
