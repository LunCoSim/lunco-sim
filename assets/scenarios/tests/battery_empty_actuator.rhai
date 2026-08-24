// ── Empty battery: physical bus authority stops actuators ───────────────────
//
// The scene authors an empty battery and no generation source. This is a
// runtime witness for the normal Modelica/ USD path:
//
//   battery terminal voltage -> motor available_demand -> wheel drive
//
// The test never writes a motor command or asks Rust to stop an actuator.

const PORT_WAIT_S = 30.0;
const DRIVE_S = 3.0;
const ZERO_TOL = 1.0e-6;
const MOTOR = "RockerL_x2f_Motor__FL";

fn member_port(field) {
    "__member_BatteryEmptyActuator_x2f_Rover_x2f_" + MOTOR + "_" + field
}

fn on_start(me) {
    this.t = 0.0;
    this.phase = "wait";
    this.done = false;
    this.rover = find("/BatteryEmptyActuator/Rover");
    this.elec = find("/BatteryEmptyActuator/Rover/Electrical");
    this.event_seen = false;
    this.drained = ();
    this.max_power = 0.0;
    this.max_authority = 0.0;
    this.max_voltage = 0.0;
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
        fail_fast("empty-battery fixture did not compose its rover/electrical scope",
                  "BATTERY EMPTY ACTUATOR", "BATTERY_EMPTY_ACTUATOR");
        return;
    }

    this.drained = get(this.elec, "battery_drained");
    let power = get(this.elec, member_port("electrical_power"));
    let authority = get(this.elec, member_port("available_demand"));
    let voltage = get(this.elec, member_port("terminal_voltage_v"));

    if this.phase == "wait" {
        if this.drained == () || power == () || authority == () || voltage == () {
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
    if power.abs() > this.max_power { this.max_power = power.abs(); }
    if authority.abs() > this.max_authority { this.max_authority = authority.abs(); }
    if voltage.abs() > this.max_voltage { this.max_voltage = voltage.abs(); }

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
    f.push(t_true(s.max_authority <= ZERO_TOL,
        "motor available_demand remained " + s.max_authority +
        " with no electrical energy"));
    f.push(t_true(s.max_power <= ZERO_TOL,
        "motor electrical power remained " + s.max_power +
        " W with no electrical energy"));
    f.push(t_true(s.max_voltage <= ZERO_TOL,
        "motor terminal voltage remained " + s.max_voltage +
        " V with an empty battery and no source"));

    print("[battery-empty] drained=" + s.drained + " event=" + s.event_seen +
          " max_authority=" + s.max_authority + " max_power=" + s.max_power +
          "W max_voltage=" + s.max_voltage + "V");
    report_verdict(f, "BATTERY EMPTY ACTUATOR", "BATTERY_EMPTY_ACTUATOR");
}
