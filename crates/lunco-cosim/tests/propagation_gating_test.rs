//! Propagation SCHEDULING + per-target network gating.
//!
//! These tests exist because both properties they cover are invisible to
//! `cargo check` and to the single-process parity scenes (which are headless,
//! single-threaded, and have no client/server split). Both are also exactly what
//! a role-gated, `FixedUpdate`-only propagation gets wrong:
//!
//! 1. **A client must still propagate into what it simulates.** Propagation is
//!    the control DAC — a rover's `SetPorts` command reaches its actuators only
//!    by being carried across `SimConnection`s into the wheel's `drive`/`steer`
//!    `Port`s. Gate that off for the whole process on `NetworkRole::Client` and
//!    a predicted rover silently stops driving on clients while working
//!    perfectly on the host.
//! 2. **Rollback replay must re-derive port values.** Replay re-runs the
//!    actuation chain (`lunco_core::RollbackReplay`) per unacked input. If
//!    propagation is not in that schedule, the replayed actuators read whatever
//!    the ports happened to hold, the replay's forces differ from the host's, and
//!    prediction diverges on the one body rollback exists to keep in sync.
//!
//! Everything here drives REAL schedules (`app.update()` / `run_schedule`) — the
//! scheduling is the thing under test, so calling the system as a bare function
//! would assert nothing.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

use lunco_core::architecture::Port;
use lunco_cosim::{ports::PORT_NAME, CoSimPlugin, SimConnection};

/// Minimal headless app: cosim over avian, with the fixed clock driven manually
/// so one `app.update()` runs at least one `FixedUpdate`.
fn app_with_role(role: Option<lunco_core::NetworkRole>) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        // avian's `bevy_diagnostic` feature registers collider-tree diagnostics,
        // whose resource `DiagnosticsPlugin` owns. `MinimalPlugins` omits it, and
        // the missing resource kills the run on the first step that moves an AABB.
        .add_plugins(bevy::diagnostic::DiagnosticsPlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(CoSimPlugin)
        // avian's collider cache reads `AssetEvent<Mesh>`; without the asset
        // registered, `clear_unused_colliders` fails parameter validation on the
        // first step and the run dies inside the compute pool.
        .init_asset::<Mesh>()
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        // One update advances well past a single fixed tick.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 30.0,
        )));
    if let Some(role) = role {
        app.insert_resource(role);
    }
    // avian registers its types, messages and diagnostics resources in
    // `finish`/`cleanup`, which a hand-driven `app.update()` loop never triggers
    // on its own. Without them the first step dies on parameter validation inside
    // the compute pool, which reads as a physics failure rather than a setup one.
    app.finish();
    app.cleanup();
    app
}

/// Spawn `source(value=7.0) --SimConnection--> target(value=0.0)` over the bare
/// [`Port`] backend, with `extra` components on the target (the network markers
/// under test). Returns the target entity.
fn wire_ports(app: &mut App, extra: impl Bundle) -> Entity {
    let source = app.world_mut().spawn(Port { value: 7.0 }).id();
    let target = app.world_mut().spawn((Port { value: 0.0 }, extra)).id();
    app.world_mut().spawn(SimConnection {
        start_element: source,
        start_connector: PORT_NAME.to_string(),
        end_element: target,
        end_connector: PORT_NAME.to_string(),
        scale: 1.0,
        offset: 0.0,
    });
    target
}

fn value_of(app: &App, e: Entity) -> f64 {
    app.world().get::<Port>(e).expect("target Port").value
}

/// **Failure mode 1, the owned case.** On a client, a connection whose target is
/// the body this peer owns and predicts (`OwnedLocally`) MUST propagate: that
/// body runs its own local physics + actuation, so its command path has to be
/// live. Under a process-wide `run_if(role != Client)` this asserts 0.0 and
/// fails — which is precisely the bug (the predicted rover stops driving).
#[test]
fn client_propagates_into_owned_locally_target() {
    let mut app = app_with_role(Some(lunco_core::NetworkRole::Client));
    // Replicated AND owned: the marker combination a possessed rover carries on
    // a client. `NetReplicate` alone would be skipped (see the test below), so
    // this isolates the `OwnedLocally` branch rather than the never-replicated one.
    let target = wire_ports(
        &mut app,
        (lunco_core::NetReplicate, lunco_core::OwnedLocally),
    );

    app.update();
    app.update();

    assert_eq!(
        value_of(&app, target),
        7.0,
        "a client must propagate into the body it owns and predicts — \
         otherwise the possessed rover's command never reaches its actuators"
    );
}

/// **Failure mode 1, the actual wheel-drive case.** `lunco-usd-sim`'s
/// `try_wire_wheel` targets bare `Port` entities (`p_drive` / `p_steer`), which
/// have no `RigidBody` and so never enter replication membership
/// (`apply_net_replication` requires one). They are local scaffolding and must
/// keep propagating on a client. Same expectation, different branch of the
/// predicate; also fails under a process-wide client gate.
#[test]
fn client_propagates_into_never_replicated_target() {
    let mut app = app_with_role(Some(lunco_core::NetworkRole::Client));
    let target = wire_ports(&mut app, ());

    app.update();
    app.update();

    assert_eq!(
        value_of(&app, target),
        7.0,
        "a purely local Port (a wheel's drive/steer node) is not replicated and \
         must keep propagating on a client"
    );
}

/// The behaviour the old global role gate actually existed for, now expressed
/// per target: a replicated body this peer neither owns nor predicts is a pure
/// snapshot proxy. Driving its ports locally would fight the snapshot stream, so
/// propagation must skip it.
#[test]
fn client_skips_replicated_only_target() {
    let mut app = app_with_role(Some(lunco_core::NetworkRole::Client));
    let target = wire_ports(&mut app, lunco_core::NetReplicate);

    app.update();
    app.update();

    assert_eq!(
        value_of(&app, target),
        0.0,
        "a replicated, non-predicted body is rendered from host snapshots — \
         cosim must not drive its ports"
    );
}

/// A client DOES simulate a freely-predicted body (`PredictedDynamic`, a prop it
/// bumped), so propagation into it must run too.
#[test]
fn client_propagates_into_predicted_dynamic_target() {
    let mut app = app_with_role(Some(lunco_core::NetworkRole::Client));
    let target = wire_ports(
        &mut app,
        (lunco_core::NetReplicate, lunco_core::PredictedDynamic),
    );

    app.update();
    app.update();

    assert_eq!(value_of(&app, target), 7.0);
}

/// Host and standalone are authoritative over every body, so no target is ever
/// skipped there — including a replicated one.
#[test]
fn host_propagates_into_replicated_target() {
    let mut app = app_with_role(Some(lunco_core::NetworkRole::Host));
    let target = wire_ports(&mut app, lunco_core::NetReplicate);

    app.update();
    app.update();

    assert_eq!(value_of(&app, target), 7.0);
}

/// **Failure mode 2.** Rollback replay re-runs the actuation chain for each
/// unacked input; propagation is part of that chain and must be registered in
/// `lunco_core::RollbackReplay`. Running that schedule alone (exactly how
/// `run_rollback_replay` drives it — no `FixedUpdate`, no `app.update()`) must
/// still carry the source value into the target. With propagation absent from
/// the schedule this asserts 0.0 and fails.
#[test]
fn rollback_replay_propagates() {
    let mut app = app_with_role(None);
    let target = wire_ports(&mut app, ());

    // NOT `app.update()` — replay runs this schedule and `PhysicsSchedule` only.
    app.world_mut().run_schedule(lunco_core::RollbackReplay);

    assert_eq!(
        value_of(&app, target),
        7.0,
        "replay must re-derive port values before the replayed actuators read \
         them, or the client's re-simulated forces differ from the host's"
    );
}

/// Replay's gating is the same per-target rule as the live tick: a client
/// replays only what it simulates, and a snapshot proxy is not that.
#[test]
fn rollback_replay_skips_replicated_only_target() {
    let mut app = app_with_role(Some(lunco_core::NetworkRole::Client));
    let target = wire_ports(&mut app, lunco_core::NetReplicate);

    app.world_mut().run_schedule(lunco_core::RollbackReplay);

    assert_eq!(value_of(&app, target), 0.0);
}
