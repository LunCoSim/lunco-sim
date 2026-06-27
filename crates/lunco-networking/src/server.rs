//! Host (listen-server) adapter: WebTransport server + connection lifecycle +
//! outbox→clients / clients→inbox ferry. Native only.

use bevy::math::DVec3;
use bevy::prelude::*;
use lightyear::netcode::server_plugin::NetcodeConfig;
use lightyear::netcode::NetcodeServer;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

use crate::sync::{
    encode_quat, quantize_pos, HandshakeMsg, OwnershipMsg, ProfilesMsg, SnapshotEntry, SnapshotMsg,
    SpawnReplicationMsg, SyncEnvelope, SyncInbox, SyncOutbox,
};
use lunco_core::{
    GlobalEntityId, NetReplicate, NetSpawn, NetStatus, SessionRegistry, SessionProfiles, SimTick, SyncChannel,
};

use crate::protocol::{CmdChannel, Frame, SnapChannel};
use crate::shared::{deserialize_env, peer_to_session, serialize_env, PRIVATE_KEY, PROTOCOL_ID};

use lunco_storage::{FileStorage, Storage, StorageHandle};
use std::path::PathBuf;
use wtransport::tls::{Certificate, CertificateChain, PrivateKey};

/// Env vars naming a CA-signed cert + key (PEM). When both are set the host
/// serves that identity and browsers validate via the normal chain — no digest.
/// Typical production values point at certbot's output, e.g.
/// `/etc/letsencrypt/live/sandbox.lunco.space/{fullchain,privkey}.pem`.
const ENV_TLS_CERT: &str = "LUNCO_TLS_CERT";
const ENV_TLS_KEY: &str = "LUNCO_TLS_KEY";

/// Resolve explicit `(cert_path, key_path)` PEM paths to serve, or `None` for
/// the dev self-signed path. Two ways to point at a cert, CLI taking precedence:
///
/// - **CLI** `--cert <path>` (the easy "just say where they are"):
///   - a **directory** (e.g. a certbot live dir) → `<dir>/fullchain.pem` +
///     `<dir>/privkey.pem`. So `--cert /etc/letsencrypt/live/sandbox.lunco.space`
///     is all you need.
///   - a **file** (ends in `.pem`/`.crt`/`.cer`) → that cert; the key comes from
///     `--key <file>`, else the sibling `privkey.pem` next to it.
/// - **Env** (fallback): `LUNCO_TLS_CERT` + `LUNCO_TLS_KEY`, both required.
///
/// `None` ⇒ both unset and no `--cert` ⇒ dev self-signed.
fn resolve_cert_paths() -> Option<(String, String)> {
    let args: Vec<String> = std::env::args().collect();
    let mut cli_cert: Option<String> = None;
    let mut cli_key: Option<String> = None;
    for i in 0..args.len() {
        match args[i].as_str() {
            "--cert" => cli_cert = args.get(i + 1).cloned(),
            "--key" => cli_key = args.get(i + 1).cloned(),
            _ => {}
        }
    }

    if let Some(cert) = cli_cert.filter(|s| !s.is_empty()) {
        // Distinguish a cert FILE from a live DIRECTORY by extension alone — no
        // `std::fs` probe (raw fs is clippy-banned workspace-wide for wasm parity).
        let is_file = [".pem", ".crt", ".cer", ".key"]
            .iter()
            .any(|ext| cert.ends_with(ext));
        if !is_file {
            // Directory layout: certbot's fullchain.pem + privkey.pem.
            return Some((format!("{cert}/fullchain.pem"), format!("{cert}/privkey.pem")));
        }
        let key = cli_key.filter(|s| !s.is_empty()).unwrap_or_else(|| {
            // Sibling privkey.pem next to the cert file.
            match cert.rfind('/') {
                Some(slash) => format!("{}/privkey.pem", &cert[..slash]),
                None => "privkey.pem".to_string(),
            }
        });
        return Some((cert, key));
    }

    match (std::env::var(ENV_TLS_CERT), std::env::var(ENV_TLS_KEY)) {
        (Ok(c), Ok(k)) => Some((c, k)),
        // Exactly one set — almost certainly a typo'd/forgotten var. Fail loud.
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => panic!(
            "🔐 only one of {ENV_TLS_CERT}/{ENV_TLS_KEY} is set; both are required. \
             Set both, pass `--cert <dir|file>`, or unset both for a dev self-signed cert."
        ),
        (Err(_), Err(_)) => None,
    }
}

/// Resolve the host's WebTransport TLS identity.
///
/// - **Production** (`--cert <dir|file>`, or `LUNCO_TLS_CERT`+`LUNCO_TLS_KEY`):
///   load that CA-signed cert (e.g. certbot `fullchain.pem` + `privkey.pem`).
///   Browsers validate via the normal chain, so clients connect with **no**
///   `#digest`. See [`resolve_cert_paths`].
/// - **Dev** (nothing specified): ECDSA-P256 self-signed for
///   `localhost`/`127.0.0.1`; print + persist the cert digest so a browser can
///   pin it via the connect URL `#<digest>`. A **native** client dialing a bare
///   IP skips validation entirely and needs no digest (see `wt_client.rs`).
///
/// **Fail-loud:** if a cert IS specified but the PEM can't be loaded, this
/// PANICS rather than silently serving a self-signed cert. An operator who set
/// it asked for that specific identity; falling back would hand browsers an
/// untrusted cert (connection refused) with no obvious server-side cause — a
/// misconfiguration that looks like a network fault. Crashing surfaces it
/// immediately in the service logs / exit code.
fn resolve_identity() -> Identity {
    if let Some((cert_path, key_path)) = resolve_cert_paths() {
        let identity = load_pem_identity(&cert_path, &key_path).unwrap_or_else(|e| {
            panic!(
                "🔐 a cert was specified but could not be loaded ({e}). Refusing to \
                 start with a fallback self-signed cert — browsers would reject it. \
                 Fix the PEM paths/permissions (cert={cert_path}, key={key_path}), or \
                 remove `--cert`/the env vars to run with a dev self-signed cert."
            )
        });
        info!("🔐 WebTransport using cert from {cert_path}");
        // A real CA cert's digest is unused (browsers validate the chain), but a
        // *self-signed* cert pinned this way still needs the hash-pin — so
        // publish the digest here too. This is the supported way to get a STABLE
        // digest across host restarts: point at a persisted self-signed cert
        // instead of minting a fresh one each launch. See announce_digest.
        announce_digest(&identity);
        return identity;
    }

    // ECDSA-P256 self-signed cert — fresh each launch (→ a new digest every
    // restart) UNLESS you pin a persisted one above. Publish the digest so
    // browser clients can pin it in the connect URL (`#<digest>`).
    let identity = Identity::self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
        .expect("self-signed certificate");
    announce_digest(&identity);
    identity
}

/// Compute the cert's DER-SHA256 digest, log it, and write it to
/// `lunco_cert_digest.txt` so browser clients can hash-pin it (`#<digest>`).
/// Called on BOTH identity paths so a persisted self-signed cert (loaded via
/// the env vars) yields a stable digest across restarts.
fn announce_digest(identity: &Identity) {
    let digest = format!("{}", identity.certificate_chain().as_slice()[0].hash());
    info!("🔐 WebTransport cert digest: {digest}");
    let digest_path = std::env::temp_dir().join("lunco_cert_digest.txt");
    // lunco-storage, not std::fs (clippy-banned workspace-wide for wasm parity).
    if FileStorage::new()
        .write_sync(&StorageHandle::File(digest_path.clone()), digest.as_bytes())
        .is_ok()
    {
        info!("🔐 digest written to {}", digest_path.display());
    }
}

/// Build an [`Identity`] from PEM cert-chain + private-key files. Reads route
/// through [`lunco_storage`] (raw `std::fs` is clippy-banned); parsing is sync
/// (`rustls_pemfile` → DER → wtransport's sync constructors).
fn load_pem_identity(cert_path: &str, key_path: &str) -> Result<Identity, String> {
    let storage = FileStorage::new();
    let cert_bytes = storage
        .read_sync(&StorageHandle::File(PathBuf::from(cert_path)))
        .map_err(|e| format!("read cert {cert_path}: {e:?}"))?;
    let key_bytes = storage
        .read_sync(&StorageHandle::File(PathBuf::from(key_path)))
        .map_err(|e| format!("read key {key_path}: {e:?}"))?;

    let mut cert_rd: &[u8] = &cert_bytes;
    let certs = rustls_pemfile::certs(&mut cert_rd)
        .map(|r| {
            r.map_err(|e| format!("parse cert PEM: {e}"))
                .and_then(|der| {
                    Certificate::from_der(der.as_ref().to_vec())
                        .map_err(|e| format!("invalid certificate: {e:?}"))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(format!("no CERTIFICATE blocks in {cert_path}"));
    }

    let mut key_rd: &[u8] = &key_bytes;
    let key_der = rustls_pemfile::private_key(&mut key_rd)
        .map_err(|e| format!("parse key PEM: {e}"))?
        .ok_or_else(|| format!("no PRIVATE KEY block in {key_path}"))?;
    // certbot writes PKCS#8 (`BEGIN PRIVATE KEY`); wtransport wraps the DER as such.
    let private_key = PrivateKey::from_der_pkcs8(key_der.secret_der().to_vec());

    Ok(Identity::new(CertificateChain::new(certs), private_key))
}

/// Spawn the server entity (WebTransport cert via [`resolve_identity`]), trigger
/// `Start`, and register the lifecycle observers + ferry systems.
pub(crate) fn setup_host(app: &mut App, port: u16) {
    let identity = resolve_identity();

    let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
    let server = app
        .world_mut()
        .spawn((
            Name::new("LunCoServer"),
            NetcodeServer::new(NetcodeConfig {
                protocol_id: PROTOCOL_ID,
                private_key: PRIVATE_KEY,
                // 30s (default was ~10s): match the client QUIC `max_idle_timeout`
                // so a client that stalls during heavy startup (USD scene load +
                // Modelica cosim compile) or under host load isn't reaped before
                // its loop resumes. Without this the netcode layer dropped clients
                // ("Disconnection from netcode client …") seconds after connect.
                client_timeout_secs: 30,
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
    // render smoothly. See `crate::sync::SyncPlugin`.
    app.add_systems(
        Update,
        (
            host_send_outbox,
            host_recv_inbox,
            update_host_netstatus,
            broadcast_ownership,
            broadcast_profiles,
        ),
    );
}

/// Broadcast the authoritative ownership table to all clients whenever it
/// changes (a claim or release). Reliable channel so the who-owns-what view
/// stays consistent. Host-only (registered in `setup_host`).
fn broadcast_ownership(registry: Res<SessionRegistry>, mut outbox: ResMut<SyncOutbox>) {
    if !registry.is_changed() {
        return;
    }
    outbox.0.push((
        SyncChannel::CommandBus,
        SyncEnvelope::Ownership(OwnershipMsg {
            entries: registry.snapshot(),
        }),
    ));
}

/// Broadcast the authoritative session profile names map to all clients whenever
/// it changes. Reliable channel. Host-only (registered in `setup_host`).
fn broadcast_profiles(profiles: Res<SessionProfiles>, mut outbox: ResMut<SyncOutbox>) {
    if !profiles.is_changed() {
        return;
    }
    let entries = profiles.profiles.iter().map(|(&s, n)| {
        let color = profiles.colors.get(&s).copied().unwrap_or_else(|| crate::sync::generate_user_color(s));
        (s, n.clone(), color)
    }).collect();
    outbox.0.push((
        SyncChannel::CommandBus,
        SyncEnvelope::Profiles(ProfilesMsg {
            entries,
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
/// `SyncChannel`.
fn server_send(
    sender: &mut ServerMultiMessageSender,
    server: &Server,
    target: &NetworkTarget,
    channel: SyncChannel,
    env: &SyncEnvelope,
) {
    let Some(bytes) = serialize_env(env) else {
        return;
    };
    let frame = Frame(bytes);
    let _ = match channel {
        SyncChannel::ControlStream => sender.send::<Frame, SnapChannel>(&frame, server, target),
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
    profiles: Res<SessionProfiles>,
    server: Single<&Server>,
    tick: Res<SimTick>,
    mut sender: ServerMultiMessageSender,
    mut rbac: ResMut<lunco_core::session::SessionRbac>,
) {
    let Ok(remote) = q_client.get(trigger.entity) else {
        return;
    };
    let peer = remote.0;
    let session = peer_to_session(peer);

    // Initialize client session in RBAC registry as an *authenticated* Observer.
    // Observer-authorized-by-default: read-only telemetry plus possession/structural
    // commands (which `authorize` gates at Observer) work immediately on connect — no
    // profile-name round-trip required. The session is promoted to Operator (gaining
    // DriveRover/BrakeRover) when it sets a name via `on_update_profile_rbac`. Inserting
    // as unauthenticated here would default-deny *every* command until the first
    // UpdateProfile lands, re-breaking possession (the MVP "structural always allowed"
    // policy) for the whole connect→name window.
    rbac.sessions.insert(session.0, lunco_core::session::UserSession {
        session_id: session,
        username: format!("Player {}", session.0),
        role: lunco_core::session::AuthorityRole::Observer,
        authenticated: true,
        token: None,
    });
    let server = server.into_inner();
    let target = NetworkTarget::Single(peer);

    server_send(
        &mut sender,
        server,
        &target,
        SyncChannel::CommandBus,
        &SyncEnvelope::Handshake(HandshakeMsg {
            session: session.0,
            tick: tick.0,
        }),
    );
    for (gid, spawn) in q_spawns.iter() {
        server_send(
            &mut sender,
            server,
            &target,
            SyncChannel::CommandBus,
            &SyncEnvelope::Spawn(SpawnReplicationMsg {
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
            // Baseline is a one-shot placement at connect; the f32 `Transform` as the
            // absolute (quantized) position + velocity zero is fine — the next 20 Hz
            // snapshot carries real velocity + the precise f64 pose within ~50 ms.
            pos_q: quantize_pos(DVec3::new(
                tf.translation.x as f64,
                tf.translation.y as f64,
                tf.translation.z as f64,
            )),
            rot_packed: encode_quat(tf.rotation),
            lv: [0.0; 3],
            av: [0.0; 3],
            last_input_seq: 0,
        })
        .collect();
    if !entries.is_empty() {
        let n = entries.len();
        server_send(
            &mut sender,
            server,
            &target,
            SyncChannel::ControlStream,
            &SyncEnvelope::Snapshot(SnapshotMsg {
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
        SyncChannel::CommandBus,
        &SyncEnvelope::Ownership(OwnershipMsg {
            entries: registry.snapshot(),
        }),
    );
    let entries = profiles.profiles.iter().map(|(&s, n)| {
        let color = profiles.colors.get(&s).copied().unwrap_or_else(|| crate::sync::generate_user_color(s));
        (s, n.clone(), color)
    }).collect();
    server_send(
        &mut sender,
        server,
        &target,
        SyncChannel::CommandBus,
        &SyncEnvelope::Profiles(ProfilesMsg {
            entries,
        }),
    );
    info!("[net] client connected: peer={peer:?} session={}", session.0);
}

/// Client dropped: free everything its session owned (G5) and remove its profile.
fn on_server_disconnected(
    trigger: On<Add, Disconnected>,
    q_client: Query<&RemoteId, With<ClientOf>>,
    mut registry: ResMut<SessionRegistry>,
    mut profiles: ResMut<SessionProfiles>,
    mut rbac: ResMut<lunco_core::session::SessionRbac>,
) {
    if let Ok(remote) = q_client.get(trigger.entity) {
        let session = peer_to_session(remote.0);
        let freed = registry.release_session(session);
        profiles.profiles.remove(&session.0);
        rbac.sessions.remove(&session.0);
        info!(
            "[net] client disconnected: session={} freed {} entities, profiles updated",
            session.0,
            freed.len()
        );
    }
}

/// Drain outgoing envelopes (snapshots, spawn replication) to all clients.
fn host_send_outbox(
    mut outbox: ResMut<SyncOutbox>,
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
    mut inbox: ResMut<SyncInbox>,
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
