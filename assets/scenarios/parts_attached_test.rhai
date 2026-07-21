// ── Nothing falls off the vehicle ───────────────────────────────────────────
//
// TEST-ONLY. Runs against `scenes/sandbox/parts_attached.usda`.
//
// WHAT IT GUARDS. That every PART of a vessel is still carried by that vessel
// after it has settled and driven. The failure it exists for is silent in every
// other gate: a component asset applies `PhysicsRigidBodyAPI`, the loader makes
// that prim its own rigid body (`lunco-usd-avian`: "one spelling of this is a
// body" — applied schema, ancestry irrelevant), nothing ever joints it to the
// hull, and the part simply drops out of the vehicle. The rover still drives,
// still steers, still hits its top speed and passes every parity test; the only
// symptom is hardware lying on the regolith where it started.
//
// That is precisely how all four drive MOTORS came off every rover in the
// sandbox scene: `components/mobility/motor.usda` carried
// `PhysicsRigidBodyAPI`, no joint targeted any of them, and their housings are
// deliberately collider-less, so they fell through the hull and out.
//
// ── WHAT IS MEASURED, AND WHY IT IS A RADIUS ────────────────────────────────
//
// For every ECS descendant of every rover root: r = |p_part − p_root|, sampled
// once after the vehicle has settled and again after a long straight drive. The
// check is |r_end − r_0|.
//
// A radius is invariant under the vehicle's own translation AND its yaw, so a
// spinning wheel, a steering knuckle and a stroking suspension all leave it
// alone, while a part left standing on the ground changes it by the entire
// length of the drive. No list of part names appears in this file — the walk is
// over `children()`, so a part added to one of these assets tomorrow is covered
// by this test today.
//
// ── WHY THE ROVERS MUST DRIVE ───────────────────────────────────────────────
//
// A dropped part falls only ~0.3 m before it lands, in lunar gravity, and a
// stationary rover keeps sitting next to it: the radius barely changes and a
// static check would call the wrong thing a pass. Driving away turns the same
// defect into a metres-scale signal. The drive also has to be PROVEN to have
// happened, so each rover's own travel is asserted — otherwise a dead command
// surface would make "nothing separated" vacuously true.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("PARTS_ATTACHED", "PASS"|"FAIL")` — the telemetry verdict `scene_test`
// turns into its exit code. Assertion and report helpers come from the prelude
// (`prelude/auto_tests.rhai`).

// Separation that counts as "came off". The loosest LEGITIMATE motion in these
// assets is the physical drivetrain's suspension piston, measured at 0.30 m over
// this drive; the defect this test exists for moves a part by 19–26 m. Half a
// metre sits an order of magnitude clear of both, so neither solver noise nor a
// stiffer spring can flip the verdict.
const TOL_M = 0.5;

/// Every ECS descendant of `root`, as `#{ id, label }`. `name()` already yields
/// the full prim path, so the label needs no joining. Iterative rather than
/// recursive, with a hard visit cap: a hierarchy cycle in a broken scene must
/// fail this test, not hang the sim.
fn descendants(root) {
    let out = [];
    let stack = [];
    for c in children(root) { stack.push(c); }
    let guard = 0;
    while stack.len() > 0 {
        guard += 1;
        if guard > 400 { break; }
        let e = stack.pop();
        out.push(#{ id: e, label: name(e) });
        for c in children(e) { stack.push(c); }
    }
    out
}

/// Distance between two `world_pos` samples, or `()` if either is missing.
fn radius(a, b) {
    if a == () || b == () { return (); }
    vlen(vsub(a, b))
}

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 3.0;    // land, stop bouncing, and let the loader's
                            // kinematic → dynamic settle finish
    this.drive_s = 12.0;    // full throttle, straight — tens of metres
    this.sample_s = 2.0;
    this.next_sample = 0.0;
    this.phase = "settle";

    this.paths = [
        "/PartsAttached/SkidRaycast",
        "/PartsAttached/SkidPhysical",
        "/PartsAttached/AckermannRaycast",
        "/PartsAttached/AckermannPhysical",
    ];
    this.roots = [];
    for p in this.paths {
        this.roots.push(find_or_none(p));
    }

    // Filled at the end of the settle phase: one entry per part, holding the
    // baseline radius. `parts[i]` belongs to `roots[i]`.
    this.parts = [];
    this.start_pos = [];    // rover position at baseline, for the travel check
    this.captured = false;

    print("[parts] ── nothing falls off the vehicle ────────────────────────");
    let i = 0;
    for p in this.paths {
        print("[parts] " + p + " gid=" + this.roots[i]);
        i += 1;
    }
    print("[parts] tolerance " + TOL_M + " m on |p_part - p_root| after a " +
          this.drive_s + "s drive");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing rover is the classic silent no-op — fail on tick one.
    for r in this.roots {
        if r == () {
            fail_fast("rover prims not found — wrong scene? expected " +
                      "scenes/sandbox/parts_attached.usda",
                      "PARTS ATTACHED", "PARTS_ATTACHED");
            this.phase = "done";
            return;
        }
    }

    this.t += dt();

    if this.phase == "settle" {
        for r in this.roots { brake(r); }
        if this.t >= this.settle_s {
            // Baseline every part's radius from its rover, INLINE: a rhai
            // function takes its arguments by value, so a helper that filled
            // `this.parts` would fill a copy and the run would measure nothing.
            //
            // Taken AFTER settling rather than at t=0 so landing, the loader's
            // kinematic→dynamic handover and suspension sag are already in the
            // baseline and cannot be mistaken for a part coming loose.
            let i = 0;
            for r in this.roots {
                let rp = world_pos(r);
                this.start_pos.push(rp);
                let list = [];
                for p in descendants(r) {
                    let e = #{ id: p.id, label: p.label, r0: radius(world_pos(p.id), rp) };
                    list.push(e);
                }
                this.parts.push(list);
                print("[parts] " + this.paths[i] + ": " + list.len() + " parts baselined");
                i += 1;
            }
            this.phase = "drive";
        }
        return;
    }

    // Straight, full throttle, no steer: the parts that are still bolted on
    // travel with the hull, and the ones that are not stay where they fell.
    for r in this.roots { drive(r, 1.0, 0.0); }

    if this.t >= this.next_sample {
        this.next_sample = this.t + this.sample_s;
        let i = 0;
        for r in this.roots {
            let d = radius(world_pos(r), this.start_pos[i]);
            print("[parts] t=" + r2(this.t) + "s  " + this.paths[i] +
                  " travelled " + r2(d) + " m");
            i += 1;
        }
    }

    if this.t >= this.settle_s + this.drive_s {
        this.phase = "done";
        for r in this.roots { brake(r); }
        verdict(#{
            paths: this.paths,
            roots: this.roots,
            parts: this.parts,
            start_pos: this.start_pos,
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   parts found     a rover whose children never resolved measures nothing, and
//                   an empty walk would otherwise pass every check below.
//   it drove        without motion a separated part is indistinguishable from a
//                   bolted one, so "nothing came off" would be vacuous.
//   nothing moved   the headline: no descendant's radius from its rover may
//                   change by more than TOL_M over the drive.
fn verdict(s) {
    let f = [];
    let worst = [];

    let i = 0;
    for r in s.roots {
        let rp = world_pos(r);
        let travelled = radius(rp, s.start_pos[i]);
        f.push(t_moved(travelled, 5.0, s.paths[i] + " travel (m)"));

        let list = s.parts[i];
        f.push(t_true(list.len() > 0,
            s.paths[i] + ": no child parts found — the walk measured nothing"));

        let loose = 0;
        let max_d = 0.0;
        let max_label = "";
        for p in list {
            if p.r0 == () { continue; }
            let r1 = radius(world_pos(p.id), rp);
            if r1 == () {
                f.push(p.label + ": no world_pos at the end of the run — despawned?");
                continue;
            }
            let d = (r1 - p.r0).abs();
            if d > max_d { max_d = d; max_label = p.label; }
            if d > TOL_M {
                loose += 1;
                let msg = p.label + " CAME OFF: |p-root| moved " +
                          r2(d) + " m (" + r2(p.r0) + " -> " + r2(r1) + " m)";
                f.push(msg);
            }
        }
        worst.push(#{ path: s.paths[i], loose: loose, max_d: max_d,
                      label: max_label, n: list.len() });
        i += 1;
    }

    print("[parts] ── results ────────────────────────────────────────────");
    for w in worst {
        let row = "[parts]   " + w.path;
        row += "  parts=" + w.n;
        row += "  came_off=" + w.loose;
        row += "  worst=" + r2(w.max_d) + " m (" + w.label + ")";
        print(row);
    }

    report_verdict(f, "PARTS ATTACHED", "PARTS_ATTACHED");
}
