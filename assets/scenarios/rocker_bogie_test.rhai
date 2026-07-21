// ── Rocker-bogie differential — on the real rover, over a step ──────────────
//
// TEST-ONLY. Drives BOTH scenes that share this rig; which case a stage is comes
// from its own `lunco:param:coupled`, not from a second copy of this file:
//
//   scenes/sandbox/rocker_bogie_test.usda         differential ON  → rockers mirror
//   scenes/sandbox/rocker_bogie_nodiff_test.usda  differential OFF → independent
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
            // Sum, not ratio: the authored invariant is `L + R ~ 0`, and near zero
            // a ratio test is noise. 15% of the excursion actually driven.
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
