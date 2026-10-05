//! Pure-client adapter: WebTransport connect + outbox→server / server→inbox
//! ferry. Compiles for native and wasm.

use bevy::prelude::*;
use lightyear::netcode::NetcodeClient;
use lightyear::netcode::client_plugin::NetcodeConfig;
// `Authentication` comes from `lightyear::prelude::*` (glob-imported below).
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

use lunco_command_contracts::{SessionId, SyncChannel};
use lunco_core_session::{
    ClientConnection, LocalSession, NetDisconnectRequest, NetStatus, NetworkRole,
};

use crate::protocol::{BulkChannel, CmdChannel, Frame, SnapChannel};
use crate::shared::{PROTOCOL_ID, netcode_key};
use lunco_networking_sync::codec::{deserialize_env, serialize_env};
use lunco_networking_sync::sync::{SyncInbox, SyncOutbox};

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
            client_recv_inbox.before(lunco_networking_sync::sync::drain_sync_inbox),
            client_send_outbox.after(lunco_networking_sync::sync::drain_sync_inbox),
            update_client_netstatus,
        )
            // Standalone pays nothing for the ferry it can't use (C12). Safe to
            // gate: `capture_command` only fills the outbox when role == Client,
            // so no producer runs while this is off. An involuntary drop keeps
            // role == Client (see `on_client_disconnected`), so the outbox-
            // clearing arm of `client_send_outbox` still runs then.
            .run_if(lunco_networking_sync::wire_is_live),
    );
    app.add_observer(on_net_disconnect_request);
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
            // The latch lives in `run_if` (the gate.rs pattern), not inside the
            // system — after the one scan, the scheduler skips it entirely
            // instead of paying a per-frame no-op call (C13). Condition order
            // matters: the inbox check comes FIRST so `run_once` only latches
            // when the scan actually runs (IPC-wired builds skip it forever —
            // the IPC path already carries the launch arg).
            seed_pending_from_deep_link_arg.run_if(lunco_core_runtime::gate::tracked(
                "net: deep-link argv scan",
                not(resource_exists::<crate::single_instance::DeepLinkInbox>).and_then(run_once),
            )),
        ),
    );
}

/// Fallback (no IPC wired): scan argv once for a `luncosim:` deep link and stage
/// it in [`PendingConnect`](crate::connection_state::PendingConnect). Skipped when a
/// [`DeepLinkInbox`](crate::single_instance::DeepLinkInbox)
/// exists — the IPC path already carries the launch arg, so this avoids a double
/// prompt. The once-only latch and the inbox check are `run_if` conditions at
/// the registration site — the system body runs at most once per process.
#[cfg(not(target_family = "wasm"))]
fn seed_pending_from_deep_link_arg(mut pending: ResMut<crate::connection_state::PendingConnect>) {
    let Some(link) = std::env::args()
        .find(|a| a.starts_with(&format!("{}:", crate::connect_link::SCHEME)))
        .and_then(|a| crate::connect_link::parse_native(&a))
    else {
        return;
    };
    info!(
        "[net] deep link → pending connect to {} (awaiting confirm)",
        link.address
    );
    pending.request = Some(crate::connection_state::PendingConnectRequest {
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
/// Connection preparation validates all user input before session mutation.
pub(crate) struct PreparedClient {
    netcode: NetcodeClient,
    io: crate::wt_client::WtUrlClientIo,
}

pub(crate) fn prepare_client(
    server: &str,
    client_id: u64,
    digest: &str,
) -> Result<PreparedClient, String> {
    let certificate_digest = if digest.trim().is_empty() {
        client_cert_digest()?
    } else {
        crate::wt_client::parse_certificate_digest(digest)?
    };
    let auth = Authentication::Manual {
        server_addr: SocketAddr::from(([127, 0, 0, 1], port_of(server))),
        client_id,
        private_key: netcode_key()?,
        protocol_id: PROTOCOL_ID,
    };
    let netcode = NetcodeClient::new(
        auth,
        NetcodeConfig {
            client_timeout_secs: 30,
            ..default()
        },
    )
    .map_err(|error| format!("netcode client setup failed for '{server}': {error}"))?;
    Ok(PreparedClient {
        netcode,
        io: crate::wt_client::WtUrlClientIo {
            url: format!("https://{server}"),
            certificate_digest,
        },
    })
}

pub(crate) fn spawn_client(commands: &mut Commands, prepared: PreparedClient) -> Entity {
    let PreparedClient { netcode, io } = prepared;
    info!("[net] connecting to {}", io.url);
    let client = commands
        .spawn((
            Name::new("LunCoClient"),
            Client::default(),
            Link::new(sim_latency_conditioner()),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)),
            netcode,
            io,
        ))
        .id();
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
    mut connection: ResMut<ClientConnection>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) -> Result<lunco_command_contracts::Ack, lunco_command_contracts::Reject> {
    let address = crate::normalize_addr(&cmd.address);
    let prepared =
        prepare_client(&address, crate::next_client_id(), &cmd.digest).map_err(|error| {
            warn!("[net] join rejected: {error}");
            status.last_error = error.clone();
            lunco_command_contracts::Reject::InvalidOp(error)
        })?;
    for e in &existing {
        commands.entity(e).try_despawn();
    }
    let entity = spawn_client(&mut commands, prepared);
    status.last_error.clear();
    connection.0 = Some(entity);
    // Standalone→Client: authority follows the role automatically
    // (`is_authoritative()` is now false), so a joined client stops minting ids —
    // no separate flag to flip in lock-step.
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
    // Keep new offline edits under the durable install author until the
    // handshake supplies the connection-bound author. The handshake then
    // atomically rebinds those entries and their DAG references.
    if let Some(journal) = journal {
        journal.set_local_author(lunco_networking_sync::journal_plane::local_author_id());
    }
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

#[lunco_core::on_command(LeaveServer)]
fn on_leave_server(
    trigger: On<LeaveServer>,
    mut commands: Commands,
    existing: Query<Entity, With<Client>>,
    mut role: ResMut<NetworkRole>,
    mut status: ResMut<NetStatus>,
    mut local: ResMut<LocalSession>,
    mut connection: ResMut<ClientConnection>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    for e in &existing {
        commands.entity(e).try_despawn();
    }
    // Back to single-player: `Standalone` is authoritative again, so the local
    // peer resumes minting ids automatically (mirrors the idle-local startup arm).
    connection.0 = None;
    *role = NetworkRole::Standalone;
    status.role = NetworkRole::Standalone;
    status.connected = false;
    status.peers = 0;
    status.endpoint = String::new();
    local.0 = SessionId::LOCAL;
    if let Some(journal) = journal {
        journal.set_local_author(lunco_networking_sync::journal_plane::local_author_id());
    }
    let _ = cmd;
    info!("[net] left session — back to local");
}

lunco_core::register_commands!(on_join_server, on_leave_server);

/// Translate the always-on application disconnect request into the adapter's
/// typed command. The sync runtime uses the same request when a peer fails a
/// mandatory handshake contract, keeping it transport-neutral.
fn on_net_disconnect_request(_trigger: On<NetDisconnectRequest>, mut commands: Commands) {
    commands.trigger(LeaveServer {});
}

/// Parse the port out of a `host:port` string for the netcode placeholder
/// address (default `5888`). The host half is irrelevant — `WtUrlClientIo`
/// dials the full hostname URL; this is only used for the netcode token.
fn port_of(server: &str) -> u16 {
    server
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(lunco_core_session::DEFAULT_HOST_PORT)
}

/// Reflect the handshake (non-zero [`LocalSession`]) into [`NetStatus`] so the
/// status bar flips from "connecting…" to "connected".
fn update_client_netstatus(local: Res<LocalSession>, mut status: ResMut<NetStatus>) {
    let connected = local.0.0 != 0;
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
    trigger: On<Add, Disconnected>,
    mut local: ResMut<LocalSession>,
    mut connection: ResMut<ClientConnection>,
    mut status: ResMut<NetStatus>,
    holds: Option<Res<lunco_cosim_core::PortHolds>>,
    global_ids: Query<&lunco_core::GlobalEntityId>,
    mut commands: Commands,
) {
    if connection.0 != Some(trigger.entity) {
        return;
    }
    connection.0 = None;
    if let Some(holds) = holds {
        let mut stable_targets = Vec::new();
        let mut missing_id_targets = Vec::new();
        for entity in holds.held_entities() {
            if let Ok(global_id) = global_ids.get(entity) {
                stable_targets.push((global_id.get(), entity));
            } else {
                missing_id_targets.push(entity);
            }
        }
        stable_targets.sort_unstable_by_key(|(global_id, _)| *global_id);
        for (_, entity) in stable_targets {
            // Release predicted/local holds at the next fixed tick so a future
            // owner starts from authored wiring rather than stale client intent.
            commands.trigger(lunco_cosim_core::commands::ReleaseControlInputs { target: entity });
        }
        for entity in missing_id_targets {
            commands.trigger(lunco_cosim_core::commands::ReleaseControlInputs { target: entity });
        }
    }
    local.0 = SessionId::LOCAL;
    // Deliberately KEEP role == Client (and the Disconnected Client entity + the
    // status endpoint) on an *involuntary* drop. The host-loss quiescence path
    // `force_kinematic_proxies` (this crate) is gated on role == Client and
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
/// Native environment or browser URL pin; empty means normal unpinned mode.
/// Invalid supplied values reject connection preparation.
fn client_cert_digest() -> Result<Option<[u8; 32]>, String> {
    #[cfg(not(target_family = "wasm"))]
    {
        let value = std::env::var_os("LUNCO_CERT_DIGEST")
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| "LUNCO_CERT_DIGEST is not valid UTF-8".to_string())
            })
            .transpose()?;
        crate::wt_client::parse_certificate_digest(value.as_deref().unwrap_or(""))
    }
    #[cfg(target_family = "wasm")]
    {
        let window = web_sys::window()
            .ok_or_else(|| "certificate pin requires a browser window".to_string())?;
        let hash = window
            .location()
            .hash()
            .map_err(|_| "cannot read browser certificate pin".to_string())?;
        crate::wt_client::parse_certificate_digest(hash.trim_start_matches('#'))
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
    connection: Res<ClientConnection>,
    mut q: Query<(Entity, &mut MessageSender<Frame>), (With<Client>, With<Connected>)>,
) {
    if outbox.0.is_empty() {
        return;
    }
    let Some((_, mut sender)) = q
        .iter_mut()
        .find(|(entity, _)| Some(*entity) == connection.0)
    else {
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
    connection: Res<ClientConnection>,
    mut q: Query<(Entity, &mut MessageReceiver<Frame>), (With<Client>, With<Connected>)>,
    mut inbox: ResMut<SyncInbox>,
) {
    let Some((entity, mut receiver)) = q
        .iter_mut()
        .find(|(entity, _)| Some(*entity) == connection.0)
    else {
        return;
    };
    for frame in receiver.receive() {
        if let Some(env) = deserialize_env(&frame.0) {
            if inbox.connection != Some(entity) {
                inbox.entries.clear();
            }
            inbox.connection = Some(entity);
            inbox
                .entries
                .push(lunco_networking_sync::sync::SyncInboxEntry {
                    sender: SessionId::LOCAL,
                    connection: entity,
                    envelope: env,
                });
        }
    }
}
