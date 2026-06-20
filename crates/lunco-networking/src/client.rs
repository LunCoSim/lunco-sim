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
use lunco_core::{LocalSession, NetStatus, NetworkRole, SessionId, SyncChannel};

use crate::protocol::{CmdChannel, Frame, SnapChannel};
use crate::shared::{deserialize_env, serialize_env, PRIVATE_KEY, PROTOCOL_ID};

/// **Build-time**: register the client ferry systems, the disconnect observer,
/// the `JoinServer`/`LeaveServer` command observers, and (wasm) the URL-dialing
/// plugin. Called once when the networking plugin builds for a client-capable
/// process. Does **not** connect — connecting is [`spawn_client`], driven either
/// by auto-connect (`?connect=` / `--connect`) or the `JoinServer` command.
pub(crate) fn register_client_systems(app: &mut App) {
    // Browser: register our hostname-URL dialing observer (lightyear's
    // ClientPlugins already added the aeronet WebTransport plugin).
    #[cfg(target_family = "wasm")]
    app.add_plugins(crate::wt_client::WtUrlClientPlugin);
    // Host connection lost (server closed / netcode timeout): leave the
    // "connected" state instead of silently dead-reckoning stale snapshots.
    app.add_observer(on_client_disconnected);
    // MUST stay in `Update` — the lightyear ferry. FixedUpdate breaks the reliable
    // CmdChannel (see server.rs note).
    app.add_systems(
        Update,
        (client_send_outbox, client_recv_inbox, update_client_netstatus),
    );
    register_all_commands(app);
}

/// **Runtime**: spawn the lightyear client entity for `server` (a `host:port`
/// string — hostname or `ip:port`) and start the link. Callable from a `Startup`
/// system (auto-connect) or the `JoinServer` command observer.
///
/// The transport IO differs by target: **native** keeps lightyear's
/// `WebTransportClientIo` (`PeerAddr`-driven, `https://{ip}`), fine for CLI dev
/// that connects by IP. **wasm** uses our [`WtUrlClientIo`](crate::wt_client)
/// which dials the hostname URL directly, so a real CA cert on a domain
/// validates with no digest. Netcode never validates the transport address (the
/// upstream check is disabled), so its `server_addr` is just token data — on
/// wasm a placeholder carrying the right port is enough.
pub(crate) fn spawn_client(commands: &mut Commands, server: &str, client_id: u64) -> Entity {
    #[cfg(not(target_family = "wasm"))]
    let server_addr = crate::resolve_socket_addr(server);
    #[cfg(target_family = "wasm")]
    let server_addr = SocketAddr::from(([127, 0, 0, 1], port_of(server)));

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
    let client = {
        let mut ent = commands.spawn((
            Name::new("LunCoClient"),
            Client::default(),
            Link::new(None),
            LocalAddr(client_addr),
            netcode,
        ));
        // Native: lightyear's IP-dialing IO. wasm: our hostname-URL IO.
        #[cfg(not(target_family = "wasm"))]
        ent.insert((
            PeerAddr(server_addr),
            WebTransportClientIo {
                certificate_digest: client_cert_digest(),
            },
        ));
        #[cfg(target_family = "wasm")]
        ent.insert(crate::wt_client::WtUrlClientIo {
            url: format!("https://{server}"),
            certificate_digest: client_cert_digest(),
        });
        ent.id()
    };
    info!("[net] connecting to {server} as client {client_id}");
    commands.trigger(Connect { entity: client });
    client
}

/// Join a networked session at `address` (`host:port` — a hostname like
/// `lunica.lunco.space:5888` or an `ip:port`). The same typed command the
/// in-sim *Connect* button, the HTTP API, MCP, and the CLI all dispatch — the
/// networking internals establish the connection. Replaces any current one.
#[lunco_core::Command(default)]
pub struct JoinServer {
    pub address: String,
}

/// Leave the current session and return to single-player (local sandbox).
#[lunco_core::Command(default)]
pub struct LeaveServer {}

#[lunco_core::on_command(JoinServer)]
fn on_join_server(
    trigger: On<JoinServer>,
    mut commands: Commands,
    existing: Query<Entity, With<Client>>,
    mut role: ResMut<NetworkRole>,
    mut status: ResMut<NetStatus>,
) {
    // Drop any current connection first, then dial the new address.
    for e in &existing {
        commands.entity(e).despawn();
    }
    let address = crate::normalize_addr(&cmd.address);
    spawn_client(&mut commands, &address, crate::next_client_id());
    *role = NetworkRole::Client;
    status.role = NetworkRole::Client;
    status.endpoint = address;
    status.connected = false;
}

#[lunco_core::on_command(LeaveServer)]
fn on_leave_server(
    trigger: On<LeaveServer>,
    mut commands: Commands,
    existing: Query<Entity, With<Client>>,
    mut role: ResMut<NetworkRole>,
    mut status: ResMut<NetStatus>,
    mut local: ResMut<LocalSession>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    *role = NetworkRole::Standalone;
    status.role = NetworkRole::Standalone;
    status.connected = false;
    status.peers = 0;
    status.endpoint = String::new();
    local.0 = SessionId::LOCAL;
    let _ = cmd;
    info!("[net] left session — back to local");
}

lunco_core::register_commands!(on_join_server, on_leave_server);

/// Parse the port out of a `host:port` string for the wasm netcode placeholder
/// address (default `5888`). The host half is irrelevant — the browser dials the
/// hostname URL via [`WtUrlClientIo`](crate::wt_client::WtUrlClientIo), not this
/// `SocketAddr`.
#[cfg(target_family = "wasm")]
fn port_of(server: &str) -> u16 {
    server
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5888)
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
