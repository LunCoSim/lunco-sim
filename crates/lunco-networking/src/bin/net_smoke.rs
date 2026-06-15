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
//! PASS (logged by the client as `[test] RESULT: PASS`) requires ALL of:
//!   - G2 proxy moved   (owned + driven + snapshot round-trip works);
//!   - G1 proxy did NOT move (unauthorized drive correctly rejected);
//!   - synced ownership shows G2 = me, G1 = host (`SessionId::LOCAL`).
//!
//! Everything on the wire — the command reflect round-trip, `Entity`↔
//! `GlobalEntityId` mapping, the avatar-strip, authority, ownership broadcast,
//! and the snapshot — is the production code path.

use bevy::app::AppExit;
use bevy::prelude::*;
use lunco_core::SessionId;
use lunco_networking::{LunCoNetworkingPlugin, NetworkMode};

/// Each peer self-exits after this many seconds. Generous so the host stays up
/// well past the client's connect/handshake latency (the active-overlap window
/// must comfortably cover the possess→drive→snapshot exchange).
const RUN_SECS: f32 = 25.0;

/// Host-owned rover id (claimed by the host at startup).
const G1_GID: u64 = 0x00AB_C001;
/// Free rover id (claimed by the client after the handshake).
const G2_GID: u64 = 0x00AB_C002;

#[derive(Component)]
struct TestRover;

/// Which rover this entity stands in for (so reports/asserts can tell them
/// apart on both peers).
#[derive(Component, Clone, Copy)]
struct RoverGid(u64);

/// Host: forward speed last commanded by an *applied* (authorized) `DriveRover`.
#[derive(Component, Default)]
struct DriveVel(f32);

/// The local stand-in rover entities (host's authoritative ones / client proxies).
#[derive(Resource, Clone, Copy)]
struct Rovers {
    g1: Entity,
    g2: Entity,
}

/// Client: furthest x each proxy reached (the PASS metric).
#[derive(Resource, Default)]
struct MaxProxyX {
    g1: f32,
    g2: f32,
}

fn main() {
    let Some(mode) = NetworkMode::from_args() else {
        eprintln!("usage: net_smoke --host [port] | --connect <addr>");
        return;
    };
    let is_host = matches!(mode, NetworkMode::Host { .. });

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::log::LogPlugin::default());
    app.add_plugins(lunco_core::LunCoCorePlugin);
    app.add_plugins(lunco_api::LunCoApiPlugin::default());
    app.add_plugins(LunCoNetworkingPlugin { mode });

    // Register the command types so the wire reflect (de)serialize + trigger
    // path works (the domain plugins normally do this; the harness skips them).
    app.register_type::<lunco_mobility::DriveRover>();
    app.register_type::<lunco_avatar::PossessVessel>();

    if is_host {
        app.add_systems(Startup, host_spawn_rovers);
        app.add_observer(host_on_possess);
        app.add_observer(host_on_drive);
        app.add_systems(Update, (host_integrate, host_report));
    } else {
        app.init_resource::<MaxProxyX>();
        app.add_systems(Startup, client_spawn_proxies);
        app.add_systems(Update, (client_act, test_apply_snapshots, client_report));
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
    commands.insert_resource(Rovers { g1, g2 });

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

/// Applied (authorized) drive → set the rover's synthetic forward speed. An
/// *unauthorized* drive never reaches here — `apply_sync_command` rejects it at
/// the authority gate before triggering the event.
fn host_on_drive(trigger: On<lunco_mobility::DriveRover>, mut q: Query<&mut DriveVel>) {
    let cmd = trigger.event();
    if let Ok(mut v) = q.get_mut(cmd.target) {
        v.0 = cmd.forward as f32;
    }
}

/// Synthetic physics: integrate forward speed into the rover's x (stands in for
/// the real cosim/avian drive→motion, which is out of scope for a wire test).
fn host_integrate(time: Res<Time>, mut q: Query<(&DriveVel, &mut Transform), With<TestRover>>) {
    let dt = time.delta_secs();
    for (v, mut tf) in q.iter_mut() {
        tf.translation.x += v.0 * 2.0 * dt; // 2 m/s at full forward
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
    commands.insert_resource(Rovers { g1, g2 });
    info!("[test] client proxies spawned g1={G1_GID:#x} g2={G2_GID:#x}");
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
    // Drive both — only the owned one (G2) should actually move.
    commands.trigger(lunco_mobility::DriveRover { target: rovers.g2, forward: 1.0, steer: 0.0, seq: 0, tick: 0 });
    commands.trigger(lunco_mobility::DriveRover { target: rovers.g1, forward: 1.0, steer: 0.0, seq: 0, tick: 0 });
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
) {
    if rovers.is_none() {
        return;
    }
    for (gid, tf) in q.iter() {
        let x = tf.translation.x;
        match gid.0 {
            G1_GID => maxx.g1 = maxx.g1.max(x),
            G2_GID => maxx.g2 = maxx.g2.max(x),
            _ => {}
        }
    }
    *t += time.delta_secs();
    if *t > 1.0 {
        *t = 0.0;
        info!("[test] client proxies x: g1={:.2} g2={:.2}", maxx.g1, maxx.g2);
    }
}

// ── Shared ────────────────────────────────────────────────────────────────────

fn report_session(local: Res<lunco_core::LocalSession>, mut last: Local<u64>) {
    let cur = local.0 .0;
    if cur != *last {
        *last = cur;
        info!("[smoke] LocalSession now = {cur}");
    }
}

fn exit_after_timeout(
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    maxx: Option<Res<MaxProxyX>>,
    local: Res<lunco_core::LocalSession>,
    registry: Option<Res<lunco_core::SessionRegistry>>,
) {
    if time.elapsed_secs() <= RUN_SECS {
        return;
    }
    // Only the client renders a verdict (it's the peer that observes both the
    // snapshot result and the synced ownership table).
    if let (Some(m), Some(reg)) = (maxx, registry) {
        let me = local.0;
        let g2_owner = reg.owner_of(G2_GID);
        let g1_owner = reg.owner_of(G1_GID);

        let g2_moved = m.g2 > 0.5; // owned + driven + snapshot round-trip
        let g1_still = m.g1 < 0.1; // unauthorized drive rejected → no motion
        let g2_mine = g2_owner == Some(me); // ownership broadcast adopted
        let g1_host = g1_owner == Some(SessionId::LOCAL); // host kept its rover

        info!(
            "[test] checks: g2_moved={g2_moved} (x={:.2})  g1_still={g1_still} (x={:.2})  \
             g2_mine={g2_mine} (owner={g2_owner:?}, me={me})  g1_host={g1_host} (owner={g1_owner:?})",
            m.g2, m.g1
        );

        if g2_moved && g1_still && g2_mine && g1_host {
            info!("[test] RESULT: PASS — exclusive possession + ownership-gated drive + sync all hold");
        } else {
            warn!("[test] RESULT: FAIL — see checks above");
        }
    }
    info!("[smoke] timeout reached, exiting");
    exit.write(AppExit::Success);
}
