// ── The co-sim fabric tracks the sun, end to end ────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/sun_tracker.usda`.
//
// WHAT IT GUARDS. Four different subsystems have to work for a solar panel to
// follow the sun, and the chain is authored entirely in USD — no actuator hack,
// no Rust in the loop:
//
//   solar bridge --(sun_azimuth)--> SunTracker.mo --(yaw)--> Hinge angle --> panel
//
// Every joint of that chain fails QUIETLY. A wire that did not resolve leaves the
// port reading zero; a Modelica model that failed to compile leaves its outputs
// at their initial values; a `inputs:angle.connect` that names the wrong prim
// simply never drives the joint. In each case the scene loads, the panel sits
// there, and it looks like a panel that happens to be pointing the right way.
//
// ── WHY A STEP, NOT A SNAPSHOT ──────────────────────────────────────────────
//
// Reading "yaw ≈ sun_azimuth" once proves almost nothing: at the scene's start
// both are near zero, so a dead controller with a dead wire and an unmoved joint
// agrees with the sun perfectly. So the sun is MOVED — `SetEnvironmentLight` with
// a new `sun_yaw` — and the whole chain has to follow it. A step the controller
// did not track is a step nothing else can fake.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("SUN_TRACKER", "PASS"|"FAIL")`. Helpers from `prelude/auto_tests.rhai`.

const STEP_YAW = 1.0;      // radians the sun is moved to (~57 deg)
const TOL_RAD = 0.15;      // servo convergence band (~8.6 deg)
const PORT_WAIT_S = 20.0;  // budget for the Modelica model to compile and publish

fn on_start(me) {
    this.t = 0.0;
    // WAIT FOR THE PORTS, do not assume a settle time. `SunTracker.mo` compiles
    // asynchronously and publishes nothing until it does — on the first run of
    // this file a fixed 4 s settle sampled `sun_azimuth` while it was still ()
    // and the "did the sun actually move" guard below skipped itself in silence.
    // That is the exact failure this test exists to catch, committed by the test.
    this.phase = "await_ports";
    this.phase_start = 0.0;
    this.settle_s = 2.0;    // after the ports appear, let the model reach steady state
    this.track_s = 12.0;    // first-order servo: give it time to slew and settle

    this.ctrl = find("/SunTrackerTest/SolarTower/Controller");
    this.hinge = find("/SunTrackerTest/SolarTower/Hinge");

    this.az_before = ();
    this.yaw_before = ();

    print("[sun] ── co-sim fabric tracks the sun ─────────────────────────");
    print("[sun] controller gid=" + this.ctrl + " hinge gid=" + this.hinge);
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.ctrl == () || this.ctrl < 0 || this.hinge == () || this.hinge < 0 {
        fail_fast("controller/hinge prims not found (ctrl=" + this.ctrl + " hinge=" +
                  this.hinge + ") — wrong scene?", "SUN TRACKER", "SUN_TRACKER");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let leg = this.t - this.phase_start;

    if this.phase == "await_ports" {
        if get(this.ctrl, "sun_azimuth") != () && get(this.ctrl, "yaw") != () {
            print("[sun] ports live at t=" + r2(this.t) + "s");
            this.phase = "settle";
            this.phase_start = this.t;
        } else if this.t > PORT_WAIT_S {
            fail_fast("Controller published no sun_azimuth/yaw within " +
                      PORT_WAIT_S + "s — SunTracker.mo never compiled, or its " +
                      "outputs are not exposed as ports", "SUN TRACKER", "SUN_TRACKER");
            this.phase = "done";
        }
    } else if this.phase == "settle" {
        if leg >= this.settle_s {
            this.az_before = get(this.ctrl, "sun_azimuth");
            this.yaw_before = get(this.ctrl, "yaw");
            print("[sun] before the step: sun_azimuth=" + r4(this.az_before) +
                  " yaw=" + r4(this.yaw_before) + " angle=" + r4(get(this.hinge, "angle")));
            // MOVE THE SUN. Everything after this is the chain's response.
            cmd("SetEnvironmentLight", #{ sun_yaw: STEP_YAW });
            print("[sun] step: SetEnvironmentLight sun_yaw=" + STEP_YAW + " rad");
            this.phase = "track";
            this.phase_start = this.t;
        }
    } else if this.phase == "track" {
        if leg >= this.track_s {
            this.phase = "done";
            verdict(#{
                az_before: this.az_before,
                yaw_before: this.yaw_before,
                az: get(this.ctrl, "sun_azimuth"),
                yaw: get(this.ctrl, "yaw"),
                angle: get(this.hinge, "angle"),
            });
        } else if leg % 4.0 < dt() {
            print("[sun] t=" + r2(this.t) + "s  az=" + r4(get(this.ctrl, "sun_azimuth")) +
                  " yaw=" + r4(get(this.ctrl, "yaw")) + " angle=" + r4(get(this.hinge, "angle")));
        }
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   ports present    a wire that did not resolve, or a model that did not
//                    compile, publishes NOTHING — and an absent port reads the
//                    same as a port reading zero, which is the answer this test
//                    would otherwise accept.
//   the sun moved    the bridge saw the step. Without this the two agreements
//                    below are "0 ≈ 0 ≈ 0" and a dead chain passes.
//   controller tracks  `yaw` converged on the NEW azimuth: the model ran and its
//                    input was really wired to the bridge.
//   the joint follows  the measured hinge `angle` matches `yaw`, so the
//                    cross-prim `inputs:angle.connect` reaches the joint motor.
//                    The controller can be perfect and the panel still not move.
fn verdict(s) {
    let f = [];
    f.push(t_present(s.az, "Controller sun_azimuth port"));
    f.push(t_present(s.yaw, "Controller yaw port"));
    f.push(t_present(s.angle, "Hinge angle port"));

    // Two halves of one guard, and the SECOND is the load-bearing one. The
    // commanded `sun_yaw` is not the azimuth the bridge publishes (different
    // convention), so how far `sun_azimuth` moves for a given step is not
    // something this file gets to predict — measured, it was 0.30 rad against a
    // 0.3 threshold, which is a test one epoch away from failing for no reason.
    // The bar is therefore "it moved at all, well clear of noise", and the real
    // evidence is that the CHAIN moved: yaw slewed 0.6 rad to follow it.
    if s.az != () && s.az_before != () {
        f.push(t_true((s.az - s.az_before).abs() > 0.1,
            "the solar bridge did not see the sun move: sun_azimuth went from " +
            r4(s.az_before) + " to " + r4(s.az) + " after a " + STEP_YAW +
            " rad step. Everything below compares two numbers that never changed."));
    }
    if s.yaw != () && s.yaw_before != () {
        f.push(t_true((s.yaw - s.yaw_before).abs() > 0.2,
            "the panel never slewed: yaw went from " + r4(s.yaw_before) + " to " +
            r4(s.yaw) + " rad across the step. A controller already sitting where " +
            "the sun ended up agrees with it for free."));
    }

    if s.az != () && s.yaw != () {
        f.push(t_true((s.yaw - s.az).abs() < TOL_RAD,
            "the controller did not track the sun: yaw=" + r4(s.yaw) +
            " against sun_azimuth=" + r4(s.az) + " rad. Either SunTracker.mo is " +
            "not running or its `sun_azimuth` input is not wired to the bridge."));
    }

    if s.angle != () && s.yaw != () {
        f.push(t_true((s.angle - s.yaw).abs() < TOL_RAD,
            "the joint did not follow the controller: hinge angle=" + r4(s.angle) +
            " against yaw=" + r4(s.yaw) + " rad. The cross-prim " +
            "`inputs:angle.connect` is not reaching the joint's motor — the " +
            "controller can be perfect and the panel still never move."));
    }

    print("[sun] ── results ───────────────────────────────────────────");
    print("[sun]   sun_azimuth  " + r4(s.az_before) + " -> " + r4(s.az) + " rad");
    print("[sun]   yaw          " + r4(s.yaw_before) + " -> " + r4(s.yaw) + " rad");
    print("[sun]   hinge angle  " + r4(s.angle) + " rad");

    report_verdict(f, "SUN TRACKER", "SUN_TRACKER");
}
