// ── USD-authored sensors publish real numbers ───────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/sensor.usda`: a 100 kg box at
// rest on the ground, carrying all three sensor kinds at once.
//
// WHAT IT GUARDS. A sensor authored in USD but not projected into the ECS
// publishes NOTHING, and a port that is absent reads the same as a port reading
// zero — which, for an accelerometer on a body at rest, is the correct answer.
// That is the trap: the IMU alone cannot tell "working" from "missing". So all
// three sensors are read together, and two of them have non-zero expected values
// that only a live sensor can produce.
//
//   imu      accel_* ≈ 0        at rest — necessary, not sufficient
//   range    ≈ 0.8 m            a downward altimeter that must SEE the ground
//   contact  = 1, ≈ mass * g    a box that is demonstrably resting on something
//
// The expected numbers are arithmetic from the scene, not recorded from a run.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("SENSORS", "PASS"|"FAIL")`.

const EXPECT_RANGE = 0.8;    // sensor height: box centre 0.5 + lunco:sensor:offset 0.3
const EXPECT_FORCE = 162.0;  // 100 kg * 1.62 m/s^2 lunar gravity

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 4.0;   // let it come to rest so "at rest" means something
    this.box = find("/SandboxScene/Box");
    this.phase = "settle";
    print("[sensors] ── imu + range + contact ─────────────────────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.box == () || this.box < 0 {
        fail_fast("Box not found — wrong scene?", "SENSORS", "SENSORS");
        this.phase = "done";
        return;
    }

    this.t += dt();
    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{
            ax: get(this.box, "accel_x"),
            ay: get(this.box, "accel_y"),
            az: get(this.box, "accel_z"),
            range: get(this.box, "range"),
            contact: get(this.box, "contact"),
            force: get(this.box, "contact_force"),
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   ports present  every one. A MISSING port is the failure this test exists
//                  for, and `t_present` is the only check that can see it —
//                  every numeric assertion below would read `()` as agreeable.
//   at rest        |accel| small. Necessary but NOT sufficient on its own.
//   range sees     ≈0.8 m — the sensor's OWN height, box centre plus its
//                  authored offset, not the box's half-height. A raycast that hit
//                  nothing reads 0 or infinity; neither is 0.8.
//   contact holds  flag set, and the force is the box's weight. This is the one
//                  a dead sensor cannot fake.
fn verdict(s) {
    let f = [];
    f.push(t_present(s.ax, "imu accel_x port"));
    f.push(t_present(s.ay, "imu accel_y port"));
    f.push(t_present(s.az, "imu accel_z port"));
    f.push(t_present(s.range, "range port"));
    f.push(t_present(s.contact, "contact port"));
    f.push(t_present(s.force, "contact_force port"));

    if s.ay != () {
        f.push(t_true(s.ax.abs() < 0.5 && s.ay.abs() < 0.5 && s.az.abs() < 0.5,
            "the IMU does not read ~0 on a body at rest (" + r2(s.ax) + ", " +
            r2(s.ay) + ", " + r2(s.az) + " m/s^2)"));
    }
    if s.range != () {
        f.push(t_rel(s.range, EXPECT_RANGE, 20.0,
            "the altimeter does not see the ground it is resting on (m)"));
    }
    if s.contact != () {
        f.push(t_true(s.contact > 0.5,
            "the contact sensor reads " + r2(s.contact) + " on a box that is " +
            "demonstrably resting on the ground"));
    }
    if s.force != () {
        f.push(t_rel(s.force, EXPECT_FORCE, 25.0,
            "contact force is not the box's weight (N)"));
    }

    print("[sensors] ── results ─────────────────────────────────────────");
    print("[sensors]   accel   " + r2(s.ax) + ", " + r2(s.ay) + ", " + r2(s.az));
    print("[sensors]   range   " + r4(s.range) + " m (expect " + EXPECT_RANGE + ")");
    print("[sensors]   contact " + r2(s.contact) + "  force " + r2(s.force) +
          " N (expect " + EXPECT_FORCE + ")");

    report_verdict(f, "SENSORS", "SENSORS");
}
