// ── Sandbox smoke contract ──────────────────────────────────────────────────
//
// `sandbox_scene.usda` is an interactive arena. It deliberately has no
// scenario and therefore correctly returns NO-VERDICT when passed directly to
// `luncosim test`; loading an arena is not a test assertion. This script is the
// explicit release/CI contract for the same composed scene.
//
// It checks independent, observable invariants:
//   * the Modelica oscillator compiled and published a real signal;
//   * the fixed-joint demo retained its authored separation;
//   * the composed mount-frame demo and cosim sink have live world positions.
// Python demos are not part of this contract because Python is an optional
// runtime and an unavailable optional interpreter must not turn the smoke gate
// into a false pass or an unrelated failure.

const WAIT_S = 30.0;

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
    this.osc = find("/SandboxScene/Oscillator");
    this.target = find("/SandboxScene/CosimTarget");
    this.joint_a = find("/SandboxScene/JointTestA");
    this.joint_b = find("/SandboxScene/JointTestB");
    this.base = find("/SandboxScene/Base");
    this.arm = find("/SandboxScene/Base/Arm");
    print("[sandbox-smoke] ── composed arena smoke contract ─────────────");
}

fn fail(message) {
    this.done = true;
    fail_fast(message, "SANDBOX SMOKE", "SANDBOX_SMOKE");
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    if this.osc < 0 || this.target < 0 || this.joint_a < 0 ||
       this.joint_b < 0 || this.base < 0 || this.arm < 0 {
        fail("required composed prim missing from sandbox smoke scene");
        return;
    }

    let signal = get(this.osc, "signal");
    if signal == () {
        if this.t >= WAIT_S {
            fail("Modelica oscillator did not publish `signal` within " + WAIT_S + "s");
        }
        return;
    }

    this.done = true;
    verdict(#{
        signal: signal,
        target_pos: world_pos(this.target),
        joint_a_pos: world_pos(this.joint_a),
        joint_b_pos: world_pos(this.joint_b),
        arm_pos: world_pos(this.arm),
        joint_gap: distance(this.joint_a, this.joint_b),
    });
}

fn verdict(s) {
    let f = [];
    f.push(t_range(s.signal, -1.01, 1.01,
        "Modelica oscillator signal outside authored amplitude"));
    f.push(t_bounded(s.target_pos, 1000.0,
        "cosim target left the composed arena"));
    f.push(t_bounded(s.joint_a_pos, 1000.0,
        "fixed-joint body A has no finite world position"));
    f.push(t_bounded(s.joint_b_pos, 1000.0,
        "fixed-joint body B has no finite world position"));
    f.push(t_bounded(s.arm_pos, 1000.0,
        "mounted arm has no finite world position"));
    f.push(t_rel(s.joint_gap, 3.0, 5.0,
        "fixed-joint bodies lost their authored 3 m separation"));

    print("[sandbox-smoke] ── results ─────────────────────────────────");
    print("[sandbox-smoke]   oscillator signal = " + r4(s.signal));
    print("[sandbox-smoke]   fixed-joint gap    = " + r4(s.joint_gap) + " m");
    report_verdict(f, "SANDBOX SMOKE", "SANDBOX_SMOKE");
}
