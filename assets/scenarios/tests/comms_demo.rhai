// ── A rover reaches Earth THROUGH a relay, not directly ─────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/comms_demo.usda`.
//
// WHAT IT GUARDS. `link.usda` proves the placements resolve and `comms_wall.usda`
// proves occlusion is real. Neither exercises the thing this scene is for:
// ROUTING. A rover in a polar site talks to Earth by hops — antenna to mast or
// relay satellite, relay to a DSN complex — and the useful question autonomy asks
// is not "is there an edge" but "is there a PATH".
//
// The failure that hides here is a reachability query that only looks one hop out.
// It answers "no" for a rover that is perfectly well connected through its relay,
// and a mission plan built on that answer keeps a rover parked waiting for a link
// it already has. Nothing logs it: no link is down, no schema is wrong.
//
// So the assertion is a MULTI-HOP path, and its shape:
//
//   the rover has NO direct Earth edge   (else the path proves nothing about hops)
//   `link_path("rover", "earth")` is >= 3 nodes long, ends on an Earth complex
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("COMMS_DEMO", "PASS"|"FAIL")`. Graph queries from `prelude/links.rhai`.

fn on_start(me) {
    this.t = 0.0;
    // Ephemeris, orbit and the graph cadence all have to land first; a verdict on
    // tick one measures a graph nobody has filled in.
    this.settle_s = 3.0;
    this.phase = "settle";
    print("[demo] ── routing: rover -> relay -> Earth ─────────────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();
    if this.t < this.settle_s { return; }
    this.phase = "done";

    let g = links();
    if g == () {
        fail_fast("query(\"Links\") answered nothing — the connectivity provider " +
                  "is absent, so nothing below is a measurement",
                  "COMMS DEMO", "COMMS_DEMO");
        return;
    }

    let rover = find("/CommsDemoScene/Rover/Comms");
    let earths = link_ids(g, "earth");
    let peers = neighbours(g, rover);
    let direct = false;
    for id in earths {
        if id in peers { direct = true; }
    }

    let path = link_path("rover", "earth");
    for n in g.nodes {
        print("[demo]   node " + n.id + "  " + n.name + "  [" + n.class + "]");
    }
    print("[demo]   path " + link_path_names("rover", "earth"));

    verdict(#{
        n_nodes: g.nodes.len(),
        n_peers: peers.len(),
        direct: direct,
        hops: path.len(),
        ends_at_earth: if path.is_empty() { false } else { path[path.len() - 1] in earths },
    });
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   the rover is connected  it has peers at all. Every statement below is about
//                           the SHAPE of its connectivity, and a rover talking to
//                           nobody makes "no direct Earth link" true for free.
//   no direct Earth edge    what makes this a routing test rather than a range
//                           test. If the rover could see a DSN complex itself,
//                           the path below would be one hop and prove nothing.
//   a path exists           at least three nodes: rover, one or more relays, and
//                           an Earth complex.
//   it ends on Earth        BFS returning SOMETHING is not BFS returning what was
//                           asked for.
fn verdict(s) {
    let f = [];
    f.push(t_true(s.n_nodes >= 6,
        "the graph carries " + s.n_nodes + " node(s); this scene authors six " +
        "(rover, mast, relay, three DSN complexes)"));

    f.push(t_true(s.n_peers > 0,
        "the rover has no peers at all — it is talking to nobody, which makes " +
        "'no direct Earth link' below true for the wrong reason"));

    f.push(t_true(!s.direct,
        "the rover has a DIRECT edge to a DSN complex, so the path below is one " +
        "hop and this scene is not testing routing at all. Check the rover " +
        "antenna's `maxRangeM` — an unbounded range makes every relay redundant."));

    f.push(t_true(s.hops >= 3,
        "link_path(rover, earth) returned " + s.hops + " node(s); a routed link " +
        "through a relay is at least three (rover, relay, complex). Zero means BFS " +
        "found no route at all for a rover that has peers."));

    f.push(t_true(s.ends_at_earth,
        "the path does not end on an Earth complex — BFS returned a route to " +
        "somewhere other than what was asked for"));

    print("[demo] ── results ──────────────────────────────────────────");
    print("[demo]   nodes " + s.n_nodes + "  rover peers " + s.n_peers);
    print("[demo]   direct Earth edge  " + s.direct + " (must be false)");
    print("[demo]   path length        " + s.hops + " node(s), ends on earth=" + s.ends_at_earth);

    report_verdict(f, "COMMS DEMO", "COMMS_DEMO");
}
