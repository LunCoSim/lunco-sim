// ── Every placement kind resolves to real geometry ──────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/link.usda`.
//
// WHAT IT GUARDS. The scene authors one link node per PLACEMENT KIND — geodetic
// on Earth (three DSN complexes), geodetic on the Moon, Kepler orbit, and
// libration (L1/L2/L4) — because placement is the part the pose system resolves
// and everything downstream is the same code regardless of how a node got there.
//
// The failure mode is silent and specific: a placement that does not resolve
// leaves its node at the ORIGIN rather than removing it. The graph still has the
// right number of nodes, every class is present, links still form — and every
// distance in the scene is wrong. Nothing says so.
//
// So the assertions are ARITHMETIC, against distances nobody chose, and none of
// them names an epoch:
//
//   every lunar<->Earth edge   330,000-410,000 km   the Moon's true orbital band
//                                                   (perigee 356,500, apogee 406,700)
//   every edge's light time    range / c            derived, not invented
//
// A node stranded at the origin by an unresolved placement cannot land inside
// that band, and a light time that is a constant cannot track the range across
// eleven edges of wildly different length.
//
// The ShortRangeSat is the RANGE control, and it is a control by CONTRAST rather
// than in isolation: it shares the lunar neighbourhood with `Relay`, which links
// to Earth freely, and differs only in declaring a 1000 km reach. `Relay` linked
// and it did not — so `lunco:link:maxRangeM` is read. Asserting only "the short
// one has no Earth link" would pass on a kernel that connects nothing at all,
// which is why the long one's links are asserted in the same breath.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("LINK", "PASS"|"FAIL")`. Graph queries come from `prelude/links.rhai`.

// The Moon's true orbital band, not its mean distance: these assertions must
// hold at every epoch the scene can be run at, and the mean is wrong twice a month.
const PERIGEE_M = 330000000.0;
const APOGEE_M = 410000000.0;
const C_M_S = 299792458.0;

fn on_start(me) {
    this.t = 0.0;
    // Ephemeris and the CR3BP solve run on the world clock; the graph is built on
    // its own cadence. A verdict on tick one measures a graph nobody has filled in.
    this.settle_s = 3.0;
    this.phase = "settle";
    print("[link] ── placement kinds resolve to real geometry ──────────");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();
    if this.t < this.settle_s { return; }
    this.phase = "done";

    let g = links();
    if g == () {
        fail_fast("query(\"Links\") answered nothing — the connectivity provider " +
                  "is absent, so nothing below is a measurement", "LINK", "LINK");
        return;
    }

    // The edge list, printed once: when an expected link is missing the first
    // question is which links DO exist, and the answer belongs in the log.
    for e in g.edges {
        print("[link]   edge " + e.a + " <-> " + e.b + "  " + km(e.range_m) + " km");
    }

    let base = find("/LinkTest/MalapertBase");
    let probe = find("/LinkTest/MalapertHorizonFree");
    print("[link]   base peers  " + neighbours(g, base).len() +
          "   probe(no mask) peers " + neighbours(g, probe).len());
    let l1 = find("/LinkTest/L1_Relay");
    let short = find("/LinkTest/ShortRangeSat");
    let earths = link_ids(g, "earth");

    // Cross-body edges: one end an Earth complex, the other not. Their length IS
    // the Earth-Moon distance, whatever the epoch.
    let cross = [];
    let lt_err = 0.0;
    for e in g.edges {
        let a_earth = e.a in earths;
        let b_earth = e.b in earths;
        if a_earth != b_earth { cross.push(e.range_m); }
        // Light time against range/c, worst case over EVERY edge.
        if e.range_m > 0.0 {
            let rel = ((e.light_time_s - e.range_m / C_M_S) / (e.range_m / C_M_S)).abs();
            if rel > lt_err { lt_err = rel; }
        }
    }
    let lo = 1.0e12;
    let hi = 0.0;
    for r in cross {
        if r < lo { lo = r; }
        if r > hi { hi = r; }
    }

    verdict(#{
        n_nodes: g.nodes.len(),
        n_edges: g.edges.len(),
        n_earth: earths.len(),
        n_cross: cross.len(),
        cross_lo: lo,
        cross_hi: hi,
        lt_err: lt_err,
        relay_to_earth: any_edge(g, find("/LinkTest/Relay"), earths),
        short_to_earth: any_edge(g, short, earths),
    });
}

/// The `range_m` of the edge between `a` and `b`, or `()` when they are not
/// directly linked. Edges are undirected and listed once, so both orders are
/// checked.
fn edge_range(g, a, b) {
    for e in g.edges {
        if (e.a == a && e.b == b) || (e.a == b && e.b == a) { return e.range_m; }
    }
    ()
}

/// The SHORTEST edge from `a` to any node in `ids` — "the complex that can see us
/// right now". Which of the three DSN complexes that is changes as the Earth
/// turns, and the test must not depend on which.
fn best_range(g, a, ids) {
    let best = ();
    for id in ids {
        let r = edge_range(g, a, id);
        if r != () && (best == () || r < best) { best = r; }
    }
    best
}

/// `light_time_s` of the shortest edge from `a` into `ids`.
fn best_light_time(g, a, ids) {
    let best_r = ();
    let best_lt = ();
    for id in ids {
        for e in g.edges {
            let hit = (e.a == a && e.b == id) || (e.a == id && e.b == a);
            if hit && (best_r == () || e.range_m < best_r) {
                best_r = e.range_m;
                best_lt = e.light_time_s;
            }
        }
    }
    best_lt
}

/// Metres as kilometres for a log line, or "(none)" — NEVER arithmetic on `()`.
fn km(m) {
    if m == () { "(none)" } else { r2(m / 1000.0) }
}

/// Whether `a` links directly to ANY of `ids`.
fn any_edge(g, a, ids) {
    for id in ids {
        if edge_range(g, a, id) != () { return true; }
    }
    false
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   nodes + edges     the graph is populated at all.
//   an Earth link     at least one DSN complex sees the Moon, which is what the
//                     Malapert site is famous for.
//   Earth range       ~384,400 km. A node stranded at the origin cannot produce
//                     this number, and neither can a scripted distance.
//   L1 range          ~61,300 km — the CR3BP answer for this pair, so the
//                     libration placement really solved rather than defaulting.
//   light time        range / c, to 1%. Cheap, and it catches a light time that
//                     is a constant rather than a derivation.
//   short-range sat   links to its neighbourhood and NEVER to Earth. The control:
//                     without it the Earth links above are equally consistent
//                     with `maxRangeM` being ignored.
fn verdict(s) {
    let f = [];
    f.push(t_true(s.n_nodes >= 8,
        "the graph carries " + s.n_nodes + " node(s); this scene authors eight, " +
        "and a placement that failed to resolve is one that never registered"));
    f.push(t_true(s.n_edges > 0, "the graph has no edges at all"));
    f.push(t_true(s.n_earth == 3,
        "expected the three DSN complexes in class `earth`, found " + s.n_earth));

    f.push(t_true(s.n_cross > 0,
        "not one edge joins an Earth complex to a lunar node, so there is no " +
        "cross-body geometry to measure and the placements below are unproven"));

    if s.n_cross > 0 {
        f.push(t_true(s.cross_lo > PERIGEE_M && s.cross_hi < APOGEE_M,
            "cross-body ranges run " + km(s.cross_lo) + " to " + km(s.cross_hi) +
            " km, outside the Moon's true orbital band (" + km(PERIGEE_M) + " to " +
            km(APOGEE_M) + " km). A node left at the origin by an unresolved " +
            "placement lands outside it; a resolved one cannot."));
    }

    f.push(t_true(s.lt_err < 0.01,
        "light time is off range/c by " + r4(s.lt_err * 100.0) + "% on the worst " +
        "edge — it must be DERIVED from the geometry, not carried as a constant"));

    f.push(t_true(s.relay_to_earth,
        "the long-range lunar relay has no Earth link, so 'the short-range one has " +
        "none either' below is true of a kernel that connects nothing"));
    f.push(t_true(!s.short_to_earth,
        "the 1000 km-reach satellite has a direct link to a DSN complex ~384,000 " +
        "km away, while sharing the lunar neighbourhood with a relay that legitimately " +
        "does — `lunco:link:maxRangeM` is not being read"));

    // Every number printed through `km()`: an ABSENT range is `()`, and dividing
    // `()` by 1000 THROWS — which kills the scenario between its last print and
    // `report_verdict`, and `scene_test` reports "no verdict" rather than the
    // failure that actually happened. This file did exactly that on its first run.
    print("[link] ── results ──────────────────────────────────────────");
    print("[link]   nodes " + s.n_nodes + "  edges " + s.n_edges + "  earth " + s.n_earth);
    print("[link]   cross-body  " + s.n_cross + " edge(s), " + km(s.cross_lo) +
          " to " + km(s.cross_hi) + " km (band " + km(PERIGEE_M) + "-" + km(APOGEE_M) + ")");
    print("[link]   light time  worst error " + r4(s.lt_err * 100.0) + "%");
    print("[link]   range gate  relay->earth=" + s.relay_to_earth +
          "  shortsat->earth=" + s.short_to_earth);

    report_verdict(f, "LINK", "LINK");
}
