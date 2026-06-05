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

#[cfg(feature = "transport-http")]
mod http;
#[cfg(feature = "transport-http")]
pub use http::*;

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
    pub tx: tokio::sync::mpsc::UnboundedSender<BridgeMessage>,
    pub waker: Option<ApiWaker>,
}

#[cfg(any(feature = "transport-http", target_arch = "wasm32"))]
impl HttpBridge {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<BridgeMessage>) -> Self {
        Self { tx, waker: None }
    }

    pub fn with_waker(mut self, waker: ApiWaker) -> Self {
        self.waker = Some(waker);
        self
    }

    pub async fn execute(&self, request: crate::schema::ApiRequest) -> Result<crate::schema::ApiResponse, ()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx.send(BridgeMessage { request, reply: tx }).map_err(|_| ())?;
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
#[cfg(feature = "transport-http")]
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
            let app = axum::Router::new()
                .route("/api/commands", axum::routing::post(http::handle_api_commands))
                .with_state(bridge);

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
