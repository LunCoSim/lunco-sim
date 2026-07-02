//! Host (listen-server) adapter: WebTransport server + connection lifecycle +
//! outbox→clients / clients→inbox ferry. Native only.

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use lightyear::netcode::server_plugin::NetcodeConfig;
use lightyear::netcode::NetcodeServer;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};

use crate::sync::{
    HandshakeMsg, NetworkConfig, OwnershipMsg, PeerInterest, ProfilesMsg, ReplicationState,
    SnapshotMsg, SyncEnvelope, SyncInbox, SyncOutbox, ViewCenters, MAX_SNAPSHOT_ENTRIES,
};
use lunco_core::{
    NetStatus, SessionId, SessionRegistry, SessionProfiles, SimTick, SyncChannel,
};
use lunco_workspace::{Twin, TwinAdded, WorkspaceResource};
use crate::scenario::{cid_for_content, scenario_revision, ScenarioAsset, ScenarioManifestMsg, ScenarioManifestResource};

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

use crate::protocol::{BulkChannel, CmdChannel, Frame, SnapChannel};
use crate::shared::{deserialize_env, peer_to_session, serialize_env, PRIVATE_KEY, PROTOCOL_ID};

use lunco_storage::{FileStorage, Storage, StorageHandle};
use std::path::{Path, PathBuf};
use wtransport::tls::{Certificate, CertificateChain, PrivateKey};
use sha2::{Digest, Sha256};

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
        // `.key` is intentionally absent: a key is never a cert chain, so a
        // private key passed as `--cert` must not be classified as a cert file
        // and loaded into the chain slot (review M6). Cert extensions only.
        let is_file = [".pem", ".crt", ".cer"]
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
    // Scenario distribution: the host's "current scenario" publisher resource,
    // plus the in-flight off-thread manifest build. The build is spawned from a
    // `Startup` system (below) rather than inline here so the
    // `AsyncComputeTaskPool` is initialized before the first spawn.
    app.init_resource::<ScenarioManifestResource>();
    app.init_resource::<PendingScenarioManifest>();
    // Phase-3 host-only serving state: CID→path index (filled by
    // `drive_scenario_manifest`) + in-flight off-thread read jobs.
    app.init_resource::<crate::scenario_sync::HostAssetPaths>();
    app.init_resource::<crate::scenario_sync::AssetServeTasks>();
    app.add_systems(Startup, spawn_initial_scenario_manifest);

    app.add_observer(on_server_connected);
    app.add_observer(on_server_disconnected);
    app.add_observer(on_twin_added_host);
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
            // Phase-3: turn queued client asset requests into off-thread read
            // jobs. Doesn't touch the lightyear sender, so it runs parallel; must
            // follow the drain that fills `PendingAssetRequests`.
            crate::scenario_sync::serve_asset_requests.after(crate::sync::drain_sync_inbox),
            // `assemble_and_send_snapshots` shares `ServerMultiMessageSender` with
            // `host_send_outbox`, so chain (not parallel) to avoid the param conflict;
            // it edge-detects `ReplicationState.generation` to fire once per gather.
            (
                // Publish a finished off-thread manifest build BEFORE the
                // broadcast that edge-detects the resource, so a scenario opened
                // this frame ships the same frame.
                drive_scenario_manifest,
                broadcast_ownership,
                broadcast_profiles,
                broadcast_scenario_manifest,
                host_send_outbox,
                assemble_and_send_snapshots,
                // Phase-3: poll finished read jobs and stream their chunks to the
                // requesting peer. Shares `ServerMultiMessageSender` with the two
                // sends above, so it's chained (not parallel) to avoid the param
                // conflict.
                drain_and_send_asset_chunks,
            )
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

/// Broadcast the host's current scenario manifest to all clients whenever its
/// revision changes (the host re-loaded the scenario, or loaded one for the
/// first time). Reliable channel — the manifest is session context, not per-tick
/// state, and a mid-session scenario swap must reach every peer reliably. The
/// per-client-on-connect send lives in [`on_server_connected`]; this is the
/// "scenario changed while clients were connected" path. Host-only.
fn broadcast_scenario_manifest(
    scenario: Res<ScenarioManifestResource>,
    mut outbox: ResMut<SyncOutbox>,
) {
    // `is_changed` fires when `setup_sandbox` fills the resource (None→Some) and
    // when a reload swaps the manifest (revision bumps). Edge-detecting on the
    // resource avoids re-sending every frame.
    if !scenario.is_changed() {
        return;
    }
    let Some(manifest) = scenario.manifest.clone() else {
        return;
    };
    outbox.0.push((
        SyncChannel::BulkData,
        SyncEnvelope::ScenarioManifest(manifest),
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
        SyncChannel::BulkData => sender.send::<Frame, BulkChannel>(&frame, server, target),
        _ => sender.send::<Frame, CmdChannel>(&frame, server, target),
    };
}

/// New client confirmed: hand it its session id + current tick, its session/ownership
/// context, and let the per-peer assembler stream the entities it should see.
///
/// B4 Phase 2: this NO LONGER replays all spawns + a full-state baseline. A freshly
/// connected peer has no view center yet, so `compute_interest_sets` fail-opens it to
/// **all** gids at the first recompute (~200 ms), and `assemble_and_send_snapshots`
/// then sends the scoped `Spawn`+baseline. Replaying here too would double-send every
/// spawn (the assembler's per-peer `spawned` set starts empty and can't know what this
/// one-shot already sent). Handshake/Ownership/Profiles stay — they're session context,
/// not entity state, and the assembler doesn't carry them.
fn on_server_connected(
    trigger: On<Add, Connected>,
    q_client: Query<&RemoteId, With<ClientOf>>,
    registry: Res<SessionRegistry>,
    profiles: Res<SessionProfiles>,
    scenario: Option<Res<ScenarioManifestResource>>,
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
    // Scenario manifest: tell the joiner which scenario the server is running
    // so it can fetch the assets it's missing (Phase 3) and load the scene
    // (Phase 4). Only sent if the host has actually loaded a scenario — a bare
    // host that's still loading sends nothing here, and the periodic
    // `broadcast_scenario_manifest` will push it once `setup_sandbox` fills the
    // resource.
    if let Some(scenario) = &scenario {
        if let Some(manifest) = &scenario.manifest {
            server_send(
                &mut sender,
                server,
                &target,
                SyncChannel::BulkData,
                &SyncEnvelope::ScenarioManifest(manifest.clone()),
            );
        }
    }
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
    mut view_centers: ResMut<ViewCenters>,
    mut interest: ResMut<PeerInterest>,
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
        // Drop AOI bookkeeping so neither map leaks across reconnects (a fresh
        // session id is minted on rejoin anyway).
        view_centers.0.remove(&session);
        interest.0.remove(&session);
        info!(
            "[net] client disconnected: session={} freed {} entities, profiles updated",
            session.0,
            freed.len()
        );
    }
}

/// Drain the broadcast outbox (ownership, profiles, spawns, despawns, cursors,
/// commands relayed to all) to every client.
///
/// B4 NOTE: snapshots are NO LONGER here — `gather_snapshot` stopped pushing them to
/// `SyncOutbox`; per-peer pose replication is done by `assemble_and_send_snapshots`
/// keyed on `PeerInterest`. The envelopes that remain are genuinely global (every
/// client must learn who owns what, what exists, and others' cursors), so blanket
/// `All` is correct for them. Spawn/despawn stay broadcast under soft-exit AOI:
/// clients know every entity *exists*; AOI only gates the *pose update* stream, and a
/// body outside a peer's interest simply freezes at its last pose (Phase 2 will make
/// spawn/despawn itself interest-scoped).
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

/// Host: assemble + send each connected peer ONLY the pose updates relevant to it
/// (B4 Phase 1 — the O(N×P)→O(Σ interest) routing flip).
///
/// For each peer we send every in-interest body whose `(pos, rot, ack)` differs from
/// what that peer was last sent (a per-peer digest). This one rule covers three cases
/// at once:
///   - **steady updates** — a moving body differs each generation, so it streams;
///   - **soft-enter baseline** — a body entering interest isn't in the peer's digest,
///     so its current pose is sent even if it didn't move this tick (no stale frozen
///     reappearance);
///   - **render-throttle robustness** — the digest holds the LAST POSE THE PEER GOT,
///     not this generation's delta, so when `Update` runs slower than the 20 Hz
///     `FixedPostUpdate` and skips generations, the latest pose is still sent (the
///     old broadcast path queued every gather's delta in the outbox; per-peer routing
///     can't, so it diffs against the peer's known state instead).
/// A body that LEAVES interest is dropped from the stream AND the digest (soft exit):
/// the client freezes its proxy at the last pose — no despawn/re-spawn churn — and a
/// later re-entry re-sends a fresh baseline.
///
/// The `(pos_q, rot_packed, last_input_seq)` key matches `gather_snapshot`'s own
/// change test (velocity is intentionally excluded, so a body isn't re-sent for
/// sub-quantum velocity wobble). Runs on the `Update` ferry but early-outs unless
/// `ReplicationState.generation` advanced, so it diffs at most once per gather.
/// Per-peer batches are chunked at `MAX_SNAPSHOT_ENTRIES` (single-fragment, L2).
fn assemble_and_send_snapshots(
    repl: Res<ReplicationState>,
    interest: Res<PeerInterest>,
    config: Res<NetworkConfig>,
    assigned: Res<AssignedSessions>,
    q_client: Query<&RemoteId, With<ClientOf>>,
    server: Single<&Server>,
    mut sender: ServerMultiMessageSender,
    mut last_gen: Local<u64>,
    // Per-session digest of the last `(pos_q, rot_packed, last_input_seq)` SENT to that
    // peer per gid. Diffed each assemble to decide what to send; out-of-interest gids
    // are evicted so a re-entry re-baselines.
    mut sent_last: Local<
        std::collections::HashMap<SessionId, std::collections::HashMap<u64, ([i32; 3], u32, u32)>>,
    >,
    // Per-session set of gids the peer has been sent a `Spawn` for (scoped-spawn,
    // Phase 2). Persists across AOI exit/re-entry (soft-exit keeps the proxy), so a
    // body is spawned at most once per peer; pruned to the live set + connected peers.
    mut spawned: Local<std::collections::HashMap<SessionId, std::collections::HashSet<u64>>>,
) {
    // Early-out: only diff once per gather generation (skip frames with no new pose).
    // `0` is the never-gathered sentinel.
    if repl.generation == 0 || repl.generation == *last_gen {
        return;
    }
    *last_gen = repl.generation;
    let server = server.into_inner();
    let mut seen: std::collections::HashSet<SessionId> = std::collections::HashSet::new();

    for remote in q_client.iter() {
        let peer = remote.0;
        let Some(session) = assigned.get(peer_to_session(peer).0) else {
            continue;
        };
        seen.insert(session);
        // No interest computed yet (connected this frame, before the first recompute):
        // skip — the peer gets its full set within one recompute interval (~200 ms). A
        // centerless peer is NOT this case: `compute_interest_sets` fail-opens it to all
        // gids, so it still receives everything until it possesses or reports a center.
        let Some(set) = interest.0.get(&session) else {
            continue;
        };
        let target = NetworkTarget::Single(peer);

        // B4 Phase 2 — scoped spawn: send a `Spawn` (reliable) for any NetSpawn body
        // ENTERING this peer's interest for the first time, before its pose baseline.
        // `spawned` tracks what the peer has been told to reconstruct; under soft-exit
        // the proxy persists, so a body is spawned at most once per peer (no re-spawn
        // when it leaves+re-enters interest). Pruned to the live set below.
        let known = spawned.entry(session).or_default();
        for &gid in set {
            if known.insert(gid) {
                if let Some(spawn) = repl.spawn_info.get(&gid) {
                    server_send(
                        &mut sender,
                        server,
                        &target,
                        SyncChannel::CommandBus,
                        &SyncEnvelope::Spawn(spawn.clone()),
                    );
                }
            }
        }
        // Forget proxies the peer no longer holds (the body was truly despawned and left
        // `spawn_info`/`entries`); gids are never reused, so this can't cause a re-spawn.
        known.retain(|gid| repl.entries.contains_key(gid));

        let digest = sent_last.entry(session).or_default();
        let batch = crate::sync::diff_peer_batch(set, &repl.entries, digest);

        if batch.is_empty() {
            continue;
        }
        for chunk in batch.chunks(MAX_SNAPSHOT_ENTRIES) {
            server_send(
                &mut sender,
                server,
                &target,
                config.snapshot_channel,
                &SyncEnvelope::Snapshot(SnapshotMsg {
                    tick: repl.tick,
                    entries: chunk.to_vec(),
                }),
            );
        }
    }
    // Drop per-session memory for peers no longer connected.
    sent_last.retain(|s, _| seen.contains(s));
    spawned.retain(|s, _| seen.contains(s));
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

/// One scenario file, resolved on the main thread (cheap path work only) so the
/// blocking read + hash can run off-thread. `abs_path` is what the build task
/// reads; `rel_path` is the scenario-root-relative, `/`-normalized manifest key.
struct AssetDescriptor {
    abs_path: PathBuf,
    rel_path: String,
    media_type: Option<String>,
}

/// Everything the off-thread manifest build needs, owned (no `&Twin` borrow) so
/// it can cross the `AsyncComputeTaskPool` boundary.
struct ScenarioBuildInput {
    scenario_id: [u8; 16],
    name: String,
    default_scene: Option<String>,
    descriptors: Vec<AssetDescriptor>,
}

/// Main-thread step: walk the Twin tree and resolve the file list — path joins,
/// extension→media-type, and **path dedup** only, no file I/O. A parent Twin's
/// `files()` already recurses into child-Twin subdirs, so the same asset can
/// surface via both the parent and the child walk; the `seen` set keeps the
/// first and drops the duplicate (review: duplicate assets from overlapping
/// parent/child walks). The blocking read + SHA-256 happens later in
/// [`build_manifest_from_input`], off the main thread.
fn collect_scenario_input(twin: &Twin) -> ScenarioBuildInput {
    let mut seen: HashSet<String> = HashSet::new();
    let mut descriptors = Vec::new();
    for t in twin.walk() {
        for entry in t.files() {
            let abs_path = t.root.join(&entry.relative_path);
            let Ok(rel_path) = abs_path.strip_prefix(&twin.root) else {
                continue;
            };
            let path = rel_path.to_string_lossy().replace('\\', "/");
            if !seen.insert(path.clone()) {
                // Already enumerated via a different Twin in the walk (parent's
                // recursive `files()` overlapping a child Twin's own `files()`).
                continue;
            }
            let media_type = match entry.relative_path.extension().and_then(|s| s.to_str()) {
                Some("usd" | "usda" | "usdc") => Some("model/vnd.usd".to_string()),
                Some("glb") => Some("model/gltf-binary".to_string()),
                Some("png") => Some("image/png".to_string()),
                Some("jpg" | "jpeg") => Some("image/jpeg".to_string()),
                _ => None,
            };
            descriptors.push(AssetDescriptor { abs_path, rel_path: path, media_type });
        }
    }

    let scenario_id = twin
        .manifest
        .as_ref()
        .and_then(|m| m.uuid)
        .map(|u| u.into_bytes())
        // No `twin.toml` uuid (unmanaged folder / pre-uuid twin): derive a
        // *stable* id from the scenario root path rather than emitting all-zeros.
        // The host owns this derivation (the client only sees the wire id and
        // can't recompute a path digest it never learns), and it keeps two
        // distinct folder-scenarios from colliding on `[0u8; 16]` — which would
        // clobber each other in the client's `scenarios/<id>/` asset cache.
        .unwrap_or_else(|| scenario_id_from_path(&twin.root));
    let name = twin
        .manifest
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| twin.root.file_name().unwrap_or_default().to_string_lossy().into_owned());
    let default_scene = twin
        .manifest
        .as_ref()
        .and_then(|m| m.usd.as_ref())
        .and_then(|usd| usd.default_scene.clone());

    ScenarioBuildInput { scenario_id, name, default_scene, descriptors }
}

/// Stable 16-byte scenario id derived from the scenario root path — the
/// fallback when a Twin has no `twin.toml` uuid. SHA-256 of the path string,
/// truncated to 16 bytes (the wire `scenario_id` width). Deterministic for a
/// given path so the same folder-scenario keeps one identity across host
/// restarts; distinct folders get distinct ids (vs the old all-zeros collapse).
fn scenario_id_from_path(root: &Path) -> [u8; 16] {
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

/// Off-thread step: read + SHA-256-hash every descriptor and assemble the
/// manifest. **Fail-closed**: if any file can't be read, the whole build
/// returns `None` rather than emitting a manifest over the readable subset — a
/// partial manifest hashes to a `revision` that matches its own truncated asset
/// list, so it looks complete and a client would never request the dropped CID,
/// leaving that asset permanently absent on every peer (review: silently dropped
/// unreadable files corrupt the asset list and revision together).
/// Off-thread build result: the wire manifest plus the host-local CID → absolute
/// path map [`serve_asset_requests`](crate::scenario_sync::serve_asset_requests)
/// reads bytes through (Phase 3). Kept out of the wire type — the client only
/// ever sees relative paths; abs paths are the host's private serving index.
type ScenarioBuildOutput = (ScenarioManifestMsg, Vec<(Vec<u8>, PathBuf)>);

fn build_manifest_from_input(input: ScenarioBuildInput) -> Option<ScenarioBuildOutput> {
    let ScenarioBuildInput { scenario_id, name, default_scene, descriptors } = input;
    let mut assets = Vec::with_capacity(descriptors.len());
    let mut cid_paths = Vec::with_capacity(descriptors.len());
    for d in descriptors {
        let bytes = match std::fs::read(&d.abs_path) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "[net] scenario manifest build aborted: unreadable asset {:?}: {e}",
                    d.abs_path
                );
                return None;
            }
        };
        let cid = cid_for_content(&bytes).to_bytes();
        cid_paths.push((cid.clone(), d.abs_path));
        assets.push(ScenarioAsset {
            path: d.rel_path,
            cid,
            size: bytes.len() as u64,
            media_type: d.media_type,
        });
    }
    // Nothing to distribute → publish nothing. An empty asset list still hashes
    // to a fixed non-empty `revision`, so without this guard a client would
    // treat "scenario at revision R with zero assets" as a real scenario and
    // could act on it; `None` instead routes callers down the bare-host path.
    if assets.is_empty() {
        return None;
    }
    let revision = scenario_revision(&assets);
    Some((ScenarioManifestMsg { scenario_id, revision, name, default_scene, assets }, cid_paths))
}

/// Host-side: an in-flight off-thread scenario-manifest build. The walk reads +
/// SHA-256-hashes every scenario file — tens-to-hundreds of MB for a Twin with
/// large binaries (terrain, meshes, textures). Run inline on the main thread it
/// would stall the netcode ferry (ownership broadcast, snapshot assembly,
/// reliable `CmdChannel` flush all run in `Update`), giving every connected
/// client a multi-second hitch. So the build is spawned on the
/// `AsyncComputeTaskPool` and drained into [`ScenarioManifestResource`] by
/// [`drive_scenario_manifest`]. Host-only.
#[derive(Resource, Default)]
pub(crate) struct PendingScenarioManifest {
    task: Option<Task<Option<ScenarioBuildOutput>>>,
}

/// Spawn an off-thread manifest build for `twin`, replacing any in-flight one
/// (a newer scenario supersedes a build still running for the previous one).
fn spawn_manifest_build(twin: &Twin, pending: &mut PendingScenarioManifest) {
    let input = collect_scenario_input(twin);
    let pool = AsyncComputeTaskPool::get();
    pending.task = Some(pool.spawn(async move { build_manifest_from_input(input) }));
}

/// Startup: if a Twin is already open when the host boots, kick off its manifest
/// build. Runs as a `Startup` system (not inline in `setup_host`) so the
/// `AsyncComputeTaskPool` is guaranteed initialized by then.
fn spawn_initial_scenario_manifest(
    workspace: Option<Res<WorkspaceResource>>,
    mut pending: ResMut<PendingScenarioManifest>,
) {
    let Some(workspace) = workspace else { return };
    let Some(active) = workspace.active_twin else { return };
    if let Some(twin) = workspace.twin(active) {
        info!("[net] Host started with active twin, building scenario manifest for {:?}", twin.root);
        spawn_manifest_build(twin, &mut pending);
    }
}

/// Poll the in-flight manifest build; when it finishes, publish the result into
/// [`ScenarioManifestResource`] (which `is_changed`-triggers the broadcast).
/// `None` from the task (a fail-closed build) leaves the previous manifest in
/// place rather than clearing it. Host-only, runs in `Update`.
fn drive_scenario_manifest(
    mut pending: ResMut<PendingScenarioManifest>,
    mut scenario: ResMut<ScenarioManifestResource>,
    mut asset_paths: ResMut<crate::scenario_sync::HostAssetPaths>,
) {
    let Some(task) = pending.task.as_mut() else {
        return;
    };
    let Some(result) = block_on(future::poll_once(task)) else {
        return;
    };
    pending.task = None;
    match result {
        Some((manifest, cid_paths)) => {
            info!("[net] scenario manifest built: {} assets", manifest.assets.len());
            // Rebuild the CID→path serving index for the new scenario (a reload
            // replaces the whole map — stale CIDs from the previous scenario must
            // not linger and serve wrong bytes).
            asset_paths.0 = cid_paths.into_iter().collect();
            scenario.manifest = Some(manifest);
        }
        None => warn!("[net] scenario manifest build failed; keeping previous manifest"),
    }
}

/// Observer: when a new Twin is opened/added on the host, kick off an off-thread
/// rebuild of the scenario manifest (see [`PendingScenarioManifest`]).
fn on_twin_added_host(
    trigger: On<TwinAdded>,
    workspace: Res<WorkspaceResource>,
    mut pending: ResMut<PendingScenarioManifest>,
) {
    if let Some(twin) = workspace.twin(trigger.event().twin) {
        info!("[net] Twin added; building scenario manifest for {:?}", twin.root);
        spawn_manifest_build(twin, &mut pending);
    }
}

/// Host (Phase 3): poll finished off-thread read jobs and stream their chunks to
/// the requesting peer over the reliable `BulkChannel`, capped at
/// [`MAX_CHUNKS_PER_FRAME`](crate::scenario_sync::MAX_CHUNKS_PER_FRAME) per frame
/// so a large multi-asset transfer can't flood lightyear's send buffer in one
/// `Update`. Leftover chunks (and chunks whose task is still running) persist to
/// the next frame; chunks for a peer that disconnected mid-transfer are dropped.
fn drain_and_send_asset_chunks(
    mut tasks: ResMut<crate::scenario_sync::AssetServeTasks>,
    assigned: Res<AssignedSessions>,
    q_client: Query<&RemoteId, With<ClientOf>>,
    server: Single<&Server>,
    mut sender: ServerMultiMessageSender,
    // Carries chunks not yet flushed (per-frame cap / still-arriving tasks) across
    // frames. A `Local` (not a resource) — this is the only reader/writer.
    mut ready: Local<Vec<(SessionId, crate::scenario::AssetChunkMsg)>>,
) {
    // Harvest finished read jobs; keep the still-running ones.
    if !tasks.0.is_empty() {
        let mut still_running = Vec::new();
        for (session, mut task) in tasks.0.drain(..) {
            match block_on(future::poll_once(&mut task)) {
                Some(chunks) => ready.extend(chunks.into_iter().map(|c| (session, c))),
                None => still_running.push((session, task)),
            }
        }
        tasks.0 = still_running;
    }
    if ready.is_empty() {
        return;
    }

    // Resolve session → connected peer once for this frame.
    let server = server.into_inner();
    let mut peer_of: std::collections::HashMap<SessionId, _> = std::collections::HashMap::new();
    for remote in q_client.iter() {
        if let Some(session) = assigned.get(peer_to_session(remote.0).0) {
            peer_of.insert(session, remote.0);
        }
    }

    // Flush FIFO up to the per-frame cap; requeue the remainder.
    let mut sent = 0usize;
    let mut requeue = Vec::new();
    for (session, chunk) in std::mem::take(&mut *ready) {
        if sent >= crate::scenario_sync::MAX_CHUNKS_PER_FRAME {
            requeue.push((session, chunk));
            continue;
        }
        match peer_of.get(&session) {
            Some(&peer) => {
                server_send(
                    &mut sender,
                    server,
                    &NetworkTarget::Single(peer),
                    SyncChannel::BulkData,
                    &SyncEnvelope::AssetChunk(chunk),
                );
                sent += 1;
            }
            // Peer disconnected mid-transfer → drop its chunks (it re-requests on
            // reconnect from a fresh manifest).
            None => {}
        }
    }
    *ready = requeue;
}
