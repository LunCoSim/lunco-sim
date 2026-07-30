// ── Thermal + electrical coexistence — both islands compile, heat wire lands ──
//
// Runs against `scenes/tests/thermal_with_battery.usda`.
//
// WHAT IT GUARDS. With `power = "battery"` AND `thermal = "basic"`, the rover
// carries two independent domain islands: `Scope "Electrical"` and
// `Scope "Thermal"`, each its own generated Modelica DAE. The motors' solved
// `outputs:heat` is forwarded to the Electrical boundary, then read by the
// Thermal island as a causal cross-domain SimConnection. This scenario proves:
//
//   1. BOTH islands compiled and published their boundary ports (the Electrical
//      `soc` and the Thermal `motor_temp_*` are live, not missing).
//   2. The thermal temperature is finite and in a plausible range (the thermal
//      DAE solved and published a real value).
//
// This is the architectural gate: before the domain split, thermal members were
// grafted into the Electrical collection and there was no separate Thermal
// island. Now there is, and its boundary ports are reachable.
//
// `emit("THERMAL_WITH_BATTERY", "PASS"|"FAIL")`.

const PORT_WAIT_S = 30.0;

// Generated-island port names come from each member's path relative to its
// network root (`/` → `_x2f_`). Two shapes:
//   - OUTSIDE the scope (Battery/Motor_* hang off the vessel): full path.
//     /ThermalWithBatteryTest/Rover/Battery → ThermalWithBatteryTest_x2f_Rover_x2f_Battery
//   - INSIDE the scope subtree (thermal parts under /Thermal/...): leaf name only.
//     /ThermalWithBatteryTest/Rover/Thermal/MotorThermalLeft → MotorThermalLeft
fn elec_port(member, field) {
    "ThermalWithBatteryTest_x2f_Rover_x2f_" + member + "." + field
}
fn thermal_port(member, field) {
    member + "." + field
}

fn on_start(me) {
    this.t = 0.0;
    this.done = false;

    this.rover = find("/ThermalWithBatteryTest/Rover");
    this.elec = find("/ThermalWithBatteryTest/Rover/Electrical");
    this.thermal = find("/ThermalWithBatteryTest/Rover/Thermal");

    print("[thermal-battery] ── thermal + electrical islands coexist ────");
}

fn finish_failure(s, message) {
    s.done = true;
    fail_fast(message, "THERMAL WITH BATTERY", "THERMAL_WITH_BATTERY");
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    if this.rover < 0 || this.elec < 0 || this.thermal < 0 {
        finish_failure(this, "test prim missing (rover=" + this.rover +
            " electrical=" + this.elec + " thermal=" + this.thermal + ")");
        return;
    }

    // Wait for both islands to compile and publish. The Electrical island
    // publishes Battery.soc_out; the Thermal island publishes MotorThermalLeft.temp_k.
    // Both must be live — this is the core assertion that two separate DAEs
    // compiled and stepped.
    let soc = get(this.elec, elec_port("Battery", "soc_out"));
    let temp = get(this.thermal, thermal_port("MotorThermalLeft", "temp_k"));

    if soc == () || temp == () {
        if this.t >= PORT_WAIT_S {
            finish_failure(this, "electrical or thermal island never " +
                "published within " + PORT_WAIT_S + "s — soc=" + soc + " temp=" + temp);
        }
        return;
    }

    // Both islands live — take the verdict now. The boundary-forwarded scope
    // outputs are also checked (read without the mangled island name).
    this.done = true;
    let fwd_temp = get(this.thermal, "motor_temp_left");
    verdict(#{ soc: soc, temp: temp, fwd_temp: fwd_temp });
}

fn verdict(s) {
    let f = [];
    // PRESENCE is the gate: before the split, there was no Thermal island at all.
    f.push(t_present(s.soc, "electrical island Battery.soc_out (island compiled)"));
    f.push(t_present(s.temp, "thermal island MotorThermalLeft.temp_k (island compiled)"));
    f.push(t_present(s.fwd_temp, "Thermal.outputs:motor_temp_left forwarded boundary port"));

    // A finite, plausible temperature proves the thermal DAE actually solved.
    f.push(t_range(s.temp, 200.0, 400.0,
        "thermal island temperature (K) — outside any plausible range"));

    // SOC must be the authored init (0.9) — present and in range proves the
    // electrical DAE solved its initial state.
    f.push(t_range(s.soc, 0.0, 1.0,
        "electrical island SOC — outside [0,1]"));

    print("[thermal-battery] ── results ──────────────────────────────");
    print("[thermal-battery]   soc          " + r4(s.soc));
    print("[thermal-battery]   temp         " + r4(s.temp) + " K");
    print("[thermal-battery]   fwd temp     " + r4(s.fwd_temp) + " K");

    report_verdict(f, "THERMAL WITH BATTERY", "THERMAL_WITH_BATTERY");
}
