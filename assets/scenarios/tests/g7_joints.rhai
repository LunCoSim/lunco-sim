// ── Spherical, distance, and reduced-D6 joints actually constrain ──────────
//
// TEST-ONLY. Runs against `scenes/tests/g7_joints.usda`.
//
// WHAT IT GUARDS. `PhysicsSphericalJoint`, `PhysicsDistanceJoint`, and a generic
// D6 joint reduced to `PhysicsRevoluteJoint` are the joint kinds whose failure
// looks like success: an UNSUPPORTED joint is dropped at load, and a dropped joint
// leaves its body FREE. A free body under gravity falls — which, for a pendulum
// hanging off an anchor, looks a great deal like swinging. The scene log says
// "Built USD joint …", but a log line is not a constraint, and nobody reads it.
//
// So both bodies are checked against the limit their joint is supposed to
// impose, not against having moved:
//
//   BallArm   swings on a 45° cone  → stays within that cone of the anchor
//   Weight    hangs on a 2 m tether → never further than 2 m from its anchor
//   D6Arm     hangs from a 1 m reduced-D6 hinge → stays 1 m from its anchor
//
// A dropped joint fails both by metres within the first second.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("G7_JOINTS", "PASS"|"FAIL")` — the telemetry verdict `scene_test` reads
// for its exit code. Helpers come from `prelude/auto_tests.rhai`.

const MAX_DISTANCE = 2.0;   // physics:maxDistance on the Tether
const CONE_DEG = 45.0;      // coneAngle0/1Limit = 0.785 rad
const D6_ARM_DISTANCE = 1.0;

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 6.0;
    this.sample_s = 1.0;
    this.next_sample = 0.0;

    this.ball_anchor = find("/SandboxScene/BallAnchor");
    this.ball_arm = find("/SandboxScene/BallArm");
    this.tether_anchor = find("/SandboxScene/TetherAnchor");
    this.weight = find("/SandboxScene/Weight");
    this.d6_anchor = find("/SandboxScene/D6Anchor");
    this.d6_arm = find("/SandboxScene/D6Arm");

    this.max_swing = 0.0;   // worst cone angle seen
    this.max_reach = 0.0;   // worst tether length seen
    this.max_d6_reach = 0.0;
    this.phase = "settle";

    print("[g7] ── spherical + distance + reduced-D6 joints ─────────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.ball_arm == () || this.ball_arm < 0 || this.weight == () || this.weight < 0 ||
       this.d6_anchor == () || this.d6_anchor < 0 || this.d6_arm == () || this.d6_arm < 0 {
        fail_fast("rig prims not found — wrong scene?", "G7 JOINTS", "G7_JOINTS");
        this.phase = "done";
        return;
    }

    this.t += dt();

    // The cone is measured from STRAIGHT DOWN at the anchor: that is the axis a
    // pendulum rests on, and the angle the joint's limit is authored about.
    let pa = world_pos(this.ball_anchor);
    let pb = world_pos(this.ball_arm);
    if pa != () && pb != () {
        let d = vsub(pb, pa);
        let a = angle_deg(d, [0.0, -1.0, 0.0]);
        if a != () && a > this.max_swing { this.max_swing = a; }
    }

    let r = distance(this.tether_anchor, this.weight);
    if r > this.max_reach { this.max_reach = r; }
    let d6_r = distance(this.d6_anchor, this.d6_arm);
    if d6_r > this.max_d6_reach { this.max_d6_reach = d6_r; }

    if this.t >= this.next_sample {
        print("[g7] t=" + r2(this.t) + "s  swing=" + r2(this.max_swing) +
              "deg  tether=" + r4(this.max_reach) + "m  d6=" + r4(this.max_d6_reach) + "m");
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{
            swing: this.max_swing,
            reach: this.max_reach,
            arm_pos: world_pos(this.ball_arm),
            weight_pos: world_pos(this.weight),
            d6_arm_pos: world_pos(this.d6_arm),
            d6_reach: this.max_d6_reach,
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   bodies bounded  a dropped joint lets its body accelerate out of the scene.
//   cone held       ≤ 45° + 10% of slack for solver compliance. A free body
//                   passes 90° on its way past the anchor, so the margin cannot
//                   hide a missing constraint.
//   tether held     ≤ 2 m + 10%. Same argument: unheld, the weight is metres
//                   below within a second.
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.arm_pos, 100.0, "ball arm"));
    f.push(t_bounded(s.weight_pos, 100.0, "tethered weight"));
    f.push(t_bounded(s.d6_arm_pos, 100.0, "reduced-D6 arm"));

    f.push(t_true(s.swing <= CONE_DEG * 1.1,
        "the spherical joint did not hold its cone: swung " + r2(s.swing) +
        " deg against a " + CONE_DEG + " deg limit — an unsupported joint kind is " +
        "DROPPED at load and its body is left free"));

    f.push(t_true(s.reach <= MAX_DISTANCE * 1.1,
        "the distance joint did not hold its tether: reached " + r4(s.reach) +
        " m against a " + MAX_DISTANCE + " m limit — the joint was dropped and the " +
        "weight is in free fall"));

    f.push(t_true(s.d6_reach <= D6_ARM_DISTANCE * 1.1,
        "the generic D6 joint did not reduce to a working revolute constraint: arm reached " +
        r4(s.d6_reach) + " m from its 1 m anchor — the unsupported shape was dropped"));

    print("[g7] ── results ─────────────────────────────────────────────");
    print("[g7]   max swing  " + r2(s.swing) + " deg (limit " + CONE_DEG + ")");
    print("[g7]   max tether " + r4(s.reach) + " m (limit " + MAX_DISTANCE + ")");
    print("[g7]   max D6 gap " + r4(s.d6_reach) + " m (expected " + D6_ARM_DISTANCE + ")");

    report_verdict(f, "G7 JOINTS", "G7_JOINTS");
}
