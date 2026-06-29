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
    GlobalEntityId, NetReplicate, NetSpawn, NetStatus, SessionId, SessionRegistry, SessionProfiles, SimTick, SyncChannel,
};

/// Host-authoritative map: live connection (deterministic netcode peer key) →
/// **server-assigned** [`SessionId`]. The authority id is drawn from server
/// entropy at connect (`lunco_core::ids::random_session_id`), NOT derived from the
/// client-chosen netcode id, so a client can neither pick nor guess its own
/// identity (review H4) and two clients cannot collide (H5). Every authority
/// decision — RBAC, ownership, and the inbound-sender binding in `host_recv_inbox`
/// — keys off this value; the client only learns its id from the handshake. The
/// peer key (a deterministic function of the connection) is just the lookup
/// handle; it never becomes the authority id.
#[derive(Resource, Default)]
struct AssignedSessions {
    by_peer: std::collections::HashMap<u64, SessionId>,
}

impl AssignedSessions {
    /// Allocate (or return the existing) server session for a connection key.
    fn assign(&mut self, peer_key: u64) -> SessionId {
        *self
            .by_peer
            .entry(peer_key)
            .or_insert_with(|| SessionId(lunco_core::ids::random_session_id()))
    }
    fn get(&self, peer_key: u64) -> Option<SessionId> {
        self.by_peer.get(&peer_key).copied()
    }
    fn remove(&mut self, peer_key: u64) -> Option<SessionId> {
        self.by_peer.remove(&peer_key)
    }
}

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
            // Sibling privkey.pem next to the cert file. Accept either
            // separator so Windows paths (`C:\certs\fullchain.pem`) resolve
            // the sibling correctly too.
            match cert.rfind(['/', '\\']) {
                Some(sep) => format!("{}{}privkey.pem", &cert[..sep], &cert[sep..=sep]),
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
/// Whether a cert was specified ⇒ likely a real CA cert ⇒ guests need no digest.
/// Returns `(identity, bare_hex_digest)`; the digest is empty when a CA cert is
/// served (guests validate the chain) and non-empty for the dev self-signed cert
/// so the invite link can pin it.
fn resolve_identity() -> (Identity, String) {
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
        let digest = announce_digest(&identity);
        return (identity, digest);
    }

    // ECDSA-P256 self-signed cert — fresh each launch (→ a new digest every
    // restart) UNLESS you pin a persisted one above. Publish the digest so
    // browser clients can pin it in the connect URL (`#<digest>`).
    let identity = Identity::self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
        .expect("self-signed certificate");
    let digest = announce_digest(&identity);
    (identity, digest)
}

/// Compute the cert's DER-SHA256 digest, log it, and write it to
/// `lunco_cert_digest.txt` so browser clients can hash-pin it (`#<digest>`).
/// Called on BOTH identity paths so a persisted self-signed cert (loaded via
/// the env vars) yields a stable digest across restarts. Returns the **bare
/// lowercase hex** form (colons stripped) for embedding in an invite link.
fn announce_digest(identity: &Identity) -> String {
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
    digest
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Best-guess primary LAN IPv4 for the invite-link prefill: open a UDP socket
/// "to" a routable address (no packets are sent — `connect` only sets the route)
/// and read back which local interface would be used. Returns `None` with no
/// default route. Std-only, no dependency.
fn primary_lan_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
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
    let (identity, digest) = resolve_identity();

    // Seed the *Copy invite link* prefill (LAN IP:port) + the cert digest a
    // browser guest must pin, onto the always-on NetStatus seam so the workbench
    // menu can offer them with no networking dependency.
    {
        let mut status = app.world_mut().resource_mut::<NetStatus>();
        if let Some(ip) = primary_lan_ip() {
            status.invite_hint = format!("{ip}:{port}");
            info!("[net] invite hint (LAN): {}", status.invite_hint);
        }
        status.invite_digest = digest;
    }

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
    app.init_resource::<AssignedSessions>();
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
            // Ferry order within the frame: recv → drain (the shared SyncPlugin
            // system) → host-authoritative pushes → send, so a client command or a
            // relay processed this frame is also SENT this frame, not next (up to
            // ~200 ms later when the host's `Update` is render-throttled while
            // unfocused). Intra-`Update` ordering only — the systems stay in
            // `Update` as the reliable-flush note above requires.
            host_recv_inbox.before(crate::sync::drain_sync_inbox),
            update_host_netstatus,
            (broadcast_ownership, broadcast_profiles, host_send_outbox)
                .chain()
                .after(crate::sync::drain_sync_inbox),
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
    outbox.0.push((
        SyncChannel::CommandBus,
        SyncEnvelope::Profiles(ProfilesMsg {
            entries: crate::sync::profile_wire_entries(&profiles),
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
    mut assigned: ResMut<AssignedSessions>,
) {
    let Ok(remote) = q_client.get(trigger.entity) else {
        return;
    };
    let peer = remote.0;
    // Server-assigned identity: allocate a fresh, entropy-drawn SessionId for this
    // connection rather than trusting the client-chosen netcode id. `peer_to_session`
    // is used only as the deterministic per-connection *lookup key*, never as the
    // authority id (review H4/H5).
    let peer_key = peer_to_session(peer).0;
    let session = assigned.assign(peer_key);
    let token = lunco_core::ids::random_token();

    // Initialize client session in RBAC registry as an *authenticated* Observer with
    // its server-issued token. Observer-authorized-by-default: read-only telemetry
    // plus possession/structural commands (which `authorize` gates at Observer) work
    // immediately on connect — no profile-name round-trip required. The session is
    // promoted to Operator (gaining DriveRover/BrakeRover) when it sets a name via
    // `on_update_profile_rbac`. The token makes the session a server-issued credential
    // (`is_authorized` requires one), closing the name-only self-promotion of M2.
    rbac.sessions.insert(session.0, lunco_core::session::UserSession {
        session_id: session,
        username: format!("Player {}", session.0),
        role: lunco_core::session::AuthorityRole::Observer,
        authenticated: true,
        token: Some(token.clone()),
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
            token,
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
    server_send(
        &mut sender,
        server,
        &target,
        SyncChannel::CommandBus,
        &SyncEnvelope::Profiles(ProfilesMsg {
            entries: crate::sync::profile_wire_entries(&profiles),
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
    mut dedup: ResMut<crate::sync::SyncDedup>,
    mut assigned: ResMut<AssignedSessions>,
) {
    if let Ok(remote) = q_client.get(trigger.entity) {
        let peer_key = peer_to_session(remote.0).0;
        // Resolve via the server-assigned map (same id authority used everywhere),
        // then drop the mapping so a reconnecting peer key gets a fresh session.
        let Some(session) = assigned.remove(peer_key) else {
            return;
        };
        let freed = registry.release_session(session);
        profiles.profiles.remove(&session.0);
        rbac.sessions.remove(&session.0);
        dedup.forget(session);
        info!(
            "[net] client disconnected: session={} freed {} entities, profiles updated",
            session.0,
            freed.len()
        );
    }
}

/// Drain outgoing envelopes (snapshots, spawn replication) to all clients.
///
/// TODO(B4 — interest management): this fans EVERY envelope to
/// `NetworkTarget::All`, and `gather_snapshot` builds a single `SnapshotMsg`
/// shared by all peers. Cost is O(moving entities × peers): every client
/// receives every entity's update regardless of relevance. Acceptable at the
/// current handful-of-peers scale; it does not scale.
///
/// This is deliberately left as a TODO and NOT a quick AOI/cell-distance filter,
/// because a correct fix is a substantial *simulation-aware* distribution system,
/// not a transport tweak. It must account for:
///   - **What the sim produces, not just where bodies are.** Relevance is per
///     data stream (rigid-body pose, joint/articulation state, wheel/cosim
///     telemetry, predicted vs authoritative tracks), each with its own rate and
///     priority — a distant-but-owned/possessed vehicle, or one a client is
///     predicting, can't be culled by raw distance alone.
///   - **Floating-origin / big_space frames.** "Distance" for an AOI test is
///     cell-relative; the same CQ-201 frame-mix that bit wheel kinematics applies
///     to any naive position-difference culling (see
///     `lunco-mobility::wheel_kinematics`). Compare in one (avian cell-local) frame.
///   - **Prediction/reconciliation coupling.** A client predicting a Dynamic body
///     needs a baseline + correction cadence even when "out of view"; dropping its
///     stream silently desyncs it (cf. the predicted-Dynamic drift TODO).
///   - **Per-target batching + lifecycle.** Spawn/despawn replication must still
///     reach a peer the instant an entity enters/leaves its interest set, with
///     `NetworkTarget::Single` sub-batches replacing the blanket `All` — without
///     racing the dedup/op-id replay window (`SyncDedup` is per-origin).
///
/// Plumbing is ready when we build it: `server_send` already takes a
/// `&NetworkTarget`, and `on_server_connected` does targeted single-client replay
/// via `ServerMultiMessageSender`. Until then, broadcast-to-all is the correct,
/// simple behaviour.
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
        // B4: blanket fan-out — see the function doc for why per-client interest
        // management is intentionally deferred to a sim-aware distribution system.
        server_send(&mut sender, server, &NetworkTarget::All, channel, &env);
    }
}

/// Pull inbound frames from each client link into the inbox, tagged with the
/// connection-derived session (the trusted origin for authority).
fn host_recv_inbox(
    mut q: Query<(&RemoteId, &mut MessageReceiver<Frame>), With<ClientOf>>,
    mut inbox: ResMut<SyncInbox>,
    assigned: Res<AssignedSessions>,
) {
    for (remote, mut receiver) in q.iter_mut() {
        // Bind every inbound envelope to the SERVER-ASSIGNED session for this
        // connection — the unforgeable trusted origin. A peer cannot spoof another
        // session: the id comes from the connection, not the wire (review H4). A
        // connection with no assignment yet (pre-`on_server_connected`) is skipped.
        let peer_key = peer_to_session(remote.0).0;
        let Some(session) = assigned.get(peer_key) else {
            // Drain so the receiver buffer doesn't grow while we wait for assignment.
            for _ in receiver.receive() {}
            continue;
        };
        for frame in receiver.receive() {
            if let Some(env) = deserialize_env(&frame.0) {
                inbox.0.push((session, env));
            }
        }
    }
}
