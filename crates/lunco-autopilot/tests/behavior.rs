//! The autopilot's behaviour is a `lunco-behavior` tree authored as DATA (rhai/JSON
//! → [`BehaviorSpec`]) and compiled to Rust-leaf nodes. These tests prove the
//! data-defined tree drives correctly and sequences waypoints — the mechanism a
//! `SetAutopilotBehavior` command hot-swaps at runtime.

use bevy::math::Vec3;
use lunco_autopilot::{nav_setpoint, AutopilotBehavior, DriveCtx};
use lunco_behavior::Node;

#[test]
fn json_tree_drives_and_sequences_waypoints() {
    // The exact shape a rhai scenario emits as data.
    let json = r#"{"kind":"sequence","children":[
        {"kind":"drive_to","target":[10.0,0.0,0.0],"speed":0.6,"radius":2.0},
        {"kind":"drive_to","target":[10.0,0.0,10.0],"speed":0.6,"radius":2.0}
    ]}"#;
    let mut behavior = AutopilotBehavior::from_json(json).expect("spec must parse + build");

    // At the origin facing +X, far from waypoint 1 → drive forward, not braking.
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, out: (0.0, 0.0, 0.0) };
    behavior.0.tick(&mut ctx);
    assert!(ctx.out.0 > 0.0, "should drive toward wp1, got {:?}", ctx.out);
    assert_eq!(ctx.out.2, 0.0, "not braking while en route");

    // Arrive at waypoint 1: the sequence advances to wp2 within the SAME tick and
    // starts driving toward it, so the setpoint is still "driving" (not a full stop),
    // and now steers to turn toward +Z.
    ctx.pos = Vec3::new(10.0, 0.0, 0.0);
    behavior.0.tick(&mut ctx);
    assert!(ctx.out.0 > 0.0, "after wp1, drives toward wp2, got {:?}", ctx.out);
    assert!(ctx.out.1.abs() > 0.0, "should steer toward wp2 (+Z), got {:?}", ctx.out);
}

#[test]
fn nav_setpoint_brakes_within_radius_drives_when_far() {
    let (_t, _s, brake, arrived) = nav_setpoint(Vec3::ZERO, Vec3::X, Vec3::new(0.5, 0.0, 0.0), 0.6, 2.0);
    assert!(arrived && brake > 0.5, "within radius → arrived + brake");

    let (throttle, _s, _b, arrived) = nav_setpoint(Vec3::ZERO, Vec3::X, Vec3::new(50.0, 0.0, 0.0), 0.6, 2.0);
    assert!(!arrived && throttle > 0.0, "far + aligned → driving forward");
}

#[test]
fn bad_spec_is_a_clean_error() {
    assert!(AutopilotBehavior::from_json("{not json").is_err());
    assert!(AutopilotBehavior::from_json(r#"{"kind":"nope"}"#).is_err());
}
