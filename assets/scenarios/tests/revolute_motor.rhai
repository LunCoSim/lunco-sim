// ── A revolute joint's angular motor answers its port ───────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/revolute_motor.usda`.
//
// WHAT IT GUARDS. The `angle` port a `PhysicsRevoluteJoint` auto-exposes must
// reach `apply_joint_drives` and become the motor's target. The rig is built so
// nothing else can move the arm: the hinge axis is vertical and the arm is
// horizontal, so gravity applies NO torque about it. The arm is therefore level
// and still until something commands it — and if the port is dead, it stays that
// way, silently, with the scene looking perfectly correct.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("REVOLUTE_MOTOR", "PASS"|"FAIL")`.

const TARGET = 1.2;   // rad, commanded through the port

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 2.0;    // confirm it is at rest BEFORE commanding
    this.done_s = 10.0;
    this.sample_s = 1.0;
    this.next_sample = 0.0;

    this.hinge = find("/SandboxScene/Hinge");
    this.arm = find("/SandboxScene/Arm");
    this.at_rest = ();
    this.sent = false;
    this.phase = "rest";

    print("[motor] ── revolute motor via the `angle` port ───────────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.hinge == () || this.hinge < 0 {
        fail_fast("Hinge not found — wrong scene?", "REVOLUTE MOTOR", "REVOLUTE_MOTOR");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let a = get(this.hinge, "angle");

    if this.t >= this.next_sample {
        print("[motor] t=" + r2(this.t) + "s  angle=" + r4(a) + " rad");
        this.next_sample = this.t + this.sample_s;
    }

    if this.phase == "rest" && this.t >= this.settle_s {
        this.at_rest = a;
        set(this.hinge, "angle", TARGET);
        this.sent = true;
        this.phase = "driven";
        print("[motor] commanded angle=" + TARGET);
    }

    if this.t >= this.done_s {
        this.phase = "done";
        verdict(#{ at_rest: this.at_rest, driven: a, sent: this.sent,
                   pos: world_pos(this.arm) });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   port present   a hinge publishing no `angle` is not measurable.
//   still at rest  gravity applies no torque about a vertical axis, so the arm
//                  must be ~0 before the command. Without this, a rig drifting
//                  on its own could reach the target and pass for the wrong
//                  reason.
//   it obeyed      within 10% of the commanded angle.
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.pos, 100.0, "arm"));
    f.push(t_present(s.driven, "Hinge `angle` port"));
    f.push(t_true(s.sent, "the scenario never issued its port write"));

    if s.at_rest != () {
        f.push(t_true(s.at_rest.abs() < 0.1,
            "the arm was not at rest before being commanded (" + r4(s.at_rest) +
            " rad) — the hinge axis is vertical, so nothing should move it, and a " +
            "rig that drifts could reach the target for the wrong reason"));
    }
    if s.driven != () {
        f.push(t_rel(s.driven, TARGET, 10.0,
            "the `angle` port did not drive the hinge to its commanded target"));
    }

    print("[motor] ── results ───────────────────────────────────────────");
    print("[motor]   at rest  " + r4(s.at_rest) + " rad");
    print("[motor]   driven   " + r4(s.driven) + " rad (asked " + TARGET + ")");

    report_verdict(f, "REVOLUTE MOTOR", "REVOLUTE_MOTOR");
}
