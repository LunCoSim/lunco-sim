//! LunCoSim networking — a **thin lightyear (WebTransport) adapter**.
//!
//! Identity primitives (`Provenance`, `GlobalEntityId`, `SimTick`) live in
//! `lunco-core`; mutation/session wire contracts live in
//! `lunco-command-contracts`; session/authority primitives (`NetworkRole`
//! — whose `is_authoritative()` is the sole authority flag — plus status,
//! possession, and prediction markers) live in `lunco-core-session`. The
//! transport-neutral synchronization runtime lives in
//! `lunco-networking-sync`, while this crate's job is to:
//! - configure the lightyear WebTransport transport (native + wasm) and run it
//!   as host or client;
//! - allocate sessions on connect and send the handshake;
//! - ferry pre-serialized [`lunco_networking_sync::sync::SyncEnvelope`]s between
//!   [`lunco_networking_sync::sync::SyncOutbox`] /
//!   [`lunco_networking_sync::sync::SyncInbox`] and two lightyear
//!   messages (reliable `CmdChannel` + best-effort `SnapChannel`).
//!
//! With the feature off the plugin is a no-op and single-player is unaffected.

use bevy::prelude::*;

#[cfg(feature = "networking")]
pub(crate) mod connection_state;

/// Connect deep-link URL format (`luncosim://connect?address=…&digest=…` and the
/// web `?connect=…#digest` form) — pure, always compiled so the host's invite
/// link builder and the native arg parser work regardless of the `networking`
/// feature.
pub mod connect_link;
mod endpoint;
pub use endpoint::{ConnectEndpoint, NetworkConfigError};

#[cfg(feature = "networking")]
mod client;
/// Client-prediction diagnostics (render-jitter / velocity / correction census).
/// Compiled only under the `net-diag` feature (off by default — not in normal
/// builds); silence a net-diag build at runtime with `LUNCO_NET_DIAG=0`. See
/// `diagnostics.rs`.
#[cfg(feature = "net-diag")]
mod diagnostics;
#[cfg(feature = "networking")]
mod protocol;
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod server;
#[cfg(feature = "networking")]
mod shared;
/// Native single-instance deep-link forwarding: route a clicked `luncosim://`
/// link into the already-running app over a local socket (else become primary).
/// (OS *scheme registration* is a desktop-integration concern and lives in the
/// app crate `lunco-luncosim`, not here — this crate only parses + dials.)
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
pub mod single_instance;
/// Layer-4 UI: the in-sim *Connect* panel (address field + Connect/Disconnect),
/// which dispatches the `JoinServer`/`LeaveServer` commands, plus the egui
/// presence-cursor / tutorial overlays. Behind the `ui` feature (which implies
/// `networking`) so headless servers never link egui (CQ-601).
#[cfg(feature = "ui")]
pub mod ui;
/// Browser-only WebTransport client IO that dials a **hostname URL**
/// (`https://lunica.lunco.space:5888`) so a real CA cert validates with no
/// digest — lightyear's built-in `WebTransportClientIo` only dials
/// `https://{SocketAddr}` (IP-only). Native keeps lightyear's IO.
#[cfg(feature = "networking")]
mod wt_client;

/// How this process participates in the session.
#[derive(Clone, Debug)]
pub enum NetworkMode {
    /// Listen-server: run the authoritative world and accept WebTransport
    /// clients on `port`. (Native only.)
    Host { port: u16 },
    /// Pure client: connect to `server` (a validated hostname or IP authority,
    /// such as `lunica.lunco.space:5888`) over WebTransport,
    /// identifying as `client_id` (must be distinct per client). Kept as a
    /// validated endpoint so a DNS name survives to the browser, which resolves
    /// it when it dials the WebTransport URL.
    Connect {
        server: ConnectEndpoint,
        client_id: u64,
    },
}

impl NetworkMode {
    /// Parse explicit networking flags. No flags means a local, idle process,
    /// including headless scene tests and the dedicated server launcher.
    pub fn from_args() -> Result<Option<Self>, NetworkConfigError> {
        Self::parse_args(&std::env::args_os().collect::<Vec<_>>())
    }

    /// Pure native CLI admission. Malformed, duplicate and conflicting network
    /// flags reject; only an omitted optional host port uses the default.
    pub fn parse_args(args: &[std::ffi::OsString]) -> Result<Option<Self>, NetworkConfigError> {
        let args: Vec<&str> = args
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                argument.to_str().ok_or_else(|| {
                    NetworkConfigError::Arguments(format!("argument {index} is not valid UTF-8"))
                })
            })
            .collect::<Result<_, _>>()?;
        let mut mode = None;
        let mut index = 1;
        while index < args.len() {
            let argument = args[index];
            let host_value = argument.strip_prefix("--host=");
            let connect_value = argument.strip_prefix("--connect=");
            if argument == "--host" || host_value.is_some() {
                if mode.is_some() {
                    return Err(NetworkConfigError::Arguments(
                        "use exactly one --host or --connect flag".into(),
                    ));
                }
                let value = if let Some(value) = host_value {
                    Some(value)
                } else if let Some(value) =
                    args.get(index + 1).filter(|value| !value.starts_with("--"))
                {
                    index += 1;
                    Some(*value)
                } else {
                    None
                };
                let port = match value {
                    None => lunco_core_session::DEFAULT_HOST_PORT,
                    Some(value) => value
                        .parse::<u16>()
                        .ok()
                        .filter(|port| *port != 0)
                        .ok_or_else(|| {
                            NetworkConfigError::Arguments("--host port must be 1..=65535".into())
                        })?,
                };
                mode = Some(Self::Host { port });
            } else if argument == "--connect" || connect_value.is_some() {
                if mode.is_some() {
                    return Err(NetworkConfigError::Arguments(
                        "use exactly one --host or --connect flag".into(),
                    ));
                }
                let value = if let Some(value) = connect_value {
                    value
                } else {
                    index += 1;
                    args.get(index)
                        .filter(|value| !value.starts_with("--"))
                        .copied()
                        .ok_or_else(|| {
                            NetworkConfigError::Arguments("--connect requires an address".into())
                        })?
                };
                mode = Some(Self::connect_to(value)?);
            }
            index += 1;
        }
        Ok(mode)
    }

    /// Resolve explicit CLI flags on native or a `?connect=` page override on
    /// wasm. Presentation/headless settings do not select a network role.
    pub fn resolve() -> Result<Option<Self>, NetworkConfigError> {
        #[cfg(not(target_family = "wasm"))]
        {
            Self::from_args()
        }
        #[cfg(target_family = "wasm")]
        {
            Self::from_url()
        }
    }

    /// Browser startup is local unless its page carries one valid `connect`
    /// query parameter. Empty, duplicate or malformed overrides reject.
    #[cfg(target_family = "wasm")]
    pub fn from_url() -> Result<Option<Self>, NetworkConfigError> {
        let window = web_sys::window()
            .ok_or_else(|| NetworkConfigError::PageUrl("browser window unavailable".into()))?;
        let href = window
            .location()
            .href()
            .map_err(|_| NetworkConfigError::PageUrl("cannot read browser URL".into()))?;
        Self::from_page_url(&href)
    }

    pub fn from_page_url(raw: &str) -> Result<Option<Self>, NetworkConfigError> {
        let url =
            url::Url::parse(raw).map_err(|error| NetworkConfigError::PageUrl(error.to_string()))?;
        let mut values = url.query_pairs().filter(|(key, _)| key == "connect");
        let Some((_, address)) = values.next() else {
            return Ok(None);
        };
        if values.next().is_some() {
            return Err(NetworkConfigError::PageUrl(
                "duplicate connect parameter".into(),
            ));
        }
        Self::connect_to(&address).map(Some)
    }

    /// Admit a typed endpoint before transport or current-session mutation.
    pub fn connect_to(address: &str) -> Result<Self, NetworkConfigError> {
        Ok(Self::Connect {
            server: ConnectEndpoint::parse(address)?,
            client_id: next_client_id(),
        })
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
        lunco_id::random_u64()
    }
}

#[cfg(test)]
mod mode_configuration_tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<NetworkMode>, NetworkConfigError> {
        NetworkMode::parse_args(
            &args
                .iter()
                .map(|argument| std::ffi::OsString::from(*argument))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn network_mode_configuration_requires_explicit_host_in_all_headless_launches() {
        for args in [
            vec!["luncosim"],
            vec!["luncosim", "--no-ui"],
            vec!["luncosim", "test", "--scene", "generic-scene"],
            vec!["luncosim-server", "--headless-max-speed"],
        ] {
            assert!(parse(&args).unwrap().is_none());
        }
        assert!(matches!(
            parse(&["luncosim", "--host"]).unwrap(),
            Some(NetworkMode::Host { port: 5888 })
        ));
        assert!(matches!(
            parse(&["luncosim", "--host", "--api", "4101"]).unwrap(),
            Some(NetworkMode::Host { port: 5888 })
        ));
        assert!(matches!(
            parse(&["luncosim", "--host=1234"]).unwrap(),
            Some(NetworkMode::Host { port: 1234 })
        ));
        let Some(NetworkMode::Connect { server, .. }) =
            parse(&["luncosim", "--connect", "[::1]:443"]).unwrap()
        else {
            panic!("expected admitted client")
        };
        assert_eq!(server.address(), "[::1]:443");
        assert_eq!(server.port(), 443);
    }

    #[test]
    fn network_mode_configuration_rejects_invalid_missing_duplicate_and_conflicting_flags() {
        for flags in [
            vec!["--host", "invalid"],
            vec!["--host", "0"],
            vec!["--host", "65536"],
            vec!["--host", "-1"],
            vec!["--host="],
            vec!["--host=0"],
            vec!["--connect"],
            vec!["--connect", "--api", "4101"],
            vec!["--connect", ""],
            vec!["--connect="],
            vec!["--connect=host:0"],
            vec!["--host", "--host"],
            vec!["--host", "--connect", "host"],
            vec!["--connect", "host", "--host"],
            vec!["--connect", "host", "--connect", "other"],
        ] {
            let mut args = vec!["luncosim"];
            args.extend(flags);
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    #[cfg(all(feature = "networking", not(target_family = "wasm")))]
    #[test]
    fn network_host_configuration_rejects_zero_port_before_admission() {
        assert_eq!(
            crate::server::prepare_host(0).err().unwrap(),
            "host port must be 1..=65535"
        );
    }

    #[test]
    fn network_mode_configuration_browser_override_decodes_and_rejects_invalid_requests() {
        assert!(
            NetworkMode::from_page_url("https://host.example/?other=x")
                .unwrap()
                .is_none()
        );
        let Some(NetworkMode::Connect { server, .. }) =
            NetworkMode::from_page_url("https://host.example/?connect=%5B%3A%3A1%5D%3A443")
                .unwrap()
        else {
            panic!("expected admitted browser endpoint")
        };
        assert_eq!(server.address(), "[::1]:443");
        for raw in [
            "https://host.example/?connect=",
            "https://host.example/?connect",
            "https://host.example/?connect=host:0",
            "https://host.example/?connect=host&connect=other",
            "https://host.example/?connect=host%2Fpath",
            "not a page URL",
        ] {
            assert!(NetworkMode::from_page_url(raw).is_err(), "{raw}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn network_mode_configuration_rejects_non_unicode_argv_without_panicking() {
        use std::os::unix::ffi::OsStringExt;
        let args = [
            std::ffi::OsString::from("luncosim"),
            std::ffi::OsString::from_vec(vec![0xff]),
        ];
        assert!(matches!(
            NetworkMode::parse_args(&args),
            Err(NetworkConfigError::Arguments(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn network_mode_configuration_rejects_unpaired_windows_surrogate_without_panicking() {
        use std::os::windows::ffi::OsStringExt;
        let args = [
            std::ffi::OsString::from("luncosim"),
            std::ffi::OsString::from_wide(&[0xd800]),
        ];
        assert!(matches!(
            NetworkMode::parse_args(&args),
            Err(NetworkConfigError::Arguments(_))
        ));
    }
}

/// The address the in-sim *Connect* button should default to: the page origin
/// host on wasm (so "Connect" joins the server that served the sandbox), and
/// localhost on native.
pub fn default_connect_host() -> String {
    #[cfg(target_family = "wasm")]
    {
        use lunco_core_session::DEFAULT_HOST_PORT;
        web_sys::window()
            .and_then(|w| w.location().hostname().ok())
            .filter(|h| !h.is_empty())
            .map(|h| format!("{h}:{DEFAULT_HOST_PORT}"))
            .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_HOST_PORT}"))
    }
    #[cfg(not(target_family = "wasm"))]
    {
        format!("127.0.0.1:{}", lunco_core_session::DEFAULT_HOST_PORT)
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
