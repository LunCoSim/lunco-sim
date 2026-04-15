//! Transport adapters.
#[cfg(feature = "transport-http")]
mod http;
#[cfg(feature = "transport-http")]
pub use http::*;

#[cfg(feature = "transport-http")]
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    pub port: u16,
}

#[cfg(feature = "transport-http")]
pub struct BridgeMessage {
    pub request: crate::schema::ApiRequest,
    pub reply: tokio::sync::oneshot::Sender<crate::schema::ApiResponse>,
}

#[cfg(feature = "transport-http")]
#[derive(Debug, Clone)]
pub struct HttpBridge {
    pub tx: tokio::sync::mpsc::UnboundedSender<BridgeMessage>,
}

#[cfg(feature = "transport-http")]
impl HttpBridge {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<BridgeMessage>) -> Self {
        Self { tx }
    }
    
    pub async fn execute(&self, request: crate::schema::ApiRequest) -> Result<crate::schema::ApiResponse, ()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx.send(BridgeMessage { request, reply: tx }).map_err(|_| ())?;
        rx.await.map_err(|_| ())
    }
}

#[cfg(feature = "transport-http")]
pub fn spawn_server(config: HttpServerConfig, bridge: HttpBridge) {
    let port = config.port;
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let app = axum::Router::new()
                .route("/api/commands", axum::routing::post(http::execute_command))
                .with_state(bridge);
            
            let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
}
