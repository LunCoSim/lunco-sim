// ── Rocker-bogie differential — on the real rover, over a step ──────────────
//
// TEST-ONLY. Drives BOTH scenes that share this rig; which case a stage is comes
// from its own `lunco:param:coupled`, not from a second copy of this file:
//
//   scenes/tests/rocker_bogie.usda         differential ON  → rockers mirror
//   scenes/tests/rocker_bogie_nodiff.usda  differential OFF → independent
//
// WHAT IT GUARDS. This is `differential_rig_test` promoted from the isolated
// stand to the vehicle: same invariant, but now with six wheels, four bogie
// hinges and a step under the left-front foot all pulling on it. A linkage that
// mirrors on a two-rocker test stand and quietly does nothing on the rover is
// exactly the failure the stand cannot see.
//
// The step is what makes the test mean anything. On flat ground both rockers sit
// at ~0 and `L + R ≈ 0` holds for a rover with no differential at all — so the
// left front foot is lifted, forcing the left rocker to pitch, and the question
// becomes whether the RIGHT one answers.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("ROCKER_BOGIE", "PASS"|"FAIL")`.

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 10.0;   // drop onto the step, then ring down
    this.sample_s = 1.0;
    this.next_sample = 0.0;

    this.hl = find("/RockerBogieTest/Rover/HingeL");
    this.hr = find("/RockerBogieTest/Rover/HingeR");
    this.rover = find("/RockerBogieTest/Rover");

    this.coupled = param(me, "coupled", -1.0);
    this.peak_l = 0.0;
    this.phase = "settle";

    print("[rb] ── rocker-bogie differential over a step ────────────────");
    print("[rb] coupled=" + this.coupled + "  hingeL=" + this.hl + " hingeR=" + this.hr);
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.hl == () || this.hl < 0 || this.hr == () || this.hr < 0 {
        fail_fast("rocker hinges not found — wrong scene?", "ROCKER BOGIE", "ROCKER_BOGIE");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let al = get(this.hl, "angle");
    let ar = get(this.hr, "angle");
    if al != () && al.abs() > this.peak_l { this.peak_l = al.abs(); }

    if this.t >= this.next_sample {
        print("[rb] t=" + r2(this.t) + "s  L=" + r4(al) + "  R=" + r4(ar) +
              "  tilt=" + r2(tilt_deg(this.rover)) + "deg");
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{ l: al, r: ar, peak_l: this.peak_l, coupled: this.coupled,
                   tilt: tilt_deg(this.rover), pos: world_pos(this.rover) });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   rover bounded    a diverging suspension throws the vehicle out of the scene.
//   the step bit     the left rocker must have PITCHED. On flat ground every
//                    check below passes for a rover with no differential at all;
//                    this is what makes the rest mean something.
//   coupled ⇒ L+R≈0  the authored invariant, and the linkage's whole purpose.
//   coupled ⇒ level  the reason a rover carries a differential: the chassis
//                    splits the difference instead of following one side.
//   uncoupled ⇒ R≈0  the control. Nothing drives the right rocker, so it must
//                    stay put while the left one is forced up the step.
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.pos, 200.0, "rover"));
    f.push(t_present(s.l, "HingeL angle port"));
    f.push(t_present(s.r, "HingeR angle port"));

    f.push(t_true(s.coupled >= 0.0,
        "the scene declared no `coupled` parameter, so this test does not know " +
        "which case it is running and would assert the wrong one"));
    f.push(t_true(s.peak_l > 0.02,
        "the left rocker never pitched (peak |L| = " + r4(s.peak_l) + " rad) — the " +
        "step did not load the suspension, so nothing below distinguishes a " +
        "working differential from an absent one"));

    if s.l != () && s.r != () {
        if s.coupled > 0.5 {
            // ── STILL ON THE STEP AT THE VERDICT ────────────────────────────
            // `peak_l` is a RUNNING MAXIMUM, so it is satisfied by the landing
            // transient alone — a rover that settles onto the step and then creeps
            // OFF it reads "level chassis, L + R ~ 0" at t = 10 and passes for the
            // same reason a rover with no differential would: nothing is loading
            // it. That is not hypothetical. It is what the rover did while its hull
            // carried `linearDamping = 0.4` (invented air drag, since removed): it
            // slid off at t ~ 8 and this test called it green.
            //
            // So assert the load is STILL THERE when the verdict is taken. The
            // uncoupled branch already has this, in its `tilt > 10` control.
            // 0.25, not 0.5: `peak_l` includes the LANDING TRANSIENT, which
            // overshoots the settled angle, so half of it is a bar a genuinely
            // loaded rover can miss (measured 0.0148 against a 0.0299 peak — off by
            // 0.00015). The failure this exists to catch is not marginal at all:
            // an unloaded rover reads |L| ≈ 0.000, i.e. ~2% of peak.
            f.push(t_true(s.l.abs() > 0.25 * s.peak_l,
                "coupled: the step must still be loading the suspension at the " +
                "verdict — |L| = " + r4(s.l.abs()) + " against a peak of " +
                r4(s.peak_l) + ", so the rover has come off the step and every " +
                "check below is being read on an unloaded, trivially level rover"));
            // Sum, not ratio: the authored invariant is `L + R ~ 0`, and near zero
            // a ratio test is noise.
            //
            // 15% of the excursion actually driven. That figure briefly went to 25%
            // when removing the hull's invented air drag exposed the EXPLICIT
            // penalty's true residual (20%), and came back the moment the coupling
            // was re-solved implicitly: it now measures 13.4% (L+R = 0.004 rad
            // against a 0.0299 excursion) on a genuinely loaded rover, which is
            // better than the number this bound was originally written against.
            //
            // The control does not cancel AT ALL (L+R = 1.047 against a peak of
            // 0.5236, i.e. 200%), so the two cases are separated by 15×.
            f.push(t_true((s.l + s.r).abs() < 0.15 * s.peak_l,
                "coupled: HingeL + HingeR must cancel, got " + r4(s.l + s.r) +
                " rad against a peak |L| of " + r4(s.peak_l) +
                " — the differential is not transferring the step to the far side"));
            f.push(t_range(s.tilt, 0.0, 8.0,
                "coupled: chassis tilt over the step (deg) — a working " +
                "differential splits the difference instead of following one side"));
        } else {
            // The control is the CONTRAPOSITIVE, not "the far side stays put".
            // On the isolated stand nothing drives the far rocker, so it does. On
            // the rover BOTH rockers carry weight: with the differential dead they
            // simply sag together, and the chassis follows one side down. What
            // must fail is the cancellation itself.
            f.push(t_true((s.l + s.r).abs() > 0.5 * s.peak_l,
                "uncoupled control: HingeL + HingeR must NOT cancel with the " +
                "differential dead, got " + r4(s.l + s.r) + " against a peak |L| of " +
                r4(s.peak_l) + " — if the rockers still mirror with no coupling, " +
                "the coupled case is measuring something else"));
            f.push(t_true(s.tilt > 10.0,
                "uncoupled control: the chassis must NOT stay level without a " +
                "differential, got " + r2(s.tilt) + " deg — a rover that rides a " +
                "step level with the linkage dead has some other thing holding it"));
        }
    }

    print("[rb] ── results ────────────────────────────────────────────");
    print("[rb]   coupling  " + s.coupled);
    print("[rb]   L         " + r4(s.l) + " rad");
    print("[rb]   R         " + r4(s.r) + " rad");
    print("[rb]   L+R       " + r4(s.l + s.r) + " rad");
    print("[rb]   peak |L|  " + r4(s.peak_l) + " rad");
    print("[rb]   tilt      " + r2(s.tilt) + " deg");

    report_verdict(f, "ROCKER BOGIE", "ROCKER_BOGIE");
}
