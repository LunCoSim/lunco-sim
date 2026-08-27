// ── Empty battery: physical bus authority stops actuators ───────────────────
//
// The scene authors an empty battery and no generation source. This is a
// runtime witness for the normal Modelica/ USD path:
//
//   battery terminal voltage -> motor winding -> solved shaft torque
//
// The test never writes a motor command or asks Rust to stop an actuator. A
// wheel may still exchange mechanical energy with a motor; the assertion is
// therefore on battery-supplied power and positive shaft work, not on a
// per-motor electrical sample that can be balanced by another physical path.

const PORT_WAIT_S = 30.0;
const DRIVE_S = 3.0;
const ZERO_TOL = 1.0e-6;
// These names are authored rover-root `outputs:*` boundary ports in the USD
// rover asset. Generated Modelica member identifiers are implementation
// details and are not a scenario contract.
const BATTERY_DISCHARGE_POWER = "battery_discharge_power";
const MOTOR_MECHANICAL_POWER = "motor_mechanical_power";

fn on_start(me) {
    this.t = 0.0;
    this.phase = "wait";
    this.done = false;
    this.rover = find("/BatteryEmptyActuator/Rover");
    this.elec = find("/BatteryEmptyActuator/Rover");
    this.event_seen = false;
    this.drained = ();
    this.max_battery_discharge_power = 0.0;
    this.max_drive_power = 0.0;
    print("[battery-empty] ── empty battery actuator boundary ─────────────");
}

fn on_event(me, evt) {
    if evt.name == "battery_drained" {
        this.event_seen = true;
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    if this.rover < 0 || this.elec < 0 {
        this.done = true;
        fail_fast("empty-battery fixture did not compose its rover/power scope",
                  "BATTERY EMPTY ACTUATOR", "BATTERY_EMPTY_ACTUATOR");
        return;
    }

    this.drained = get(this.elec, "battery_drained");
    let drive_power = get(this.elec, MOTOR_MECHANICAL_POWER);

    if this.phase == "wait" {
        let battery_discharge = get(this.elec, BATTERY_DISCHARGE_POWER);
        if this.drained == () || battery_discharge == () || drive_power == () {
            if this.t >= PORT_WAIT_S {
                this.done = true;
                fail_fast("empty-battery electrical outputs did not publish within " +
                          PORT_WAIT_S + "s", "BATTERY EMPTY ACTUATOR",
                          "BATTERY_EMPTY_ACTUATOR");
            }
            return;
        }
        this.phase = "drive";
        this.phase_started = this.t;
    }

    drive(this.rover, 1.0, 0.0);
    let battery_discharge = get(this.elec, BATTERY_DISCHARGE_POWER);
    if battery_discharge > this.max_battery_discharge_power {
        this.max_battery_discharge_power = battery_discharge;
    }
    if drive_power > this.max_drive_power { this.max_drive_power = drive_power; }

    if this.t - this.phase_started < DRIVE_S { return; }

    brake(this.rover);
    this.done = true;
    verdict(this);
}

fn verdict(s) {
    let f = [];
    f.push(t_true(s.drained >= 0.5,
        "battery_drained boundary output was " + s.drained + ", expected 1"));
    f.push(t_true(s.event_seen,
        "authored battery_drained event was not published"));
    f.push(t_true(s.max_battery_discharge_power <= ZERO_TOL,
        "battery supplied " + s.max_battery_discharge_power +
        " W at the empty boundary"));
    f.push(t_true(s.max_drive_power <= ZERO_TOL,
        "motor delivered " + s.max_drive_power +
        " W to the shaft with no electrical source"));

    print("[battery-empty] drained=" + s.drained + " event=" + s.event_seen +
          " max_battery_discharge_power=" + s.max_battery_discharge_power +
          " W max_drive_power=" + s.max_drive_power + " W");
    report_verdict(f, "BATTERY EMPTY ACTUATOR", "BATTERY_EMPTY_ACTUATOR");
}
