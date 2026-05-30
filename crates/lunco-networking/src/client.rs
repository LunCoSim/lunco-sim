//! Pure-client adapter: WebTransport connect + outbox→server / server→inbox
//! ferry. Compiles for native and wasm.

use bevy::prelude::*;
use lightyear::netcode::client_plugin::NetcodeConfig;
use lightyear::netcode::NetcodeClient;
// `Authentication` comes from `lightyear::prelude::*` (glob-imported below).
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

use lunco_api::{WireInbox, WireOutbox};
use lunco_core::{SessionId, WireChannel};

use crate::protocol::{CmdChannel, Frame, SnapChannel};
use crate::shared::{deserialize_env, serialize_env, PRIVATE_KEY, PROTOCOL_ID};

/// Spawn the client entity and trigger `Connect`, then register the ferry
/// systems.
pub(crate) fn setup_client(app: &mut App, server_addr: SocketAddr, client_id: u64) {
    let auth = Authentication::Manual {
        server_addr,
        client_id,
        private_key: PRIVATE_KEY,
        protocol_id: PROTOCOL_ID,
    };
    let netcode = NetcodeClient::new(
        auth,
        NetcodeConfig {
            client_timeout_secs: 5,
            ..default()
        },
    )
    .expect("netcode client");

    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);
    let client = app
        .world_mut()
        .spawn((
            Name::new("LunCoClient"),
            Client::default(),
            Link::new(None),
            LocalAddr(client_addr),
            PeerAddr(server_addr),
            netcode,
            WebTransportClientIo {
                certificate_digest: client_cert_digest(),
            },
        ))
        .id();
    info!("[net] connecting to {server_addr} as client {client_id}");

    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Connect { entity: client });
    });
    app.add_systems(Update, (client_send_outbox, client_recv_inbox));
}

/// Native: empty digest + the `dangerous-configuration` feature ⇒ no cert
/// validation (localhost dev). The browser path reads the digest from the URL.
#[cfg(not(target_family = "wasm"))]
fn client_cert_digest() -> String {
    String::new()
}

/// Browser: the digest is supplied in the connect URL hash (`#<digest>`), which
/// dodges the spike's baked-digest staleness (SPIKE_PH0 §dev-cert-gotchas #4).
#[cfg(target_family = "wasm")]
fn client_cert_digest() -> String {
    web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .map(|h| h.trim_start_matches('#').to_string())
        .unwrap_or_default()
}

/// Drain outgoing commands to the server on their declared channel.
fn client_send_outbox(
    mut outbox: ResMut<WireOutbox>,
    mut q: Query<&mut MessageSender<Frame>, With<Client>>,
) {
    if outbox.0.is_empty() {
        return;
    }
    let Some(mut sender) = q.iter_mut().next() else {
        return;
    };
    for (channel, env) in outbox.0.drain(..) {
        let Some(bytes) = serialize_env(&env) else {
            continue;
        };
        let frame = Frame(bytes);
        match channel {
            WireChannel::ControlStream => sender.send::<SnapChannel>(frame),
            _ => sender.send::<CmdChannel>(frame),
        }
    }
}

/// Pull inbound frames (handshake, snapshots, spawn replication) into the inbox.
/// Sender session is irrelevant on a client (everything is host-attributed).
fn client_recv_inbox(
    mut q: Query<&mut MessageReceiver<Frame>, With<Client>>,
    mut inbox: ResMut<WireInbox>,
) {
    let Some(mut receiver) = q.iter_mut().next() else {
        return;
    };
    for frame in receiver.receive() {
        if let Some(env) = deserialize_env(&frame.0) {
            inbox.0.push((SessionId(0), env));
        }
    }
}
