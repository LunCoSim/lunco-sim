// ── A USD-authored linear drive holds its target ────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/prismatic_drive.usda`.
//
// WHAT IT GUARDS. `PhysicsDriveAPI:linear` authored on a prismatic joint must be
// live AT LOAD, with no wire and no script: the Platform hangs 2 m below a
// kinematic Base, free to slide along Y, and the drive alone must pull it to
// `targetPosition` and HOLD it there against gravity. Drop the drive on the
// floor of the loader and nothing errors — the Platform simply slides down its
// own slider, which from a screenshot is indistinguishable from a suspension.
//
// Then the same joint is commanded through its cosim port. A drive that only
// works when authored, or only when written, is half a mechanism.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("PRISMATIC_DRIVE", "PASS"|"FAIL")`.

const AUTHORED_TARGET = -1.5;   // physics:targetPosition on the Lift drive
const COMMANDED = -0.5;         // what the port is asked for, mid-run

fn on_start(me) {
    this.t = 0.0;
    this.hold_s = 5.0;      // let the authored drive settle
    this.command_s = 10.0;  // then command the port and let THAT settle
    this.sample_s = 1.0;
    this.next_sample = 0.0;

    this.lift = find("/SandboxScene/Lift");
    this.platform = find("/SandboxScene/Platform");
    this.authored = ();
    this.commanded = ();
    this.sent = false;
    this.phase = "authored";

    print("[drive] ── prismatic drive: authored, then commanded ──────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.lift == () || this.lift < 0 {
        fail_fast("Lift joint not found — wrong scene?", "PRISMATIC DRIVE", "PRISMATIC_DRIVE");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let x = get(this.lift, "displacement");

    if this.t >= this.next_sample {
        print("[drive] t=" + r2(this.t) + "s  displacement=" + r4(x) + " m");
        this.next_sample = this.t + this.sample_s;
    }

    // Sample the authored hold, THEN command the port. Reading before writing is
    // what keeps the two claims independent.
    if this.phase == "authored" && this.t >= this.hold_s {
        this.authored = x;
        set(this.lift, "displacement", COMMANDED);
        this.sent = true;
        this.phase = "commanded";
        print("[drive] commanded displacement=" + COMMANDED);
    }

    if this.t >= this.command_s {
        this.phase = "done";
        this.commanded = x;
        verdict(#{
            authored: this.authored,
            commanded: this.commanded,
            sent: this.sent,
            pos: world_pos(this.platform),
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   port present   a joint publishing no `displacement` is not measurable.
//   authored hold  within 10% of the authored target. A dropped drive leaves the
//                  Platform free-falling down its slider, which this catches by
//                  metres, and a drive too weak to hold shows up as sag.
//   commanded      the port must MOVE it — and to the commanded value, not just
//                  somewhere else.
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.pos, 100.0, "platform"));
    f.push(t_present(s.authored, "Lift `displacement` port"));
    f.push(t_true(s.sent, "the scenario never issued its port write"));

    if s.authored != () {
        f.push(t_rel(s.authored, AUTHORED_TARGET, 10.0,
            "the AUTHORED drive did not hold its target — `PhysicsDriveAPI:linear` " +
            "must be live at load, with no wire and no script"));
    }
    if s.commanded != () {
        f.push(t_rel(s.commanded, COMMANDED, 10.0,
            "the `displacement` port did not move the joint to its commanded value"));
    }

    print("[drive] ── results ───────────────────────────────────────────");
    print("[drive]   authored hold  " + r4(s.authored) + " m (target " + AUTHORED_TARGET + ")");
    print("[drive]   commanded      " + r4(s.commanded) + " m (asked " + COMMANDED + ")");

    report_verdict(f, "PRISMATIC DRIVE", "PRISMATIC_DRIVE");
}
