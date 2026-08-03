// ── Raw Avian facts + Modelica conversions publish real numbers ────────────
//
// TEST-ONLY. Runs against scenes/tests/sensor.usda: a 100 kg box at rest with
// a reusable IMU conversion, a raw-ray/altimeter conversion, and native Avian
// collider contact.
//
// The test reads the Modelica outputs from the mounted conversion prims and
// reads contact directly from the collider. No production controller logic is
// hidden in this script.

const EXPECT_RANGE = 0.8;    // box centre 0.5 + altimeter mount translation 0.3
const EXPECT_FORCE = 162.0;  // 100 kg * 1.62 m/s^2 lunar gravity

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 4.0;
    this.box = find("/SandboxScene/Box");
    this.imu = find("/SandboxScene/Box/IMU");
    this.altimeter = find("/SandboxScene/Box/Altimeter/Model");
    this.phase = "settle";
    print("[sensors] ── raw Avian + Modelica ─────────────────────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.box == () || this.box < 0 || this.imu == () || this.imu < 0
        || this.altimeter == () || this.altimeter < 0 {
        fail_fast("sensor conversion prim not found — wrong scene?", "SENSORS", "SENSORS");
        this.phase = "done";
        return;
    }

    this.t += dt();
    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{
            fx: get(this.imu, "specific_force_x"),
            fy: get(this.imu, "specific_force_y"),
            fz: get(this.imu, "specific_force_z"),
            range: get(this.altimeter, "range_m"),
            range_valid: get(this.altimeter, "range_valid"),
            contact: get(this.box, "contact"),
            force: get(this.box, "contact_force"),
        });
    }
}

fn verdict(s) {
    let f = [];
    f.push(t_present(s.fx, "IMU specific_force_x output"));
    f.push(t_present(s.fy, "IMU specific_force_y output"));
    f.push(t_present(s.fz, "IMU specific_force_z output"));
    f.push(t_present(s.range, "altimeter range_m output"));
    f.push(t_present(s.range_valid, "altimeter range_valid output"));
    f.push(t_present(s.contact, "native Avian contact port"));
    f.push(t_present(s.force, "native Avian contact_force port"));

    if s.fy != () {
        f.push(t_true(s.fx.is_finite() && s.fy.is_finite() && s.fz.is_finite(),
            "the Modelica IMU conversion returned a non-finite reading"));
    }
    if s.range != () {
        f.push(t_rel(s.range, EXPECT_RANGE, 20.0,
            "the raw Avian ray does not see the ground at the authored mount"));
    }
    if s.range_valid != () {
        f.push(t_true(s.range_valid > 0.5,
            "the altimeter conversion did not preserve raw ray validity"));
    }
    if s.contact != () {
        f.push(t_true(s.contact > 0.5,
            "native Avian contact is not active for the resting box"));
    }
    if s.force != () {
        f.push(t_rel(s.force, EXPECT_FORCE, 25.0,
            "native Avian contact force is not the box's lunar weight"));
    }

    print("[sensors] ── results ─────────────────────────────────────────");
    print("[sensors]   force   " + r2(s.fx) + ", " + r2(s.fy) + ", " + r2(s.fz));
    print("[sensors]   range   " + r4(s.range) + " m (expect " + EXPECT_RANGE + ")");
    print("[sensors]   contact " + r2(s.contact) + "  force " + r2(s.force) +
          " N (expect " + EXPECT_FORCE + ")");

    report_verdict(f, "SENSORS", "SENSORS");
}
