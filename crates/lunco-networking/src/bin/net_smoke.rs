//! Headless networking smoke-test harness.
//!
//! Runs the *real* `LunCoNetworkingPlugin` (lightyear WebTransport + our
//! cert/handshake/ferry/codec/authority/snapshots) with no window or scene.
//!
//!   net_smoke --host 5888
//!   net_smoke --connect 127.0.0.1:5888
//!
//! The two processes must overlap (both self-exit after `RUN_SECS`). The harness
//! exercises the full **possess → drive → snapshot** loop *and* the
//! **exclusive-possession / ownership-sync** guarantees end-to-end, with two
//! rovers:
//!
//! - **G1** — the host claims it at startup (its own rover);
//! - **G2** — free; the client claims it after the handshake.
//!
//! After the handshake the client:
//!   1. possesses **G2** (free → granted; host records the client as owner),
//!   2. *also* tries to possess **G1** (owned by the host → must be **denied**
//!      under the default `Exclusive` policy),
//!   3. continuously drives **both** G1 and G2 forward.
//!
//! The host authorizes each drive against ownership: G2's drives are applied (a
//! synthetic integrator moves it) and snapshotted back; G1's drives are
//! **rejected** (the client doesn't own it), so G1 never moves. The host also
//! broadcasts the authoritative ownership table, which the client adopts.
//!
//! Both peers also activate the SAME scripted (rhai) convergent **merge policy**
//! at startup, so the journal plane resolves concurrent edits (host's `H1`,
//! client's `C1`) via the hook — proving conflict resolution "in rhai" over the
//! real wire (the author-descending policy orders `["H1", "C1"]` where the
//! built-in key would give `["C1", "H1"]`).
//!
//! PASS (logged by the client as `[test] RESULT: PASS`) requires ALL of:
//!   - G2 proxy moved   (owned + driven + snapshot round-trip works);
//!   - G1 proxy did NOT move (unauthorized drive correctly rejected);
//!   - synced ownership shows G2 = me, G1 = host (`SessionId::LOCAL`);
//!   - the host's journal edit reached the client (journal-plane sync);
//!   - the scripted merge policy is active and drives the convergent order.
//!
//! Everything on the wire — the command reflect round-trip, `Entity`↔
//! `GlobalEntityId` mapping, the avatar-strip, authority, ownership broadcast,
//! and the snapshot — is the production code path.

use bevy::app::AppExit;
use bevy::prelude::*;
use lunco_core::SessionId;
use lunco_doc::DocumentId;
use lunco_doc_bevy::JournalResource;
use lunco_networking::{LunCoNetworkingPlugin, NetworkMode};
use lunco_twin_journal::{AuthorId, AuthorTag, DomainKind, EntryKind, MergeStrategy, TwinId};

/// When (seconds) the host authors a journal edit — after the handshake so it
/// ships via the live `broadcast_journal_entries` tail (not the on-connect
/// full-journal replay), exercising the steady-state path.
const HOST_JOURNAL_AT: f32 = 3.0;
/// When (seconds) the client authors its journal edit (client→host upload).
const CLIENT_JOURNAL_AT: f32 = 5.0;

/// Each peer self-exits after this many seconds. Generous so the host stays up
/// well past the client's connect/handshake latency (the active-overlap window
/// must comfortably cover the possess→drive→snapshot exchange).
const RUN_SECS: f32 = 25.0;

/// Host-owned rover id (claimed by the host at startup).
const G1_GID: u64 = 0x00AB_C001;
/// Free rover id (claimed by the client after the handshake).
const G2_GID: u64 = 0x00AB_C002;
/// Despawn-test rover id: host spawns it (replicated), then despawns it mid-run;
/// the client must remove its proxy in response (B5 despawn replication).
const G3_GID: u64 = 0x00AB_C003;
/// When (seconds) the host despawns G3 — after the handshake + proxy
/// registration, well before `RUN_SECS` so the client observes the removal.
const G3_DESPAWN_AT: f32 = 10.0;

/// Cadence sub-test (proves the server-side input buffer): the client drives G2
/// with a **changing** throttle SWEEP, one seq-stamped `SetPorts` per fixed tick.
/// The host's `Update` is throttled below its fixed rate, so without the buffer
/// its `drain`+latch *subsamples* the sweep and its integrated distance falls
/// short of the client's ideal integral — the cadence divergence behind the
/// post-turn wobble. With `LUNCO_INPUT_BUFFER=1` the host consumes the buffer one
/// input per fixed tick, in seq order, and the distance matches.
///
/// Host `Update` target rate (below the 64 Hz fixed rate → forces subsampling).
const HOST_UPDATE_HZ: f64 = 40.0;
/// Peak forward speed at throttle 1.0 (matches `host_apply_and_integrate`).
const FWD_SPEED: f32 = 2.0;

/// Throttle for cadence tick `seq`: a positive sweep in [0.15, 1.0] (never 0 so G2
/// keeps moving forward for the existing "G2 moved" assert), value CHANGING every
/// tick so subsampling actually loses information.
fn cadence_throttle(seq: u32) -> f64 {
    0.575 + 0.425 * ((seq as f64) * 0.20).sin()
}

/// Whether the host consumes the per-tick input buffer (`LUNCO_INPUT_BUFFER=1`)
/// instead of the latched, render-cadence-subsampled `DriveVel`.
#[derive(Resource)]
struct BufferEnabled(bool);

/// Client-side running integral of the sweep it SENT — the distance the host
/// should reach if it applied every input (the cadence ground truth).
#[derive(Resource, Default)]
struct ClientIdealX(f32);

#[derive(Component)]
struct TestRover;

/// Which rover this entity stands in for (so reports/asserts can tell them
/// apart on both peers).
#[derive(Component, Clone, Copy)]
struct RoverGid(u64);

/// Host: forward speed last commanded by an *applied* (authorized) `SetPorts`.
#[derive(Component, Default)]
struct DriveVel(f32);

/// The local stand-in rover entities (host's authoritative ones / client proxies).
#[derive(Resource, Clone, Copy)]
struct Rovers {
    g1: Entity,
    g2: Entity,
    g3: Entity,
}

/// Client: furthest x each proxy reached (the PASS metric).
#[derive(Resource, Default)]
struct MaxProxyX {
    g1: f32,
    g2: f32,
}

/// Latches the handshake-assigned session id the first time it is non-zero, so the
/// end-of-run verdict survives the disconnect that resets `LocalSession` back to 0
/// when the host exits a moment before the client.
#[derive(Resource, Default)]
struct MySession(u64);

fn main() {
    let Some(mode) = NetworkMode::from_args() else {
        eprintln!("usage: net_smoke --host [port] | --connect <addr>");
        return;
    };
    let is_host = matches!(mode, NetworkMode::Host { .. });

    let mut app = App::new();
    // Throttle the HOST's Update loop below the fixed rate so `drain_sync_inbox`
    // (Update) subsamples the client's per-fixed-tick input stream — reproducing
    // the render-cadence input loss the buffer fixes. The client runs unthrottled.
    if is_host {
        app.add_plugins(MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / HOST_UPDATE_HZ),
        )));
    } else {
        app.add_plugins(MinimalPlugins);
    }
    app.add_plugins(bevy::log::LogPlugin::default());
    // A domain plugin (core/api/networking) uses `init_state`, which needs the
    // `StateTransition` schedule — absent from `MinimalPlugins`. `DefaultPlugins`
    // would pull it in, but this harness is windowless; add just `StatesPlugin`.
    app.add_plugins(bevy::state::app::StatesPlugin);
    // The SyncPlugin's tutor-mode input-blocking systems (`block_bevy_inputs`,
    // `block_perspective_inputs`) take `ResMut<ButtonInput<…>>`; `MinimalPlugins`
    // omits those resources (no window), so seed empty ones here to keep the systems
    // from panicking on a missing resource. Cheaper than a full `InputPlugin`.
    app.init_resource::<ButtonInput<KeyCode>>();
    app.init_resource::<ButtonInput<MouseButton>>();
    // `apply_tutorial_mirroring` takes `ResMut<WorkspaceResource>` (non-optional);
    // the full app provides it via the workspace plugin, absent here — seed an empty.
    app.init_resource::<lunco_workspace::WorkspaceResource>();
    app.init_resource::<MySession>();
    // Journal plane: give both peers a `JournalResource` so the journal-sync
    // systems (`stamp_host_journal_author`, `broadcast_journal_entries`, the
    // inbound merge arm) are live. The host stamps author "host" at Startup; the
    // client stamps "peer-<session>" on handshake. Faithful to the full app's
    // `TwinJournalPlugin`; inserted directly to keep the harness minimal.
    app.insert_resource(JournalResource::new(TwinId::new("smoke"), AuthorId::local()));
    app.add_plugins(lunco_core::LunCoCorePlugin);
    app.add_plugins(lunco_api::LunCoApiPlugin::default());
    app.add_plugins(LunCoNetworkingPlugin { mode: Some(mode) });

    // Register the command types so the wire reflect (de)serialize + trigger
    // path works (the domain plugins normally do this; the harness skips them).
    app.register_type::<lunco_cosim::SetPorts>();
    app.register_type::<lunco_avatar::PossessVessel>();

    // Both peers activate the SAME scripted (rhai) convergent merge policy before
    // any edits are authored — exercising conflict resolution over the real wire.
    app.add_systems(Startup, activate_smoke_merge_policy);

    if is_host {
        let buffer_on = std::env::var("LUNCO_INPUT_BUFFER").as_deref() == Ok("1");
        info!("[test] HOST input buffer: {}", if buffer_on { "ON" } else { "OFF (render-cadence subsample)" });
        app.insert_resource(BufferEnabled(buffer_on));
        app.add_systems(Startup, host_spawn_rovers);
        app.add_observer(host_on_possess);
        app.add_observer(host_on_drive);
        // Integrate per FIXED tick (not render Update): the cadence divergence is a
        // fixed-vs-render-rate artifact, so the integrator must live on the fixed clock.
        app.add_systems(FixedUpdate, host_apply_and_integrate);
        app.add_systems(
            Update,
            (host_report, host_despawn_g3, host_author_journal_entry, host_journal_report),
        );
    } else {
        app.init_resource::<MaxProxyX>();
        app.init_resource::<ClientIdealX>();
        app.add_systems(Startup, client_spawn_proxies);
        // The cadence drive runs on the FIXED clock (one seq-stamped input per tick).
        app.add_systems(FixedUpdate, client_drive_cadence);
        app.add_systems(
            Update,
            (client_act, test_apply_snapshots, client_report, client_author_journal_entry),
        );
    }

    app.add_systems(Update, (report_session, exit_after_timeout));
    app.run();
}

// ── Host (authoritative) ──────────────────────────────────────────────────────

fn host_spawn_rovers(mut commands: Commands) {
    let spawn = |commands: &mut Commands, name: &str, gid: u64| {
        commands
            .spawn((
                Name::new(name.to_string()),
                Transform::default(),
                GlobalTransform::default(),
                lunco_core::GlobalEntityId::from_raw(gid),
                lunco_core::NetReplicate,
                TestRover,
                RoverGid(gid),
                DriveVel::default(),
            ))
            .id()
    };
    let g1 = spawn(&mut commands, "HostRover_G1", G1_GID);
    let g2 = spawn(&mut commands, "HostRover_G2", G2_GID);
    let g3 = spawn(&mut commands, "HostRover_G3", G3_GID);
    commands.insert_resource(Rovers { g1, g2, g3 });

    // The host claims its own rover (G1) through the real possession observer.
    // `guard` is None here → the claim is attributed to the host's `LocalSession`
    // (`SessionId::LOCAL`). The client must NOT be able to take it.
    commands.trigger(lunco_avatar::PossessVessel { avatar: Some(g1), target: g1 });
    info!("[test] host rovers spawned g1(self)={G1_GID:#x} g2(free)={G2_GID:#x}");
}

/// Mirror of `record_possession_authority`: claim ownership for the possessing
/// session (origin from the wire-apply guard, else the local host session) so
/// its drives are authorized (G4). Exclusivity is enforced by `claim`.
fn host_on_possess(
    trigger: On<lunco_avatar::PossessVessel>,
    guard: Res<lunco_core::SyncApplyGuard>,
    local: Res<lunco_core::LocalSession>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut reg: ResMut<lunco_core::SessionRegistry>,
) {
    let cmd = trigger.event();
    let origin = guard.0.unwrap_or(local.0);
    match q_gid.get(cmd.target) {
        Ok(g) => match reg.claim(origin, g.get()) {
            Ok(()) => info!("[test] possession CLAIMED gid={:#x} by session={origin}", g.get()),
            Err(c) => warn!(
                "[test] possession DENIED gid={:#x} for {origin} (owned by {c})",
                g.get()
            ),
        },
        Err(_) => warn!("[test] possess target {:?} has no gid", cmd.target),
    }
}

/// Applied (authorized) drive → set the rover's synthetic forward speed from the
/// `"throttle"` port write. An *unauthorized* drive never reaches here —
/// `apply_sync_command` rejects it at the authority gate before triggering.
fn host_on_drive(
    trigger: On<lunco_cosim::SetPorts>,
    mut q: Query<(&lunco_core::GlobalEntityId, &mut DriveVel)>,
    mut buf: ResMut<lunco_core::BufferedClientInputs>,
) {
    let cmd = trigger.event();
    if let Ok((gid, mut v)) = q.get_mut(cmd.target) {
        if let Some((_, throttle)) = cmd.writes.iter().find(|(n, _)| n == "throttle") {
            // Latched value — what the render-cadence path uses (buffer OFF): the
            // LAST input seen this Update wins, dropping the intervening sweep.
            v.0 = *throttle as f32;
        }
        // Buffered by seq — the per-tick path uses this (buffer ON).
        buf.push(gid.get(), cmd.seq, cmd.writes.clone());
    }
}

/// Despawn the replicated G3 rover mid-run. `broadcast_despawns` should emit a
/// `Despawn(G3_GID)` over the wire, which the client's `drain_sync_inbox` Despawn
/// arm resolves (via `ApiEntityRegistry`) and uses to remove its G3 proxy. This is
/// the end-to-end exercise of B5 despawn replication.
fn host_despawn_g3(
    time: Res<Time>,
    rovers: Option<Res<Rovers>>,
    mut commands: Commands,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < G3_DESPAWN_AT {
        return;
    }
    if let Some(rovers) = rovers {
        commands.entity(rovers.g3).despawn();
        *done = true;
        info!("[test] host despawned G3 ({G3_GID:#x}) — expect client proxy removal");
    }
}

/// Synthetic physics on the FIXED clock: integrate forward speed into the rover's
/// x. When the buffer is ON, pull this tick's throttle from the per-tick input
/// buffer (seq-ordered, one per fixed tick) instead of the render-cadence-latched
/// `DriveVel` — so the host integrates the client's full input sequence.
fn host_apply_and_integrate(
    time: Res<Time>,
    buffer_on: Res<BufferEnabled>,
    mut buf: ResMut<lunco_core::BufferedClientInputs>,
    mut q: Query<(&RoverGid, &mut DriveVel, &mut Transform), With<TestRover>>,
) {
    let dt = time.delta_secs();
    for (gid, mut v, mut tf) in q.iter_mut() {
        if buffer_on.0 {
            if let Some(writes) = buf.next_for_tick(gid.0, 8) {
                if let Some((_, throttle)) = writes.iter().find(|(n, _)| n == "throttle") {
                    v.0 = *throttle as f32;
                }
            }
        }
        tf.translation.x += v.0 * FWD_SPEED * dt;
    }
}

fn host_report(
    time: Res<Time>,
    mut t: Local<f32>,
    q: Query<(&RoverGid, &Transform, &DriveVel), With<TestRover>>,
) {
    *t += time.delta_secs();
    if *t > 1.0 {
        *t = 0.0;
        for (gid, tf, v) in q.iter() {
            info!("[test] host rover {:#x} x={:.2} drive={:.2}", gid.0, tf.translation.x, v.0);
        }
    }
}

// ── Client (proxy) ────────────────────────────────────────────────────────────

fn client_spawn_proxies(mut commands: Commands) {
    let spawn = |commands: &mut Commands, name: &str, gid: u64| {
        commands
            .spawn((
                Name::new(name.to_string()),
                Transform::default(),
                GlobalTransform::default(),
                lunco_core::GlobalEntityId::from_raw(gid),
                TestRover,
                RoverGid(gid),
            ))
            .id()
    };
    let g1 = spawn(&mut commands, "ClientProxy_G1", G1_GID);
    let g2 = spawn(&mut commands, "ClientProxy_G2", G2_GID);
    let g3 = spawn(&mut commands, "ClientProxy_G3", G3_GID);
    commands.insert_resource(Rovers { g1, g2, g3 });
    info!("[test] client proxies spawned g1={G1_GID:#x} g2={G2_GID:#x} g3={G3_GID:#x}");
}

/// After the handshake lands (LocalSession non-zero): possess G2 (free → should
/// be granted) and *also* try to possess G1 (host-owned → should be denied),
/// then drive **both** every frame. G2's drives are authorized and move it;
/// G1's are rejected by the host (the client never owns it).
fn client_act(
    local: Res<lunco_core::LocalSession>,
    rovers: Option<Res<Rovers>>,
    mut commands: Commands,
    mut acted: Local<bool>,
) {
    if local.0 .0 == 0 {
        return; // handshake not yet received
    }
    let Some(rovers) = rovers else {
        return;
    };
    if !*acted {
        // Claim the free rover…
        commands.trigger(lunco_avatar::PossessVessel { avatar: Some(rovers.g2), target: rovers.g2 });
        // …and attempt to steal the host's rover (must be refused).
        commands.trigger(lunco_avatar::PossessVessel { avatar: Some(rovers.g1), target: rovers.g1 });
        *acted = true;
        info!("[test] client requested possession of G2 (free) and G1 (host-owned)");
    }
    // Drive G1 (host-owned) — must be REJECTED by the host authority gate (the
    // client never owns it). G2 (owned) is driven by `client_drive_cadence` on the
    // fixed clock — the seq-stamped sweep that the cadence sub-test measures.
    commands.trigger(lunco_cosim::SetPorts { target: rovers.g1, writes: vec![("throttle".into(), 1.0), ("steer".into(), 0.0)], seq: 0, tick: 0 });
}

/// Client cadence drive (FIXED clock): once possessed, send ONE seq-stamped
/// `SetPorts` per fixed tick for G2 with a CHANGING throttle sweep, and accumulate
/// the ideal x (integral of the sweep) — the distance the host reaches iff it
/// applies EVERY input. The host's actual G2 x (adopted back via snapshot into the
/// client's own G2 proxy) is compared against this in `client_report`: they match
/// with the buffer, and fall short without it.
fn client_drive_cadence(
    time: Res<Time>,
    local: Res<lunco_core::LocalSession>,
    rovers: Option<Res<Rovers>>,
    mut seq: Local<u32>,
    mut ideal: ResMut<ClientIdealX>,
    mut commands: Commands,
) {
    if local.0 .0 == 0 {
        return; // handshake not yet received
    }
    let Some(rovers) = rovers else {
        return;
    };
    *seq += 1;
    let throttle = cadence_throttle(*seq);
    commands.trigger(lunco_cosim::SetPorts {
        target: rovers.g2,
        writes: vec![("throttle".into(), throttle)],
        seq: *seq,
        tick: *seq as u64,
    });
    ideal.0 += (throttle as f32) * FWD_SPEED * time.delta_secs();
}

/// Client-side snapshot apply (mirrors `lunco_sandbox_edit::apply_incoming_snapshots`
/// minus the avian `Position` write the stand-in rover doesn't have).
fn test_apply_snapshots(
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut snaps: ResMut<lunco_core::IncomingSnapshots>,
    mut q: Query<&mut Transform>,
) {
    if snaps.0.is_empty() {
        return;
    }
    for s in snaps.0.drain(..).collect::<Vec<_>>() {
        if let Some(e) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(s.gid)) {
            if let Ok(mut tf) = q.get_mut(e) {
                tf.translation = Vec3::from(s.t);
                tf.rotation = Quat::from_array(s.r);
            }
        }
    }
}

fn client_report(
    time: Res<Time>,
    mut t: Local<f32>,
    rovers: Option<Res<Rovers>>,
    q: Query<(&RoverGid, &Transform), With<TestRover>>,
    mut maxx: ResMut<MaxProxyX>,
    ideal: Res<ClientIdealX>,
) {
    if rovers.is_none() {
        return;
    }
    let mut g2_now = 0.0;
    for (gid, tf) in q.iter() {
        let x = tf.translation.x;
        match gid.0 {
            G1_GID => maxx.g1 = maxx.g1.max(x),
            G2_GID => {
                maxx.g2 = maxx.g2.max(x);
                g2_now = x;
            }
            _ => {}
        }
    }
    *t += time.delta_secs();
    if *t > 1.0 {
        *t = 0.0;
        // CADENCE metric: host's actual G2 x (via snapshot) vs the client's ideal
        // integral of the sweep it sent. ratio → 1.0 means the host applied every
        // input (buffer ON / no subsample); ratio < 1 is dropped-input cadence loss.
        let ratio = if ideal.0 > 0.01 { g2_now / ideal.0 } else { 1.0 };
        info!(
            "[test] CADENCE g2_actual={:.2} ideal={:.2} ratio={:.3} (g1={:.2})",
            g2_now, ideal.0, ratio, maxx.g1
        );
    }
}

// ── Journal plane (host ↔ client edit-history sync over the real wire) ─────────

/// The scripted convergent merge policy both peers activate. Orders concurrent
/// entries by author **descending** — the reverse of the built-in `(lamport,
/// author)` key — so its effect is visible in the merged order.
const SMOKE_MERGE_HOOK: &str = "smoke.merge.author_desc";
const SMOKE_MERGE_SRC: &str =
    "fn cmp(a, b) { if a.author < b.author { 1 } else if a.author > b.author { -1 } else { 0 } }";

/// Both peers, at startup: activate the SAME scripted merge policy, so the whole
/// journal plane (client scene replay, `append_remote` main re-pointing,
/// `merged_head`) resolves concurrent edits via the rhai hook rather than the
/// built-in key. Identical hook id + source on every peer ⇒ convergent (the
/// [`MergeStrategy`] determinism contract). This is the "conflict resolution
/// strategies but in rhai" path exercised over the real WebTransport wire.
fn activate_smoke_merge_policy(journal: Option<Res<JournalResource>>) {
    let Some(j) = journal else {
        return;
    };
    match lunco_networking::journal_plane::activate_scripted_merge_policy(
        &j,
        SMOKE_MERGE_HOOK,
        "cmp",
        SMOKE_MERGE_SRC,
    ) {
        Ok(()) => info!("[test] activated scripted merge policy '{SMOKE_MERGE_HOOK}' (author-descending)"),
        Err(e) => warn!("[test] scripted merge policy failed to compile: {e}"),
    }
}

/// Author one journal `Op` entry (a stand-in USD edit) into `journal`, tagged as
/// `marker`. `EntryId.author` is the journal's stamped local author (host="host",
/// client="peer-<session>").
fn author_marker(journal: &JournalResource, marker: &str) {
    journal.with_write(|j| {
        j.append_local(
            AuthorTag::for_tool("test"),
            DocumentId::new(1),
            EntryKind::Op {
                domain: DomainKind::Usd,
                op: serde_json::json!({ "marker": marker }),
                inverse: serde_json::json!({}),
            },
            None,
        );
    });
}

/// Host: author one journal edit mid-run. `broadcast_journal_entries` ships it to
/// the client over the real wire; the client's `append_remote` merges it — the
/// host→client journal-sync exercise.
fn host_author_journal_entry(
    time: Res<Time>,
    journal: Option<Res<JournalResource>>,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < HOST_JOURNAL_AT {
        return;
    }
    if let Some(j) = journal {
        author_marker(&j, "H1");
        *done = true;
        info!("[test] host authored journal entry H1 (expect client to receive it)");
    }
}

/// Host: report how many journal entries authored by a PEER (client) it holds —
/// non-zero proves the client→host journal upload landed (the bidirectional leg).
fn host_journal_report(time: Res<Time>, mut t: Local<f32>, journal: Option<Res<JournalResource>>) {
    *t += time.delta_secs();
    if *t < 1.0 {
        return;
    }
    *t = 0.0;
    if let Some(j) = journal {
        let (total, peers) = j.with_read(|jj| {
            let me = jj.local_author();
            (jj.len(), jj.entries().filter(|e| &e.id.author != me).count())
        });
        info!("[test] HOST-JOURNAL total={total} peer_entries={peers}");
    }
}

/// Client: after the handshake, author one journal edit — the journal plane ships
/// it UP to the host (client→host), the bidirectional leg.
fn client_author_journal_entry(
    time: Res<Time>,
    local: Res<lunco_core::LocalSession>,
    journal: Option<Res<JournalResource>>,
    mut done: Local<bool>,
) {
    if *done || local.0 .0 == 0 || time.elapsed_secs() < CLIENT_JOURNAL_AT {
        return;
    }
    if let Some(j) = journal {
        author_marker(&j, "C1");
        *done = true;
        info!("[test] client authored journal entry C1 (expect host to receive it)");
    }
}

// ── Shared ────────────────────────────────────────────────────────────────────

fn report_session(
    local: Res<lunco_core::LocalSession>,
    mut mine: ResMut<MySession>,
    mut last: Local<u64>,
) {
    let cur = local.0 .0;
    if cur != 0 {
        mine.0 = cur; // latch the assigned id; never overwrite with a disconnect's 0
    }
    if cur != *last {
        *last = cur;
        info!("[smoke] LocalSession now = {cur}");
    }
}

fn exit_after_timeout(
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    maxx: Option<Res<MaxProxyX>>,
    mine: Res<MySession>,
    registry: Option<Res<lunco_core::SessionRegistry>>,
    q_proxies: Query<&RoverGid, With<TestRover>>,
    journal: Option<Res<JournalResource>>,
) {
    if time.elapsed_secs() <= RUN_SECS {
        return;
    }
    // Only the client renders a verdict (it's the peer that observes both the
    // snapshot result and the synced ownership table).
    if let (Some(m), Some(reg)) = (maxx, registry) {
        let me = SessionId(mine.0);
        let g2_owner = reg.owner_of(G2_GID);
        let g1_owner = reg.owner_of(G1_GID);

        let g2_moved = m.g2 > 0.5; // owned + driven + snapshot round-trip
        let g1_still = m.g1 < 0.1; // unauthorized drive rejected → no motion
        let g2_mine = g2_owner == Some(me); // ownership broadcast adopted
        let g1_host = g1_owner == Some(SessionId::LOCAL); // host kept its rover
        // B5: the host despawned G3 mid-run; its proxy must be gone on the client.
        let g3_despawned = !q_proxies.iter().any(|g| g.0 == G3_GID);

        // Journal plane: the host's authored edit (marker `H1`) must have
        // reached this client over the wire (host→client journal sync); this
        // client only ever authors `C1`. Checked by CONTENT, not authorship —
        // journal authors are persistent MACHINE-unique identities
        // (db316619), so two smoke processes on one box share the author id
        // and a `foreign author` test can never pass in the standard
        // single-machine invocation (it failed spuriously from 2026-07-02
        // until 2026-07-11 while the plane itself worked). The client→host
        // leg is checked on the host side (`HOST-JOURNAL peer_entries`).
        let journal_from_host = journal
            .as_ref()
            .map(|j| {
                j.with_read(|jj| {
                    jj.entries().any(|e| {
                        matches!(&e.kind, EntryKind::Op { op, .. }
                            if op.get("marker").and_then(|m| m.as_str()) == Some("H1"))
                    })
                })
            })
            .unwrap_or(false);

        // The scripted (rhai) merge policy must be active on this peer, and the
        // convergent merged order is what it dictates (concurrent edits ordered by
        // author DESCENDING) — proving the journal plane ran WITH the scripted
        // conflict-resolution strategy over the wire.
        let (policy_active, markers) = journal
            .as_ref()
            .map(|j| {
                j.with_read(|jj| {
                    let active = matches!(jj.merge_strategy(), MergeStrategy::Scripted(id) if id == SMOKE_MERGE_HOOK);
                    let order = jj
                        .merged_order()
                        .iter()
                        .filter_map(|e| match &e.kind {
                            EntryKind::Op { op, .. } => {
                                op.get("marker").and_then(|m| m.as_str()).map(String::from)
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    (active, order)
                })
            })
            .unwrap_or((false, Vec::new()));

        info!(
            "[test] checks: g2_moved={g2_moved} (x={:.2})  g1_still={g1_still} (x={:.2})  \
             g2_mine={g2_mine} (owner={g2_owner:?}, me={me})  g1_host={g1_host} (owner={g1_owner:?})  \
             g3_despawned={g3_despawned}  journal_from_host={journal_from_host}  \
             policy_active={policy_active}  merged_markers={markers:?}",
            m.g2, m.g1
        );

        if g2_moved && g1_still && g2_mine && g1_host && g3_despawned && journal_from_host && policy_active {
            info!("[test] RESULT: PASS — exclusive possession + ownership-gated drive + sync + despawn-repl + journal-sync (scripted merge policy) all hold");
        } else {
            warn!("[test] RESULT: FAIL — see checks above");
        }
    }
    info!("[smoke] timeout reached, exiting");
    exit.write(AppExit::Success);
}
