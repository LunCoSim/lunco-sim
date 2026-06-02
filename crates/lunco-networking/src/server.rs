//! Host (listen-server) adapter: WebTransport server + connection lifecycle +
//! outbox→clients / clients→inbox ferry. Native only.

use bevy::prelude::*;
use lightyear::netcode::server_plugin::NetcodeConfig;
use lightyear::netcode::NetcodeServer;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

use lunco_api::{
    HandshakeMsg, OwnershipMsg, SnapshotEntry, SnapshotMsg, SpawnReplicationMsg, WireEnvelope,
    WireInbox, WireOutbox,
};
use lunco_core::{
    GlobalEntityId, NetReplicate, NetSpawn, NetStatus, SessionRegistry, SimTick, WireChannel,
};

use crate::protocol::{CmdChannel, Frame, SnapChannel};
use crate::shared::{deserialize_env, peer_to_session, serialize_env, PRIVATE_KEY, PROTOCOL_ID};

/// Spawn the server entity (self-signed WebTransport cert), trigger `Start`,
/// and register the lifecycle observers + ferry systems.
pub(crate) fn setup_host(app: &mut App, port: u16) {
    // ECDSA-P256 self-signed cert. Print + persist the digest so browser
    // clients can pass it in the connect URL (`#<digest>`).
    let identity = Identity::self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
        .expect("self-signed certificate");
    let digest = format!("{}", identity.certificate_chain().as_slice()[0].hash());
    info!("🔐 WebTransport cert digest: {digest}");
    let digest_path = std::env::temp_dir().join("lunco_cert_digest.txt");
    if std::fs::write(&digest_path, &digest).is_ok() {
        info!("🔐 digest written to {}", digest_path.display());
    }

    let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
    let server = app
        .world_mut()
        .spawn((
            Name::new("LunCoServer"),
            NetcodeServer::new(NetcodeConfig {
                protocol_id: PROTOCOL_ID,
                private_key: PRIVATE_KEY,
                ..default()
            }),
            LocalAddr(server_addr),
            WebTransportServerIo {
                certificate: identity,
            },
        ))
        .id();
    info!("[net] host listening on {server_addr}");

    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Start { entity: server });
    });
    app.add_observer(on_server_connected);
    app.add_observer(on_server_disconnected);
    // NOTE: these MUST stay in `Update` (the lightyear message ferry). Moving them
    // to `FixedUpdate` silently breaks the RELIABLE `CmdChannel` (client→host
    // PossessVessel/SpawnEntity never arrive) — lightyear's reliable flush is
    // schedule-sensitive. The render-throttle-when-unfocused issue (this peer's
    // `Update` drops to ~5 Hz, so the ferry sends snapshots in bursts) is handled
    // WITHOUT touching the ferry: snapshot GENERATION (`gather_snapshot`) runs in
    // `FixedUpdate` at a steady 20 Hz and tick-stamps each batch, and the client
    // interpolates in tick-space (`interpolate_proxies`), so bursty sends still
    // render smoothly. See `lunco_api::wire::WirePlugin`.
    app.add_systems(
        Update,
        (
            host_send_outbox,
            host_recv_inbox,
            update_host_netstatus,
            broadcast_ownership,
        ),
    );
}

/// Broadcast the authoritative ownership table to all clients whenever it
/// changes (a claim or release). Reliable channel so the who-owns-what view
/// stays consistent. Host-only (registered in `setup_host`).
fn broadcast_ownership(registry: Res<SessionRegistry>, mut outbox: ResMut<WireOutbox>) {
    if !registry.is_changed() {
        return;
    }
    outbox.0.push((
        WireChannel::CommandBus,
        WireEnvelope::Ownership(OwnershipMsg {
            entries: registry.snapshot(),
        }),
    ));
}

/// Mirror the live connected-client count into [`NetStatus`] for the status bar.
fn update_host_netstatus(
    q: Query<(), (With<ClientOf>, With<Connected>)>,
    mut status: ResMut<NetStatus>,
) {
    let n = q.iter().count() as u32;
    if status.peers != n {
        status.peers = n;
    }
}

/// Serialize + send one envelope to `target` on the channel matching its
/// `WireChannel`.
fn server_send(
    sender: &mut ServerMultiMessageSender,
    server: &Server,
    target: &NetworkTarget,
    channel: WireChannel,
    env: &WireEnvelope,
) {
    let Some(bytes) = serialize_env(env) else {
        return;
    };
    let frame = Frame(bytes);
    let _ = match channel {
        WireChannel::ControlStream => sender.send::<Frame, SnapChannel>(&frame, server, target),
        _ => sender.send::<Frame, CmdChannel>(&frame, server, target),
    };
}

/// New client confirmed: hand it its session id + current tick, and replay
/// existing networked spawns so late-joiners see current rovers.
fn on_server_connected(
    trigger: On<Add, Connected>,
    q_client: Query<&RemoteId, With<ClientOf>>,
    q_spawns: Query<(&GlobalEntityId, &NetSpawn)>,
    q_repl: Query<(&GlobalEntityId, &Transform), With<NetReplicate>>,
    registry: Res<SessionRegistry>,
    server: Single<&Server>,
    tick: Res<SimTick>,
    mut sender: ServerMultiMessageSender,
) {
    let Ok(remote) = q_client.get(trigger.entity) else {
        return;
    };
    let peer = remote.0;
    let session = peer_to_session(peer);
    let server = server.into_inner();
    let target = NetworkTarget::Single(peer);

    server_send(
        &mut sender,
        server,
        &target,
        WireChannel::CommandBus,
        &WireEnvelope::Handshake(HandshakeMsg {
            session: session.0,
            tick: tick.0,
        }),
    );
    for (gid, spawn) in q_spawns.iter() {
        server_send(
            &mut sender,
            server,
            &target,
            WireChannel::CommandBus,
            &WireEnvelope::Spawn(SpawnReplicationMsg {
                gid: gid.get(),
                entry_id: spawn.entry_id.clone(),
                position: spawn.position.to_array(),
            }),
        );
    }
    // Full state baseline: current pose of every replicated body (balloons,
    // cosim targets, rovers) so the joiner sees them at the right place
    // immediately — not just future spawns/changes. Rides the snapshot channel,
    // applied by the client's `apply_incoming_snapshots`.
    let entries: Vec<SnapshotEntry> = q_repl
        .iter()
        .map(|(gid, tf)| SnapshotEntry {
            gid: gid.get(),
            t: tf.translation.to_array(),
            r: tf.rotation.to_array(),
            // Baseline is a one-shot placement at connect; velocity zero + the f32
            // transform as the absolute `pos` + cell 0 is fine — the next 20 Hz
            // snapshot carries real velocity + precise f64 `pos` within ~50 ms.
            lv: [0.0; 3],
            av: [0.0; 3],
            last_input_seq: 0,
            pos: [
                tf.translation.x as f64,
                tf.translation.y as f64,
                tf.translation.z as f64,
            ],
            cell: [0; 3],
        })
        .collect();
    if !entries.is_empty() {
        let n = entries.len();
        server_send(
            &mut sender,
            server,
            &target,
            WireChannel::ControlStream,
            &WireEnvelope::Snapshot(SnapshotMsg {
                tick: tick.0,
                entries,
            }),
        );
        info!("[net] sent {n}-entity state baseline to new client");
    }
    // Current ownership table, so the joiner immediately knows who owns what
    // (the periodic broadcast only fires on change).
    server_send(
        &mut sender,
        server,
        &target,
        WireChannel::CommandBus,
        &WireEnvelope::Ownership(OwnershipMsg {
            entries: registry.snapshot(),
        }),
    );
    info!("[net] client connected: peer={peer:?} session={}", session.0);
}

/// Client dropped: free everything its session owned (G5).
fn on_server_disconnected(
    trigger: On<Add, Disconnected>,
    q_client: Query<&RemoteId, With<ClientOf>>,
    mut registry: ResMut<SessionRegistry>,
) {
    if let Ok(remote) = q_client.get(trigger.entity) {
        let session = peer_to_session(remote.0);
        let freed = registry.release_session(session);
        info!(
            "[net] client disconnected: session={} freed {} entities",
            session.0,
            freed.len()
        );
    }
}

/// Drain outgoing envelopes (snapshots, spawn replication) to all clients.
fn host_send_outbox(
    mut outbox: ResMut<WireOutbox>,
    server: Single<&Server>,
    mut sender: ServerMultiMessageSender,
) {
    if outbox.0.is_empty() {
        return;
    }
    let server = server.into_inner();
    for (channel, env) in outbox.0.drain(..) {
        server_send(&mut sender, server, &NetworkTarget::All, channel, &env);
    }
}

/// Pull inbound frames from each client link into the inbox, tagged with the
/// connection-derived session (the trusted origin for authority).
fn host_recv_inbox(
    mut q: Query<(&RemoteId, &mut MessageReceiver<Frame>), With<ClientOf>>,
    mut inbox: ResMut<WireInbox>,
) {
    for (remote, mut receiver) in q.iter_mut() {
        let session = peer_to_session(remote.0);
        for frame in receiver.receive() {
            if let Some(env) = deserialize_env(&frame.0) {
                inbox.0.push((session, env));
            }
        }
    }
}
