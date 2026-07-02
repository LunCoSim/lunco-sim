//! Headless proof of the spec-034 control-authority mechanism: an autopilot is an
//! `AiAgent` session that possesses + drives a vessel, and stops the instant it
//! loses ownership (a takeover). No rendering, no avatar — just the session
//! substrate + `AutopilotPlugin`, exactly as a `--no-ui` server runs it.

use bevy::prelude::*;
use lunco_autopilot::{autopilot_session, Autopilot, AutopilotPlugin};
use lunco_core::session::{AuthorityRole, SessionRbac};
use lunco_core::{GlobalEntityId, NetworkRole, SessionId, SessionRegistry};
use lunco_cosim::SetPorts;

/// Records the target of every `SetPorts` the autopilot emits.
#[derive(Resource, Default)]
struct DriveLog(Vec<Entity>);

fn capture(t: On<SetPorts>, mut log: ResMut<DriveLog>) {
    log.0.push(t.event().target);
}

fn build() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        // Authoritative single-player peer (autopilot systems run on !Client).
        .insert_resource(NetworkRole::Standalone)
        .init_resource::<SessionRegistry>()
        .init_resource::<SessionRbac>()
        .init_resource::<DriveLog>()
        .add_observer(capture)
        .add_plugins(AutopilotPlugin);
    app
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
    assert!(reg.owns(ap_session, 0x11), "autopilot must own the vessel it engaged");

    // FixedUpdate → drive_autopilots emits one SetPorts for the owned vessel.
    app.world_mut().run_schedule(FixedUpdate);
    let log = app.world().resource::<DriveLog>();
    assert_eq!(log.0, vec![rover], "engaged autopilot drives the vessel it owns");
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
    assert_ne!(autopilot_session(0), autopilot_session(1), "distinct actors, distinct sessions");

    app.world_mut().run_schedule(FixedUpdate);
    let mut driven = app.world().resource::<DriveLog>().0.clone();
    driven.sort();
    let mut expected = vec![rover_a, rover_b];
    expected.sort();
    assert_eq!(driven, expected, "each autopilot drives its own vessel, no interference");
}
