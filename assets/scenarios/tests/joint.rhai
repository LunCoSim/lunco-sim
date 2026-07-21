// ── A fixed joint holds a body up ───────────────────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/joint.usda`.
//
// WHAT IT GUARDS. The most basic thing a joint does, isolated from every
// mechanism: `PhysicsFixedJoint` welds `CubeB` (dynamic) to `CubeA` (kinematic,
// therefore immovable), 4 m apart and 5 m above the ground. Held, B hangs in the
// air at its authored place forever. Dropped — an unsupported joint kind is
// DISCARDED at load, and a discarded joint leaves its body free — B falls 5 m and
// lands. There is nothing else in the scene to confuse the two.
//
// The scene's `FreeCube` is the anti-trivial guard, and it is the whole reason
// this test means anything: an unjointed twin of B, at the same height, which
// MUST land. "B did not fall" is a sentence a frozen world satisfies perfectly.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("JOINT", "PASS"|"FAIL")` — the telemetry verdict `scene_test` reads for
// its exit code. Helpers come from `prelude/auto_tests.rhai`.

const SPAWN_Y = 5.0;        // both cubes' authored height
const SEPARATION = 4.0;     // A at x=-2, B at x=+2
const GROUND_TOP = 0.0;     // ground surface; a landed 1 m cube rests at y=0.5

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 4.0;    // free fall from 5 m under lunar gravity is ~2.5 s
    this.sample_s = 1.0;
    this.next_sample = 0.0;

    this.a = find("/SandboxScene/CubeA");
    this.b = find("/SandboxScene/CubeB");
    this.free = find("/SandboxScene/FreeCube");

    this.phase = "settle";
    print("[joint] ── a fixed joint holds a body up ────────────────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.a == () || this.a < 0 || this.b == () || this.b < 0 || this.free == () || this.free < 0 {
        fail_fast("cubes not found (A=" + this.a + " B=" + this.b + " free=" +
                  this.free + ") — wrong scene?", "JOINT", "JOINT");
        this.phase = "done";
        return;
    }

    this.t += dt();

    if this.t >= this.next_sample {
        let pb = world_pos(this.b);
        let pf = world_pos(this.free);
        print("[joint] t=" + r2(this.t) + "s  B.y=" + r4(pb[1]) + "  free.y=" +
              r4(pf[1]) + "  |A-B|=" + r4(distance(this.a, this.b)));
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{
            a_pos: world_pos(this.a),
            b_pos: world_pos(this.b),
            free_pos: world_pos(this.free),
            gap: distance(this.a, this.b),
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   witness fell    the anti-trivial guard, asserted FIRST because everything
//                   below is a statement about a body NOT moving, and a frozen
//                   world satisfies all of them.
//   B held up       still at its authored height. A dropped joint puts it on the
//                   ground, 4.5 m lower — no tolerance can blur that.
//   spacing kept    still 4 m from A. A joint that resolved but seated wrongly
//                   moves B without dropping it, which the height alone misses.
fn verdict(s) {
    let f = [];
    f.push(t_present(s.b_pos, "CubeB world position"));

    f.push(t_true(s.free_pos[1] < SPAWN_Y - 1.0,
        "the unjointed witness cube is still at y=" + r4(s.free_pos[1]) +
        " after " + 4.0 + "s — nothing is falling, so the world is not simulating " +
        "and 'CubeB stayed up' below would be true of a frozen scene"));

    f.push(t_true(s.b_pos[1] > SPAWN_Y - 0.5,
        "CubeB fell to y=" + r4(s.b_pos[1]) + " from its authored " + SPAWN_Y +
        " — the fixed joint did not hold it, which is what a joint DISCARDED at " +
        "load looks like (the body is simply free)"));

    f.push(t_true(s.b_pos[1] > GROUND_TOP + 1.0,
        "CubeB is resting on the ground (y=" + r4(s.b_pos[1]) + ") — it is meant " +
        "to hang in the air held by nothing but the joint"));

    f.push(t_rel(s.gap, SEPARATION, 5.0,
        "CubeA and CubeB are no longer their authored distance apart (m) — the " +
        "joint resolved but did not seat where it was authored"));

    print("[joint] ── results ──────────────────────────────────────────");
    print("[joint]   CubeB     y=" + r4(s.b_pos[1]) + " (authored " + SPAWN_Y + ")");
    print("[joint]   witness   y=" + r4(s.free_pos[1]) + " (must have fallen)");
    print("[joint]   |A-B|     " + r4(s.gap) + " m (authored " + SEPARATION + ")");

    report_verdict(f, "JOINT", "JOINT");
}
