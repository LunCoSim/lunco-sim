//! LunCoSim networking — a **thin lightyear (WebTransport) adapter**.
//!
//! Identity/session/authority primitives (`Provenance`, `GlobalEntityId`,
//! `SimTick`, `IsServer`, `NetworkRole`, `Mutation`) live always-on in
//! `lunco-core`. The networking **wire** (codec, command capture/apply, snapshot
//! state — see [`wire`]) lives in *this* crate behind the `networking` feature,
//! so single-player builds that omit `lunco-networking` carry no networking code
//! at all. On top of the wire, this crate's job is to:
//! - configure the lightyear WebTransport transport (native + wasm) and run it
//!   as host or client;
//! - allocate sessions on connect and send the handshake;
//! - ferry pre-serialized [`sync::SyncEnvelope`]s between
//!   [`sync::SyncOutbox`]/[`sync::SyncInbox`] and two lightyear
//!   messages (reliable `CmdChannel` + best-effort `SnapChannel`).
//!
//! With the feature off the plugin is a no-op and single-player is unaffected.

use bevy::prelude::*;
use std::net::SocketAddr;

#[cfg(feature = "networking")]
mod protocol;
#[cfg(feature = "networking")]
mod shared;
/// Transport-agnostic networking wire: codec, command capture/apply, and state
/// snapshots (no lightyear dep). Driven by this crate's lightyear adapter.
#[cfg(feature = "networking")]
pub mod sync;
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod server;
#[cfg(feature = "networking")]
mod client;
/// Client-prediction diagnostics (render-jitter / velocity / correction census).
/// Compiled only under the `net-diag` feature (off by default — not in normal
/// builds); silence a net-diag build at runtime with `LUNCO_NET_DIAG=0`. See
/// `diagnostics.rs`.
#[cfg(feature = "net-diag")]
mod diagnostics;

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
                    let server = parse_server_addr(&raw)
                        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 5888)));
                    // Distinct per process so two clients get distinct sessions.
                    let client_id = std::process::id() as u64;
                    return Some(NetworkMode::Connect { server, client_id });
                }
                _ => {}
            }
        }
        None
    }

    /// Resolve the mode for the current target: CLI argv on native, the page
    /// URL on wasm. Single entry point so `sandbox.rs` doesn't need a target
    /// `cfg`. Returns `None` for single-player.
    pub fn resolve() -> Option<Self> {
        #[cfg(not(target_family = "wasm"))]
        {
            Self::from_args()
        }
        #[cfg(target_family = "wasm")]
        {
            Self::from_url()
        }
    }

    /// Browser entry point. **Default is single-player (local sandbox)** — the
    /// page boots offline; the user joins a session with the in-sim *Connect*
    /// button (whose address field defaults to [`default_connect_host`], the page
    /// origin). `?connect=host[:port]` is the dev / deep-link override that
    /// auto-connects on load instead of waiting for the button.
    ///
    /// Port defaults to `5888` when the address carries none. Only `Connect` is
    /// reachable on wasm — hosting is native-only.
    #[cfg(target_family = "wasm")]
    pub fn from_url() -> Option<Self> {
        let window = web_sys::window()?;
        let search = window.location().search().ok()?;
        let raw = search
            .trim_start_matches('?')
            .split('&')
            .find_map(|pair| {
                let mut it = pair.splitn(2, '=');
                match (it.next(), it.next()) {
                    (Some("connect"), Some(v)) if !v.is_empty() => Some(v.to_string()),
                    _ => None,
                }
            })?;
        let server = parse_server_addr(&raw)?;
        Some(NetworkMode::Connect {
            server,
            client_id: browser_client_id(),
        })
    }

    /// Build a [`Connect`](NetworkMode::Connect) mode from a user-typed address
    /// (the in-sim *Connect* button). Accepts `host`, `host:port`, or `ip:port`;
    /// the port defaults to `5888`. Returns `None` if the address can't be
    /// resolved to a `SocketAddr` (e.g. a bare DNS name on the IP-only transport
    /// path — see the crate's connect-by-name limitation).
    pub fn connect_to(addr: &str) -> Option<Self> {
        let server = parse_server_addr(addr)?;
        #[cfg(target_family = "wasm")]
        let client_id = browser_client_id();
        #[cfg(not(target_family = "wasm"))]
        let client_id = std::process::id() as u64;
        Some(NetworkMode::Connect { server, client_id })
    }
}

/// Parse a `host`/`host:port`/`ip:port` string to a [`SocketAddr`], defaulting
/// the port to `5888`. Note the transport path is currently IP-only, so a bare
/// DNS name (e.g. `lunica.lunco.space`) yields `None` until connect-by-name is
/// wired through netcode/WebTransport.
fn parse_server_addr(raw: &str) -> Option<SocketAddr> {
    let with_port = if raw.contains(':') {
        raw.to_string()
    } else {
        format!("{raw}:5888")
    };
    with_port.parse().ok()
}

/// The address the in-sim *Connect* button should default to: the page origin
/// host on wasm (so "Connect" joins the server that served the sandbox), and
/// localhost on native.
pub fn default_connect_host() -> String {
    #[cfg(target_family = "wasm")]
    {
        web_sys::window()
            .and_then(|w| w.location().hostname().ok())
            .filter(|h| !h.is_empty())
            .map(|h| format!("{h}:5888"))
            .unwrap_or_else(|| "127.0.0.1:5888".to_string())
    }
    #[cfg(not(target_family = "wasm"))]
    {
        "127.0.0.1:5888".to_string()
    }
}

/// A per-tab client id for browser sessions. `performance.now()` is
/// sub-millisecond and differs per page load, so concurrent tabs get distinct
/// sessions.
#[cfg(target_family = "wasm")]
fn browser_client_id() -> u64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now().to_bits())
        .unwrap_or(1)
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
