//! Pure-client adapter: WebTransport connect + outbox→server / server→inbox
//! ferry. Compiles for native and wasm.

use bevy::prelude::*;
use lightyear::netcode::client_plugin::NetcodeConfig;
use lightyear::netcode::NetcodeClient;
// `Authentication` comes from `lightyear::prelude::*` (glob-imported below).
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

use crate::sync::{SyncInbox, SyncOutbox};
use lunco_core::{LocalSession, NetStatus, SessionId, SyncChannel};

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
    // Host connection lost (server closed / netcode timeout): leave the
    // "connected" state instead of silently dead-reckoning stale snapshots.
    app.add_observer(on_client_disconnected);
    // MUST stay in `Update` — the lightyear ferry. FixedUpdate breaks the reliable
    // CmdChannel (see server.rs note).
    app.add_systems(
        Update,
        (client_send_outbox, client_recv_inbox, update_client_netstatus),
    );
}

/// Reflect the handshake (non-zero [`LocalSession`]) into [`NetStatus`] so the
/// status bar flips from "connecting…" to "connected".
fn update_client_netstatus(local: Res<LocalSession>, mut status: ResMut<NetStatus>) {
    let connected = local.0 .0 != 0;
    if status.connected != connected {
        status.connected = connected;
        status.peers = u32::from(connected);
    }
}

/// The client connection dropped (host closed, or netcode `client_timeout_secs`
/// elapsed with no server). Lightyear adds [`Disconnected`] to our `Client`
/// entity; mirror the server's `on_server_disconnected` and reset session +
/// status so the UI leaves "connected" and the prediction/proxy systems (which
/// key off [`LocalSession`]/role) stop acting on now-stale snapshots.
/// `update_client_netstatus` then keeps `NetStatus` consistent with the cleared
/// `LocalSession` on subsequent frames.
fn on_client_disconnected(
    _trigger: On<Add, Disconnected>,
    mut local: ResMut<LocalSession>,
    mut status: ResMut<NetStatus>,
) {
    local.0 = SessionId::LOCAL;
    status.connected = false;
    status.peers = 0;
    warn!("[net] host connection lost — client disconnected");
}

/// Native: empty digest + the `dangerous-configuration` feature ⇒ no cert
/// validation (localhost dev). The browser path reads the digest from the URL.
#[cfg(not(target_family = "wasm"))]
fn client_cert_digest() -> String {
    String::new()
}

/// Browser: the digest is supplied in the connect URL hash (`#<digest>`), which
/// dodges the spike's baked-digest staleness (SPIKE_PH0 §dev-cert-gotchas #4).
///
/// Normalized to **bare lowercase hex**: lightyear hex-decodes this string, so
/// the colon-separated form the host logs (`ba:ae:…`) must have its separators
/// stripped or it panics ("Hex string does not have an even number of digits").
/// An empty hash ⇒ empty digest ⇒ no `serverCertificateHashes` ⇒ the browser
/// does normal CA validation (the production path with a real cert on a domain).
#[cfg(target_family = "wasm")]
fn client_cert_digest() -> String {
    web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .map(|h| {
            h.trim_start_matches('#')
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .flat_map(char::to_lowercase)
                .collect()
        })
        .unwrap_or_default()
}

/// Drain outgoing commands to the server on their declared channel.
fn client_send_outbox(
    mut outbox: ResMut<SyncOutbox>,
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
            SyncChannel::ControlStream => sender.send::<SnapChannel>(frame),
            _ => sender.send::<CmdChannel>(frame),
        }
    }
}

/// Pull inbound frames (handshake, snapshots, spawn replication) into the inbox.
/// Sender session is irrelevant on a client (everything is host-attributed).
fn client_recv_inbox(
    mut q: Query<&mut MessageReceiver<Frame>, With<Client>>,
    mut inbox: ResMut<SyncInbox>,
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
