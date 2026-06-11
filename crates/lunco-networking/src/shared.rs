//! Mode dispatch + helpers shared by the host and client adapters.

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::*;
use crate::wire::{DeclareChannelExt, WireEnvelope};
use lunco_core::{IsServer, NetStatus, NetworkRole, SessionId, WireChannel};

use crate::NetworkMode;

/// Dev protocol id + key. Host and client MUST agree. (Localhost MVP; a real
/// deployment would load a real key.)
pub(crate) const PROTOCOL_ID: u64 = 0x004C_554E_434F_0001; // "LUNCO"
pub(crate) const PRIVATE_KEY: [u8; 32] = [0u8; 32];

pub(crate) fn serialize_env(env: &WireEnvelope) -> Option<Vec<u8>> {
    serde_json::to_vec(env).ok()
}

pub(crate) fn deserialize_env(bytes: &[u8]) -> Option<WireEnvelope> {
    serde_json::from_slice(bytes).ok()
}

/// Deterministic, collision-free `PeerId` → `SessionId`. Netcode peers carry a
/// distinct `u64`, so sessions are unique per connection without a side table.
pub(crate) fn peer_to_session(peer: PeerId) -> SessionId {
    let raw = match peer {
        PeerId::Netcode(n) | PeerId::Local(n) | PeerId::Entity(n) | PeerId::Steam(n) => n,
        PeerId::Server => 0,
        PeerId::Raw(_) => u64::MAX,
    };
    SessionId(raw)
}

/// Add the lightyear plugins, the protocol, the wire-channel declarations, and
/// the host/client setup for `mode`.
pub(crate) fn build_networking(app: &mut App, mode: &NetworkMode) {
    // The transport-agnostic wire (codec, capture/apply, snapshots) the lightyear
    // ferry below drives. Both Host and Client need it.
    app.add_plugins(crate::wire::WirePlugin);

    // Prediction diagnostics — compiled only under the `net-diag` feature (off in
    // normal builds). Added on both peers so you can compare host (silent) vs client
    // while chasing jitter. Silence a net-diag build at runtime with `LUNCO_NET_DIAG=0`.
    #[cfg(feature = "net-diag")]
    app.add_plugins(crate::diagnostics::NetDiagnosticsPlugin);

    let tick = Duration::from_secs_f64(lunco_core::SECS_PER_TICK);
    match mode {
        NetworkMode::Host { port } => {
            #[cfg(not(target_family = "wasm"))]
            {
                app.insert_resource(NetworkRole::Host);
                app.insert_resource(IsServer(true));
                app.insert_resource(NetStatus {
                    role: NetworkRole::Host,
                    endpoint: format!(":{port}"),
                    peers: 0,
                    connected: true,
                });
                app.add_plugins(lightyear::prelude::server::ServerPlugins { tick_duration: tick });
                add_protocol(app);
                crate::server::setup_host(app, *port);
            }
            #[cfg(target_family = "wasm")]
            {
                let _ = port;
                warn!("Host mode is unsupported on wasm; use --connect");
            }
        }
        NetworkMode::Connect { server, client_id } => {
            app.insert_resource(NetworkRole::Client);
            app.insert_resource(IsServer(false));
            app.insert_resource(NetStatus {
                role: NetworkRole::Client,
                endpoint: server.to_string(),
                peers: 0,
                connected: false,
            });
            app.add_plugins(lightyear::prelude::client::ClientPlugins { tick_duration: tick });
            add_protocol(app);
            crate::client::setup_client(app, *server, *client_id);
        }
    }
}

/// Protocol must be registered *after* the lightyear plugins and *before*
/// spawning the connection entities.
fn add_protocol(app: &mut App) {
    app.add_plugins(crate::protocol::ProtocolPlugin);

    // Which wire channel each networked command rides (+ registers its capture
    // observer). Control inputs ride best-effort; structural commands ride the
    // reliable bus.
    app.declare_channel::<lunco_mobility::DriveRover>(WireChannel::ControlStream);
    app.declare_channel::<lunco_mobility::BrakeRover>(WireChannel::ControlStream);
    app.declare_channel::<lunco_avatar::PossessVessel>(WireChannel::CommandBus);
    app.declare_channel::<lunco_avatar::ReleaseVessel>(WireChannel::CommandBus);
    app.declare_channel::<lunco_sandbox_edit::commands::SpawnEntity>(WireChannel::CommandBus);
}
