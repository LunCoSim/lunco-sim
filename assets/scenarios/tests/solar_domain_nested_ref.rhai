// ── The solar electrical island, at one arc and at two ──────────────────────
//
// Runs against `scenes/tests/solar_domain_nested_ref.usda`, which stands three
// rovers side by side:
//
//   /SolarDomainNestedRef/BatteryOnly  battery bus, NO panel        (baseline)
//   /SolarDomainNestedRef/Direct       + solar, scene → base        (one arc)
//   /SolarDomainNestedRef/Nested       + solar, scene → profile → base (two arcs)
//
// WHAT IS READ. The generated island's ports, off the `Electrical` scope. They
// are named by INSTANCE, not by the network's authored `outputs:` — a member's
// variable surfaces as `<mangled-network-root>_<member>.<var>`, e.g.
//
//     SolarDomainNestedRef_x2f_BatteryOnly_x2f_Battery.soc_out
//
// (`get(elec, "soc")` reads nothing, whatever `outputs:soc` says on the prim.)
// An island that never steps publishes no port at all, so absence is the signal.
//
// WHAT THIS RIG HAS ESTABLISHED, in the order the layers came off:
//
//   1. The domain projection rejected every solar rover with `SolarPanel is
//      outside collection Electrical`. Cause: a sibling `over "SolarPanel"`
//      erased the `def` that referenced the component, so the panel composed
//      with no `LunCoProgramAPI` and `read_network` skipped it. Assets fixed;
//      both islands now project from 8 components and compile.
//   2. The reference-arc count was never the problem. `Direct` and `Nested`
//      have behaved identically at every stage. The workshop report's
//      "nested profile-reference composition defect" does not exist.
//   3. What remains is the SOLVER: adding the panel to the battery bus makes
//      the DAE unsolvable by the rk45 backend —
//
//        rk45 backend only supports a narrow explicit ODE subset: algebraic
//        refresh row 2 cannot be solved for `…Battery.p.i`: the residual does
//        not depend on it
//
//      The baseline rover, same bus without the panel, steps and publishes.
//      That is why the baseline is in the scene: it is the difference between
//      "generated islands never work" and "the solar source is what breaks it".
//
// Verdict: emit("SOLAR_DOMAIN_NESTED_REF", "PASS"|"FAIL").

/// A member variable's port name on the island that owns it.
///
/// `rover` is the rover prim's name; the network root is always its `Electrical`
/// child, and the mangling replaces each `/` with `_x2f_`.
fn port_name(rover, member, var) {
    "SolarDomainNestedRef_x2f_" + rover + "_x2f_" + member + "." + var
}

/// Whether an island is stepping — i.e. publishing the member port at all.
fn island_live(elec, rover, member, var) {
    if elec < 0 { return false; }
    get(elec, port_name(rover, member, var)) != ()
}

fn on_start(me) {
    this.t = 0.0;

    // Modelica compiles asynchronously, so ports appear after the scene
    // composes. The test decides as soon as every island is up, so the deadline
    // only costs wall clock on a real failure.
    this.deadline_s = 30.0;
    this.done = false;

    this.b_elec = find("/SolarDomainNestedRef/BatteryOnly/Electrical");
    this.d_elec = find("/SolarDomainNestedRef/Direct/Electrical");
    this.n_elec = find("/SolarDomainNestedRef/Nested/Electrical");
    this.d_panel = find("/SolarDomainNestedRef/Direct/SolarPanel");
    this.n_panel = find("/SolarDomainNestedRef/Nested/SolarPanel");

    print("[solar-nested] ── the solar electrical island, at one arc and at two ──");
    print("[solar-nested] elec  battery=" + this.b_elec + " direct=" + this.d_elec +
          " nested=" + this.n_elec);
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    // Structural guard, decidable immediately: the panel prim must compose, and
    // must arrive as the referenced COMPONENT rather than as the husk a sibling
    // `over` leaves behind. This is the regression that produced the original
    // `outside collection` error, and it is a different fix from anything below.
    if this.d_panel < 0 || this.n_panel < 0 {
        this.done = true;
        fail_fast("the generation=\"solar\" variant contributed no SolarPanel prim " +
                  "(direct=" + this.d_panel + " nested=" + this.n_panel + ")",
                  "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
        return;
    }

    let b_live = island_live(this.b_elec, "BatteryOnly", "Battery", "soc_out");
    let d_live = island_live(this.d_elec, "Direct", "SolarPanel", "power_out");
    let n_live = island_live(this.n_elec, "Nested", "SolarPanel", "power_out");

    if !b_live || !d_live || !n_live {
        if this.t < this.deadline_s { return; }
    }

    this.done = true;

    let words = if b_live { "battery=up" } else { "battery=DOWN" };
    words += if d_live { " direct=up" } else { " direct=DOWN" };
    words += if n_live { " nested=up" } else { " nested=DOWN" };
    print("[solar-nested] t=" + r2(this.t) + "  " + words);

    // ── The anti-trivial gate ────────────────────────────────────────────────
    // The baseline carries no panel, so it is unaffected by everything this rig
    // is about. If IT is down, generated electrical islands are broken outright
    // and neither solar result means anything.
    if !b_live {
        let why = "the BASELINE battery island (no panel) publishes no ports " +
                  "either, so generated electrical islands are not working at " +
                  "all and nothing here is a statement about solar. Fix that " +
                  "first — the two solar rovers cannot be read until it is up";
        fail_fast(why, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
        return;
    }

    let f = [];

    // ── The claims under test ────────────────────────────────────────────────
    let m_solar = "the solar island does not step: adding the panel to a bus " +
                  "that works without it makes the DAE unsolvable by the rk45 " +
                  "backend (`algebraic refresh row 2 cannot be solved for " +
                  "Battery.p.i`). The baseline proves the island machinery is " +
                  "fine; the photovoltaic source is what it cannot handle";
    f.push(t_true(d_live, "direct (one reference arc): " + m_solar));
    f.push(t_true(n_live, "nested (two reference arcs): " + m_solar));

    // Arc count is a SEPARATE claim from whether solar works at all. Asserted on
    // its own so a future regression that breaks only the profile path is not
    // hidden behind the solver failure both rovers currently share.
    f.push(t_true(d_live == n_live,
        "the two rovers disagree (direct=" + d_live + " nested=" + n_live + ") — " +
        "identical variant selection, so the reference-arc count has started to " +
        "matter, which it never has at any earlier stage of this rig"));

    if d_live && n_live {
        let d_p = get(this.d_elec, port_name("Direct", "SolarPanel", "power_out"));
        let n_p = get(this.n_elec, port_name("Nested", "SolarPanel", "power_out"));
        f.push(t_true(d_p > 0.0,
            "direct panel power_out = " + r2(d_p) + ", expected > 0"));
        f.push(t_rel(n_p, d_p, 1.0,
            "nested panel power_out (" + r2(n_p) + ") and direct (" + r2(d_p) +
            ") disagree — same component, same nameplate"));
    }

    report_verdict(f, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
}
