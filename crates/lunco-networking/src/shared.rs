//! Mode dispatch + helpers shared by the host and client adapters.

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::*;
use lunco_command_contracts::{SessionId, SyncChannel};
use lunco_core_session::{NetStatus, NetworkRole};
use lunco_networking_sync::sync::DeclareChannelExt;

use crate::NetworkMode;

/// Protocol id shared by host and client.
pub(crate) const PROTOCOL_ID: u64 = 0x004C_554E_434F_0001; // "LUNCO"

/// Explicitly-marked development key. It is non-zero so an accidental public
/// bind cannot be mistaken for authenticated netcode. Hosts using this key are
/// restricted to loopback; deployments must provide a real key.
pub(crate) const DEV_NETCODE_KEY: [u8; 32] = [
    0x4c, 0x75, 0x6e, 0x43, 0x6f, 0x2d, 0x64, 0x65, 0x76, 0x2d, 0x6e, 0x65, 0x74, 0x2d, 0x6b, 0x65,
    0x79, 0x2d, 0x6f, 0x6e, 0x6c, 0x79, 0x2d, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x2d, 0x76, 0x31, 0x00,
];

const ENV_NETCODE_KEY: &str = "LUNCO_NETCODE_KEY";
const ENV_NETCODE_KEY_FILE: &str = "LUNCO_NETCODE_KEY_FILE";
/// Fixed-size authentication input, allowing whitespace around its 64 digits.
const MAX_NETCODE_KEY_FILE_BYTES: usize = 1024;

/// Parse the 32-byte netcode key from canonical lowercase/uppercase hex.
fn parse_netcode_key(value: &str) -> Result<[u8; 32], String> {
    let value = value.trim();
    if value.len() != 64 {
        return Err(format!(
            "expected 64 hex characters for {ENV_NETCODE_KEY}, got {}",
            value.len()
        ));
    }
    let mut key = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair[0] as char)
            .to_digit(16)
            .ok_or_else(|| format!("invalid hex at byte {index}"))?;
        let low = (pair[1] as char)
            .to_digit(16)
            .ok_or_else(|| format!("invalid hex at byte {index}"))?;
        key[index] = ((high << 4) | low) as u8;
    }
    if key == [0; 32] {
        return Err(format!("{ENV_NETCODE_KEY} must not be all zeroes"));
    }
    Ok(key)
}

/// Resolve the shared netcode authentication key.
///
/// Native deployments use `LUNCO_NETCODE_KEY` or
/// `LUNCO_NETCODE_KEY_FILE`. A missing value selects the explicitly-limited
/// loopback development key. Browser builds may provide the key at compile
/// time through the same variable; otherwise they use that development key.
pub(crate) fn netcode_key() -> Result<[u8; 32], String> {
    #[cfg(not(target_family = "wasm"))]
    {
        if let Some(value) = std::env::var_os(ENV_NETCODE_KEY) {
            let value = value
                .into_string()
                .map_err(|_| format!("{ENV_NETCODE_KEY} is not valid UTF-8"))?;
            return parse_netcode_key(&value)
                .map_err(|error| format!("invalid {ENV_NETCODE_KEY}: {error}"));
        }
        if let Some(path) = std::env::var_os(ENV_NETCODE_KEY_FILE) {
            if path.is_empty() {
                return Err(format!("{ENV_NETCODE_KEY_FILE} is empty"));
            }
            let storage = lunco_storage::FileStorage::new();
            let handle = lunco_storage::StorageHandle::File(path.into());
            let bytes =
                bevy::tasks::block_on(storage.read_bounded(&handle, MAX_NETCODE_KEY_FILE_BYTES))
                    .map_err(|error| format!("cannot read {ENV_NETCODE_KEY_FILE}: {error}"))?;
            let value = String::from_utf8(bytes)
                .map_err(|error| format!("{ENV_NETCODE_KEY_FILE} is not UTF-8: {error}"))?;
            return parse_netcode_key(&value)
                .map_err(|error| format!("invalid key in {ENV_NETCODE_KEY_FILE}: {error}"));
        }
    }

    #[cfg(target_family = "wasm")]
    if let Some(value) = option_env!("LUNCO_NETCODE_KEY") {
        return parse_netcode_key(value)
            .map_err(|error| format!("invalid build-time {ENV_NETCODE_KEY}: {error}"));
    }

    Ok(DEV_NETCODE_KEY)
}

pub(crate) fn is_dev_netcode_key(key: &[u8; 32]) -> bool {
    key == &DEV_NETCODE_KEY
}

/// Deterministic, collision-free `PeerId` → `SessionId`. Netcode peers carry a
/// distinct `u64`, so sessions are unique per connection without a side table.
/// `Raw` peers carry a `SocketAddr` instead of a `u64`; hashing its address
/// keeps distinct raw peers distinct (a fixed sentinel collapsed them all to one
/// session, breaking per-peer ownership/authority for raw connections).
pub(crate) fn peer_to_session(peer: PeerId) -> SessionId {
    let raw = match peer {
        PeerId::Netcode(n) | PeerId::Local(n) | PeerId::Entity(n) | PeerId::Steam(n) => n,
        PeerId::Server => 0,
        PeerId::Raw(addr) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            addr.hash(&mut h);
            h.finish()
        }
    };
    SessionId(raw)
}

/// Add the lightyear plugins, the protocol, the wire-channel declarations, and
/// the host/client setup for `mode`.
///
/// `None` and `Some(Connect)` both build a **client-capable** app (the runtime
/// `JoinServer` path needs `ClientPlugins` present from startup — Bevy can't add
/// plugins later); `Some(Connect)` additionally auto-connects at `Startup`,
/// while `None` stays idle (`NetworkRole::Standalone`, single-player) until a
/// `JoinServer` command dials a server. `Some(Host)` builds the listen-server.
pub(crate) fn build_networking(app: &mut App, mode: &Option<NetworkMode>) {
    // Wire-fed session state (review C7: moved out of the always-on
    // always-on set — every reader/writer lives in this crate, behind this
    // feature). Initialized up front so every registration below — client
    // deep-link seeding, the ui confirm modal, net-diag's divergence report —
    // can take `Res`/`ResMut` without ordering worries. The always-on session
    // substrate itself is installed by `LunCoCoreSessionPlugin` at the host.
    app.init_resource::<crate::connection_state::PendingConnect>();
    app.init_resource::<lunco_networking_core::session::IncomingSnapshots>();
    app.init_resource::<lunco_networking_core::session::DivergenceStats>();

    // The transport-agnostic wire (codec, capture/apply, snapshots) the lightyear
    // ferry below drives. Both Host and Client need it.
    app.add_plugins(lunco_networking_sync::sync::SyncPlugin);

    // Prediction diagnostics — compiled only under the `net-diag` feature (off in
    // normal builds). Added on both peers so you can compare host (silent) vs client
    // while chasing jitter. Silence a net-diag build at runtime with `LUNCO_NET_DIAG=0`.
    #[cfg(feature = "net-diag")]
    app.add_plugins(crate::diagnostics::NetDiagnosticsPlugin);

    let tick = Duration::from_secs_f64(lunco_core_runtime::SECS_PER_TICK);
    let mut startup_error = None;
    if let Some(NetworkMode::Host { port }) = mode {
        #[cfg(not(target_family = "wasm"))]
        match crate::server::prepare_host(*port) {
            Ok(prepared) => {
                app.insert_resource(NetworkRole::Host);
                app.insert_resource(NetStatus {
                    role: NetworkRole::Host,
                    endpoint: format!(":{port}"),
                    connected: true,
                    ..Default::default()
                });
                app.add_plugins(lightyear::prelude::server::ServerPlugins {
                    tick_duration: tick,
                });
                add_protocol(app);
                crate::server::setup_host(app, prepared);
                return;
            }
            Err(error) => startup_error = Some(error),
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = port;
            startup_error = Some("Host mode is unsupported on wasm; use --connect".to_string());
        }
    }

    // The client-capable local stack also owns rejected host startup: the app
    // remains usable, with the explicit error published and no host admitted.
    app.add_plugins(lightyear::prelude::client::ClientPlugins {
        tick_duration: tick,
    });
    add_protocol(app);
    crate::client::register_client_systems(app);
    if let Some(NetworkMode::Connect { server, client_id }) = mode {
        app.insert_resource(NetworkRole::Client);
        app.insert_resource(NetStatus {
            role: NetworkRole::Client,
            endpoint: server.clone(),
            ..Default::default()
        });
        let server = server.clone();
        let client_id = *client_id;
        app.add_systems(
            Startup,
            move |mut commands: Commands,
                  mut status: ResMut<NetStatus>,
                  mut connection: ResMut<lunco_core_session::ClientConnection>,
                  mut role: ResMut<NetworkRole>| {
                match crate::client::prepare_client(&server, client_id, "") {
                    Ok(prepared) => {
                        connection.0 = Some(crate::client::spawn_client(&mut commands, prepared))
                    }
                    Err(error) => {
                        warn!("[net] connection startup rejected: {error}");
                        status.last_error = error;
                        status.role = NetworkRole::Standalone;
                        *role = NetworkRole::Standalone;
                    }
                }
            },
        );
    } else {
        if let Some(error) = startup_error.as_ref() {
            warn!("[net] host startup rejected: {error}");
        }
        app.insert_resource(NetworkRole::Standalone);
        app.insert_resource(NetStatus {
            last_error: startup_error.unwrap_or_default(),
            ..Default::default()
        });
    }
}

/// Protocol must be registered *after* the lightyear plugins and *before*
/// spawning the connection entities.
fn add_protocol(app: &mut App) {
    app.add_plugins(crate::protocol::ProtocolPlugin);

    // Which wire channel each networked command rides (+ registers its capture
    // observer). Continuous SetPorts samples ride the best-effort stream;
    // atomic transactions and structural commands ride the reliable bus.
    app.declare_channel::<lunco_cosim_core::commands::SetPorts>(SyncChannel::ControlStream);
    app.declare_channel::<lunco_cosim_core::commands::SetPortsBatch>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_cosim_core::commands::ReleaseControl>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_core_session::commands::ClaimControl>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_core_session::commands::ReleaseControlClaim>(
        SyncChannel::CommandBus,
    );
    app.declare_channel::<lunco_control_core::commands::AcquireControl>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_control_core::commands::ReleaseControlSource>(
        SyncChannel::CommandBus,
    );
    app.declare_channel::<lunco_core_session::commands::UpdateProfile>(SyncChannel::CommandBus);
    // `SpawnEntity`'s TYPE lives in lunco-core (review A6) — declaring its channel
    // needs the type, not the editor that handles it. This crate has no dependency
    // on either editor package.
    app.declare_channel::<lunco_core::SpawnEntity>(SyncChannel::CommandBus);
    // `UpdateObstacleFieldSpec` no longer rides the command bus — it is journaled
    // (`DomainKind::ObstacleField`) and syncs via the journal plane instead. See
    // the note where `sync_obstacle_field_spec` used to live.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_key_is_nonzero_and_marked() {
        assert_ne!(DEV_NETCODE_KEY, [0; 32]);
        assert!(is_dev_netcode_key(&DEV_NETCODE_KEY));
    }

    #[test]
    fn key_parser_rejects_zero_and_wrong_length() {
        assert!(parse_netcode_key(&"00".repeat(32)).is_err());
        assert!(parse_netcode_key("deadbeef").is_err());
        assert!(parse_netcode_key(&"gg".repeat(32)).is_err());
        assert!(parse_netcode_key("").is_err());
    }

    #[test]
    fn key_parser_accepts_32_bytes_of_hex() {
        let key = parse_netcode_key(&"ab".repeat(32)).expect("valid key");
        assert_eq!(key, [0xab; 32]);
    }
}
