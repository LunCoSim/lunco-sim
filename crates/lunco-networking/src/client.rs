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

use crate::protocol::{BulkChannel, CmdChannel, Frame, SnapChannel};
use crate::shared::{deserialize_env, serialize_env, PRIVATE_KEY, PROTOCOL_ID};

/// **Build-time**: register the client ferry systems, the disconnect observer,
/// the `JoinServer`/`LeaveServer` command observers, and (wasm) the URL-dialing
/// plugin. Called once when the networking plugin builds for a client-capable
/// process. Does **not** connect — connecting is [`spawn_client`], driven either
/// by auto-connect (`?connect=` / `--connect`) or the `JoinServer` command.
pub(crate) fn register_client_systems(app: &mut App) {
    // Both native and wasm use our hostname-URL dialing observer so that
    // a real CA cert for `sandbox.lunco.space` validates correctly (lightyear's
    // built-in `WebTransportClientIo` dials `https://{ip}`, which breaks SNI).
    app.add_plugins(crate::wt_client::WtUrlClientPlugin);
    // Host connection lost (server closed / netcode timeout): leave the
    // "connected" state instead of silently dead-reckoning stale snapshots.
    app.add_observer(on_client_disconnected);
    // MUST stay in `Update` — the lightyear ferry. FixedUpdate breaks the reliable
    // CmdChannel (see server.rs note).
    app.add_systems(
        Update,
        (
            // Mirror the host ferry order: recv → drain (SyncPlugin) → send, so an
            // inbound snapshot/handshake is processed and any command captured this
            // frame is sent the same frame. Intra-`Update` only (see the
            // reliable-flush note in server.rs).
            client_recv_inbox.before(crate::sync::drain_sync_inbox),
            client_send_outbox.after(crate::sync::drain_sync_inbox),
            update_client_netstatus,
        ),
    );
    register_all_commands(app);

    // Native deep-link plumbing. A clicked `luncosim://connect?…` link is always
    // *staged for confirmation* (never auto-dialed) — a planted link must not
    // silently redirect the session. Two sources feed the same `PendingConnect`:
    //   - the single-instance IPC inbox (forwarded links + the launch arg), when
    //     the binary wired `single_instance::acquire`; and
    //   - a fallback argv scan for builds that didn't wire the IPC.
    // wasm's `?connect=` stays auto-connect (web is trusted by design).
    #[cfg(not(target_family = "wasm"))]
    app.add_systems(
        Update,
        (
            crate::single_instance::drain_deep_link_inbox,
            seed_pending_from_deep_link_arg,
        ),
    );
}

/// Fallback (no IPC wired): scan argv once for a `luncosim:` deep link and stage
/// it in [`PendingConnect`]. Skipped when a [`DeepLinkInbox`](crate::single_instance::DeepLinkInbox)
/// exists — the IPC path already carries the launch arg, so this avoids a double
/// prompt. Runs every frame but guarded by a `done` latch + the inbox check.
#[cfg(not(target_family = "wasm"))]
fn seed_pending_from_deep_link_arg(
    inbox: Option<Res<crate::single_instance::DeepLinkInbox>>,
    mut pending: ResMut<lunco_core::session::PendingConnect>,
    mut done: Local<bool>,
) {
    if *done || inbox.is_some() {
        return;
    }
    *done = true;
    let Some(link) = std::env::args()
        .find(|a| a.starts_with(&format!("{}:", crate::connect_link::SCHEME)))
        .and_then(|a| crate::connect_link::parse_native(&a))
    else {
        return;
    };
    info!("[net] deep link → pending connect to {} (awaiting confirm)", link.address);
    pending.request = Some(lunco_core::session::PendingConnectRequest {
        address: link.address,
        digest: link.digest,
    });
}

/// **Runtime**: spawn the lightyear client entity for `server` (a `host:port`
/// string — hostname or `ip:port`) and start the link. Callable from a `Startup`
/// system (auto-connect) or the `JoinServer` command observer.
///
/// Both native and wasm use [`WtUrlClientIo`](crate::wt_client) which dials a
/// `https://{server}` URL directly. This lets the OS/browser resolve DNS and
/// present the hostname in the TLS SNI field, so a CA cert for
/// `sandbox.lunco.space` validates correctly on native builds. Netcode never
/// validates the transport address (the upstream check is disabled), so its
/// `server_addr` is just token data — a placeholder carrying the right port.
pub(crate) fn spawn_client(
    commands: &mut Commands,
    server: &str,
    client_id: u64,
    digest: &str,
) -> Entity {
    // Netcode needs a `SocketAddr` for its token, but never validates it against
    // the transport — the real dial is done by `WtUrlClientIo` below.
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
            // Match the 30 s server/QUIC timeout. A short 5 s reaper raced the
            // documented "don't race" rationale and killed a briefly-stalled
            // host during scene-load / cosim-compile (review M5).
            client_timeout_secs: 30,
            ..default()
        },
    )
    .expect("netcode client");

    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);
    let client = {
        let mut ent = commands.spawn((
            Name::new("LunCoClient"),
            Client::default(),
            Link::new(sim_latency_conditioner()),
            LocalAddr(client_addr),
            netcode,
        ));
        // Both native and wasm: dial the hostname URL so DNS resolves and SNI
        // carries the domain name — required for CA cert validation on native.
        // An explicit UI/command-supplied digest (Connect panel "Cert digest"
        // field) wins; empty ⇒ fall back to the ambient source (env on native,
        // URL `#hash` on wasm). Normalized to bare lowercase hex so a pasted
        // colon-separated host digest (`ab:cd:…`) decodes.
        let normalized = normalize_digest(digest);
        let certificate_digest = if normalized.is_empty() {
            client_cert_digest()
        } else {
            normalized
        };
        ent.try_insert(crate::wt_client::WtUrlClientIo {
            url: format!("https://{server}"),
            certificate_digest,
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
    /// Optional self-signed cert SHA-256 digest to pin (hex; colons/whitespace
    /// tolerated). Empty ⇒ fall back to the ambient digest source
    /// ([`client_cert_digest`]: `LUNCO_CERT_DIGEST` on native, the URL `#hash` on
    /// wasm). A browser joining a self-signed LAN host by IP must supply this.
    #[reflect(default)]
    pub digest: String,
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
    mut local: ResMut<LocalSession>,
) {
    // Drop any current connection first, then dial the new address.
    for e in &existing {
        commands.entity(e).try_despawn();
    }
    let address = crate::normalize_addr(&cmd.address);
    spawn_client(&mut commands, &address, crate::next_client_id(), &cmd.digest);
    *role = NetworkRole::Client;
    status.role = NetworkRole::Client;
    status.endpoint = address;
    status.connected = false;
    // Clear any session carried over from a prior connection. Until the new
    // host's Handshake lands, this client has no authoritative identity —
    // leaving the stale `LocalSession` in place makes `update_client_netstatus`
    // report `connected` and lets prediction/proxy systems act under the old
    // session during the connect window. `on_leave_server` does the same reset.
    local.0 = SessionId::LOCAL;
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
        commands.entity(e).try_despawn();
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
/// Parse the port out of a `host:port` string for the netcode placeholder
/// address (default `5888`). The host half is irrelevant — `WtUrlClientIo`
/// dials the full hostname URL; this is only used for the netcode token.
/// Strip a cert digest to bare lowercase hex. The host prints/logs it
/// colon-separated (`ab:cd:…`); `wt_client::from_hex` needs the colons and any
/// stray whitespace gone, so accept whatever the user pastes.
fn normalize_digest(d: &str) -> String {
    d.chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn port_of(server: &str) -> u16 {
    server
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(lunco_core::session::DEFAULT_HOST_PORT)
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
    // Deliberately KEEP role == Client (and the Disconnected Client entity + the
    // status endpoint) on an *involuntary* drop. The host-loss quiescence path
    // `force_kinematic_proxies` (lunco-sandbox-edit) is gated on role == Client and
    // re-pins proxies Kinematic so a Dynamic body re-inserted after host loss doesn't
    // free-fall through the terrain (the "-195 km cosim ball" fix). Flipping to
    // Standalone here makes that system no-op — so we don't. A clean user-initiated
    // exit (`on_leave_server`) is what returns to Standalone + despawns the entity.
    // Outbox growth meanwhile is bounded by `client_send_outbox` (clears when there's
    // no live sender).
    status.connected = false;
    status.peers = 0;
    warn!("[net] host connection lost — client disconnected");
}

/// Returns the cert digest for `WtUrlClientIo`.
///
/// - **Native**: always empty → `WtUrlClientIo` uses the system CA store.
///   Pass `--connect sandbox.lunco.space` and the Let's Encrypt cert validates.
///   For localhost dev with a self-signed cert you'll need to pass the digest
///   another way (e.g. an env var); that path isn't wired yet.
/// - **Browser**: read from the URL hash (`#<digest>`). Empty hash ⇒ normal CA
///   validation. Non-empty ⇒ pin for a self-signed dev cert (localhost only).
fn client_cert_digest() -> String {
    #[cfg(not(target_family = "wasm"))]
    {
        // Production: system CA store validates the server cert normally.
        // Dev override: set LUNCO_CERT_DIGEST=<hex> to pin a self-signed cert.
        std::env::var("LUNCO_CERT_DIGEST").unwrap_or_default()
    }
    #[cfg(target_family = "wasm")]
    {
        // Digest in the URL hash (`#<hex>`). Stripped of colons so lightyear's
        // hex decoder doesn't panic on the colon-separated form the host logs.
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
}

/// Test knob (`LUNCO_SIM_LATENCY_MS=<ms>`): attach a receive-side link conditioner
/// that delays inbound payloads (snapshots, spawns) by the given milliseconds — so
/// the client sees the host's authoritative state that much later, i.e. its
/// rendered rover lags the local input by ~this ping. Used to validate prediction
/// (the render-lead) at realistic 200–500 ms latencies on localhost. Off when
/// unset or 0. Only the client side is conditioned; input still reaches the host
/// fast, so the input→display latency the render-lead must hide is ≈ this value.
pub(crate) fn sim_latency_conditioner() -> Option<RecvLinkConditioner> {
    let ms: u64 = std::env::var("LUNCO_SIM_LATENCY_MS").ok()?.parse().ok()?;
    if ms == 0 {
        return None;
    }
    warn!("[net] SIMULATED inbound latency ENABLED: {ms} ms (LUNCO_SIM_LATENCY_MS)");
    Some(RecvLinkConditioner::new(LinkConditionerConfig::new(
        std::time::Duration::from_millis(ms),
        std::time::Duration::ZERO,
        0.0,
    )))
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
        // No live Client sender (still connecting, or dropped before the role
        // reset lands): drop the queued commands instead of letting `capture_command`
        // grow the outbox unbounded while there's nothing to ferry them to.
        outbox.0.clear();
        return;
    };
    for (channel, env) in outbox.0.drain(..) {
        let Some(bytes) = serialize_env(&env) else {
            continue;
        };
        let frame = Frame(bytes);
        match channel {
            SyncChannel::ControlStream => sender.send::<SnapChannel>(frame),
            SyncChannel::BulkData => sender.send::<BulkChannel>(frame),
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
