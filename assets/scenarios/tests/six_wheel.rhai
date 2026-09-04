// ── Authored wheel→port bindings drive a non-2×2 layout ─────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/six_wheel.usda`.
//
// WHAT IT GUARDS. `vessels/rovers/six_wheel_rover.usda` is the fleet's proof
// that "a dynamic vehicle is a USD file, not a Rust struct": six wheels, three a
// side, each naming the port it listens to —
//
//     float inputs:drive.connect = </SixWheelRover.outputs:drive_left>
//
// — and nothing anywhere assuming four wheels or deriving a side from an index.
// The failure this guards is a loader that falls back to an index-derived split when the
// layout is not 2×2: with six wheels, even/odd alternates down each bank instead
// of splitting left from right, so both banks receive the same average command.
// Such a rover still spawns, still drives forward, still looks right — and cannot
// turn, because every wheel is being told the same thing.
//
// So the discriminator is YAW UNDER STEER, not motion. Straight-line driving
// passes on the broken wiring too.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("SIX_WHEEL", "PASS"|"FAIL")` — the telemetry verdict `scene_test` reads
// for its exit code. Helpers come from `prelude/auto_tests.rhai` and
// `prelude/nav.rhai` (`yaw_delta_deg`, which reports right-hand yaw POSITIVE).

fn port_value(target, name, direction) {
    let record = query("ReadPorts", #{ api_id: target });
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name && port.direction == direction {
            return port.value;
        }
    }
    ()
}

fn set_headlights(target, value) {
    let result = cmd("SetPorts", #{
        target: target,
        writes: [["headlights", value]],
    });
    result.ok
}

fn on_start(me) {
    this.t = 0.0;
    this.readiness_elapsed = 0.0;
    this.readiness_timeout = 10.0;
    this.phase_start = 0.0;
    this.settle_s = 2.0;    // let it sit down on its wheels
    this.drive_s = 8.0;     // straight leg
    this.steer_s = 8.0;     // steering leg
    this.sample_s = 2.0;
    this.next_sample = 0.0;

    // USD composition is projected asynchronously.  A composed child is not
    // a missing asset just because the root script ran first; retain the
    // authored paths and resolve their live handles at the simulation boundary.
    this.rover = -1;
    this.camera = -1;
    this.controller = -1;
    this.light = -1;
    this.light_right = -1;

    this.dist_straight = 0.0;
    this.yaw_straight = 0.0;
    this.yaw_steer = 0.0;
    // `yaw_delta_deg` answers () when the orientation is degenerate. A rover whose
    // heading was NEVER readable would finish with both accumulators at 0.0 and
    // sail through "drove straight" — so the samples are counted, and a run with
    // none is a failure rather than a free pass.
    this.yaw_ok = 0;
    this.camera_samples = 0;
    this.camera_gap_error = 0.0;
    this.camera_fwd_min_dot = 1.0;
    this.camera_baseline = ();

    this.pos_prev = ();
    this.fprev = ();
    this.phase = "settle";

    print("[6w] ── authored wheel→port bindings ─────────────────────────");
    print("[6w] rover gid=" + this.rover + " camera gid=" + this.camera +
          " headlight controller gid=" + this.controller + " light gid=" + this.light);
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    this.readiness_elapsed += dt();

    if this.rover == () || this.rover < 0 { this.rover = find("/SixWheelTest/Rover"); }
    if this.camera == () || this.camera < 0 {
        this.camera = find("/SixWheelTest/Rover/FrontCamera");
    }
    if this.controller == () || this.controller < 0 {
        this.controller = find("/SixWheelTest/Rover/HeadlightController_L");
    }
    if this.light == () || this.light < 0 {
        this.light = find("/SixWheelTest/Rover/Headlight_L/Light");
    }
    if this.light_right == () || this.light_right < 0 {
        this.light_right = find("/SixWheelTest/Rover/Headlight_R/Light");
    }

    if this.rover == () || this.rover < 0 || this.camera == () || this.camera < 0 ||
       this.controller == () || this.controller < 0 || this.light == () || this.light < 0 ||
       this.light_right == () || this.light_right < 0 {
        if this.readiness_elapsed >= this.readiness_timeout {
            fail_fast("authored rover composition did not materialize before " +
                      r2(this.readiness_timeout) + " s (rover=" + this.rover +
                      ", camera=" + this.camera + ", controller=" + this.controller +
                      ", left_light=" + this.light + ", right_light=" + this.light_right + ")",
                      "SIX WHEEL", "SIX_WHEEL");
            this.phase = "done";
        }
        return;
    }

    let p = world_pos(this.rover);
    let fwd = world_forward(this.rover);
    let camera_pos = world_pos(this.camera);
    let camera_fwd = world_forward(this.camera);
    if p == () || fwd == () || camera_pos == () || camera_fwd == () {
        if this.readiness_elapsed >= this.readiness_timeout {
            fail_fast("authored rover composition has no live pose after " +
                      r2(this.readiness_timeout) + " s",
                      "SIX WHEEL", "SIX_WHEEL");
            this.phase = "done";
        }
        return;
    }

    this.t += dt();
    let leg = this.t - this.phase_start;

    let camera_gap = vlen(vsub(camera_pos, p));
    if this.camera_baseline != () {
        let gap_error = (camera_gap - this.camera_baseline.gap).abs();
        if gap_error > this.camera_gap_error { this.camera_gap_error = gap_error; }
    }
    if fwd != () {
        let alignment = vdot(vnorm(camera_fwd), vnorm(fwd));
        if alignment < this.camera_fwd_min_dot { this.camera_fwd_min_dot = alignment; }
    }
    this.camera_samples += 1;
    let dyaw = ();
    if this.fprev != () && fwd != () {
        dyaw = yaw_delta_deg(this.fprev, fwd);
    }
    if dyaw != () { this.yaw_ok += 1; }

    if this.phase == "settle" {
        if leg >= this.settle_s {
            this.camera_baseline = #{
                rover: p,
                camera: camera_pos,
                gap: camera_gap,
                height: camera_pos[1] - p[1],
            };
            this.phase = "straight";
            this.phase_start = this.t;
            print("[6w] STRAIGHT leg: throttle 1.0, steer 0.0");
        }
    } else if this.phase == "straight" {
        drive(this.rover, 1.0, 0.0);
        this.dist_straight += seg(this.pos_prev, p);
        if dyaw != () { this.yaw_straight += dyaw; }
        if leg >= this.drive_s {
            this.phase = "steer";
            this.phase_start = this.t;
            print("[6w] STEER leg: throttle 1.0, steer +0.6 (must yaw RIGHT)");
        }
    } else if this.phase == "steer" {
        drive(this.rover, 1.0, 0.6);
        if dyaw != () { this.yaw_steer += dyaw; }
        if leg >= this.steer_s {
            drive(this.rover, 0.0, 0.0);
            if !set_headlights(this.rover, 1.0) {
                fail_fast("Rhai could not issue the semantic headlights=1 command",
                          "SIX WHEEL", "SIX_WHEEL");
                this.phase = "done";
                return;
            }
            this.phase = "lights_on";
            this.phase_start = this.t;
            print("[6w] LIGHTS ON: Rhai command -> Modelica photometry/load");
        }
    } else if this.phase == "lights_on" {
        if leg >= 0.5 {
            this.lights_on = port_value(this.rover, "headlight_left_intensity", "out");
            this.lights_on_right = port_value(this.rover, "headlight_right_intensity", "out");
            this.power_on = port_value(this.rover, "headlight_left_power_w", "out");
            this.power_on_right = port_value(this.rover, "headlight_right_power_w", "out");
            this.render_on = port_value(this.light, "light_intensity", "in");
            this.render_on_right = port_value(this.light_right, "light_intensity", "in");
            if this.lights_on == () || this.lights_on_right == () || this.power_on == () ||
               this.power_on_right == () || this.render_on == () || this.render_on_right == () {
                fail_fast("headlight on-state did not expose Modelica and scene-property ports",
                          "SIX WHEEL", "SIX_WHEEL");
                this.phase = "done";
                return;
            }
            if !set_headlights(this.rover, 0.0) {
                fail_fast("Rhai could not issue the semantic headlights=0 command",
                          "SIX WHEEL", "SIX_WHEEL");
                this.phase = "done";
                return;
            }
            this.phase = "lights_off";
            this.phase_start = this.t;
            print("[6w] LIGHTS OFF: Modelica output must return to zero");
        }
    } else if this.phase == "lights_off" {
        if leg >= 0.5 {
            this.lights_off = port_value(this.rover, "headlight_left_intensity", "out");
            this.lights_off_right = port_value(this.rover, "headlight_right_intensity", "out");
            this.power_off = port_value(this.rover, "headlight_left_power_w", "out");
            this.power_off_right = port_value(this.rover, "headlight_right_power_w", "out");
            this.render_off = port_value(this.light, "light_intensity", "in");
            this.render_off_right = port_value(this.light_right, "light_intensity", "in");
            this.phase = "done";
            verdict(#{
                dist: this.dist_straight,
                yaw_straight: this.yaw_straight,
                yaw_steer: this.yaw_steer,
                yaw_ok: this.yaw_ok,
                camera: this.camera_baseline,
                camera_samples: this.camera_samples,
                camera_gap_error: this.camera_gap_error,
                camera_fwd_min_dot: this.camera_fwd_min_dot,
                pos: p,
                lights_on: this.lights_on,
                lights_on_right: this.lights_on_right,
                lights_off: this.lights_off,
                lights_off_right: this.lights_off_right,
                power_on: this.power_on,
                power_on_right: this.power_on_right,
                power_off: this.power_off,
                power_off_right: this.power_off_right,
                render_on: this.render_on,
                render_on_right: this.render_on_right,
                render_off: this.render_off,
                render_off_right: this.render_off_right,
            });
            return;
        }
    }

    this.pos_prev = p;
    if fwd != () { this.fprev = fwd; }

    if this.t >= this.next_sample {
        print("[6w] t=" + r2(this.t) + "s  " + this.phase + "  dist=" +
              r2(this.dist_straight) + "m  yaw_str=" + r2(this.yaw_straight) +
              "deg  yaw_steer=" + r2(this.yaw_steer) + "deg");
        this.next_sample = this.t + this.sample_s;
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   still a vehicle   bounded position: a rover that left the world can produce
//                     any yaw number at all.
//   heading readable  the yaw samples are admissible. Zero of them means both
//                     accumulators are 0.0 for a reason that has nothing to do
//                     with steering.
//   it drove          the anti-trivial guard. Every check below is about HOW it
//                     moved, and a rover that never moved satisfies "went
//                     straight" perfectly.
//   went straight     both banks got the same command, so the six wheels did not
//                     fight each other.
//   +steer yaws RIGHT the assertion that index-derived wiring cannot pass. If
//                     ignored, the explicit bindings give both banks equal commands and the rover keeps
//                     going straight; wired by the AUTHORED left/right binding it
//                     turns, and the sign says the bands are the right way round.
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.pos, 500.0, "rover"));

    f.push(t_true(s.yaw_ok > 10,
        "the rover's heading was readable on only " + s.yaw_ok + " ticks, so the " +
        "yaw figures below are not a measurement of anything"));

    f.push(t_moved(s.dist, 2.0, "straight-leg travel"));

    f.push(t_true(s.yaw_straight.abs() < 25.0,
        "with steer 0 the rover yawed " + r2(s.yaw_straight) + " deg — the two " +
        "banks are not receiving the same command, so the authored bindings are " +
        "not splitting the wheels left/right the way they say"));

    f.push(t_true(s.yaw_steer > 10.0,
        "with steer +0.6 the rover yawed " + r2(s.yaw_steer) + " deg, and it must " +
        "turn RIGHT by a clear margin. Near zero is what index-derived wiring " +
        "produces on a six-wheel layout: both banks get the same average command " +
        "and the rover can only go straight. Negative means the banks are swapped."));

    f.push(t_range(s.camera.height, 0.35, 0.75,
        "six-wheel camera height above the rover body is " + r2(s.camera.height) +
        " m; expected a lowered but above-chassis mount"));

    f.push(t_true(s.camera_samples > 100,
        "the mounted camera produced only " + s.camera_samples + " live pose samples"));

    f.push(t_true(s.camera_gap_error < 0.05,
        "mounted camera separation changed by " + r4(s.camera_gap_error) +
        " m during steering instead of following rigidly"));

    f.push(t_true(s.camera_fwd_min_dot > 0.98,
        "mounted camera forward alignment fell to " + r4(s.camera_fwd_min_dot) +
        "; the onboard view no longer faces the rover's travel direction"));

    f.push(t_true(s.lights_on > 1000.0,
        "Modelica headlight output stayed at " + r2(s.lights_on) + " lm after the on command"));
    f.push(t_true(s.lights_on_right > 1000.0,
        "right Modelica headlight output stayed at " + r2(s.lights_on_right) +
        " lm after the on command"));
    f.push(t_true(s.power_on > 0.1,
        "Modelica headlight load stayed at " + r2(s.power_on) + " W after the on command"));
    f.push(t_true(s.power_on_right > 0.1,
        "right Modelica headlight load stayed at " + r2(s.power_on_right) +
        " W after the on command"));
    f.push(t_true(s.render_on > 1000.0 && (s.render_on - s.lights_on).abs() < 1.0,
        "USD light received " + r2(s.render_on) + " lm instead of the Modelica result " +
        r2(s.lights_on) + " lm"));
    f.push(t_true(s.render_on_right > 1000.0 &&
                 (s.render_on_right - s.lights_on_right).abs() < 1.0,
        "right USD light received " + r2(s.render_on_right) +
        " lm instead of the Modelica result " + r2(s.lights_on_right) + " lm"));
    f.push(t_true(s.lights_off.abs() < 0.001 && s.power_off.abs() < 0.001,
        "Modelica headlight off command left intensity=" + r2(s.lights_off) +
        " lm and power=" + r2(s.power_off) + " W"));
    f.push(t_true(s.lights_off_right.abs() < 0.001 && s.power_off_right.abs() < 0.001,
        "right Modelica headlight off command left intensity=" + r2(s.lights_off_right) +
        " lm and power=" + r2(s.power_off_right) + " W"));
    f.push(t_true(s.render_off.abs() < 0.001,
        "USD light stayed at " + r2(s.render_off) + " lm after the off command"));
    f.push(t_true(s.render_off_right.abs() < 0.001,
        "right USD light stayed at " + r2(s.render_off_right) +
        " lm after the off command"));

    print("[6w] ── results ───────────────────────────────────────────");
    print("[6w]   straight travel " + r2(s.dist) + " m");
    print("[6w]   yaw straight    " + r2(s.yaw_straight) + " deg (must be small)");
    print("[6w]   yaw under steer " + r2(s.yaw_steer) + " deg (must be clearly +)");
    print("[6w]   camera height   " + r2(s.camera.height) + " m");
    print("[6w]   camera gap err  " + r4(s.camera_gap_error) + " m");
    print("[6w]   camera fwd dot  " + r4(s.camera_fwd_min_dot));
    print("[6w]   headlight on    " + r2(s.lights_on) + " lm / " + r2(s.power_on) + " W");
    print("[6w]   headlight on R  " + r2(s.lights_on_right) + " lm / " +
          r2(s.power_on_right) + " W");
    print("[6w]   headlight off   " + r2(s.lights_off) + " lm / " + r2(s.power_off) + " W");
    print("[6w]   headlight off R " + r2(s.lights_off_right) + " lm / " +
          r2(s.power_off_right) + " W");

    report_verdict(f, "SIX WHEEL", "SIX_WHEEL");
}
