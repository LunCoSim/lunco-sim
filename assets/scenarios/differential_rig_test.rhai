// ── Differential linkage — the two sides mirror, or they do not ─────────────
//
// TEST-ONLY. Drives BOTH scenes that share this rig, and which one it is running
// is read from the scene, not from a second copy of this file:
//
//   scenes/sandbox/differential_rig_test.usda         coupling ON  → mirrored
//   scenes/sandbox/differential_rig_nodiff_test.usda  coupling OFF → independent
//
// WHAT IT GUARDS. A rocker-bogie rover keeps its chassis level by coupling the
// left and right rockers through a differential: push one side up and the other
// goes down by the same angle. That is one authored `PhysicsDriveAPI` on the
// coupling joint, and it fails QUIETLY — a rig with the coupling dead still
// drops, still settles, still looks like a suspension. The only thing that
// changes is whether the far side answers, and nothing says so on screen.
//
// The control scene is half the test. "Both hinges read ≈0" passes a mirror
// check trivially, so a rig that never moved at all would look coupled. The
// no-coupling scene is what proves the measurement can tell the difference.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("DIFF_RIG", "PASS"|"FAIL")` — the telemetry verdict `scene_test` reads
// for its exit code. Assertion helpers come from `prelude/auto_tests.rhai`.

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 8.0;    // drop onto the stand, then ring down
    this.sample_s = 1.0;
    this.next_sample = 0.0;

    this.hl = find("/DiffRigTest/Rig/HingeL");
    this.hr = find("/DiffRigTest/Rig/HingeR");

    // WHICH scene this is, DECLARED by the scene rather than sniffed. Both stages
    // reference the same rig and differ only in whether the coupling drive is
    // live; each states which case it is as a parameter on its own program prim.
    // Inferring it from the drive's stiffness would make the test's expectation
    // depend on the very authoring it is checking.
    this.coupled = param(me, "coupled", -1.0);

    this.peak_l = 0.0;
    this.phase = "settle";

    print("[diff] ── differential linkage ─────────────────────────────────");
    print("[diff] hingeL gid=" + this.hl + " hingeR gid=" + this.hr +
          "  coupling stiffness=" + this.coupled);
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.hl == () || this.hl < 0 || this.hr == () || this.hr < 0 {
        fail_fast("hinge prims not found (L=" + this.hl + " R=" + this.hr +
                  ") — wrong scene?", "DIFFERENTIAL RIG", "DIFF_RIG");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let al = get(this.hl, "angle");
    let ar = get(this.hr, "angle");

    // The driven side's largest excursion, so "it never moved" cannot pass as
    // "it mirrored".
    if al != () && al.abs() > this.peak_l { this.peak_l = al.abs(); }

    if this.t >= this.next_sample {
        print("[diff] t=" + r2(this.t) + "s  L=" + r4(al) + " rad  R=" + r4(ar) +
              " rad  peak|L|=" + r4(this.peak_l));
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{ l: al, r: ar, peak_l: this.peak_l, coupled: this.coupled });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   ports present   a revolute joint publishing no `angle` is not measurable,
//                   and a missing port read as 0.0 mirrors perfectly.
//   it articulated  the driven side must have MOVED. Every check below compares
//                   the two sides, and two motionless rockers satisfy all of
//                   them — this is the assertion that stops that.
//   coupled ⇒ R ≈ -L   the linkage's whole purpose, within 25%: the rig is a
//                   real mechanism with joint compliance, not an equation.
//   uncoupled ⇒ R ≈ 0  the control. With the drive dead the far rocker has
//                   nothing driving it and must stay put while L swings.
fn verdict(s) {
    let f = [];
    f.push(t_present(s.l, "HingeL angle port"));
    f.push(t_present(s.r, "HingeR angle port"));

    f.push(t_true(s.coupled >= 0.0,
        "the scene declared no `coupled` parameter on its program prim, so this " +
        "test does not know which case it is running and would assert the wrong one"));
    f.push(t_true(s.peak_l > 0.02,
        "the driven rocker never moved (peak |L| = " + r4(s.peak_l) + " rad) — " +
        "a rig at rest mirrors trivially, so nothing below would mean anything"));

    if s.l != () && s.r != () {
        if s.coupled > 0.5 {
            // 5% — `t_rel` takes a PERCENTAGE. The rig mirrors to ~0.5%, so this
            // is loose enough for joint compliance and tight enough that a
            // half-coupled linkage cannot slip through.
            f.push(t_rel(s.r, -s.l, 5.0,
                "coupled: HingeR must mirror HingeL (got R=" + r4(s.r) +
                " against -L=" + r4(-s.l) + ")"));
        } else {
            f.push(t_true(s.r.abs() < 0.25 * s.peak_l,
                "uncoupled control: HingeR must stay put with the drive dead, " +
                "got " + r4(s.r) + " rad against a peak |L| of " + r4(s.peak_l) +
                " — if this mirrors, the coupling is live when it should not be, " +
                "and the coupled case proves nothing"));
        }
    }

    print("[diff] ── results ───────────────────────────────────────────");
    print("[diff]   coupling  " + s.coupled);
    print("[diff]   L         " + r4(s.l) + " rad");
    print("[diff]   R         " + r4(s.r) + " rad");
    print("[diff]   peak |L|  " + r4(s.peak_l) + " rad");

    report_verdict(f, "DIFFERENTIAL RIG", "DIFF_RIG");
}
