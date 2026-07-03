//! The autopilot's behaviour is a `lunco-behavior` tree authored as DATA (rhai/JSON
//! → [`BehaviorSpec`]) and compiled to Rust-leaf nodes. These tests prove the
//! data-defined tree drives correctly and sequences waypoints — the mechanism a
//! `SetAutopilotBehavior` command hot-swaps at runtime.

use bevy::math::Vec3;
use lunco_autopilot::{nav_setpoint, AutopilotBehavior, Clearance, DriveCtx, TargetState, TargetStates};
use std::sync::Arc;

#[test]
fn json_tree_drives_and_sequences_waypoints() {
    // The exact shape a rhai scenario emits as data.
    let json = r#"{"kind":"sequence","children":[
        {"kind":"drive_to","target":[10.0,0.0,0.0],"speed":0.6,"radius":2.0},
        {"kind":"drive_to","target":[10.0,0.0,10.0],"speed":0.6,"radius":2.0}
    ]}"#;
    let mut behavior = AutopilotBehavior::from_json(json).expect("spec must parse + build");

    // At the origin facing +X, far from waypoint 1 → drive forward, not braking.
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
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
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
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
    let mut far = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    behavior.0.tick(&mut far);
    assert!(far.out.0 > 0.0 && far.out.2 < 0.5, "far → drive, got {:?}", far.out);

    // At the goal → guard succeeds → brake branch taken.
    let mut near = DriveCtx { pos: Vec3::new(10.0, 0.0, 0.0), fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
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
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
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
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "arrived wins the race");
}

#[test]
fn face_pivots_in_place_then_succeeds_when_aligned() {
    use lunco_behavior::Status;
    // Face a point off to +Z while pointing +X: steer with NO throttle (pivot),
    // Running until aligned; when the heading swings onto the target, Success.
    let mut behavior =
        AutopilotBehavior::from_json(r#"{"kind":"face","target":[0.0,0.0,5.0],"tolerance":8.0}"#).unwrap();
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    assert_eq!(ctx.out.0, 0.0, "face uses no throttle (pivot in place)");
    assert!(ctx.out.1.abs() > 0.0, "face steers toward the target, got {:?}", ctx.out);

    // Now pointing at the target → within tolerance → Success, steering released.
    ctx.fwd = Vec3::Z;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success);
    assert_eq!(ctx.out, (0.0, 0.0, 0.0), "aligned → release");
}

#[test]
fn invert_negates_the_arrived_condition() {
    use lunco_behavior::Status;
    let mut inv =
        AutopilotBehavior::from_json(r#"{"kind":"invert","child":{"kind":"arrived","target":[0.0,0.0,0.0],"radius":2.0}}"#)
            .unwrap();
    // Far from the target → arrived Failure → invert Success ("not arrived").
    let mut far = DriveCtx { pos: Vec3::new(50.0, 0.0, 0.0), fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(inv.0.tick(&mut far), Status::Success);
    // At the target → arrived Success → invert Failure.
    let mut near = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(inv.0.tick(&mut near), Status::Failure);
}

#[test]
fn force_and_retry_decorators_map_and_re_attempt() {
    use lunco_behavior::Status;
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    // force_success swallows a failing child; force_failure overrides a success.
    let mut fs = AutopilotBehavior::from_json(r#"{"kind":"force_success","child":{"kind":"fail"}}"#).unwrap();
    assert_eq!(fs.0.tick(&mut ctx), Status::Success);
    let mut ff = AutopilotBehavior::from_json(r#"{"kind":"force_failure","child":{"kind":"succeed"}}"#).unwrap();
    assert_eq!(ff.0.tick(&mut ctx), Status::Failure);
    // retry(times:2, fail): 1st failure retries (Running), 2nd exhausts → Failure.
    let mut rt = AutopilotBehavior::from_json(r#"{"kind":"retry","times":2,"child":{"kind":"fail"}}"#).unwrap();
    assert_eq!(rt.0.tick(&mut ctx), Status::Running);
    assert_eq!(rt.0.tick(&mut ctx), Status::Failure);
}

#[test]
fn reactive_selector_switches_to_brake_the_instant_it_arrives() {
    use lunco_behavior::Status;
    // Priority: "if at the goal, brake; else drive to it" — re-checked every tick.
    let json = r#"{"kind":"reactive_selector","children":[
        {"kind":"sequence","children":[
            {"kind":"arrived","target":[0.0,0.0,0.0],"radius":2.0},
            {"kind":"brake"}
        ]},
        {"kind":"drive_to","target":[100.0,0.0,0.0],"radius":2.0}
    ]}"#;
    let mut behavior = AutopilotBehavior::from_json(json).unwrap();
    // Away from the goal → arrived fails → falls through to drive_to → throttle up.
    let mut ctx = DriveCtx { pos: Vec3::new(50.0, 0.0, 0.0), fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    behavior.0.tick(&mut ctx);
    assert!(ctx.out.0 > 0.0 && ctx.out.2 < 0.5, "far → drive, got {:?}", ctx.out);
    // Teleport onto the goal → the higher-priority branch preempts → brake.
    ctx.pos = Vec3::ZERO;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success);
    assert!(ctx.out.2 > 0.5, "at goal → brake branch, got {:?}", ctx.out);
}

#[test]
fn timeout_aborts_a_running_child_on_the_mission_clock() {
    use lunco_behavior::Status;
    // A cruise never ends on its own; the timeout fails it after 2 mission-seconds
    // and brakes. The budget is mission time, so a frozen clock never trips it.
    let mut behavior = AutopilotBehavior::from_json(
        r#"{"kind":"timeout","seconds":2.0,"child":{"kind":"cruise","throttle":0.5}}"#,
    )
    .unwrap();
    let mut ctx = DriveCtx { pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, self_gid: 0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running, "armed at t=0 (deadline 2)");
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running, "frozen clock → still running");
    ctx.now = 1.9;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    ctx.now = 2.0;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure, "budget spent → abort");
    assert!(ctx.out.2 > 0.5, "timeout brakes, got {:?}", ctx.out);
}

#[test]
fn follow_tracks_a_live_target_and_fails_when_it_vanishes() {
    use lunco_behavior::Status;
    // Follow entity gid 42; the target's live pose is resolved from ctx.targets.
    let mut behavior =
        AutopilotBehavior::from_json(r#"{"kind":"follow","target":42,"speed":0.6,"radius":3.0}"#).unwrap();

    // Target present and far ahead → drive toward it, and keep Running (following
    // never "arrives"/finishes the way drive_to does).
    let mut targets = TargetStates::new();
    targets.insert(42, TargetState { pos: Vec3::new(50.0, 0.0, 0.0), vel: Vec3::ZERO });
    let mut ctx = DriveCtx {
        pos: Vec3::ZERO,
        fwd: Vec3::X,
        now: 0.0,
        self_gid: 0,
        out: (0.0, 0.0, 0.0),
        targets: Arc::new(targets),
        clearance: Default::default(),
    };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running, "keeps following, never finishes");
    assert!(ctx.out.0 > 0.0, "drives toward the live target, got {:?}", ctx.out);

    // Target moves right next to us → hold station (brake), still Running.
    let mut near = TargetStates::new();
    near.insert(42, TargetState { pos: Vec3::new(1.0, 0.0, 0.0), vel: Vec3::ZERO });
    ctx.targets = Arc::new(near);
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    assert!(ctx.out.2 > 0.5, "holds station (brakes) within radius, got {:?}", ctx.out);

    // Target vanishes from the snapshot → brake + Failure so a fallback can take over.
    ctx.targets = Arc::new(TargetStates::new());
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure, "lost target → fail");
    assert!(ctx.out.2 > 0.5, "brakes when the target is lost, got {:?}", ctx.out);
}

#[test]
fn intercept_leads_a_moving_target_and_succeeds_on_contact() {
    use lunco_behavior::Status;
    // Lead 2 s. Target at +X=20 moving in +Z at 5 u/s → aim point leads into +Z.
    let mut behavior = AutopilotBehavior::from_json(
        r#"{"kind":"intercept","target":7,"speed":0.7,"radius":3.0,"lead":2.0}"#,
    )
    .unwrap();
    let mut targets = TargetStates::new();
    targets.insert(7, TargetState { pos: Vec3::new(20.0, 0.0, 0.0), vel: Vec3::new(0.0, 0.0, 5.0) });
    let mut ctx = DriveCtx {
        pos: Vec3::ZERO,
        fwd: Vec3::X,
        now: 0.0,
        self_gid: 0,
        out: (0.0, 0.0, 0.0),
        targets: Arc::new(targets),
        clearance: Default::default(),
    };
    // Not yet in contact → Running, driving. The lead point is (20, 0, 10), i.e. off
    // to +Z of the target, so we steer toward the future position, not the tail.
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    assert!(ctx.out.0 > 0.0, "drives toward the lead point, got {:?}", ctx.out);
    // Steering leads: with the target dead ahead (+X) but its lead point off to +Z,
    // the command must steer (nonzero), not drive straight.
    assert!(ctx.out.1.abs() > 0.0, "leads the target (steers off the tail), got {:?}", ctx.out);

    // Reach the target's actual position → Success (a catch-it pursuit finishes).
    ctx.pos = Vec3::new(20.0, 0.0, 0.0);
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "contact → intercepted");

    // Target gone → Failure + brake.
    ctx.targets = Arc::new(TargetStates::new());
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure);
    assert!(ctx.out.2 > 0.5, "brakes when the target is lost, got {:?}", ctx.out);
}

#[test]
fn obstacle_ahead_senses_a_vessel_in_the_forward_cone_and_excludes_self() {
    use lunco_behavior::Status;
    let mut behavior =
        AutopilotBehavior::from_json(r#"{"kind":"obstacle_ahead","distance":6.0,"cone":60.0}"#).unwrap();
    // Facing +X. Another vessel (gid 2) 4 m dead ahead → obstacle. Our own vessel
    // (gid 1) is in the snapshot at our position but must be excluded.
    let mut targets = TargetStates::new();
    targets.insert(1, TargetState { pos: Vec3::ZERO, vel: Vec3::ZERO }); // self
    targets.insert(2, TargetState { pos: Vec3::new(4.0, 0.0, 0.0), vel: Vec3::ZERO });
    let mut ctx = DriveCtx {
        self_gid: 1,
        pos: Vec3::ZERO,
        fwd: Vec3::X,
        now: 0.0,
        out: (0.0, 0.0, 0.0),
        targets: Arc::new(targets),
        clearance: Default::default(),
    };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "vessel ahead → obstacle");

    // Same vessel but behind us (−X) → outside the forward cone → clear.
    let mut behind = TargetStates::new();
    behind.insert(1, TargetState { pos: Vec3::ZERO, vel: Vec3::ZERO });
    behind.insert(2, TargetState { pos: Vec3::new(-4.0, 0.0, 0.0), vel: Vec3::ZERO });
    ctx.targets = Arc::new(behind);
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure, "behind → not ahead");

    // Only ourselves present → never an obstacle.
    let mut just_me = TargetStates::new();
    just_me.insert(1, TargetState { pos: Vec3::ZERO, vel: Vec3::ZERO });
    ctx.targets = Arc::new(just_me);
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure, "self is excluded");
}

#[test]
fn facing_guards_heading_and_hold_stays_running() {
    use lunco_behavior::Status;
    // facing? Success when pointed at the target within tolerance, else Failure.
    let mut facing =
        AutopilotBehavior::from_json(r#"{"kind":"facing","target":[10.0,0.0,0.0],"tolerance":8.0}"#).unwrap();
    let mut aligned = DriveCtx { self_gid: 0, pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(facing.0.tick(&mut aligned), Status::Success);
    let mut off = DriveCtx { self_gid: 0, pos: Vec3::ZERO, fwd: Vec3::Z, now: 0.0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(facing.0.tick(&mut off), Status::Failure);

    // hold: brakes and never finishes (stays Running).
    let mut hold = AutopilotBehavior::from_json(r#"{"kind":"hold"}"#).unwrap();
    assert_eq!(hold.0.tick(&mut aligned), Status::Running);
    assert!(aligned.out.2 > 0.5, "hold brakes, got {:?}", aligned.out);
}

#[test]
fn cooldown_blocks_re_entry_for_the_lockout_window() {
    use lunco_behavior::Status;
    // cooldown(2s, succeed): fires, then is blocked (Failure) until 2 mission-seconds
    // pass, then fires again.
    let mut behavior =
        AutopilotBehavior::from_json(r#"{"kind":"cooldown","seconds":2.0,"child":{"kind":"succeed"}}"#).unwrap();
    let mut ctx = DriveCtx { self_gid: 0, pos: Vec3::ZERO, fwd: Vec3::X, now: 0.0, out: (0.0, 0.0, 0.0), targets: Default::default(), clearance: Default::default() };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "first fire allowed");
    ctx.now = 1.0;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure, "within lockout → blocked");
    ctx.now = 2.0;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "lockout elapsed → fires again");
}

#[test]
fn path_blocked_reads_the_forward_raycast_clearance() {
    use lunco_behavior::Status;
    let mut behavior = AutopilotBehavior::from_json(r#"{"kind":"path_blocked","distance":5.0}"#).unwrap();
    let mut ctx = DriveCtx {
        self_gid: 0,
        pos: Vec3::ZERO,
        fwd: Vec3::X,
        now: 0.0,
        out: (0.0, 0.0, 0.0),
        targets: Default::default(),
        clearance: Clearance { ahead: Some(3.0), left: None, right: None, range: 20.0 },
    };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Success, "hit at 3 m < 5 m → blocked");
    // Hit beyond the threshold → clear.
    ctx.clearance.ahead = Some(9.0);
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure, "hit at 9 m > 5 m → clear");
    // No hit at all → clear.
    ctx.clearance.ahead = None;
    assert_eq!(behavior.0.tick(&mut ctx), Status::Failure, "no hit → clear");
}

#[test]
fn steer_clear_goes_straight_when_open_and_turns_toward_the_open_side() {
    use lunco_behavior::Status;
    let mut behavior = AutopilotBehavior::from_json(r#"{"kind":"steer_clear","speed":0.6}"#).unwrap();
    // Wide open ahead → drive straight, no steer.
    let mut ctx = DriveCtx {
        self_gid: 0,
        pos: Vec3::ZERO,
        fwd: Vec3::X,
        now: 0.0,
        out: (0.0, 0.0, 0.0),
        targets: Default::default(),
        clearance: Clearance { ahead: None, left: None, right: None, range: 20.0 },
    };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    assert!(ctx.out.0 > 0.0 && ctx.out.1.abs() < 1e-6, "open → straight, got {:?}", ctx.out);

    // Blocked ahead, more room on the LEFT probe → steer toward it (nonzero steer),
    // throttle eased.
    ctx.clearance = Clearance { ahead: Some(4.0), left: Some(18.0), right: Some(5.0), range: 20.0 };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    assert!(ctx.out.1.abs() > 0.0, "blocked → steers toward open side, got {:?}", ctx.out);
    assert!(ctx.out.0 > 0.0 && ctx.out.0 < 0.6, "throttle eased when tight, got {:?}", ctx.out);

    // Boxed in on all probes → brake.
    ctx.clearance = Clearance { ahead: Some(1.0), left: Some(1.0), right: Some(1.0), range: 20.0 };
    assert_eq!(behavior.0.tick(&mut ctx), Status::Running);
    assert!(ctx.out.2 > 0.5, "boxed in → brake, got {:?}", ctx.out);
}
