//! TDD for [`RunToolNode`] — the one-shot leaf that queues a [`ToolInvocation`]
//! when a behaviour tree's `run_tool` node fires. The load-bearing property is
//! the **latch**: it fires exactly once per activation, and `reset()` re-arms
//! it (so a patrol loop re-photos each lap). A regression to "fires every tick"
//! would spam `ToolFired` events.

use bevy::math::Vec3;
use lunco_behavior::{Node, Status};
use lunco_autopilot::{
    AutopilotBehavior, BehaviorSpec, DriveCtx, RunToolNode, ToolInvocation,
};

fn ctx() -> DriveCtx {
    DriveCtx {
        self_gid: 0,
        pos: Vec3::ZERO,
        fwd: Vec3::X,
        now: 0.0,
        out: (0.0, 0.0, 0.0),
        targets: Default::default(),
        clearance: Default::default(),
        fired: Vec::new(),
    }
}

#[test]
fn fires_exactly_once_then_latches() {
    let mut node = RunToolNode::new("science::take_photo".into(), "{}".into());
    // Tick 1: fires (queues the invocation) + returns Success.
    let mut c = ctx();
    assert_eq!(node.tick(&mut c), Status::Success);
    assert_eq!(c.fired.len(), 1);
    assert_eq!(c.fired[0], ToolInvocation { tool: "science::take_photo".into(), args: "{}".into() });

    // Tick 2 + 3 on the same activation: latch holds — no re-fire.
    let mut c2 = ctx();
    assert_eq!(node.tick(&mut c2), Status::Success);
    assert!(c2.fired.is_empty(), "latch must suppress re-fire on same activation");
    let mut c3 = ctx();
    node.tick(&mut c3);
    assert!(c3.fired.is_empty());
}

#[test]
fn reset_re_arms_for_next_activation() {
    let mut node = RunToolNode::new("ping".into(), String::new());
    // First activation: fires once.
    let mut c = ctx();
    node.tick(&mut c);
    assert_eq!(c.fired.len(), 1);
    // Reset (as a `Repeat::forever` / `Cooldown` decorator drives each lap).
    node.reset();
    // Second activation: fires again — reset re-armed the latch.
    let mut c2 = ctx();
    node.tick(&mut c2);
    assert_eq!(c2.fired.len(), 1, "reset must re-arm so it fires on the next activation");
    // Third tick on the second activation: latched again.
    let mut c3 = ctx();
    node.tick(&mut c3);
    assert!(c3.fired.is_empty());
}

#[test]
fn holds_position_while_firing() {
    // A tool call is not a drive command — the leaf brakes while/after firing.
    let mut node = RunToolNode::new("x".into(), String::new());
    let mut c = ctx();
    c.out = (0.5, 0.1, 0.0); // pretend we were driving
    node.tick(&mut c);
    assert_eq!(c.out, (0.0, 0.0, 1.0), "RunToolNode must brake (out = hold) while firing");
}

#[test]
fn patrol_on_arrival_action_fires_when_vessel_reaches_waypoint() {
    // T3: a patrol waypoint's `on_arrival` action compiles into the tree and
    // actually fires when the vessel arrives. This is the declarative "fire a
    // tool at a patrol waypoint" path — the reason `on_arrival` exists.
    use lunco_autopilot::{PatrolWaypoint, WaypointAction};
    let spec = BehaviorSpec::Patrol {
        waypoints: vec![PatrolWaypoint {
            pos: [0.0, 0.0, 0.0], // vessel starts here → drive_to succeeds immediately
            dwell: None,
            on_arrival: vec![WaypointAction::RunTool {
                tool: "science::take_photo".into(),
                args: String::new(),
            }],
        }],
        speed: 0.5,
        radius: 2.0,
        dwell: 0.0,
    };
    let mut tree = AutopilotBehavior::new(&spec);
    // Vessel is AT the waypoint (within radius 2.0), so the drive_to leaf
    // succeeds on the first tick → the on_arrival run_tool fires.
    let mut c = ctx();
    c.pos = Vec3::ZERO;
    tree.0.tick(&mut c);
    assert_eq!(
        c.fired.len(),
        1,
        "on_arrival action must fire when the vessel reaches the waypoint"
    );
    assert_eq!(c.fired[0].tool, "science::take_photo");
}

#[test]
fn patrol_on_arrival_does_not_fire_before_arrival() {
    // Negative case: vessel is far from the waypoint → drive_to is still
    // Running → the on_arrival action must NOT have fired yet.
    use lunco_autopilot::{PatrolWaypoint, WaypointAction};
    let spec = BehaviorSpec::Patrol {
        waypoints: vec![PatrolWaypoint {
            pos: [100.0, 0.0, 0.0], // far away
            dwell: None,
            on_arrival: vec![WaypointAction::RunTool {
                tool: "science::take_photo".into(),
                args: String::new(),
            }],
        }],
        speed: 0.5,
        radius: 2.0,
        dwell: 0.0,
    };
    let mut tree = AutopilotBehavior::new(&spec);
    let mut c = ctx();
    c.pos = Vec3::ZERO; // vessel at origin, waypoint at x=100 → not arrived
    tree.0.tick(&mut c);
    assert!(c.fired.is_empty(), "on_arrival must NOT fire before the vessel arrives");
}
