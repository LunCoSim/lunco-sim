// ── The solar electrical island, at one arc and at two, at rest and under load ─
//
// Runs against `scenes/tests/solar_domain_nested_ref.usda`, which stands three
// rovers side by side:
//
//   /SolarDomainNestedRef/BatteryOnly  battery bus, NO panel        (baseline)
//   /SolarDomainNestedRef/Direct       + solar, scene → base        (one arc)
//   /SolarDomainNestedRef/Nested       + solar, scene → profile → base (two arcs)
//
// WHAT IS READ. The generated island's ports, off the `Electrical` scope.
// Authored boundary outputs keep names such as `soc` and `solar_power`; a
// member-owned causal output uses the explicit `__member_` alias. An island
// that never steps publishes no port at all, so absence is the signal.
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

/// A generated island output name.
///
/// Network boundary outputs retain their authored names. Causal outputs that
/// remain member-owned use the synthesizer's explicit `__member_` alias over
/// the full USD path; a raw `Member.field` spelling is not a runtime contract.
fn port_name(rover, member, field) {
    if member == "Battery" && field == "soc_out" { return "soc"; }
    if member == "SolarPanel" && field == "power_out" { return "solar_power"; }
    "__member_SolarDomainNestedRef_x2f_" + rover + "_x2f_" + member + "_" + field
}

/// Whether an island is stepping — i.e. publishing the member port at all.
fn island_live(elec, rover, member, field) {
    if elec < 0 { return false; }
    get(elec, port_name(rover, member, field)) != ()
}

/// A member variable's value, or `0.0` when the port is absent. Only called
/// after `island_live` has confirmed the island publishes, so a zero here is a
/// real zero rather than a missing port.
fn port_or_zero(elec, rover, member, field) {
    let v = get(elec, port_name(rover, member, field));
    if v == () { 0.0 } else { v }
}

/// The front-left motor of a rocker-bogie. One motor is enough to witness that
/// demand reached the island: all six carry the same two demand inputs, so a
/// wiring break that starved one starves the side.
///
/// ⚠ `Motor__FL`, with TWO underscores. The prim-path → Modelica-identifier
/// escaping is total, not just a slash substitution: `/` becomes `_x2f_` and a
/// literal `_` becomes `__`, so that no two distinct prim paths can collide on
/// one identifier. `Battery` and `SolarPanel` carry no underscore and hide this;
/// every motor has one. Spelling it `Motor_FL` names a port that does not exist,
/// which reads back exactly like an island that failed to publish.
const MOTOR = "RockerL_x2f_Motor__FL";

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

    let b_live = island_live(this.b_elec, "BatteryOnly", "Battery", "soc_out");
    let d_live = island_live(this.d_elec, "Direct", "SolarPanel", "power_out");
    let n_live = island_live(this.n_elec, "Nested", "SolarPanel", "power_out");

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
                let why = "the BASELINE battery island (no panel) publishes no " +
                          "ports either, so generated electrical islands are not " +
                          "working at all and nothing here is a statement about " +
                          "solar. Fix that first — the two solar rovers cannot " +
                          "be read until it is up";
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

        // All three up. Take the at-rest panel reading, then load the bus.
        this.d_panel_rest = port_or_zero(this.d_elec, "Direct", "SolarPanel", "power_out");
        this.n_panel_rest = port_or_zero(this.n_elec, "Nested", "SolarPanel", "power_out");
        print("[solar-nested] t=" + r2(this.t) + "  battery=up direct=up nested=up" +
              "  panel@rest direct=" + r2(this.d_panel_rest) +
              "W nested=" + r2(this.n_panel_rest) + "W");

        this.phase = "drive";
        this.drive_until = this.t + this.drive_s;
        print("[solar-nested] loading the bus: full throttle straight for " +
              r2(this.drive_s) + "s");
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
        if !island_live(this.b_elec, "BatteryOnly", MOTOR, "electrical_power") {
            this.done = true;
            fail_fast("the baseline island publishes no `" +
                      port_name("BatteryOnly", MOTOR, "electrical_power") +
                      "` port at all, so the motor is not a member of the " +
                      "generated model and no draw could be read from any rover",
                      "SOLAR DOMAIN NESTED REF", "SOLAR_DOMAIN_NESTED_REF");
            return;
        }

        let b = port_or_zero(this.b_elec, "BatteryOnly", MOTOR, "electrical_power");
        let d = port_or_zero(this.d_elec, "Direct", MOTOR, "electrical_power");
        let n = port_or_zero(this.n_elec, "Nested", MOTOR, "electrical_power");

        // One-shot trace of the whole demand chain, printed on the first loaded
        // tick. When the draw comes back zero this is the line that says WHERE
        // the demand stopped: at the vehicle, at the island boundary, or at the
        // motor.
        // The demand chain, once, on the first loaded tick. When a draw comes back
        // zero this line says WHERE it stopped, which is three different bugs:
        // the vessel never took the command (`throttle` 0), the island never
        // received it (`island.drive_left` 0 under a live throttle — the actuator
        // -port read that this rig found broken), or the motor has it and the
        // solver produced nothing. Note `throttle` is read off the VESSEL and
        // `drive_left` off the ISLAND: the vessel's own `drive_left` lives on a
        // child `Port` entity and is not readable by name here.
        if !this.traced {
            this.traced = true;
            print("[solar-nested] demand chain  rover.throttle=" +
                  get(this.b_rover, "throttle") +
                  "  island.drive_left=" + get(this.b_elec, "drive_left") +
                  "  motor.demand=" +
                  get(this.b_elec, port_name("BatteryOnly", MOTOR, "demand")) +
                  "  motor.electrical_power=" + b);
        }
        if b > this.b_draw { this.b_draw = b; }
        if d > this.d_draw { this.d_draw = d; }
        if n > this.n_draw { this.n_draw = n; }

        if this.t < this.drive_until { return; }
    }

    this.done = true;
    print("[solar-nested] t=" + r2(this.t) + "  peak motor draw  battery=" +
          r2(this.b_draw) + "W direct=" + r2(this.d_draw) + "W nested=" +
          r2(this.n_draw) + "W");

    // ── The second anti-trivial gate ─────────────────────────────────────────
    // The baseline's motors are on the same demand wiring as the solar rovers'.
    // If the BASELINE drew nothing under full throttle, the demand never reached
    // any island and the two solar zeroes below would be about the wiring, not
    // about solar. A test that reported "solar draws nothing" in that state
    // would be blaming the wrong subsystem, loudly.
    if this.b_draw <= 0.0 {
        let why = "the BASELINE rover (no panel) drew NO motor current under " +
                  "full throttle, so `Electrical.inputs:drive_left|right` are " +
                  "not reaching the motors on any rover and the solar readings " +
                  "below measure the wiring, not the panel. Fix the boundary " +
                  "inputs first";
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
        "direct panel power_out at rest = " + r2(this.d_panel_rest) +
        "W, expected > 0"));

    // 3. Arc count is a SEPARATE claim from whether solar works at all, asserted
    //    on its own so a future regression that breaks only the profile path is
    //    not hidden behind a failure both rovers share. Same component, same
    //    nameplate, same attitude at rest — the numbers must agree.
    f.push(t_rel(this.n_panel_rest, this.d_panel_rest, 1.0,
        "nested panel power_out at rest (" + r2(this.n_panel_rest) +
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
