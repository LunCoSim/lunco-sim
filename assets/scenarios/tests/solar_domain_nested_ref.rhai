// ── The solar variant, one arc deeper ───────────────────────────────────────
//
// Runs against `scenes/tests/solar_domain_nested_ref.usda`, which stands two
// rovers side by side with the SAME two variants selected:
//
//   /SolarDomainNestedRef/Direct   scene → base                 (one arc)
//   /SolarDomainNestedRef/Nested   scene → profile → base       (two arcs)
//
// `Direct` is the CONTROL. It is the arrangement that ships today and works;
// `Nested` is how a purchasable rover configuration is actually authored. The
// arc count is the only difference between them.
//
// WHAT IS READ. The electrical island's own ports, off the `Electrical` scope.
// `project_domain_islands` rejecting a network leaves NO `SimComponent` behind,
// so the symptom of the reported bug is not an exception, a missing prim or a
// stopped rover — it is `get(elec, "solar_power") == ()` on a rover that looks
// entirely healthy. That is exactly why it went unnoticed long enough to be
// discovered by a workshop build instead of by us.
//
// THE ANTI-TRIVIAL GATE. If the CONTROL has no island either, this rig cannot
// say anything about nesting — the solar variant would be broken at every depth.
// The scenario reports that as its own distinct failure rather than as a red on
// `Nested`, because the two demand different fixes and a report that confuses
// them sends the next person to the wrong file.
//
// Verdict: emit("SOLAR_DOMAIN_NESTED_REF", "PASS"|"FAIL").

/// Whether an electrical island actually exists on `elec`.
///
/// A rejected projection never wraps a `SimComponent`, so the port is absent
/// rather than zero. `()` is the answer this test is looking for; 0.0 would be a
/// panel in the dark, which is a different statement entirely.
fn island_live(elec) {
    if elec < 0 { return false; }
    get(elec, "solar_power") != ()
}

fn on_start(me) {
    this.t = 0.0;

    // Modelica for a generated island is compiled asynchronously, so the ports
    // appear some time after the scene composes. Long enough that a slow cold
    // compile is not read as a rejected island; the test finishes as soon as
    // BOTH sides are live, so the deadline only costs time on a real failure.
    this.deadline_s = 30.0;
    this.done = false;

    this.d_root = find("/SolarDomainNestedRef/Direct");
    this.n_root = find("/SolarDomainNestedRef/Nested");
    this.d_elec = find("/SolarDomainNestedRef/Direct/Electrical");
    this.n_elec = find("/SolarDomainNestedRef/Nested/Electrical");
    this.d_panel = find("/SolarDomainNestedRef/Direct/SolarPanel");
    this.n_panel = find("/SolarDomainNestedRef/Nested/SolarPanel");

    print("[solar-nested] ── the solar variant, one arc deeper ──────────");
    print("[solar-nested] direct  root=" + this.d_root + " elec=" + this.d_elec +
          " panel=" + this.d_panel);
    print("[solar-nested] nested  root=" + this.n_root + " elec=" + this.n_elec +
          " panel=" + this.n_panel);
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    // ── Structural guards ────────────────────────────────────────────────────
    // These are about PRIMS, not ports, so they are decidable immediately and
    // must not wait out the compile deadline. They also separate the two ways
    // this can go wrong: a variant that contributed no panel at all is a
    // different defect from a panel that composed but fell out of the
    // collection, and only the second one is what the workshop build reported.
    if this.d_root < 0 || this.n_root < 0 {
        this.done = true;
        fail_fast("a rover prim is missing (direct=" + this.d_root + " nested=" +
                  this.n_root + ") — the scene did not compose, so nothing about " +
                  "the solar domain was tested",
                  "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
        return;
    }
    if this.d_panel < 0 || this.n_panel < 0 {
        this.done = true;
        fail_fast("the generation=\"solar\" variant contributed no SolarPanel prim " +
                  "(direct=" + this.d_panel + " nested=" + this.n_panel + ") — the " +
                  "variant selection itself did not compose through the reference, " +
                  "which is a stronger failure than the collection membership this " +
                  "rig was written for",
                  "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
        return;
    }

    let d_live = island_live(this.d_elec);
    let n_live = island_live(this.n_elec);

    // Both up: decide now rather than burning the deadline.
    if !d_live || !n_live {
        if this.t < this.deadline_s { return; }
    }

    this.done = true;

    print("[solar-nested] direct  solar_power=" + get(this.d_elec, "solar_power") +
          " soc=" + get(this.d_elec, "soc"));
    print("[solar-nested] nested  solar_power=" + get(this.n_elec, "solar_power") +
          " soc=" + get(this.n_elec, "soc"));

    let d_word = if d_live { "yes" } else { "no" };
    let n_word = if n_live { "yes" } else { "no" };
    print("[solar-nested] t=" + r2(this.t) + "  direct_island=" + d_word +
          "  nested_island=" + n_word);

    // ── The anti-trivial gate ────────────────────────────────────────────────
    if !d_live {
        let why = "the CONTROL rover has no electrical island either, so the ";
        why += "profile's second reference arc is NOT the cause. A `prepend` on ";
        why += "`collection:components:includes` from a stronger layer is not ";
        why += "reaching the composed collection at ANY depth — the same failure ";
        why += "the shipped scenes/sandbox/solar_rover_demo.usda has, where the ";
        why += "membership is authored in the scene itself. Fix the relationship ";
        why += "list-op composition behind StageView::rel_targets (the ";
        why += "`explicitOnly` path of collection_members); re-authoring assets ";
        why += "cannot fix it, and this rig stays red until it is";
        fail_fast(why, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
        return;
    }

    let f = [];

    // ── The claim under test ─────────────────────────────────────────────────
    let msg = "the NESTED rover (scene → profile → base) has no electrical ";
    msg += "island while the identically-configured DIRECT rover (scene → base) ";
    msg += "does. The extra reference arc is the only difference: the solar ";
    msg += "variant's absolute-path `collection:components:includes` did not ";
    msg += "survive it, so project_domain_islands rejected the network";
    f.push(t_true(n_live, msg));

    // A live island must also carry a plausible number. `solar_power` is
    // authored nameplate × a fixed unit incidence, so it is large and positive
    // on both rovers; a live port reading 0.0 would mean the panel joined the
    // collection but contributes nothing, which is not a pass.
    let d_p = get(this.d_elec, "solar_power");
    f.push(t_true(d_p > 0.0,
        "direct solar_power = " + r2(d_p) + ", expected > 0 — the panel is in the " +
        "island but generating nothing"));

    if n_live {
        let n_p = get(this.n_elec, "solar_power");
        f.push(t_true(n_p > 0.0,
            "nested solar_power = " + r2(n_p) + ", expected > 0"));
        f.push(t_rel(n_p, d_p, 1.0,
            "nested solar_power (" + r2(n_p) + ") and direct (" + r2(d_p) + ") " +
            "disagree — same panel, same nameplate, so the profile changed " +
            "something it was written not to change"));

        // `soc` comes from the power variant, not the generation variant. It is
        // read so a pass says the whole island composed rather than just the one
        // member this bug is about.
        f.push(t_present(get(this.n_elec, "soc"),
            "nested soc is absent — the battery half of the island is missing too"));
    }

    report_verdict(f, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
}
