//! LunCoSim networking — a **thin lightyear (WebTransport) adapter**.
//!
//! All command capture/apply, session/authority, codec and snapshot logic lives
//! in the always-on substrate (`lunco-core` + `lunco-api::wire`). This crate's
//! only job, behind the `networking` feature (D7), is to:
//! - configure the lightyear WebTransport transport (native + wasm) and run it
//!   as host or client;
//! - allocate sessions on connect and send the handshake;
//! - ferry pre-serialized [`lunco_api::WireEnvelope`]s between
//!   [`lunco_api::WireOutbox`]/[`lunco_api::WireInbox`] and two lightyear
//!   messages (reliable `CmdChannel` + best-effort `SnapChannel`).
//!
//! With the feature off the plugin is a no-op and single-player is unaffected.

use bevy::prelude::*;
use std::net::SocketAddr;

#[cfg(feature = "networking")]
mod protocol;
#[cfg(feature = "networking")]
mod shared;
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod server;
#[cfg(feature = "networking")]
mod client;

/// How this process participates in the session.
#[derive(Clone, Debug)]
pub enum NetworkMode {
    /// Listen-server: run the authoritative world and accept WebTransport
    /// clients on `port`. (Native only.)
    Host { port: u16 },
    /// Pure client: connect to `server` over WebTransport, identifying as
    /// `client_id` (must be distinct per client).
    Connect { server: SocketAddr, client_id: u64 },
}

impl NetworkMode {
    /// Parse `--host [port]` / `--connect <addr[:port]>` from argv. Returns
    /// `None` for single-player (no networking flags). A `--connect` host
    /// without a port defaults to `:5888`; `--host` defaults to port `5888`.
    pub fn from_args() -> Option<Self> {
        let args: Vec<String> = std::env::args().collect();
        for i in 0..args.len() {
            match args[i].as_str() {
                "--host" => {
                    let port = args
                        .get(i + 1)
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(5888);
                    return Some(NetworkMode::Host { port });
                }
                "--connect" => {
                    let raw = args.get(i + 1).cloned().unwrap_or_default();
                    let with_port = if raw.contains(':') {
                        raw
                    } else {
                        format!("{raw}:5888")
                    };
                    let server: SocketAddr = with_port.parse().unwrap_or_else(|_| {
                        SocketAddr::from(([127, 0, 0, 1], 5888))
                    });
                    // Distinct per process so two clients get distinct sessions.
                    let client_id = std::process::id() as u64;
                    return Some(NetworkMode::Connect { server, client_id });
                }
                _ => {}
            }
        }
        None
    }
}

/// Plugin that wires the lightyear WebTransport adapter for the chosen
/// [`NetworkMode`]. Add it only when networking is desired (single-player omits
/// it entirely).
pub struct LunCoNetworkingPlugin {
    pub mode: NetworkMode,
}

impl Plugin for LunCoNetworkingPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "networking")]
        shared::build_networking(app, &self.mode);
        #[cfg(not(feature = "networking"))]
        {
            let _ = (app, &self.mode);
            warn!("lunco-networking built without the `networking` feature — no-op");
        }
    }
}
