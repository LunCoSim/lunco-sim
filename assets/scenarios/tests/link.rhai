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
//   the geodetic node's radius   the body's surface        placement resolved
//   the Kepler node's radius     its semi-major axis       placement resolved
//   every Earth<->Moon-surface edge 330,000-410,000 km     the Moon's true orbital
//                                                          band (perigee 356,500,
//                                                          apogee 406,700)
//   every edge's light time      range / c                 derived, not invented
//
// A node stranded at the origin by an unresolved placement sits ~1 AU from the
// Moon, so it can satisfy neither radius; and a light time that is a constant
// cannot track the range across edges of wildly different length.
//
// ── Why placement is asserted as a RADIUS and not as connectivity ────────────
//
// Because "this node has peers" is a claim about OTHER nodes' skies at one epoch,
// and this scene cannot make it true: the DSN complexes gate at 5 deg from their
// own end, Earth and L1 sit within ~±6.7 deg of an 85.4 deg S horizon depending
// on libration, L2 is behind the Moon by construction, and an equatorial orbiter
// at a = 6540 km can never clear that horizon at all (it needs a central angle
// under arccos(1737/6540) = 74.6 deg against an actual 85.4). A geodetic
// placement can resolve perfectly and still have no peers here. The radius says
// only what the file set out to check.
//
// The ShortRangeSat is the RANGE control, and it is measured on its OWN edges:
// its companion flies the same orbit 4 deg ahead (456 km, inside the 1000 km
// reach) while everything else in the scene is tens of thousands of km outside
// it. So the pair "links to the companion, never to Earth, no edge over 1000 km"
// pins `lunco:link:maxRangeM` without asking whether some third node happens to
// be visible. Asserting only "the short one has no Earth link" would pass on a
// kernel that connects nothing at all, which is why the companion link is
// asserted in the same breath.
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
    let name_of = #{};
    for n in g.nodes { name_of["" + n.id] = n.name; }
    let short = find("/LinkTest/ShortRangeSat");
    let companion = find("/LinkTest/ShortRangeCompanion");
    let earths = link_ids(g, "earth");
    let lunar_surface = link_ids(g, "base");

    // Did each placement RESOLVE? Measured against the body it was placed on, not
    // against who it can see: an unresolved placement leaves its node at the solar
    // ORIGIN, which puts it ~1 AU from the Moon — an error no epoch can produce.
    let moon = query("BodyPosition", #{ body: 301 });
    let base_r = radius_from(base, moon);
    let relay_r = radius_from(find("/LinkTest/Relay"), moon);

    // Every edge the short-range node is on. Its reach is 1000 km; the companion
    // flies 456 km away and everything else in the scene is tens of thousands of
    // km off, so this list IS the range gate's output.
    let short_edges = [];
    for e in g.edges {
        if e.a == short || e.b == short { short_edges.push(e.range_m); }
    }
    let short_max = 0.0;
    for r in short_edges {
        if r > short_max { short_max = r; }
    }

    // Cross-body Earth↔Moon-surface edges. L1/L2/L4 relays are intentionally
    // between the Earth and Moon and therefore have shorter/longer relay hops;
    // including them here would measure relay geometry while claiming to test
    // the Earth-Moon orbital band. The `base` class pins the actual lunar body
    // endpoint authored for this assertion.
    let cross = [];
    let lt_err = 0.0;
    for e in g.edges {
        let a_earth = e.a in earths;
        let a_surface = e.a in lunar_surface;
        let b_earth = e.b in earths;
        let b_surface = e.b in lunar_surface;
        if (a_earth && b_surface) || (b_earth && a_surface) {
            cross.push(e.range_m);
        }
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
        base_r: base_r,
        relay_r: relay_r,
        short_edges: short_edges.len(),
        short_max: short_max,
        short_to_companion: any_edge(g, short, [companion]),
        short_to_earth: any_edge(g, short, earths),
        moon_r: if moon == () { () } else { moon.radius },
    });
}

/// Distance in metres from `body`'s centre to `gid`'s solar-frame position, or `()`
/// when either is unresolvable. This is the arithmetic that separates a placement
/// that ran from one that silently did not: a node the pose system never wrote sits
/// at the solar origin, so its distance from the Moon is the Moon's own heliocentric
/// distance rather than a body radius or an orbit.
fn radius_from(gid, body) {
    if body == () { return (); }
    if !body.found { return (); }
    let p = query("SolarPose", #{ entity: gid });
    if p == () { return (); }
    if !p.found { return (); }
    let dx = p.pos[0] - body.pos[0];
    let dy = p.pos[1] - body.pos[1];
    let dz = p.pos[2] - body.pos[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
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
//   geodetic radius   the Malapert node sits on the Moon's surface — the geodetic
//                     placement resolved rather than leaving it at the origin.
//   Kepler radius     the lunar relay sits at its authored 6540 km semi-major
//                     axis, so the orbital placement resolved too.
//   cross-body range  330,000-410,000 km, the Moon's true orbital band, for the
//                     Earth↔surface edge only. Relay hops are separate geometry.
//   light time        range / c, to 1%. Cheap, and it catches a light time that
//                     is a constant rather than a derivation.
//   short-range sat   links to its 456 km companion, never to Earth, and holds no
//                     edge over its authored 1000 km. Read on its own edges, so
//                     the range gate is pinned without depending on any other
//                     node being visible at this epoch.
fn verdict(s) {
    let f = [];
    f.push(t_true(s.n_nodes >= 10,
        "the graph carries " + s.n_nodes + " node(s); this scene authors ten, " +
        "and a placement that failed to resolve is one that never registered"));
    f.push(t_true(s.n_edges > 0, "the graph has no edges at all"));
    f.push(t_true(s.n_earth == 3,
        "expected the three DSN complexes in class `earth`, found " + s.n_earth));

    // ── The SURFACE placement resolved ───────────────────────────────────────
    //
    // Asserted as a RADIUS, not as connectivity. Whether the base can see anything
    // is a question about other nodes' skies at this epoch, and this scene answers
    // it "no": the DSN complexes gate at 5 deg from THEIR end, Earth and L1 sit
    // within ~±6.7 deg of an 85.4 deg S horizon depending on libration, L2 is
    // behind the Moon by construction, and an equatorial orbiter at a = 6540 km
    // needs a central angle under arccos(1737/6540) = 74.6 deg to clear that
    // horizon against an actual 85.4 — so it is never visible, at any epoch.
    //
    // None of that is a placement failure, and a test that reads it as one is
    // asserting an epoch. The failure this file names in its own header is "an
    // unresolved placement leaves its node at the ORIGIN", so measure exactly
    // that: a resolved geodetic node sits on the body's surface. The tolerance is
    // wide enough for the ellipsoid at this latitude and still rejects the origin
    // by eight orders of magnitude.
    if s.base_r == () || s.moon_r == () {
        f.push(t_true(false,
            "the Malapert node has no solar-frame pose at all, so its geodetic " +
            "placement never resolved"));
    } else {
        f.push(t_true((s.base_r - s.moon_r).abs() < 10000.0,
            "the Malapert node sits " + km(s.base_r) + " km from the Moon's centre, " +
            "not on its surface (" + km(s.moon_r) + " km) — its geodetic placement " +
            "did not resolve"));
    }

    // The KEPLER placement, the same way: a circular orbit's radius IS its
    // semi-major axis, at every epoch.
    if s.relay_r == () {
        f.push(t_true(false,
            "the lunar relay has no solar-frame pose, so its Kepler placement " +
            "never resolved"));
    } else {
        f.push(t_true((s.relay_r - 6540000.0).abs() < 1000.0,
            "the lunar relay orbits at " + km(s.relay_r) + " km from the Moon's " +
            "centre; its authored circular orbit is 6540.0 km"));
    }

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

    // ── The RANGE gate ───────────────────────────────────────────────────────
    //
    // Read off ONE node's own edges, so no third node's visibility is smuggled in.
    // The companion is 456 km away and inside the reach; everything else in the
    // scene is tens of thousands of km outside it. Both halves are load-bearing:
    // without the companion link, "no Earth link" is equally true of a node that
    // never resolved, and without the ceiling it is equally true of a kernel that
    // connects nothing.
    f.push(t_true(s.short_to_companion,
        "the 1000 km-reach satellite has no link to its companion 456 km away, so " +
        "'it does not reach Earth' below is true of a node that links to nothing"));
    f.push(t_true(!s.short_to_earth,
        "the 1000 km-reach satellite has a direct link to a DSN complex ~384,000 " +
        "km away — `lunco:link:maxRangeM` is not being read"));
    f.push(t_true(s.short_max <= 1000000.0,
        "the 1000 km-reach satellite holds an edge of " + km(s.short_max) +
        " km, beyond its authored reach"));

    // Every number printed through `km()`: an ABSENT range is `()`, and dividing
    // `()` by 1000 THROWS — which kills the scenario between its last print and
    // `report_verdict`, and `scene_test` reports "no verdict" rather than the
    // failure that actually happened. This file did exactly that on its first run.
    print("[link] ── results ──────────────────────────────────────────");
    print("[link]   nodes " + s.n_nodes + "  edges " + s.n_edges + "  earth " + s.n_earth);
    print("[link]   cross-body  " + s.n_cross + " edge(s), " + km(s.cross_lo) +
          " to " + km(s.cross_hi) + " km (band " + km(PERIGEE_M) + "-" + km(APOGEE_M) + ")");
    print("[link]   light time  worst error " + r4(s.lt_err * 100.0) + "%");
    print("[link]   placement   base " + km(s.base_r) + " km from Moon centre (surface " +
          km(s.moon_r) + ")  relay " + km(s.relay_r) + " km (orbit 6540.0)");
    print("[link]   range gate  short-range sat: " + s.short_edges + " edge(s), longest " +
          km(s.short_max) + " km (reach 1000.0)  ->companion=" + s.short_to_companion +
          "  ->earth=" + s.short_to_earth);

    report_verdict(f, "LINK", "LINK");
}
