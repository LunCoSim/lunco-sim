//! Mode dispatch + helpers shared by the host and client adapters.

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::*;
use crate::sync::{DeclareChannelExt, SyncEnvelope};
use lunco_core::{IsServer, NetStatus, NetworkRole, SessionId, SyncChannel};

use crate::NetworkMode;

/// Dev protocol id + key. Host and client MUST agree. (Localhost MVP; a real
/// deployment would load a real key.)
pub(crate) const PROTOCOL_ID: u64 = 0x004C_554E_434F_0001; // "LUNCO"
pub(crate) const PRIVATE_KEY: [u8; 32] = [0u8; 32];

// Wire envelope codec = **bincode** (binary, positional — no field names). This is
// the hot 20 Hz snapshot path; JSON here roughly doubled the byte count. The inner
// Reflect command payload (`SyncCommand.data`) still uses serde_json — bincode just
// frames the envelope around it. Host and client always build together, so the
// schema-coupled (non-self-describing) binary format is safe.
/// Hard cap on a single decoded envelope. A hostile/corrupt frame whose bincode
/// length prefix claims a huge `Vec`/`String` would otherwise make bincode
/// pre-allocate that many bytes before reading a single field (memory DoS). 16
/// MiB comfortably exceeds a full connect-baseline snapshot while bounding the
/// blast radius. `with_fixint_encoding()` matches the format the free
/// `bincode::serialize` below emits (the free fns are fixint; `options()` defaults
/// to varint), so adding `.with_limit()` is purely a decode-side guard — the wire
/// bytes are unchanged.
pub(crate) const MAX_ENVELOPE_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn serialize_env(env: &SyncEnvelope) -> Option<Vec<u8>> {
    match bincode::serialize(env) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            warn!("[sync] envelope encode failed: {e}");
            None
        }
    }
}

pub(crate) fn deserialize_env(bytes: &[u8]) -> Option<SyncEnvelope> {
    // Reject oversize frames up front, then cap bincode's internal allocation so
    // a smaller frame with a lying length prefix can't pre-allocate gigabytes.
    if bytes.len() as u64 > MAX_ENVELOPE_BYTES {
        warn!("[sync] envelope decode rejected: {} bytes exceeds cap", bytes.len());
        return None;
    }
    use bincode::Options;
    let opts = bincode::options()
        .with_fixint_encoding()
        .with_limit(MAX_ENVELOPE_BYTES);
    match opts.deserialize(bytes) {
        Ok(env) => Some(env),
        Err(e) => {
            warn!("[sync] envelope decode failed ({} bytes): {e}", bytes.len());
            None
        }
    }
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
///
/// `None` and `Some(Connect)` both build a **client-capable** app (the runtime
/// `JoinServer` path needs `ClientPlugins` present from startup — Bevy can't add
/// plugins later); `Some(Connect)` additionally auto-connects at `Startup`,
/// while `None` stays idle (`NetworkRole::Standalone`, single-player) until a
/// `JoinServer` command dials a server. `Some(Host)` builds the listen-server.
pub(crate) fn build_networking(app: &mut App, mode: &Option<NetworkMode>) {
    // The transport-agnostic wire (codec, capture/apply, snapshots) the lightyear
    // ferry below drives. Both Host and Client need it.
    app.add_plugins(crate::sync::SyncPlugin);

    // Prediction diagnostics — compiled only under the `net-diag` feature (off in
    // normal builds). Added on both peers so you can compare host (silent) vs client
    // while chasing jitter. Silence a net-diag build at runtime with `LUNCO_NET_DIAG=0`.
    #[cfg(feature = "net-diag")]
    app.add_plugins(crate::diagnostics::NetDiagnosticsPlugin);

    let tick = Duration::from_secs_f64(lunco_core::SECS_PER_TICK);
    match mode {
        Some(NetworkMode::Host { port }) => {
            #[cfg(not(target_family = "wasm"))]
            {
                app.insert_resource(NetworkRole::Host);
                app.insert_resource(IsServer(true));
                app.insert_resource(NetStatus {
                    role: NetworkRole::Host,
                    endpoint: format!(":{port}"),
                    peers: 0,
                    connected: true,
                    ..Default::default()
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
        // None (idle local) or Some(Connect) — both are client-capable.
        client_mode => {
            app.insert_resource(IsServer(false));
            app.add_plugins(lightyear::prelude::client::ClientPlugins { tick_duration: tick });
            add_protocol(app);
            // Ferry systems, disconnect observer, JoinServer/LeaveServer commands,
            // and (wasm) the hostname-URL dialing plugin.
            crate::client::register_client_systems(app);

            if let Some(NetworkMode::Connect { server, client_id }) = client_mode {
                // Auto-connect (CLI `--connect` / browser `?connect=`).
                app.insert_resource(NetworkRole::Client);
                app.insert_resource(NetStatus {
                    role: NetworkRole::Client,
                    endpoint: server.clone(),
                    peers: 0,
                    connected: false,
                    ..Default::default()
                });
                let server = server.clone();
                let client_id = *client_id;
                app.add_systems(Startup, move |mut commands: Commands| {
                    crate::client::spawn_client(&mut commands, &server, client_id);
                });
            } else {
                // Idle local sandbox — single-player until `JoinServer`.
                app.insert_resource(NetworkRole::Standalone);
                app.insert_resource(NetStatus {
                    role: NetworkRole::Standalone,
                    endpoint: String::new(),
                    peers: 0,
                    connected: false,
                    ..Default::default()
                });
            }
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
    app.declare_channel::<lunco_mobility::DriveRover>(SyncChannel::ControlStream);
    app.declare_channel::<lunco_mobility::BrakeRover>(SyncChannel::ControlStream);
    app.declare_channel::<lunco_avatar::PossessVessel>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_avatar::ReleaseVessel>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_avatar::UpdateProfile>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_sandbox_edit::commands::SpawnEntity>(SyncChannel::CommandBus);
    app.declare_channel::<lunco_obstacle_field::plugin::UpdateObstacleFieldSpec>(SyncChannel::CommandBus);
}
