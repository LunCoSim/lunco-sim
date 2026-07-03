//! The autopilot's behaviour is a `lunco-behavior` tree authored as DATA (rhai/JSON
//! → [`BehaviorSpec`]) and compiled to Rust-leaf nodes. These tests prove the
//! data-defined tree drives correctly and sequences waypoints — the mechanism a
//! `SetAutopilotBehavior` command hot-swaps at runtime.

use bevy::math::Vec3;
use lunco_autopilot::{nav_setpoint, AutopilotBehavior, DriveCtx};

#[test]
fn json_tree_drives_and_sequences_waypoints() {
    // The exact shape a rhai scenario emits as data.
    let json = r#"{"kind":"sequence","children":[
        {"kind":"drive_to","target":[10.0,0.0,0.0],"speed":0.6,"radius":2.0},
        {"kind":"drive_to","target":[10.0,0.0,10.0],"speed":0.6,"radius":2.0}
    ]}"#;
    let mut behavior = AutopilotBehavior::from_json(json).expect("spec must parse + build");

    // At the origin facing +X, far from waypoint 1 → drive forward, not braking.
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0) };
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

/// Drive a behaviour tree from `start` toward its goals for up to `max` ticks,
/// teleporting the "vessel" onto whatever point the tree steers hard toward once it
/// gets close — a crude kinematic stand-in that lets multi-waypoint trees advance.
/// Returns the number of ticks the tree spent braking (`out.2 > 0.5`).
fn brake_ticks(behavior: &mut AutopilotBehavior, waypoints: &[Vec3], max: usize) -> usize {
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0) };
    let mut braked = 0;
    let mut wp = 0;
    for _ in 0..max {
        behavior.0.tick(&mut ctx);
        ctx.now += 1.0; // advance mission time one second per tick (drives WaitNode)
        if ctx.out.2 > 0.5 {
            braked += 1;
        }
        // If a waypoint is set and we're driving toward it, snap there so the tree
        // registers arrival and moves on.
        if wp < waypoints.len() && ctx.out.0 > 0.0 {
            ctx.pos = waypoints[wp];
            wp += 1;
        }
    }
    braked
}

#[test]
fn patrol_loops_waypoints_and_dwells_at_each() {
    // A patrol with a dwell brakes at each waypoint, and — because it loops forever —
    // keeps dwelling across laps (the WaitNode timer resets each lap).
    let json = r#"{"kind":"patrol","dwell":2.0,"radius":2.0,"waypoints":[
        [5.0,0.0,0.0],[5.0,0.0,5.0]
    ]}"#;
    let mut behavior = AutopilotBehavior::from_json(json).expect("patrol spec must build");
    let wps = [Vec3::new(5.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 5.0)];
    // With dt=1.0 and dwell=2.0, each waypoint holds ~2 ticks. Over many ticks the
    // rover both drives (throttle) and repeatedly brakes — proving the loop runs.
    let braked = brake_ticks(&mut behavior, &wps, 12);
    assert!(braked >= 2, "patrol should dwell (brake) at waypoints; braked {braked} ticks");
}

#[test]
fn selector_with_arrived_guard_brakes_when_close_drives_when_far() {
    // "If arrived at goal, brake; otherwise drive to it" — a real fallback using the
    // arrived condition as the guard on the first branch.
    let json = r#"{"kind":"selector","children":[
        {"kind":"sequence","children":[
            {"kind":"arrived","target":[10.0,0.0,0.0],"radius":2.0},
            {"kind":"brake"}
        ]},
        {"kind":"drive_to","target":[10.0,0.0,0.0],"radius":2.0}
    ]}"#;
    let mut behavior = AutopilotBehavior::from_json(json).expect("selector spec must build");

    // Far away → guard fails → falls through to drive_to → throttle up, not braking.
    let mut far = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0) };
    behavior.0.tick(&mut far);
    assert!(far.out.0 > 0.0 && far.out.2 < 0.5, "far → drive, got {:?}", far.out);

    // At the goal → guard succeeds → brake branch taken.
    let mut near = DriveCtx { pos: Vec3::new(10.0, 0.0, 0.0), fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0) };
    behavior.0.tick(&mut near);
    assert!(near.out.2 > 0.5, "at goal → brake, got {:?}", near.out);
}

#[test]
fn wait_latches_a_mission_time_deadline_and_resets_across_repeats() {
    use lunco_behavior::Status;
    // A wait of 3s latches deadline = now(0) + 3 on the first tick; it holds (braking,
    // Running) until mission time reaches 3s, then Succeeds. The clock is `now`
    // (WorldTime.sim_secs), so a frozen clock freezes the wait.
    let mut behavior = AutopilotBehavior::from_json(r#"{"kind":"wait","seconds":3.0}"#).unwrap();
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0) };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    assert!(ctx.out.2 > 0.5, "wait holds the brakes");
    // Clock frozen (now unchanged) → still waiting: the deadline is mission-time, not
    // tick-count, so a paused sim never completes the wait.
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running, "frozen clock → still waiting");
    ctx.now = 2.9;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    ctx.now = 3.0;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "deadline reached → done");

    // repeat(times:2, wait 1s): each lap the wait clears its deadline, so it re-arms
    // against the current time and succeeds a second time.
    let mut rep = AutopilotBehavior::from_json(
        r#"{"kind":"repeat","times":2,"child":{"kind":"wait","seconds":1.0}}"#,
    )
    .unwrap();
    ctx.now = 0.0;
    assert_eq!(rep.0.tick(&mut ctx), Status::Running, "lap 1 armed at t=0 (deadline 1)");
    ctx.now = 1.0;
    assert_eq!(rep.0.tick(&mut ctx), Status::Running, "lap 1 done at t=1; child cleared");
    ctx.now = 2.0;
    assert_eq!(rep.0.tick(&mut ctx), Status::Running, "lap 2 re-arms at t=2 (deadline 3)");
    ctx.now = 3.0;
    assert_eq!(rep.0.tick(&mut ctx), Status::Success, "lap 2 done at t=3 → repeat succeeds");
}

#[test]
fn parallel_require_one_succeeds_when_first_child_arrives() {
    use lunco_behavior::Status;
    // Race a wait against an arrived-guard: at the goal, the guard succeeds
    // immediately, so require_one resolves without waiting out the timer.
    let json = r#"{"kind":"parallel","require":"one","children":[
        {"kind":"wait","seconds":99.0},
        {"kind":"arrived","target":[0.0,0.0,0.0],"radius":2.0}
    ]}"#;
    let mut behavior = AutopilotBehavior::from_json(json).expect("parallel spec must build");
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0) };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "arrived wins the race");
}
