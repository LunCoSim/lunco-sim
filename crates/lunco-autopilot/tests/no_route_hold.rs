//! An engaged autopilot with **no waypoints holds** — it never creeps forward.
//!
//! The failure this pins: pressing Engage (Command Deck, Toggle Autopilot, or a bare
//! `cmd("EngageAutopilot", #{ vessel })`) on a rover that has no route drove it away
//! in a straight line at a fabricated 0.5 throttle. From the outside that is
//! indistinguishable from a broken autopilot — the rover ignores the waypoints it
//! does not have and leaves the site.
//!
//! Two ways to have no route, both must hold:
//!   * no behaviour tree at all (this is the `Autopilot::holding` shape), and
//!   * a tree whose route is empty / fully consumed, which writes no setpoint.
//!
//! Holding means *brake*, not merely zero throttle: a rover parked on a lunar slope
//! with the drive released rolls.

use bevy::prelude::*;
use lunco_autopilot::{drive_autopilots, setup_autopilot_session, Autopilot, AutopilotBehavior};
use lunco_core::session::SessionRbac;
use lunco_core::{GlobalEntityId, NetworkRole, SessionRegistry};
use lunco_cosim::SetPorts;

/// Records every port write the autopilot emits.
#[derive(Resource, Default)]
struct Writes(Vec<(String, f64)>);

fn capture(t: On<SetPorts>, mut log: ResMut<Writes>) {
    log.0.extend(t.event().writes.iter().cloned());
}

fn build() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(NetworkRole::Standalone)
        .init_resource::<SessionRegistry>()
        .init_resource::<SessionRbac>()
        .init_resource::<lunco_time::WorldTime>()
        .init_resource::<Writes>()
        .add_observer(capture)
        .add_systems(Update, setup_autopilot_session)
        .add_systems(FixedUpdate, drive_autopilots);
    app
}

fn port(app: &App, name: &str) -> f64 {
    app.world()
        .resource::<Writes>()
        .0
        .iter()
        .rev()
        .find(|(k, _)| k == name)
        .unwrap_or_else(|| panic!("autopilot never wrote `{name}`"))
        .1
}

#[test]
fn an_autopilot_with_no_behaviour_tree_holds_instead_of_driving_forward() {
    let mut app = build();
    let rover = app
        .world_mut()
        .spawn((GlobalEntityId::from_raw(0x51), Transform::default(), GlobalTransform::default()))
        .id();
    app.world_mut().spawn(Autopilot::holding(rover, 0));

    app.update(); // claim the vessel
    app.world_mut().run_schedule(FixedUpdate); // drive

    assert_eq!(port(&app, "throttle"), 0.0, "a routeless autopilot must not drive forward");
    assert_eq!(port(&app, "brake"), 1.0, "holding means BRAKE — zero throttle still rolls downhill");
}

#[test]
fn an_autopilot_whose_route_is_empty_holds_too() {
    let mut app = build();
    let rover = app
        .world_mut()
        .spawn((GlobalEntityId::from_raw(0x52), Transform::default(), GlobalTransform::default()))
        .id();
    // A tree that names no waypoints — the shape a fully-consumed patrol compiles to.
    let behavior = AutopilotBehavior::from_json(r#"{"kind":"sequence","children":[]}"#)
        .expect("an empty sequence is a valid tree");
    app.world_mut().spawn((Autopilot::holding(rover, 0), behavior));

    app.update();
    app.world_mut().run_schedule(FixedUpdate);

    assert_eq!(port(&app, "throttle"), 0.0, "an empty route must not drive forward");
    assert_eq!(port(&app, "brake"), 1.0, "an empty route holds the vessel");
}

/// Cruise is still available — it is just *opt-in*, never a default. This is the
/// distinction the fix rests on: the setpoint has to be asked for by name.
#[test]
fn an_explicitly_requested_constant_cruise_still_drives() {
    let mut app = build();
    let rover = app
        .world_mut()
        .spawn((GlobalEntityId::from_raw(0x53), Transform::default(), GlobalTransform::default()))
        .id();
    app.world_mut().spawn(Autopilot::forward(rover, 0, 0.8));

    app.update();
    app.world_mut().run_schedule(FixedUpdate);

    assert_eq!(port(&app, "throttle"), 0.8, "a named throttle is honoured");
    assert_eq!(port(&app, "brake"), 0.0, "cruising is not braking");
}
