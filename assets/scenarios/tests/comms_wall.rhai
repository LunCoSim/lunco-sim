// ── Connectivity is occlusion, not distance ─────────────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/comms_wall.usda`.
//
// WHAT IT GUARDS. A link kernel that only checks RANGE passes every casual
// inspection: nodes near each other connect, distant ones do not, and the graph
// looks alive. What it cannot do is notice a wall. That failure is invisible
// exactly where it matters — a rover driving behind a ridge keeps its link, and
// the mission plan built on that link is wrong.
//
// The scene is built so occlusion is the ONLY variable:
//
//   Mast       z = -20, x =  0   sight-line crosses the wall     -> must be DOWN
//   MastClear  z = -20, x = +12  sight-line misses it by 2 m     -> must be UP
//   Rover      z = +20
//
// 40 m against 41.8 m. The BLOCKED link is the SHORTER one, so a range-only
// kernel cannot produce this pair of verdicts in any ordering — if range were
// what decided it, the blocked mast would be the one that stayed up.
//
// ⚠ The assertion is on the DIRECT EDGE, not on `reachable`. `reachable` is
// breadth-first over the whole graph, and the control mast is a perfectly good
// relay: rover → MastClear → Mast is two up links and answers "yes" while the
// blocked segment is severed exactly as it should be. Asking the reachability
// question here measures the relay, not the wall. (This file asked it first, and
// went red against a kernel that was doing its job.)
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("COMMS_WALL", "PASS"|"FAIL")` — the telemetry verdict `scene_test` reads
// for its exit code. Link queries come from `prelude/links.rhai`.

fn on_start(me) {
    this.t = 0.0;
    // The graph is rebuilt on its own cadence and the poses have to settle first;
    // a verdict taken on tick one measures a graph that has not been built.
    this.settle_s = 3.0;
    this.phase = "settle";
    print("[wall] ── connectivity is occlusion, not distance ───────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();
    if this.t < this.settle_s { return; }

    this.phase = "done";
    // Geometry, printed alongside the verdict: when a link that should be severed
    // is not, the first question is always "is the wall where the kernel thinks
    // the segment is", and the answer has to be in the log rather than in a
    // debugger.
    let wall = find("/CommsWallTest/Wall/Body");
    let ra = find("/CommsWallTest/Rover/Comms");
    let ma = find("/CommsWallTest/Mast/Antenna");
    print("[wall] wall body  gid=" + wall + " pos=" + world_pos(wall));
    print("[wall] rover ant  gid=" + ra + " pos=" + world_pos(ra));
    print("[wall] mast ant   gid=" + ma + " pos=" + world_pos(ma));

    let g = links();
    let peers = if g == () { [] } else { neighbours(g, ra) };
    verdict(#{
        graph: g,
        n_nodes: if g == () { -1 } else { g.nodes.len() },
        n_peers: peers.len(),
        blocked: ma in peers,
        clear: find("/CommsWallTest/MastClear/Antenna") in peers,
    });
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   graph exists    `links()` answers `()` when the provider is absent, and
//                   `reachable` then returns false for EVERYTHING — which would
//                   satisfy "the blocked link is down" perfectly. This check is
//                   what stops a missing provider reading as a pass.
//   nodes present   three link nodes, so both masts and the rover registered.
//   clear link UP   the control. Without it, "blocked is down" is equally true of
//                   a kernel that connects nothing at all.
//   blocked link DOWN  the thing under test: same rover, same tick, 1.8 m
//                   CLOSER, and the only difference is a wall on the segment.
fn verdict(s) {
    let f = [];
    f.push(t_true(s.graph != (),
        "query(\"Links\") answered nothing — the connectivity provider is absent, " +
        "so every reachability question below answers 'no' for a reason that has " +
        "nothing to do with the wall"));

    f.push(t_true(s.n_nodes >= 3,
        "the link graph carries " + s.n_nodes + " node(s); this scene authors " +
        "three (rover, base, base_clear) and a missing one cannot be occluded"));

    f.push(t_true(s.n_peers > 0,
        "the rover has no direct peers at all, so 'the blocked mast is not among " +
        "them' is true of a rover talking to nobody"));

    f.push(t_true(s.clear,
        "the CLEAR sight-line to MastClear is not a direct link. It is 41.8 m — " +
        "LONGER than the blocked one — and nothing stands on it, so either the " +
        "link range is too short for this scene or the kernel is connecting " +
        "nothing at all. Without this link up, the blocked verdict below proves " +
        "nothing."));

    f.push(t_true(!s.blocked,
        "the rover has a DIRECT link to the mast THROUGH the wall. Connectivity is " +
        "being decided by range alone: this is the shorter of the two baselines, " +
        "and the wall's `lunco:occluder` is not reaching the kernel."));

    print("[wall] ── results ──────────────────────────────────────────");
    print("[wall]   nodes            " + s.n_nodes + "  rover peers " + s.n_peers);
    print("[wall]   direct -> base   " + s.blocked + "  (40.0 m, wall on the line — must be false)");
    print("[wall]   direct -> clear  " + s.clear + "  (41.8 m, nothing on the line — must be true)");

    report_verdict(f, "COMMS WALL", "COMMS_WALL");
}
