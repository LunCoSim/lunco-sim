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
/// `WinitSettings`, may never arrive while the window is idle.
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
pub type ApiWaker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Late-bound host wake-up hook.
///
/// The transport can be built before a host installs its event-loop resources.
/// Keeping the slot separate from [`HttpBridge`] lets the host bind its real
/// wake mechanism at the lifecycle point where that resource exists, while
/// every bridge clone observes the same binding. A missing hook is valid for
/// hosts that deliberately run continuously (headless and wasm); a windowed
/// host binds it before its first idle period.
#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
#[derive(Clone, Default)]
pub struct ApiWakerSlot(std::sync::Arc<std::sync::RwLock<Option<ApiWaker>>>);

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
impl ApiWakerSlot {
    pub fn install(&self, waker: ApiWaker) {
        if let Ok(mut slot) = self.0.write() {
            *slot = Some(waker);
        }
    }

    fn wake(&self) {
        let waker = self.0.read().ok().and_then(|slot| slot.clone());
        if let Some(waker) = waker {
            waker();
        }
    }
}

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
#[derive(Clone)]
pub struct HttpBridge {
    pub tx: tokio::sync::mpsc::Sender<BridgeMessage>,
    pub waker: ApiWakerSlot,
}

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
impl HttpBridge {
    pub fn new(tx: tokio::sync::mpsc::Sender<BridgeMessage>) -> Self {
        Self {
            tx,
            waker: ApiWakerSlot::default(),
        }
    }

    pub fn with_waker(self, waker: ApiWaker) -> Self {
        self.waker.install(waker);
        self
    }

    pub fn with_waker_slot(mut self, waker: ApiWakerSlot) -> Self {
        self.waker = waker;
        self
    }

    pub async fn execute(
        &self,
        request: crate::schema::ApiRequest,
    ) -> Result<crate::schema::ApiResponse, ()> {
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
        self.tx
            .send(BridgeMessage { request, reply: tx })
            .await
            .map_err(|_| ())?;
        self.waker.wake();
        rx.await.map_err(|_| ())
    }
}

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
#[cfg(test)]
mod tests {
    use super::ApiWakerSlot;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn a_waker_can_be_bound_after_the_bridge_is_created() {
        let slot = ApiWakerSlot::default();
        let calls = Arc::new(AtomicUsize::new(0));

        // Before the host lifecycle reaches its event-loop binding point,
        // requests remain queueable and no invalid wake target is guessed.
        slot.wake();
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        let calls_for_waker = Arc::clone(&calls);
        slot.install(Arc::new(move || {
            calls_for_waker.fetch_add(1, Ordering::Relaxed);
        }));
        slot.wake();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
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
            // Four routes, all of them real (the docs used to list ones that were
            // never registered — every curl example 404'd):
            //   POST /api/commands        — the one command funnel
            //   GET  /api/health          — liveness; no world access
            //   GET  /api/ready           — readiness; reads the world's
            //                               `ReadinessRegistry` via `GetReadiness`
            //   GET  /api/diagnostics     — co-sim wiring health; reads the world's
            //                               `CosimDiagnostics` via `GetBrokenConnections`
            //   GET  /api/commands/schema — the `DiscoverSchema` result, i.e.
            //                               the same derived list the MCP tool
            //                               surface is built from
            let app = axum::Router::new()
                .route(
                    "/api/commands",
                    axum::routing::post(http::handle_api_commands),
                )
                .route(
                    "/api/commands/schema",
                    axum::routing::get(http::handle_schema),
                )
                .route("/api/health", axum::routing::get(http::handle_health))
                .route("/api/ready", axum::routing::get(http::handle_ready))
                .route(
                    "/api/diagnostics",
                    axum::routing::get(http::handle_diagnostics),
                )
                .with_state(bridge);

            // This is a trusted local boundary: it binds loopback only and has
            // no local user-authentication layer. Networked peers use the
            // authenticated session/RBAC path and must not be routed here.
            let listener = match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await
            {
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
