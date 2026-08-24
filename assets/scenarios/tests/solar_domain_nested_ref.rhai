// ── The solar electrical island, at one arc and at two, at rest and under load ─
//
// Runs against `scenes/tests/solar_domain_nested_ref.usda`, which stands three
// rovers side by side:
//
//   /SolarDomainNestedRef/BatteryOnly  battery bus, NO panel        (baseline)
//   /SolarDomainNestedRef/Direct       + solar, scene → base        (one arc)
//   /SolarDomainNestedRef/Nested       + solar, scene → profile → base (two arcs)
//
// WHAT IS READ. The authored Electrical boundary ports, off the `Electrical`
// scope. An island that never steps publishes no port at all, so absence is
// the signal.
//
// WHAT THIS RIG ESTABLISHED, in the order the layers came off:
//
//   1. The domain projection rejected every solar rover with `SolarPanel is
//      outside collection Electrical`. Cause: a sibling `over "SolarPanel"`
//      erased the `def` that referenced the component, so the panel composed
//      with no `LunCoProgramAPI` and `read_network` skipped it. Five assets
//      fixed; both islands now project from 8 components and compile.
//   2. The reference-arc count was never the problem. `Direct` and `Nested`
//      have behaved identically at every stage. The workshop report's
//      "nested profile-reference composition defect" does not exist.
//   3. The panel and the motors were CONSTANT-POWER devices (`p.v * p.i` fixed),
//      which closes a nonlinear algebraic loop the rk45 backend cannot solve:
//      `algebraic refresh row 2 cannot be solved for …Battery.p.i`. Rewritten as
//      current sources — the panel sets `p.i` from irradiance, the motor sets
//      `p.i` from demand — the loop is linear in the unknowns and steps.
//   4. Live islands were resolving to an adaptive implicit backend that owns its
//      own step sequence, stalling the worker. Solver capability is now data
//      (`usable_live`), not a constant.
//
// ── WHY THIS DRIVES ──────────────────────────────────────────────────────────
//
// An earlier version of this file asserted only that the islands STEP, with the
// rovers parked. Parked means `demand = 0`, which means every motor's current is
// zero and the bus carries only the panel — so the constant-power loop of (3)
// was never entered and the test would have gone green against the exact defect
// it exists to catch. The load phase below is not extra coverage; without it the
// green is about a system that isn't being asked to do anything.
//
// Verdict: emit("SOLAR_DOMAIN_NESTED_REF", "PASS"|"FAIL").

const BATTERY_SOC = "soc";
const SOLAR_POWER = "solar_power";
const MOTOR_ELECTRICAL_POWER = "motor_electrical_power";

/// Whether an island is stepping — i.e. publishing the authored boundary port.
fn island_live(elec, port) {
    if elec < 0 { return false; }
    get(elec, port) != ()
}

/// A boundary value, or `0.0` when the port is absent. Only called after
/// `island_live` has confirmed the island publishes, so a zero here is a real
/// zero rather than a missing port.
fn port_or_zero(elec, port) {
    let v = get(elec, port);
    if v == () { 0.0 } else { v }
}

const ENVIRONMENT_WARMUP_S = 1.0;

fn on_start(me) {
    this.t = 0.0;

    // Modelica compiles asynchronously, so ports appear after the scene
    // composes. The rig moves on as soon as every island is up, so the deadline
    // only costs wall clock on a real failure.
    this.deadline_s = 30.0;
    this.phase = "wait";
    this.done = false;

    // Long enough for the current to establish and for the motors to spin up
    // under a demand step; short enough that the rovers stay on the flat pad and
    // roughly sun-facing, which is what keeps the panel comparison meaningful.
    this.drive_s = 4.0;
    this.drive_until = 0.0;
    this.traced = false;

    this.b_elec = find("/SolarDomainNestedRef/BatteryOnly/Electrical");
    this.d_elec = find("/SolarDomainNestedRef/Direct/Electrical");
    this.n_elec = find("/SolarDomainNestedRef/Nested/Electrical");
    this.d_panel = find("/SolarDomainNestedRef/Direct/SolarPanel");
    this.n_panel = find("/SolarDomainNestedRef/Nested/SolarPanel");

    // The bodies, for the drive phase. Distinct from the `Electrical` scopes
    // above: `drive` addresses the vehicle, `get` addresses its island.
    this.b_rover = find("/SolarDomainNestedRef/BatteryOnly");
    this.d_rover = find("/SolarDomainNestedRef/Direct");
    this.n_rover = find("/SolarDomainNestedRef/Nested");

    // Panel power sampled AT REST, before any motion tilts the array. The
    // direct-vs-nested equality claim is about composition, and comparing two
    // driving rovers would fold their attitudes into it.
    this.d_panel_rest = 0.0;
    this.n_panel_rest = 0.0;

    // Peak motor draw over the load phase, per rover. Peak rather than final:
    // the demand step is a transient and the interesting question is whether
    // ANY current flowed, not what it settled at.
    this.b_draw = 0.0;
    this.d_draw = 0.0;
    this.n_draw = 0.0;

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

    let b_live = island_live(this.b_elec, BATTERY_SOC);
    let d_live = island_live(this.d_elec, SOLAR_POWER);
    let n_live = island_live(this.n_elec, SOLAR_POWER);

    // ── Phase 1: wait for the three islands to publish ───────────────────────
    if this.phase == "wait" {
        if !b_live || !n_live || !d_live {
            if this.t < this.deadline_s { return; }

            // Deadline reached with something still down. Report which, and stop
            // — the load phase cannot say anything about an island that never
            // started, and driving into it would only add a second symptom.
            this.done = true;
            let words = if b_live { "battery=up" } else { "battery=DOWN" };
            words += if d_live { " direct=up" } else { " direct=DOWN" };
            words += if n_live { " nested=up" } else { " nested=DOWN" };
            print("[solar-nested] t=" + r2(this.t) + "  " + words);

            // ── The anti-trivial gate ────────────────────────────────────────
            // The baseline carries no panel, so it is unaffected by everything
            // this rig is about. If IT is down, generated electrical islands are
            // broken outright and neither solar result means anything.
            if !b_live {
                let why = "the BASELINE battery island (no panel) publishes no ";
                why += "ports either, so generated electrical islands are not ";
                why += "working at all and nothing here is a statement about ";
                why += "solar. Fix that first — the two solar rovers cannot ";
                why += "be read until it is up";
                fail_fast(why, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
                return;
            }

            let m_solar = "the solar island does not step within " +
                          r2(this.deadline_s) + "s: adding the panel to a bus " +
                          "that works without it broke it. The baseline proves " +
                          "the island machinery is fine; the photovoltaic " +
                          "source is what it cannot handle";
            let f = [];
            f.push(t_true(d_live, "direct (one reference arc): " + m_solar));
            f.push(t_true(n_live, "nested (two reference arcs): " + m_solar));
            report_verdict(f, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
            return;
        }

        // All three up. The environment provider may publish its first finite
        // mount-frame Sun vector after the generated islands become live. Keep
        // the rovers at rest until that startup projection has had a bounded
        // chance to arrive; otherwise a valid panel is sampled as zero on the
        // first participant tick and the test blames reference composition.
        if this.t < ENVIRONMENT_WARMUP_S { return; }

        // Take the at-rest panel reading, then load the bus.
        this.d_panel_rest = port_or_zero(this.d_elec, SOLAR_POWER);
        this.n_panel_rest = port_or_zero(this.n_elec, SOLAR_POWER);
        let panel_line = "[solar-nested] t=" + r2(this.t);
        panel_line += "  battery=up direct=up nested=up";
        panel_line += "  panel@rest direct=" + r2(this.d_panel_rest);
        panel_line += "W nested=" + r2(this.n_panel_rest) + "W";
        print(panel_line);

        this.phase = "drive";
        this.drive_until = this.t + this.drive_s;
        let load_line = "[solar-nested] loading the bus: full throttle straight for ";
        load_line += r2(this.drive_s) + "s";
        print(load_line);
        return;
    }

    // ── Phase 2: load the bus ────────────────────────────────────────────────
    // Demand reaches the motors through the island's BOUNDARY INPUTS
    // (`Electrical.inputs:drive_left|right`, wired from the rover's control
    // outputs). Those are the wires a too-broad phantom-wire filter once
    // dropped, leaving every motor at zero draw while the island still looked
    // healthy — which is precisely what a parked test cannot see.
    if this.phase == "drive" {
        drive(this.b_rover, 1.0, 0.0);
        drive(this.d_rover, 1.0, 0.0);
        drive(this.n_rover, 1.0, 0.0);

        // A MISSING port and a zero-valued one are different failures and get
        // different messages: absent means the island never published the motor
        // at all, zero means it published and no current flowed. Collapsing them
        // would report a wiring fault as a solver fault.
        if !island_live(this.b_elec, MOTOR_ELECTRICAL_POWER) {
            this.done = true;
            let missing = "the baseline island publishes no `" +
                          MOTOR_ELECTRICAL_POWER +
                          "` port at all, so no draw could be read from any rover";
            fail_fast(missing,
                      "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
            return;
        }

        let b = port_or_zero(this.b_elec, MOTOR_ELECTRICAL_POWER);
        let d = port_or_zero(this.d_elec, MOTOR_ELECTRICAL_POWER);
        let n = port_or_zero(this.n_elec, MOTOR_ELECTRICAL_POWER);

        // One-shot trace of the whole demand chain, printed on the first loaded
        // tick. When the draw comes back zero this line says WHERE it stopped:
        // at the vehicle, at the island boundary, or at the motor.
        // the vessel never took the command (`throttle` 0), the island never
        // received it (`island.drive_left` 0 under a live throttle — the actuator
        // -port read that this rig found broken), or the motor has it and the
        // solver produced nothing. Note `throttle` is read off the VESSEL and
        // `drive_left` off the ISLAND: the vessel's own `drive_left` lives on a
        // child `Port` entity and is not readable by name here.
        if !this.traced {
            this.traced = true;
            let trace = "[solar-nested] demand chain  rover.throttle=";
            trace += get(this.b_rover, "throttle");
            trace += "  island.drive_left=" + get(this.b_elec, "drive_left");
            trace += "  motor.electrical_power=" + b;
            print(trace);
        }
        if b > this.b_draw { this.b_draw = b; }
        if d > this.d_draw { this.d_draw = d; }
        if n > this.n_draw { this.n_draw = n; }

        if this.t < this.drive_until { return; }
    }

    this.done = true;
    let peak_line = "[solar-nested] t=" + r2(this.t);
    peak_line += "  peak motor draw  battery=" + r2(this.b_draw) + "W";
    peak_line += " direct=" + r2(this.d_draw) + "W nested=";
    peak_line += r2(this.n_draw) + "W";
    print(peak_line);

    // ── The second anti-trivial gate ─────────────────────────────────────────
    // The baseline's motors are on the same demand wiring as the solar rovers'.
    // If the BASELINE drew nothing under full throttle, the demand never reached
    // any island and the two solar zeroes below would be about the wiring, not
    // about solar. A test that reported "solar draws nothing" in that state
    // would be blaming the wrong subsystem, loudly.
    if this.b_draw <= 0.0 {
        let why = "the BASELINE rover (no panel) drew NO motor current under ";
        why += "full throttle, so `Electrical.inputs:drive_left|right` are ";
        why += "not reaching the motors on any rover and the solar readings ";
        why += "below measure the wiring, not the panel. Fix the boundary ";
        why += "inputs first";
        fail_fast(why, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
        return;
    }

    let f = [];

    // ── The claims under test ────────────────────────────────────────────────
    // 1. Both solar islands carry load. This is the constant-power loop: with
    //    the motors drawing, the panel, the battery and six motors are all
    //    solved together, which is the case that used to be unsolvable.
    let m_load = "motor drew no current under full throttle while the baseline " +
                 "on identical wiring did, so adding the panel to the bus stops " +
                 "the loaded island from solving (peak draw was ";
    f.push(t_true(this.d_draw > 0.0,
        "direct (one reference arc): " + m_load + r2(this.d_draw) + "W)"));
    f.push(t_true(this.n_draw > 0.0,
        "nested (two reference arcs): " + m_load + r2(this.n_draw) + "W)"));

    // 2. The panel generates. Read at rest, before attitude enters the picture.
    f.push(t_true(this.d_panel_rest > 0.0,
        "direct " + SOLAR_POWER + " at rest = " + r2(this.d_panel_rest) +
        "W, expected > 0"));

    // 3. Arc count is a SEPARATE claim from whether solar works at all, asserted
    //    on its own so a future regression that breaks only the profile path is
    //    not hidden behind a failure both rovers share. Same component, same
    //    nameplate, same attitude at rest — the numbers must agree.
    f.push(t_rel(this.n_panel_rest, this.d_panel_rest, 1.0,
        "nested " + SOLAR_POWER + " at rest (" + r2(this.n_panel_rest) +
        "W) and direct (" + r2(this.d_panel_rest) + "W) disagree — same " +
        "component, same nameplate, so the reference-arc count has started to " +
        "matter, which it never has at any earlier stage of this rig"));

    // 4. The two rovers must also agree under load. A solar rover whose island
    //    solves at rest and diverges when loaded is the failure this phase was
    //    added for, and it needs its own assertion rather than being folded into
    //    "both drew something".
    f.push(t_rel(this.n_draw, this.d_draw, 10.0,
        "peak motor draw disagrees under identical throttle (direct=" +
        r2(this.d_draw) + "W nested=" + r2(this.n_draw) + "W) — same rover, " +
        "same demand, so the profile arc is changing what the island solves"));

    report_verdict(f, "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
}
