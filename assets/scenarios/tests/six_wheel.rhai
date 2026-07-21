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
// The failure this guards is a loader that falls back to INDEX PARITY when the
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

fn on_start(me) {
    this.t = 0.0;
    this.phase_start = 0.0;
    this.settle_s = 2.0;    // let it sit down on its wheels
    this.drive_s = 8.0;     // straight leg
    this.steer_s = 8.0;     // steering leg
    this.sample_s = 2.0;
    this.next_sample = 0.0;

    this.rover = find("/SixWheelTest/Rover");

    this.dist_straight = 0.0;
    this.yaw_straight = 0.0;
    this.yaw_steer = 0.0;
    // `yaw_delta_deg` answers () when the orientation is degenerate. A rover whose
    // heading was NEVER readable would finish with both accumulators at 0.0 and
    // sail through "drove straight" — so the samples are counted, and a run with
    // none is a failure rather than a free pass.
    this.yaw_ok = 0;

    this.pos_prev = ();
    this.fprev = ();
    this.phase = "settle";

    print("[6w] ── authored wheel→port bindings ─────────────────────────");
    print("[6w] rover gid=" + this.rover);
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.rover == () || this.rover < 0 {
        fail_fast("rover prim not found (gid=" + this.rover + ") — wrong scene?",
                  "SIX WHEEL", "SIX_WHEEL");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let leg = this.t - this.phase_start;

    let p = world_pos(this.rover);
    let fwd = world_forward(this.rover);
    let dyaw = ();
    if this.fprev != () && fwd != () {
        dyaw = yaw_delta_deg(this.fprev, fwd);
    }
    if dyaw != () { this.yaw_ok += 1; }

    if this.phase == "settle" {
        if leg >= this.settle_s {
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
            this.phase = "done";
            verdict(#{
                dist: this.dist_straight,
                yaw_straight: this.yaw_straight,
                yaw_steer: this.yaw_steer,
                yaw_ok: this.yaw_ok,
                pos: p,
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
//   +steer yaws RIGHT the assertion that index parity cannot pass. Wired by
//                     parity the banks receive equal commands and the rover keeps
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
        "turn RIGHT by a clear margin. Near zero is what INDEX PARITY wiring " +
        "produces on a six-wheel layout: both banks get the same average command " +
        "and the rover can only go straight. Negative means the banks are swapped."));

    print("[6w] ── results ───────────────────────────────────────────");
    print("[6w]   straight travel " + r2(s.dist) + " m");
    print("[6w]   yaw straight    " + r2(s.yaw_straight) + " deg (must be small)");
    print("[6w]   yaw under steer " + r2(s.yaw_steer) + " deg (must be clearly +)");

    report_verdict(f, "SIX WHEEL", "SIX_WHEEL");
}
