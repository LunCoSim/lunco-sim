// ── Thermal + electrical coexistence — composite units compile, heat wire lands ──
//
// Runs against `scenes/tests/thermal_with_battery.usda`.
//
// WHAT IT GUARDS. With `power = "battery"` AND `thermal = "basic"`, the rover
// carries two runtime domain participants: the rover-root network and
// `Scope "Thermal"`. The thermal participant contains two synthesizer-owned
// disconnected Modelica units (left and right banks). The motors' solved
// `outputs:heat` is consumed directly by the Thermal participant as a causal
// cross-domain SimConnection. This scenario proves:
//
//   1. BOTH participants compiled and published their boundary ports (the
//      rover-root `soc` and the Thermal `motor_temp_*` are live, not missing).
//   2. The thermal temperature is finite and in a plausible range (the thermal
//      DAE solved and published a real value).
//
// This is the architectural gate: before the domain split, thermal members were
// grafted into the rover-root collection. Now the Thermal root is separate,
// and its two generated units plus boundary ports are reachable.
//
// `emit("THERMAL_WITH_BATTERY", "PASS"|"FAIL")`.

const PORT_WAIT_S = 30.0;

fn on_start(me) {
    this.t = 0.0;
    this.done = false;

    this.rover = find("/ThermalWithBatteryTest/Rover");
    this.elec = find("/ThermalWithBatteryTest/Rover");
    this.thermal = find("/ThermalWithBatteryTest/Rover/Thermal");

    print("[thermal-battery] ── thermal + electrical participants coexist ────");
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

    // Wait for both participants to compile and publish. The rover-root
    // participant publishes its authored `soc` boundary, whose USD source is
    // Battery.soc_out; the Thermal participant publishes
    // both bank temperatures. Both must be live — this proves the composite
    // root and its synthesizer-owned units compiled and stepped.
    // Composite roots expose their authored boundary, not the private child
    // model instances. These three outputs prove that the rover-root network and
    // both disconnected Thermal units compiled and stepped.
    let soc = get(this.elec, "soc");
    let temp = get(this.thermal, "motor_temp_left");
    let right_temp = get(this.thermal, "motor_temp_right");

    if soc == () || temp == () || right_temp == () {
        if this.t >= PORT_WAIT_S {
            finish_failure(this, "electrical or thermal participant never " +
                "published within " + PORT_WAIT_S + "s — soc=" + soc +
                " temp_left=" + temp + " temp_right=" + right_temp);
        }
        return;
    }

    // Both participants live — take the verdict now. The values are already
    // the root boundary outputs, so no child-instance naming leaks into the
    // scenario contract.
    this.done = true;
    verdict(#{ soc: soc, temp: temp, right_temp: right_temp });
}

fn verdict(s) {
    let f = [];
    // PRESENCE is the gate: the authored Thermal boundary and both child units
    // must be live.
    f.push(t_present(s.soc, "electrical participant soc boundary (compiled)"));
    f.push(t_present(s.temp, "thermal unit MotorThermalLeft.temp_k (compiled)"));
    f.push(t_present(s.right_temp, "thermal unit MotorThermalRight.temp_k (compiled)"));
    // A finite, plausible temperature proves the thermal DAE actually solved.
    f.push(t_range(s.temp, 200.0, 400.0,
        "thermal unit temperature (K) — outside any plausible range"));
    f.push(t_range(s.right_temp, 200.0, 400.0,
        "right thermal unit temperature (K) — outside any plausible range"));

    // SOC must be the authored init (0.9) — present and in range proves the
    // electrical DAE solved its initial state.
    f.push(t_range(s.soc, 0.0, 1.0,
        "electrical participant SOC — outside [0,1]"));

    print("[thermal-battery] ── results ──────────────────────────────");
    print("[thermal-battery]   soc          " + r4(s.soc));
    print("[thermal-battery]   temp         " + r4(s.temp) + " K");
    print("[thermal-battery]   right temp   " + r4(s.right_temp) + " K");

    report_verdict(f, "THERMAL WITH BATTERY", "THERMAL_WITH_BATTERY");
}
