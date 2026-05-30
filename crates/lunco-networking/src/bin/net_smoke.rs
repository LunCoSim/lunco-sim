//! Headless networking smoke-test harness.
//!
//! Runs the *real* `LunCoNetworkingPlugin` (lightyear WebTransport + our
//! cert/handshake/ferry/codec/authority/snapshots) with no window or scene.
//!
//!   net_smoke --host 5888
//!   net_smoke --connect 127.0.0.1:5888
//!
//! The two processes must overlap (both self-exit after ~15s). The harness
//! exercises the full **possess → drive → snapshot** loop end-to-end:
//!
//! - host spawns a `NetReplicate` stand-in rover (fixed id `TEST_GID`);
//! - client spawns a proxy at the same id, then (after the handshake) sends a
//!   real `PossessVessel` followed by continuous `DriveRover`s;
//! - host authorizes (ownership from the possession), applies the drive, and a
//!   *synthetic* integrator moves the rover (real physics/cosim is not a
//!   networking concern and is out of scope here);
//! - the host snapshots the moved transform; the client applies it to its proxy.
//!
//! PASS = the client logs `[test] RESULT: PASS` (its proxy moved purely from
//! networked drive + snapshot). Everything on the wire — the command reflect
//! round-trip, `Entity`↔`GlobalEntityId` mapping, authority, and the snapshot —
//! is the production code path.

use bevy::app::AppExit;
use bevy::prelude::*;
use lunco_networking::{LunCoNetworkingPlugin, NetworkMode};

/// Fixed id both peers pin their stand-in rover to (bypasses catalog/USD spawn
/// replication, which needs the asset pipeline — tested in the GUI build).
const TEST_GID: u64 = 0x00AB_CDEF;

#[derive(Component)]
struct TestRover;

/// Host: forward speed last commanded by an applied `DriveRover`.
#[derive(Component, Default)]
struct DriveVel(f32);

/// The local stand-in rover entity (host's authoritative one / client's proxy).
#[derive(Resource)]
struct LocalRover(Entity);

/// Client: furthest x the proxy reached (the PASS metric).
#[derive(Resource, Default)]
struct MaxProxyX(f32);

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
        app.add_systems(Startup, host_spawn_rover);
        app.add_observer(host_on_possess);
        app.add_observer(host_on_drive);
        app.add_systems(Update, (host_integrate, host_report));
    } else {
        app.init_resource::<MaxProxyX>();
        app.add_systems(Startup, client_spawn_proxy);
        app.add_systems(Update, (client_drive, test_apply_snapshots, client_report));
    }

    app.add_systems(Update, (report_session, exit_after_timeout));
    app.run();
}

// ── Host (authoritative) ──────────────────────────────────────────────────────

fn host_spawn_rover(mut commands: Commands) {
    let e = commands
        .spawn((
            Name::new("HostRover"),
            Transform::default(),
            GlobalTransform::default(),
            lunco_core::GlobalEntityId::from_raw(TEST_GID),
            lunco_core::NetReplicate,
            TestRover,
            DriveVel::default(),
        ))
        .id();
    commands.insert_resource(LocalRover(e));
    info!("[test] host rover spawned entity={e:?} gid={TEST_GID}");
}

/// Mirror of `record_possession_authority`: claim ownership for the possessing
/// session (origin from the wire-apply guard) so its drives are authorized (G4).
fn host_on_possess(
    trigger: On<lunco_avatar::PossessVessel>,
    guard: Res<lunco_core::WireApplyGuard>,
    local: Res<lunco_core::LocalSession>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut reg: ResMut<lunco_core::SessionRegistry>,
) {
    let cmd = trigger.event();
    let origin = guard.0.unwrap_or(local.0);
    match q_gid.get(cmd.target) {
        Ok(g) => match reg.claim(origin, g.get()) {
            Ok(()) => info!("[test] possession CLAIMED gid={} by session={origin}", g.get()),
            Err(c) => warn!("[test] possession denied gid={} (owned by {c})", g.get()),
        },
        Err(_) => warn!("[test] possess target {:?} has no gid", cmd.target),
    }
}

/// Applied (authorized) drive → set the rover's synthetic forward speed.
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
    q: Query<(&Transform, &DriveVel), With<TestRover>>,
) {
    *t += time.delta_secs();
    if *t > 1.0 {
        *t = 0.0;
        if let Some((tf, v)) = q.iter().next() {
            info!("[test] host rover x={:.2} drive={:.2}", tf.translation.x, v.0);
        }
    }
}

// ── Client (proxy) ────────────────────────────────────────────────────────────

fn client_spawn_proxy(mut commands: Commands) {
    let e = commands
        .spawn((
            Name::new("ClientProxy"),
            Transform::default(),
            GlobalTransform::default(),
            lunco_core::GlobalEntityId::from_raw(TEST_GID),
            TestRover,
        ))
        .id();
    commands.insert_resource(LocalRover(e));
    info!("[test] client proxy spawned entity={e:?} gid={TEST_GID}");
}

/// After the handshake lands (LocalSession non-zero), possess the rover once,
/// then drive forward every frame (robust to reliable/unreliable channel
/// ordering — drives keep coming until the possession is recorded).
fn client_drive(
    local: Res<lunco_core::LocalSession>,
    rover: Option<Res<LocalRover>>,
    mut commands: Commands,
    mut possessed: Local<bool>,
) {
    if local.0 .0 == 0 {
        return; // handshake not yet received
    }
    let Some(rover) = rover else {
        return;
    };
    if !*possessed {
        commands.trigger(lunco_avatar::PossessVessel {
            avatar: rover.0,
            target: rover.0,
        });
        *possessed = true;
        info!("[test] client sent PossessVessel target={:?}", rover.0);
    }
    commands.trigger(lunco_mobility::DriveRover {
        target: rover.0,
        forward: 1.0,
        steer: 0.0,
    });
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
    rover: Option<Res<LocalRover>>,
    q: Query<&Transform, With<TestRover>>,
    mut maxx: ResMut<MaxProxyX>,
) {
    let Some(rover) = rover else {
        return;
    };
    if let Ok(tf) = q.get(rover.0) {
        if tf.translation.x > maxx.0 {
            maxx.0 = tf.translation.x;
        }
        *t += time.delta_secs();
        if *t > 1.0 {
            *t = 0.0;
            info!("[test] client proxy x={:.2}", tf.translation.x);
        }
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
) {
    if time.elapsed_secs() > 15.0 {
        if let Some(m) = maxx {
            if m.0 > 0.5 {
                info!(
                    "[test] RESULT: PASS — proxy drove to x={:.2} via networked drive+snapshot",
                    m.0
                );
            } else {
                warn!("[test] RESULT: FAIL — proxy did not move (x={:.2})", m.0);
            }
        }
        info!("[smoke] timeout reached, exiting");
        exit.write(AppExit::Success);
    }
}
