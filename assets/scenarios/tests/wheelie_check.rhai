// ── Launch attitude — full throttle from rest, flat ground ──────────────────
//
// TEST-ONLY. Drives `scenes/tests/wheelie_check.usda`.
//
// WHAT IT MEASURES. A wheel motor's reaction torque goes into the link the wheel
// hinges to. On a rocker-bogie the middle and rear motors share one FREE bogie
// hinge, so the launch transient torques the bogie about its own pivot with
// nothing but the vehicle's weight to resist it. Oversized drive authority
// therefore does not show up as wheelspin — it shows up as the bogie slamming to
// its ±20° mechanical stop, the middle wheel lifting, and the rover driving
// nose-up on four wheels.
//
// Sampled through the drive phase, reported as running maxima:
//
//   bogie   |BogieHingeL/R.angle| — the hinge that carries two of the three
//           motors per side. At the stop (0.349 rad = 20°) a wheel is airborne.
//   rocker  |HingeL/R.angle| — the chassis↔rocker hinge, held by the
//           differential; the chassis pitch follows it.
//   tilt    the chassis's own tilt from level (deg). On flat ground with no
//           obstacle this is pure launch pitch and should stay near zero.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("WHEELIE_CHECK", "PASS"|"FAIL")`.

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 4.0;     // drop and ring down before any throttle
    this.drive_s = 6.0;      // full throttle, straight
    this.sample_s = 0.5;
    this.next_sample = 0.0;

    // The rover's Comms/EarthTracker carries three `earth_mount_*` wires that
    // never land in this scene — it has no celestial content, so no Earth
    // direction exists to publish and the antenna holds its authored pose (by
    // design, `lunco-environment/src/earth.rs`). Declare them as expected
    // faults so the never-landed gate doesn't mask the physics verdict. See
    // `expect_fault` in auto_tests.rhai.
    expect_fault("earth_mount_x");
    expect_fault("earth_mount_y");
    expect_fault("earth_mount_z");

    this.rover = find("/WheelieCheck/Rover");
    this.bl = find("/WheelieCheck/Rover/BogieHingeL");
    this.br = find("/WheelieCheck/Rover/BogieHingeR");
    this.hl = find("/WheelieCheck/Rover/HingeL");
    this.hr = find("/WheelieCheck/Rover/HingeR");

    this.peak_bogie = 0.0;
    this.peak_rocker = 0.0;
    this.peak_tilt = 0.0;
    this.start_pos = ();
    this.phase = "settle";

    print("[wheelie] ── launch attitude, full throttle on the flat ───────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.bl == () || this.bl < 0 || this.br == () || this.br < 0 {
        fail_fast("bogie hinges not found — wrong scene?", "WHEELIE CHECK", "WHEELIE_CHECK");
        this.phase = "done";
        return;
    }

    this.t += dt();

    // Settle first: the drop transient is not the thing under test, and folding
    // it into the maxima would let a landing bounce answer for a launch wheelie.
    if this.phase == "settle" && this.t >= this.settle_s {
        this.phase = "drive";
        this.start_pos = world_pos(this.rover);
        print("[wheelie] t=" + r2(this.t) + "s  DRIVE phase: throttle 1.0, straight");
    }

    if this.phase == "drive" {
        drive(this.rover, 1.0, 0.0);

        let b = max_abs(get(this.bl, "angle"), get(this.br, "angle"));
        let r = max_abs(get(this.hl, "angle"), get(this.hr, "angle"));
        let tl = tilt_deg(this.rover);
        if b != () && b > this.peak_bogie { this.peak_bogie = b; }
        if r != () && r > this.peak_rocker { this.peak_rocker = r; }
        if tl != () && tl > this.peak_tilt { this.peak_tilt = tl; }

        if this.t >= this.next_sample {
            print("[wheelie] t=" + r2(this.t) + "s  bogie=" + r4(b) +
                  " rocker=" + r4(r) + " tilt=" + r2(tl) + "deg");
            this.next_sample = this.t + this.sample_s;
        }
    }

    if this.t >= this.settle_s + this.drive_s {
        this.phase = "done";
        let pos = world_pos(this.rover);
        verdict(#{ bogie: this.peak_bogie, rocker: this.peak_rocker,
                   tilt: this.peak_tilt, pos: pos,
                   travelled: seg(this.start_pos, pos) });
    }
}

fn max_abs(a, b) {
    if a == () && b == () { return (); }
    let x = if a == () { 0.0 } else { a.abs() };
    let y = if b == () { 0.0 } else { b.abs() };
    if x > y { x } else { y }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   rover bounded   a suspension that diverges throws the vehicle out of scene.
//   it drove        the anti-trivial guard. A rover that never moved launches
//                   perfectly flat, and every bound below would pass on it.
//   bogie < 10°     THE TEST. The ±20° stop is where a wheel is off the ground;
//                   half of it is already a suspension doing something other
//                   than following terrain, and on flat ground there is no
//                   terrain to follow.
//   tilt < 5°       the visible symptom — the nose-up launch a driver sees.
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.pos, 400.0, "rover"));
    f.push(t_moved(s.travelled, 2.0, "rover under full throttle"));
    f.push(t_true(s.bogie < 0.175,
        "bogie hinge reached " + r4(s.bogie) + " rad (" + r2(s.bogie * 57.2958) +
        " deg) under throttle on FLAT ground — the ±20° stop is at 0.349, so the " +
        "middle/rear motors' reaction torque is folding the suspension instead of " +
        "driving the rover. Size the tier's motor against the traction cone " +
        "(`components/mobility/motor.usda`), not against ambition"));
    f.push(t_range(s.tilt, 0.0, 5.0,
        "peak chassis tilt during launch (deg) — flat ground, so this is pure " +
        "nose-up pitch from drive reaction"));

    print("[wheelie] ── results ──────────────────────────────────────");
    print("[wheelie]   peak bogie   " + r4(s.bogie) + " rad (" + r2(s.bogie * 57.2958) + " deg)");
    print("[wheelie]   peak rocker  " + r4(s.rocker) + " rad (" + r2(s.rocker * 57.2958) + " deg)");
    print("[wheelie]   peak tilt    " + r2(s.tilt) + " deg");
    print("[wheelie]   travelled    " + r2(s.travelled) + " m");

    report_verdict(f, "WHEELIE CHECK", "WHEELIE_CHECK");
}
