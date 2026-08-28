// ── A fixed joint holds a body, then DetachJoint releases it ────────────────
//
// TEST-ONLY. Runs against `scenes/tests/joint.usda`.
//
// WHAT IT GUARDS. The most basic thing a joint does, isolated from every
// mechanism: `PhysicsFixedJoint` welds `CubeB` (dynamic) to `CubeA` (kinematic,
// therefore immovable), 4 m apart and 5 m above the ground. First the test
// proves the authored joint holds B. It then sends the same public `DetachJoint`
// command used by the editor/API and proves B falls after the live joint is
// removed.
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
    this.drop_s = 3.0;      // enough time for B to land after detachment
    this.sample_s = 1.0;
    this.next_sample = 0.0;
    this.detach_ack = false;
    this.pre_detach_gap = ();

    this.a = find("/SandboxScene/CubeA");
    this.b = find("/SandboxScene/CubeB");
    this.free = find("/SandboxScene/FreeCube");
    this.hinge = find("/SandboxScene/Hinge");

    this.phase = "settle";
    print("[joint] ── a fixed joint holds a body up ────────────────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.a == () || this.a < 0 || this.b == () || this.b < 0 ||
            this.free == () || this.free < 0 || this.hinge == () || this.hinge < 0 {
        fail_fast("joint fixture not found (A=" + this.a + " B=" + this.b +
                  " free=" + this.free + " hinge=" + this.hinge + ") — wrong scene?",
                  "JOINT", "JOINT");
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

    if this.phase == "settle" && this.t >= this.settle_s {
        this.pre_detach_b_pos = world_pos(this.b);
        if this.pre_detach_b_pos == () {
            fail_fast("CubeB position unavailable before DetachJoint", "JOINT", "JOINT");
            this.phase = "done";
            return;
        }

        // This is the production command path: Rhai -> reflected public command
        // -> live joint observer. The post-command physics response is checked
        // below, so an acknowledgement alone cannot make this test pass.
        let result = cmd("DetachJoint", #{
            target: this.hinge,
            // This scene checks the live interactive action; persistence has
            // its own authored-layer command contract.
            intent: "Interactive",
        });
        if !result.ok {
            fail_fast("DetachJoint rejected: " + result.error, "JOINT", "JOINT");
            this.phase = "done";
            return;
        }
        this.detach_ack = true;
        this.pre_detach_gap = distance(this.a, this.b);
        this.detached_at = this.t;
        this.phase = "detached";
        print("[joint] DetachJoint accepted; waiting for CubeB to fall");
        return;
    }

    if this.phase == "detached" && this.t >= this.detached_at + this.drop_s {
        this.phase = "done";
        verdict(#{
            a_pos: world_pos(this.a),
            pre_detach_b_pos: this.pre_detach_b_pos,
            post_detach_b_pos: world_pos(this.b),
            free_pos: world_pos(this.free),
            pre_detach_gap: this.pre_detach_gap,
            detach_ack: this.detach_ack,
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   witness fell    the anti-trivial guard, asserted FIRST because everything
//                   below is a statement about a body NOT moving, and a frozen
//                   world satisfies all of them.
//   B held up       still at its authored height before the command.
//   B released      falls after the real DetachJoint command; an acknowledgement
//                   without a live physics change does not satisfy this check.
//   spacing kept    still 4 m from A before detachment. A joint that resolved but
//                   seated wrongly moves B without dropping it, which the height
//                   alone misses.
fn verdict(s) {
    let f = [];
    f.push(t_true(s.detach_ack, "DetachJoint did not acknowledge"));
    f.push(t_present(s.pre_detach_b_pos, "CubeB position before DetachJoint"));
    f.push(t_present(s.post_detach_b_pos, "CubeB position after DetachJoint"));

    f.push(t_true(s.free_pos[1] < SPAWN_Y - 1.0,
        "the unjointed witness cube is still at y=" + r4(s.free_pos[1]) +
        " after " + 4.0 + "s — nothing is falling, so the world is not simulating " +
        "and 'CubeB stayed up' below would be true of a frozen scene"));

    f.push(t_true(s.pre_detach_b_pos[1] > SPAWN_Y - 0.5,
        "CubeB was already low before DetachJoint (y=" + r4(s.pre_detach_b_pos[1]) +
        ") — the authored fixed joint did not hold it"));

    f.push(t_true(s.post_detach_b_pos[1] < SPAWN_Y - 1.0,
        "CubeB did not fall after DetachJoint (y=" + r4(s.post_detach_b_pos[1]) +
        ") — the command was acknowledged but the live joint still held it"));

    f.push(t_true(s.post_detach_b_pos[1] < GROUND_TOP + 1.0,
        "CubeB did not reach the ground after detachment (y=" +
        r4(s.post_detach_b_pos[1]) + ")"));

    f.push(t_rel(s.pre_detach_gap, SEPARATION, 5.0,
        "CubeA and CubeB were not their authored distance apart before detachment (m) — " +
        "the joint resolved but did not seat where it was authored"));

    print("[joint] ── results ──────────────────────────────────────────");
    print("[joint]   CubeB     y=" + r4(s.pre_detach_b_pos[1]) + " -> " +
          r4(s.post_detach_b_pos[1]) + " after DetachJoint");
    print("[joint]   witness   y=" + r4(s.free_pos[1]) + " (must have fallen)");
    print("[joint]   |A-B|     " + r4(s.pre_detach_gap) + " m before detach (authored " +
          SEPARATION + ")");

    report_verdict(f, "JOINT", "JOINT");
}
