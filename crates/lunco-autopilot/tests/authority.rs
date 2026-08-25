//! Headless proof of the spec-034 control-authority mechanism: an autopilot is an
//! `AiAgent` session that possesses + drives a vessel, and stops the instant it
//! loses ownership (a takeover). No rendering, no avatar — just the session
//! substrate + `AutopilotPlugin`, exactly as a `--no-ui` server runs it.

use bevy::math::DQuat;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_autopilot::{
    autopilot_session, drive_autopilots, setup_autopilot_session, Autopilot, AutopilotBehavior,
};
use lunco_core::coords::{GridRot, VehicleFrame};
use lunco_core::session::{AuthorityRole, SessionRbac};
use lunco_core::{GlobalEntityId, NetworkRole, PhysicsStatePending, SessionId, SessionRegistry};
use lunco_cosim::SetPorts;
use lunco_physics::PhysicsPoseSeeded;

/// Records the target of every `SetPorts` the autopilot emits.
#[derive(Resource, Default)]
struct DriveLog(Vec<Entity>);

fn capture(t: On<SetPorts>, mut log: ResMut<DriveLog>) {
    log.0.push(t.event().target);
}

/// Records the actual command payload so an authority test can prove the full
/// producer-to-consumer path, not just the steering helper in isolation.
#[derive(Resource, Default)]
struct SetpointLog(Vec<(Entity, Vec<(String, f64)>)>);

fn capture_setpoint(t: On<SetPorts>, mut log: ResMut<SetpointLog>) {
    log.0.push((t.event().target, t.event().writes.clone()));
}

fn build() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        // Authoritative single-player peer (autopilot systems run on !Client).
        .insert_resource(NetworkRole::Standalone)
        .init_resource::<SessionRegistry>()
        .init_resource::<SessionRbac>()
        // The one clock the driver reads (`WorldTime.sim_secs`). These constant-drive
        // tests don't dwell, so the default `0.0` is fine; a real app gets it from
        // `TimePlugin` (which `AutopilotPlugin` guard-adds).
        .init_resource::<lunco_time::WorldTime>()
        .init_resource::<DriveLog>()
        .init_resource::<SetpointLog>()
        .add_observer(capture)
        .add_observer(capture_setpoint)
        // Add the autopilot systems directly (not the full plugin) so this stays a
        // minimal headless harness independent of the command-registration infra.
        .add_systems(Update, setup_autopilot_session)
        .add_systems(FixedUpdate, drive_autopilots);
    let frame = app
        .world_mut()
        .spawn((
            lunco_core::WorldGridConfig::default().grid(),
            CellCoord::ZERO,
            Transform::default(),
        ))
        .id();
    app.insert_resource(lunco_core::ActivePhysicsFrame(frame));
    app
}

#[test]
fn behavior_navigation_uses_physics_rotation_not_render_transform() {
    let mut app = build();
    let physics_rotation = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
    let frame = app.world().resource::<lunco_core::ActivePhysicsFrame>().0;
    let rover = app
        .world_mut()
        .spawn((
            GlobalEntityId::from_raw(0x33),
            // Deliberately leave the render frame facing -Z while the
            // authoritative rigid body faces -X. A GlobalTransform-based
            // implementation would steer toward the wrong frame here.
            ChildOf(frame),
            Transform::default(),
            GlobalTransform::IDENTITY,
            avian3d::prelude::RigidBody::Dynamic,
            avian3d::prelude::Position(bevy::math::DVec3::ZERO),
            avian3d::prelude::Rotation(physics_rotation),
            PhysicsPoseSeeded,
        ))
        .id();
    let behavior = AutopilotBehavior::from_json(
        r#"{"kind":"drive_to","target":[-20.0,0.0,0.0],"speed":0.6,"radius":2.0}"#,
    )
    .expect("drive_to behavior JSON is valid");
    app.world_mut()
        .spawn((Autopilot::holding(rover, 0), behavior));

    app.update(); // register and claim the vessel
    app.world_mut().run_schedule(FixedUpdate); // run the real driver

    let log = app.world().resource::<SetpointLog>();
    let (_, writes) = log.0.last().expect("autopilot emitted SetPorts");
    let value = |name: &str| {
        writes
            .iter()
            .find_map(|(port, value)| (port == name).then_some(*value))
            .unwrap_or_else(|| panic!("SetPorts omitted {name:?}: {writes:?}"))
    };
    let expected_forward = VehicleFrame::yaw_forward(GridRot(physics_rotation));

    assert!(
        (expected_forward - bevy::math::DVec3::NEG_X).length() < 1e-9,
        "unexpected canonical physics forward: {expected_forward:?}"
    );
    assert!(
        value("throttle") > 0.0,
        "physics-facing waypoint was not driven"
    );
    assert!(
        value("steer").abs() < 1e-9,
        "waypoint on the physics forward axis was steered: writes={writes:?}"
    );
}

#[test]
fn behavior_navigation_uses_physics_position_not_render_hierarchy() {
    let mut app = build();
    let frame = app
        .world_mut()
        .spawn((
            lunco_core::WorldGridConfig::default().grid(),
            CellCoord::ZERO,
            Transform::default(),
        ))
        .id();
    app.insert_resource(lunco_core::ActivePhysicsFrame(frame));

    let rover = app
        .world_mut()
        .spawn((
            GlobalEntityId::from_raw(0x34),
            ChildOf(frame),
            // Presentation is deliberately at the origin; physics is 100 m
            // east. A hierarchy-position reader would compute a false right
            // bearing for the same forward waypoint.
            Transform::default(),
            GlobalTransform::IDENTITY,
            avian3d::prelude::RigidBody::Dynamic,
            avian3d::prelude::Position(bevy::math::DVec3::new(100.0, 0.0, 0.0)),
            avian3d::prelude::Rotation(DQuat::IDENTITY),
            PhysicsPoseSeeded,
        ))
        .id();
    let behavior = AutopilotBehavior::from_json(
        r#"{"kind":"drive_to","target":[100.0,0.0,-100.0],"speed":0.6,"radius":2.0}"#,
    )
    .expect("drive_to behavior JSON is valid");
    app.world_mut()
        .spawn((Autopilot::holding(rover, 0), behavior));

    app.update();
    app.world_mut().run_schedule(FixedUpdate);

    let log = app.world().resource::<SetpointLog>();
    let (_, writes) = log.0.last().expect("autopilot emitted SetPorts");
    let steer = writes
        .iter()
        .find_map(|(port, value)| (port == "steer").then_some(*value))
        .expect("SetPorts omitted steer");
    assert!(
        steer.abs() < 1.0e-9,
        "physics position was not used; render-frame offset changed bearing: {writes:?}"
    );
}

/// Spawn a vessel carrying a `GlobalEntityId` (the ownership key).
fn spawn_vessel(app: &mut App, gid: u64) -> Entity {
    app.world_mut().spawn(GlobalEntityId::from_raw(gid)).id()
}

#[test]
fn autopilot_engages_registers_and_drives_only_what_it_owns() {
    let mut app = build();
    let rover = spawn_vessel(&mut app, 0x11);

    let ap_session = autopilot_session(0);
    app.world_mut().spawn(Autopilot::forward(rover, 0, 0.8));

    // Update → setup_autopilot_session registers the AiAgent session + claims.
    app.update();

    let rbac = app.world().resource::<SessionRbac>();
    assert_eq!(
        rbac.sessions.get(&ap_session.0).map(|s| s.role),
        Some(AuthorityRole::AiAgent),
        "autopilot must register as an AiAgent session"
    );
    let reg = app.world().resource::<SessionRegistry>();
    assert!(
        reg.owns(ap_session, 0x11),
        "autopilot must own the vessel it engaged"
    );

    // FixedUpdate → drive_autopilots emits one SetPorts for the owned vessel.
    app.world_mut().run_schedule(FixedUpdate);
    let log = app.world().resource::<DriveLog>();
    assert_eq!(
        log.0,
        vec![rover],
        "engaged autopilot drives the vessel it owns"
    );
}

#[test]
fn autopilot_stops_the_moment_it_loses_ownership() {
    let mut app = build();
    let rover = spawn_vessel(&mut app, 0x22);
    let ap_session = autopilot_session(0);
    app.world_mut().spawn(Autopilot::forward(rover, 0, 0.8));

    app.update(); // engage + claim
    app.world_mut().run_schedule(FixedUpdate); // drives once
    assert_eq!(app.world().resource::<DriveLog>().0.len(), 1);

    // A human (LocalSession) takes the vessel — the takeover releases the autopilot
    // and claims for the human. Here we simulate the resulting ownership transfer.
    let human = SessionId::LOCAL;
    {
        let mut reg = app.world_mut().resource_mut::<SessionRegistry>();
        reg.release_session(ap_session);
        reg.claim(human, 0x22).unwrap();
    }

    // Next tick: the autopilot no longer owns → it must NOT write (single writer).
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(
        app.world().resource::<DriveLog>().0.len(),
        1,
        "autopilot must stop driving the instant it loses ownership (no jitter)"
    );
    assert!(
        app.world().resource::<SessionRegistry>().owns(human, 0x22),
        "the human now owns the vessel"
    );
}

#[test]
fn autopilot_does_not_write_during_physics_admission() {
    let mut app = build();
    let rover = app
        .world_mut()
        .spawn((GlobalEntityId::from_raw(0x23), PhysicsStatePending))
        .id();
    app.world_mut().spawn(Autopilot::forward(rover, 0, 0.8));

    app.update();
    app.world_mut().run_schedule(FixedUpdate);
    assert!(
        app.world().resource::<DriveLog>().0.is_empty(),
        "autopilot must wait for the authoritative physics admission boundary"
    );

    app.world_mut()
        .entity_mut(rover)
        .remove::<PhysicsStatePending>();
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(
        app.world().resource::<DriveLog>().0,
        vec![rover],
        "the first control write belongs to the first admitted fixed tick"
    );
}

#[test]
fn disengaged_actor_cannot_emit_a_final_same_tick_command() {
    let mut app = build();
    let rover = spawn_vessel(&mut app, 0x23);
    let actor = app
        .world_mut()
        .spawn(Autopilot::forward(rover, 0, 0.8))
        .id();

    app.update(); // register and claim before the lifecycle boundary
    app.world_mut()
        .get_mut::<Autopilot>(actor)
        .expect("actor remains available until deferred cleanup")
        .disengage();

    // This models a DisengageAutopilot observer running before the producer in
    // the same FixedUpdate. The actor is still an entity, but it must be inert.
    app.world_mut().run_schedule(FixedUpdate);
    assert!(
        app.world().resource::<DriveLog>().0.is_empty(),
        "a synchronously disengaged actor must not write SetPorts before its deferred despawn"
    );
}

#[test]
fn multi_actor_two_autopilots_own_distinct_vessels() {
    let mut app = build();
    let rover_a = spawn_vessel(&mut app, 0xA1);
    let rover_b = spawn_vessel(&mut app, 0xB2);
    app.world_mut().spawn(Autopilot::forward(rover_a, 0, 0.5));
    app.world_mut().spawn(Autopilot::forward(rover_b, 1, 0.5));

    app.update();
    let reg = app.world().resource::<SessionRegistry>();
    assert!(reg.owns(autopilot_session(0), 0xA1));
    assert!(reg.owns(autopilot_session(1), 0xB2));
    assert_ne!(
        autopilot_session(0),
        autopilot_session(1),
        "distinct actors, distinct sessions"
    );

    app.world_mut().run_schedule(FixedUpdate);
    let mut driven = app.world().resource::<DriveLog>().0.clone();
    driven.sort();
    let mut expected = vec![rover_a, rover_b];
    expected.sort();
    assert_eq!(
        driven, expected,
        "each autopilot drives its own vessel, no interference"
    );
}
