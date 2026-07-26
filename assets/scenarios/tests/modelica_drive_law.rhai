// ── Modelica drive law — is the MODEL actually driving the rover? ───────────
//
// TEST-ONLY. Runs against `scenes/tests/modelica_drive_law.usda`, which puts the
// same skid rover on the same ground twice: one with
// `components/mobility/drive_laws/modelica_skid.usda` referenced onto its root,
// one with nothing overlaid.
//
// WHAT IT GUARDS. `LunCo.Mobility.RoverDrivetrain` had NO coverage of any kind.
// Its component file says a `driveLaw = "modelica"` variant references it in,
// but no rover authors that variantSet, so nothing composed it and no scene
// selected it. The model could have been deleted outright and every gate in the
// repo would have stayed green — which is precisely what let it be moved between
// packages with nothing able to notice.
//
// ── WHY A CONTROL, AND WHY THIS PARTICULAR MEASUREMENT ──────────────────────
//
// "The rover moved" proves nothing here. The drive-law overlay works by setting
// `lunco:driveKernel = "external"` to stand the built-in mixing down and wiring
// the model's outputs onto the drive ports. If the program failed to compile, or
// the kernel stand-down failed, the built-in kernel would drive the rover
// anyway — it would move, reach the right speed, and look completely healthy.
// A liveness check cannot tell those apart.
//
// What CAN tell them apart is that the two laws differ in their dynamics:
//
//   built-in kernel:  mixes instantly, torque steps with the command
//   RoverDrivetrain:  der(tl) = (cmd_l - tl) / tau_m,  tau_m = 0.15 s
//
// A first-order lag on the command is, at steady state, a pure time delay of
// `tau_m`. So the Modelica rover ends up permanently BEHIND the control by
// about `tau_m * v_terminal` in distance, while running at the SAME speed. That
// pair — "same speed, persistent distance deficit" — is a signature the built-in
// kernel cannot produce, because it has no state to lag with.
//
// A deficit of ZERO therefore means the model is not driving. That is the whole
// test, and it is why the deficit has a lower bound rather than only an upper
// one.

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 3.0;    // land and stop bouncing
    this.drive_s = 10.0;    // full throttle, straight — long enough to plateau
    this.plateau_s = 3.0;   // trailing window averaged as v_terminal
    this.sample_s = 1.0;    // log cadence

    // The authored lag under test (`tau_m` in RoverDrivetrain.mo). Kept here so
    // the expected deficit is derived from the model's own constant rather than
    // from a number fitted to whatever the rig happened to do.
    this.tau_m = 0.15;

    this.phase = "settle";
    this.next_sample = 0.0;

    this.md = find("/ModelicaDriveLaw/RoverModelica");
    this.bi = find("/ModelicaDriveLaw/RoverBuiltin");

    this.d_md = 0.0;  this.d_bi = 0.0;
    this.p_md = ();   this.p_bi = ();
    this.vsum_md = 0.0; this.vsum_bi = 0.0; this.vn = 0;
    this.vterm_md = 0.0; this.vterm_bi = 0.0;

    let banner = "[drivelaw] modelica=" + this.md;
    banner += " builtin=" + this.bi;
    print(banner);
    if this.md < 0 || this.bi < 0 {
        print("  FAIL: a rover prim was not found - the scene did not compose");
        print("TESTS_FAIL 1/1");
        print("MODELICA DRIVE LAW: FAIL");
        emit("MODELICA_DRIVE_LAW", "FAIL");
    }
}

fn on_tick(me) {
    if this.md < 0 || this.bi < 0 { return; }
    this.t += dt();
    let t = this.t;

    let pos_md = world_pos(this.md);
    let pos_bi = world_pos(this.bi);

    // Speed from POSITION DELTA. The reference is advanced EVERY tick, before
    // any phase can return — advancing it only inside the driving phase makes
    // this divide accumulated displacement by one tick's dt, which reads as a
    // rover doing several hundred m/s.
    let d_t = dt();
    let prev_md = this.p_md;
    let prev_bi = this.p_bi;
    let v_md = if d_t > 0.0 { seg(prev_md, pos_md) / d_t } else { 0.0 };
    let v_bi = if d_t > 0.0 { seg(prev_bi, pos_bi) / d_t } else { 0.0 };
    this.p_md = pos_md;
    this.p_bi = pos_bi;

    if this.phase == "settle" {
        brake(this.md);
        brake(this.bi);
        if t >= this.settle_s {
            this.phase = "drive";
            // Reset odometers here: the drop-and-settle is not travel, and
            // charging it to whichever rover bounced more would forge the very
            // deficit this test measures.
            this.d_md = 0.0;
            this.d_bi = 0.0;
            this.next_sample = t;
            print("[drivelaw] settled - FULL THROTTLE, straight, both rovers, same tick");
        }
        return;
    }

    if this.phase == "drive" {
        // Same command, same tick, both rovers. Steer stays 0: the lag is a
        // longitudinal property, and steering would mix yaw dynamics into the
        // distance comparison.
        drive(this.md, 1.0, 0.0);
        drive(this.bi, 1.0, 0.0);

        this.d_md += seg(prev_md, pos_md);
        this.d_bi += seg(prev_bi, pos_bi);

        let t_end = this.settle_s + this.drive_s;
        if t >= t_end - this.plateau_s {
            this.vsum_md += v_md;
            this.vsum_bi += v_bi;
            this.vn += 1;
        }

        if t >= this.next_sample {
            let s = "[drivelaw] t=" + r2(t) + "s";
            s += "  v_mod=" + r2(v_md);
            s += "  v_bi=" + r2(v_bi);
            s += "  d_mod=" + r2(this.d_md);
            s += "  d_bi=" + r2(this.d_bi);
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if t >= t_end {
            if this.vn > 0 {
                this.vterm_md = this.vsum_md / this.vn;
                this.vterm_bi = this.vsum_bi / this.vn;
            }
            brake(this.md);
            brake(this.bi);
            this.phase = "done";
            verdict(#{
                vterm_md: this.vterm_md, vterm_bi: this.vterm_bi,
                d_md:     this.d_md,     d_bi:     this.d_bi,
                tau_m:    this.tau_m
            });
        }
        return;
    }
}

fn on_stop(me) {
    let md = find("/ModelicaDriveLaw/RoverModelica");
    let bi = find("/ModelicaDriveLaw/RoverBuiltin");
    if md >= 0 { brake(md); }
    if bi >= 0 { brake(bi); }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
// TOLERANCES — why these numbers.
//
//   liveness        > 2 m    Both rovers must actually travel. 0 == 0 must never
//                            read as agreement; this is the anti-trivial gate.
//   terminal speed  +/-15%   The lag is a TRANSIENT. Once `tl` has caught up to
//                            the command both laws ask for the same torque, so a
//                            steady-state speed difference would mean the model
//                            got the gain wrong, not the dynamics.
//   deficit LOWER   > 0.05 m THE TEST. Zero deficit means the built-in kernel is
//                            driving both rovers and the overlay did nothing.
//   deficit UPPER   < 4x     Expected is `tau_m * v_term` (~0.32 m at 2.1 m/s).
//                            The band is wide because the deficit also absorbs
//                            the acceleration transient, but a deficit far above
//                            it means the model is not merely lagging — it is
//                            delivering the wrong torque.
fn verdict(s) {
    let f = [];

    f.push(t_true(s.d_bi > 2.0,
                  "control rover did not move (" + r2(s.d_bi) + " m) - dead command surface"));
    f.push(t_true(s.d_md > 2.0,
                  "modelica rover did not move (" + r2(s.d_md) + " m) - the drive law is not driving"));

    f.push(t_rel(s.vterm_md, s.vterm_bi, 15.0, "terminal speed (m/s)"));

    // The signature. Expected deficit is derived from the model's own `tau_m`
    // and the speed actually measured, not from a fitted constant.
    let expected = s.tau_m * s.vterm_bi;
    let deficit = s.d_bi - s.d_md;

    let m_zero = "NO lag: the modelica rover is not behind the control (deficit=";
    m_zero += r2(deficit) + " m). A first-order law MUST lag an instant one - ";
    m_zero += "this reads as the built-in kernel driving BOTH rovers, i.e. the ";
    m_zero += "drive-law overlay did nothing";
    f.push(t_true(deficit > 0.05, m_zero));

    let hi = expected * 4.0 + 0.5;
    let m_band = "distance deficit vs tau_m*v_term=" + r2(expected) + " m";
    f.push(t_range(deficit, 0.05, hi, m_band));

    print("[drivelaw] ── results ──────────────────────────────────────────");
    print("[drivelaw]   quantity           modelica    builtin");
    print("[drivelaw]   v_terminal (m/s)   " + r2(s.vterm_md) + "       " + r2(s.vterm_bi));
    print("[drivelaw]   distance   (m)     " + r2(s.d_md) + "       " + r2(s.d_bi));
    print("[drivelaw]   deficit    (m)     " + r2(deficit) + "  (expected ~" + r2(expected) + ")");

    report_verdict(f, "MODELICA DRIVE LAW", "MODELICA_DRIVE_LAW");
}
