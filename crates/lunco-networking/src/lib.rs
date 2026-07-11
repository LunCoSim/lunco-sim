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

/// Connect deep-link URL format (`luncosim://connect?address=…&digest=…` and the
/// web `?connect=…#digest` form) — pure, always compiled so the host's invite
/// link builder and the native arg parser work regardless of the `networking`
/// feature.
pub mod connect_link;

#[cfg(feature = "networking")]
mod protocol;
#[cfg(feature = "networking")]
mod shared;
/// Transport-agnostic networking wire: codec, command capture/apply, and state
/// snapshots (no lightyear dep). Driven by this crate's lightyear adapter.
#[cfg(feature = "networking")]
pub mod sync;
/// Scenario distribution: the server publishes its scenario manifest (CID-
/// addressed assets + a Merkle revision), clients fetch the assets they're
/// missing over the same WebTransport. IPFS-CID interop. Phase 1 ships the
/// manifest; asset chunk transfer is Phase 3.
#[cfg(feature = "networking")]
pub mod scenario;
/// Scenario asset transfer (Phase 3): one-way host→client byte streaming of the
/// CID-addressed assets a manifest advertises, into `<cache_dir>/scenarios/<id>/`.
#[cfg(feature = "networking")]
pub mod scenario_sync;
/// The **bytes plane**: fetch a scenario's CID-addressed assets over HTTP rather
/// than streaming them through the reliable QUIC channel (which queues without
/// bound and stalls on multi-MB twins). Used whenever the host advertises an
/// `asset_base_url`; `scenario_sync`'s chunk path is the fallback.
#[cfg(feature = "networking")]
pub mod http_fetch;
/// The journal replication plane: authored Twin-journal entries host→client,
/// merged via `append_remote`. Separated from the command/state/content planes
/// (see module docs); the transport ferry only routes to it.
#[cfg(feature = "networking")]
pub mod journal_plane;
/// The scripted-policy plane: distribute + activate rhai policies (merge /
/// authorization / drive-kernel) host→client so every peer runs the identical one.
#[cfg(feature = "networking")]
pub mod scripted_policy;
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod server;
#[cfg(feature = "networking")]
mod client;
/// Native single-instance deep-link forwarding: route a clicked `luncosim://`
/// link into the already-running app over a local socket (else become primary).
/// (OS *scheme registration* is a desktop-integration concern and lives in the
/// app crate `lunco-sandbox`, not here — this crate only parses + dials.)
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
pub mod single_instance;
/// Browser-only WebTransport client IO that dials a **hostname URL**
/// (`https://lunica.lunco.space:5888`) so a real CA cert validates with no
/// digest — lightyear's built-in `WebTransportClientIo` only dials
/// `https://{SocketAddr}` (IP-only). Native keeps lightyear's IO.
#[cfg(feature = "networking")]
mod wt_client;
/// Layer-4 UI: the in-sim *Connect* panel (address field + Connect/Disconnect),
/// which dispatches the `JoinServer`/`LeaveServer` commands, plus the egui
/// presence-cursor / tutorial overlays. Behind the `ui` feature (which implies
/// `networking`) so headless servers never link egui (CQ-601).
#[cfg(feature = "ui")]
pub mod ui;
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
    /// Pure client: connect to `server` (a `host:port` string — a **hostname**
    /// like `lunica.lunco.space:5888` or an `ip:port`) over WebTransport,
    /// identifying as `client_id` (must be distinct per client). Kept as a
    /// string so a DNS name survives to the browser, which resolves it when it
    /// dials the WebTransport URL (a `SocketAddr` couldn't hold a hostname).
    Connect { server: String, client_id: u64 },
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
                        .unwrap_or(lunco_core::session::DEFAULT_HOST_PORT);
                    return Some(NetworkMode::Host { port });
                }
                "--connect" => {
                    let raw = args.get(i + 1).cloned().unwrap_or_default();
                    return Some(NetworkMode::Connect {
                        server: normalize_addr(&raw),
                        client_id: next_client_id(),
                    });
                }
                _ => {}
            }
        }
        None
    }

    /// Resolve the mode for the current target: CLI argv on native, the page
    /// URL on wasm. Single entry point so `sandbox.rs` doesn't need a target
    /// `cfg`. Returns `None` for single-player.
    pub fn resolve(headless: bool) -> Option<Self> {
        #[cfg(not(target_family = "wasm"))]
        {
            let mode = Self::from_args();
            if mode.is_none() {
                // If running headless / as a dedicated server, default to Host mode
                let is_headless = headless
                    || std::env::args().any(|a| a == "--no-ui")
                    || std::env::var("LUNCO_NO_UI").is_ok_and(|v| v != "0" && !v.is_empty())
                    || std::env::current_exe()
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .is_some_and(|n| n.contains("sandbox-server"));
                if is_headless {
                    return Some(NetworkMode::Host {
                        port: lunco_core::session::DEFAULT_HOST_PORT,
                    });
                }
            }
            mode
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = headless;
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
        Some(NetworkMode::Connect {
            server: normalize_addr(&raw),
            client_id: browser_client_id(),
        })
    }

    /// Build a [`Connect`](NetworkMode::Connect) mode from a user-typed address
    /// (the in-sim *Connect* button / the `JoinServer` command). Accepts a bare
    /// `host`, `host:port`, or `ip:port`; the port defaults to `5888`. A bare DNS
    /// name is fine now — the browser resolves it when it dials the WebTransport
    /// URL. Returns `None` only for an empty address.
    pub fn connect_to(addr: &str) -> Option<Self> {
        if addr.trim().is_empty() {
            return None;
        }
        Some(NetworkMode::Connect {
            server: normalize_addr(addr),
            client_id: next_client_id(),
        })
    }
}

/// Normalize a user/URL address to a `host:port` string, defaulting the port to
/// `5888`. Accepts a bare hostname (`lunica.lunco.space`), `host:port`, or
/// `ip:port`. The host is kept as-is (hostname or IP) so the browser can resolve
/// a DNS name when it dials the WebTransport URL.
pub(crate) fn normalize_addr(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("{raw}:{}", lunco_core::session::DEFAULT_HOST_PORT)
    }
}

/// A distinct **netcode connection id** for a new connection. This is only the
/// transport-level peer handle — it no longer determines authority identity (the
/// host assigns a server-side `SessionId` at connect; see
/// `server::AssignedSessions`). Drawn from fresh entropy so two clients can't
/// collide, fixing the old `std::process::id()` reuse across machines (review H5).
pub(crate) fn next_client_id() -> u64 {
    #[cfg(target_family = "wasm")]
    {
        browser_client_id()
    }
    #[cfg(not(target_family = "wasm"))]
    {
        lunco_core::ids::random_u64()
    }
}

/// The address the in-sim *Connect* button should default to: the page origin
/// host on wasm (so "Connect" joins the server that served the sandbox), and
/// localhost on native.
pub fn default_connect_host() -> String {
    #[cfg(target_family = "wasm")]
    {
        use lunco_core::session::DEFAULT_HOST_PORT;
        web_sys::window()
            .and_then(|w| w.location().hostname().ok())
            .filter(|h| !h.is_empty())
            .map(|h| format!("{h}:{DEFAULT_HOST_PORT}"))
            .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_HOST_PORT}"))
    }
    #[cfg(not(target_family = "wasm"))]
    {
        format!("127.0.0.1:{}", lunco_core::session::DEFAULT_HOST_PORT)
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

/// Plugin that wires the lightyear WebTransport adapter.
///
/// `mode` is an [`Option`]: `Some(Host|Connect)` boots into that role (CLI
/// `--host`/`--connect`, browser `?connect=`), while **`None` boots a
/// client-capable but idle local sandbox** — single-player until a `JoinServer`
/// command (the in-sim *Connect* button / HTTP API / MCP) dials a server at
/// runtime. So this plugin is now added whenever the `networking` feature is on,
/// not only when an address was supplied up front.
pub struct LunCoNetworkingPlugin {
    pub mode: Option<NetworkMode>,
}

impl Plugin for LunCoNetworkingPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "networking")]
        shared::build_networking(app, &self.mode);
        #[cfg(not(feature = "networking"))]
        {
            let _ = app;
            // A requested Host/Connect mode being silently swallowed is a broken
            // build/launch, not a benign default — say so at error severity.
            if self.mode.is_some() {
                error!(
                    "lunco-networking built without the `networking` feature — requested \
                     network mode {:?} is IGNORED (rebuild with `--features networking`)",
                    self.mode
                );
            } else {
                warn!("lunco-networking built without the `networking` feature — no-op");
            }
        }
    }
}
