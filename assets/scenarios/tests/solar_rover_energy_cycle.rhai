// ── Solar rover: tracked sun, generated power island, route traverse, recharge
//
// Runs against `scenes/tests/solar_rover_energy_cycle.usda`.
//
// This is the one end-to-end scenario for the authored stack:
//
//   environment provider -> SunTracker.mo -> hinge -> articulated panel
//   panel + battery + motors -> generated Modelica electrical island
//   scene-level route program -> rover movement / motor demand
//   Rhai -> bounded phase policy and machine-readable verdict
//
// The scene starts with a deliberately small but real battery and panel. While
// the route program drives, motor load exceeds solar generation and SOC falls.
// Once Rhai stops the route through its authored action intent, the still-lit
// panel charges the same battery back up. No script writes SOC, panel power, a
// Sun vector, or a motor demand.

const PORT_WAIT_S = 30.0;
const TRACK_S = 12.0;
const DRIVE_S = 5.0;
const CHARGE_S = 12.0;
const CELESTIAL_NIGHT_WAIT_S = 36.0;
const NIGHT_POWER_MAX_W = 0.1;
const SUN_STEP = 1.0;
const YAW_DELTA_MIN = 0.2;
const HINGE_TOL = 0.18;
const SOC_DROP_MIN = 0.01;
const SOC_RECOVER_MIN = 0.005;
const MOTION_MIN_M = 0.25;
const IDLE_DRIVE_TOL = 0.001;
const MOTOR_LOAD_MIN_W = 1.0;
const POWER_TOL_W = 0.1;

// Every observed value is an authored rover-root boundary port. Generated
// member identifiers are implementation details, not a scenario contract.
const BATTERY_SOC = "soc";
const SOLAR_POWER = "solar_power";
const SOLAR_AVAILABLE_POWER = "solar_available_power";
const SOLAR_INCIDENCE = "solar_incidence";
const MOTOR_ELECTRICAL_POWER = "motor_electrical_power";
const MOTOR_TERMINAL_CURRENT = "motor_terminal_current";

fn simulation_dependencies(me, ctx) {
    #{
        modelica_entities: [
            find_path("/SolarRoverTest/SolarRover"),
            find_path("/SolarRoverTest/SolarRover/SunTrackerController"),
            find_path("/SolarRoverTest/SolarRover/Comms/EarthTrackerController"),
        ],
        required_inputs: [],
    }
}

fn island_value(elec, port) {
    get(elec, port)
}

fn have_ports(s) {
    island_value(s.elec, BATTERY_SOC) != () &&
    island_value(s.elec, SOLAR_POWER) != () &&
    island_value(s.elec, SOLAR_AVAILABLE_POWER) != () &&
    get(s.elec, "battery_discharge_power") != () &&
    get(s.elec, "battery_charge_power") != () &&
    island_value(s.elec, SOLAR_INCIDENCE) != () &&
    island_value(s.elec, MOTOR_ELECTRICAL_POWER) != () &&
    island_value(s.elec, MOTOR_TERMINAL_CURRENT) != () &&
    get(s.tracker, "yaw") != () && get(s.hinge, "angle") != () &&
    get(s.earth_tracker, "target_mount_x") != () &&
    get(s.earth_tracker, "target_mount_y") != () &&
    get(s.earth_tracker, "target_mount_z") != ()
}

fn on_start(me, ctx) {
    this.t = 0.0;
    this.phase_started = 0.0;
    this.phase = "await_ports";
    this.done = false;

    this.rover = find_path("/SolarRoverTest/SolarRover");
    this.elec = find_path("/SolarRoverTest/SolarRover");
    this.tracker = find_path("/SolarRoverTest/SolarRover/SunTrackerController");
    this.earth_tracker = find_path("/SolarRoverTest/SolarRover/Comms/EarthTrackerController");
    this.hinge = find_path("/SolarRoverTest/SolarRover/Hinge");
    this.earth_direction_x = ();
    this.earth_direction_y = ();
    this.earth_direction_z = ();

    this.soc_before_drive = ();
    this.soc_after_drive = ();
    this.soc_after_charge = ();
    this.night_power = ();
    this.night_incidence = ();
    this.night_sun_up = ();
    this.night_rate_command_ok = false;
    this.night_reached = false;
    this.night_clock_before = ();
    this.night_clock_after = ();
    this.night_first_sample_rate = ();
    this.power_before = ();
    this.power_after_step = ();
    this.available_before = ();
    this.incidence_before = ();
    this.incidence_after_step = ();
    this.yaw_before = ();
    this.yaw_after_step = ();
    this.hinge_after_step = ();
    this.position_before_drive = ();
    this.position_after_drive = ();
    this.route = ();
    this.island_drive_left_after_stop = ();
    this.island_drive_right_after_stop = ();
    this.peak_motor_power = 0.0;
    this.peak_motor_current = 0.0;
    this.peak_battery_discharge_power = 0.0;
    this.peak_battery_charge_power = 0.0;
    this.loaded_drive_samples = 0;
    this.drive_charge_samples = 0;
    this.drive_charge_observed_samples = 0;
    this.drive_net_observed_samples = 0;
    this.peak_drive_charge_power = 0.0;
    this.max_loaded_drive_net_power = ();

    print("[solar-cycle] ── solar rover energy cycle ─────────────────────");
}

fn finish_failure(s, message) {
    s.done = true;
    fail_fast(message, "SOLAR ROVER ENERGY CYCLE", "SOLAR_ROVER_ENERGY_CYCLE");
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.t += dt();
    let phase_t = this.t - this.phase_started;

    if this.rover < 0 || this.elec < 0 || this.tracker < 0 ||
       this.earth_tracker < 0 || this.hinge < 0 {
        finish_failure(this, "solar rover test prim missing (rover=" + this.rover +
            " electrical=" + this.elec + " tracker=" + this.tracker +
            " Earth tracker=" + this.earth_tracker + " hinge=" + this.hinge +
            ") — wrong composed scene?");
        return;
    }

    if this.phase == "await_ports" {
        if !have_ports(this) {
            if this.t >= PORT_WAIT_S {
                let earth_x = get(this.earth_tracker, "target_mount_x");
                let earth_y = get(this.earth_tracker, "target_mount_y");
                let earth_z = get(this.earth_tracker, "target_mount_z");
                if earth_x == () || earth_y == () || earth_z == () {
                    finish_failure(this, "EarthTracker did not receive the connected `earth` target " +
                        "through generic target_mount_x/y/z inputs within " + PORT_WAIT_S + "s");
                } else {
                    finish_failure(this, "generated electrical island or SunTracker ports never appeared " +
                        "within " + PORT_WAIT_S + "s");
                }
            }
            return;
        }

        this.power_before = island_value(this.elec, SOLAR_POWER);
        this.available_before = get(this.elec, "solar_available_power");
        this.incidence_before = island_value(this.elec, SOLAR_INCIDENCE);
        this.yaw_before = get(this.tracker, "yaw");
        this.earth_direction_x = get(this.earth_tracker, "target_mount_x");
        this.earth_direction_y = get(this.earth_tracker, "target_mount_y");
        this.earth_direction_z = get(this.earth_tracker, "target_mount_z");
        this.phase = "track_sun";
        this.phase_started = this.t;
        cmd("SetEnvironmentLight", #{ sun_yaw: SUN_STEP });
        print("[solar-cycle] ports live; stepping canonical environment sun");
        return;
    }

    if this.phase == "track_sun" {
        if phase_t < TRACK_S { return; }
        this.power_after_step = island_value(this.elec, SOLAR_POWER);
        this.incidence_after_step = island_value(this.elec, SOLAR_INCIDENCE);
        this.yaw_after_step = get(this.tracker, "yaw");
        this.hinge_after_step = get(this.hinge, "angle");
        this.soc_before_drive = island_value(this.elec, BATTERY_SOC);
        this.position_before_drive = world_pos(this.rover);
        this.route = find_path("/SolarRoverTest/Route");
        if this.route < 0 {
            finish_failure(this, "scene-level route was not composed");
            return;
        }
        emit("route_start", 1);
        this.phase = "drive";
        this.phase_started = this.t;
        print("[solar-cycle] tracker settled; starting scene-level route");
        return;
    }

    if this.phase == "drive" {
        let motor = island_value(this.elec, MOTOR_ELECTRICAL_POWER);
        if motor != () && motor > this.peak_motor_power {
            this.peak_motor_power = motor;
        }
        let current = island_value(this.elec, MOTOR_TERMINAL_CURRENT);
        if current != () && current > this.peak_motor_current {
            this.peak_motor_current = current;
        }
        let discharge = get(this.elec, "battery_discharge_power");
        if discharge != () && discharge > this.peak_battery_discharge_power {
            this.peak_battery_discharge_power = discharge;
        }
        let charge = get(this.elec, "battery_charge_power");
        if motor != () && motor > MOTOR_LOAD_MIN_W {
            this.loaded_drive_samples += 1;
            if charge != () {
                this.drive_charge_observed_samples += 1;
                if charge > this.peak_drive_charge_power {
                    this.peak_drive_charge_power = charge;
                }
                if charge > POWER_TOL_W {
                    this.drive_charge_samples += 1;
                }
            }
            let net = get(this.elec, "battery_net_power");
            if net != () {
                this.drive_net_observed_samples += 1;
                if this.max_loaded_drive_net_power == () ||
                   net > this.max_loaded_drive_net_power {
                    this.max_loaded_drive_net_power = net;
                }
            }
        }
        if phase_t < DRIVE_S { return; }
        this.soc_after_drive = island_value(this.elec, BATTERY_SOC);
        this.position_after_drive = world_pos(this.rover);
        let stopped = intent_pulse(this.rover, "action");
        if stopped == () || stopped.ok != true {
            finish_failure(this, "route action intent could not stop the authored program");
            return;
        }
        this.phase = "charge";
        this.phase_started = this.t;
        print("[solar-cycle] route stopped; observing idle solar recharge");
        return;
    }

    if this.phase == "charge" {
        // Read the USD-forwarded actuator outputs after the lifecycle boundary,
        // not merely the logical throttle request. This makes a failed safe-stop
        // distinguishable from an idle electrical-model defect.
        this.island_drive_left_after_stop = get(this.elec, "drive_left");
        this.island_drive_right_after_stop = get(this.elec, "drive_right");
        let charge = get(this.elec, "battery_charge_power");
        if charge != () && charge > this.peak_battery_charge_power {
            this.peak_battery_charge_power = charge;
        }
        if phase_t < CHARGE_S { return; }
        this.soc_after_charge = island_value(this.elec, BATTERY_SOC);
        this.night_clock_before = clock_snapshot();
        let rate_command = cmd("SetCelestialClock", #{ scale: 100000.0 });
        this.night_rate_command_ok = rate_command != () && rate_command.ok == true;
        this.phase = "celestial_night";
        this.phase_started = this.t;
        print("[solar-cycle] advancing the shared celestial sample through a lunar day");
        return;
    }

    if this.phase == "celestial_night" {
        if this.night_first_sample_rate == () {
            let clock = clock_snapshot();
            let world_step = clock.world_sim_s - this.night_clock_before.world_sim_s;
            let celestial_step_s = (clock.celestial_epoch_jd -
                this.night_clock_before.celestial_epoch_jd) * 86400.0;
            if world_step > 0.0 {
                this.night_first_sample_rate = celestial_step_s / world_step;
            }
        }
        let available = island_value(this.elec, SOLAR_AVAILABLE_POWER);
        let sun_up = get(this.tracker, "target_mount_y");
        if available != () && available <= NIGHT_POWER_MAX_W &&
           sun_up != () && sun_up <= 0.0 {
            this.night_reached = true;
            this.night_power = available;
            this.night_incidence = island_value(this.elec, SOLAR_INCIDENCE);
            this.night_sun_up = sun_up;
        }
        if phase_t < CELESTIAL_NIGHT_WAIT_S { return; }
        this.night_clock_after = clock_snapshot();
        this.done = true;
        verdict(this);
    }
}

fn verdict(s) {
    let f = [];
    f.push(t_present(s.soc_before_drive, "battery SOC before drive"));
    f.push(t_present(s.soc_after_drive, "battery SOC after drive"));
    f.push(t_present(s.soc_after_charge, "battery SOC after charge"));
    f.push(t_present(s.night_power, "solar panel power at lunar night"));
    f.push(t_present(s.night_incidence, "solar panel incidence at lunar night"));
    f.push(t_present(s.night_sun_up, "Sun direction up component at lunar night"));
    f.push(t_present(s.power_before, "panel power before sun step"));
    f.push(t_present(s.power_after_step, "panel power after sun step"));
    f.push(t_present(s.available_before, "panel available power before drive"));
    f.push(t_present(s.incidence_before, "panel incidence before sun step"));
    f.push(t_present(s.incidence_after_step, "panel incidence after sun step"));
    f.push(t_present(s.yaw_after_step, "SunTracker yaw"));
    f.push(t_present(s.hinge_after_step, "panel hinge angle"));
    f.push(t_present(s.earth_direction_x, "Earth direction x at the rover antenna mount"));
    f.push(t_present(s.earth_direction_y, "Earth direction y at the rover antenna mount"));
    f.push(t_present(s.earth_direction_z, "Earth direction z at the rover antenna mount"));
    f.push(t_present(s.position_before_drive, "rover position before route"));
    f.push(t_present(s.position_after_drive, "rover position after route"));

    if s.yaw_before != () && s.yaw_after_step != () {
        f.push(t_true((s.yaw_after_step - s.yaw_before).abs() > YAW_DELTA_MIN,
            "SunTracker yaw did not respond to the environment sun step"));
    }
    if s.hinge_after_step != () && s.yaw_after_step != () {
        f.push(t_true((s.hinge_after_step - s.yaw_after_step).abs() < HINGE_TOL,
            "hinge angle does not follow SunTracker yaw"));
    }
    if s.earth_direction_x != () && s.earth_direction_y != () &&
       s.earth_direction_z != () {
        let earth_norm2 = s.earth_direction_x^2 + s.earth_direction_y^2 +
                          s.earth_direction_z^2;
        f.push(t_true((earth_norm2 - 1.0).abs() < 1.0e-10,
            "Earth direction at the antenna mount must be a normalized f64 target vector; norm²=" +
            earth_norm2));
    }
    if s.power_after_step != () {
        f.push(t_true(s.power_after_step > 0.0,
            "solar panel reports no generated power after the environment sun step"));
    }
    if s.incidence_before != () && s.incidence_after_step != () {
        f.push(t_true(s.incidence_after_step > 0.0,
            "panel lost positive sun incidence after the tracker step"));
    }
    if s.position_before_drive != () && s.position_after_drive != () {
        f.push(t_true(vlen(vsub(s.position_after_drive, s.position_before_drive)) > MOTION_MIN_M,
            "scene-level route did not move the rover while active"));
    }
    f.push(t_true(s.peak_motor_power > 0.0,
        "generated motor model reported no electrical draw under route drive"));
    f.push(t_true(s.peak_motor_current > 0.0,
        "generated motor model reported no terminal current under route drive"));
    f.push(t_true(s.peak_battery_discharge_power > 0.0,
        "battery reported no discharge power under motor load"));
    f.push(t_true(s.loaded_drive_samples > 100,
        "motor load was not observable for enough drive samples"));
    f.push(t_true(s.drive_charge_observed_samples > 100,
        "battery charge power was not observable for enough loaded-drive samples"));
    f.push(t_true(s.drive_charge_samples == 0,
        "battery charged while the loaded rover was driving (peak " +
        r2(s.peak_drive_charge_power) + " W)"));
    f.push(t_true(s.drive_net_observed_samples > 100,
        "battery net power was not observable for enough loaded-drive samples"));
    if s.max_loaded_drive_net_power != () {
        f.push(t_true(s.max_loaded_drive_net_power < -POWER_TOL_W,
            "battery net power was not negative while the motors were loaded (maximum " +
            r2(s.max_loaded_drive_net_power) + " W)"));
    }
    f.push(t_true(s.peak_battery_charge_power > 0.0,
        "battery reported no charge power after the route stopped"));
    if s.soc_before_drive != () && s.soc_after_drive != () {
        f.push(t_true(s.soc_before_drive - s.soc_after_drive > SOC_DROP_MIN,
            "battery SOC did not fall under motor load"));
    }
    if s.soc_after_drive != () && s.soc_after_charge != () {
        f.push(t_true(s.soc_after_charge - s.soc_after_drive > SOC_RECOVER_MIN,
            "battery SOC did not recover after the route stopped"));
    }
    f.push(t_true(s.night_rate_command_ok,
        "SetCelestialClock must accept the 100000x rate for the night check"));
    f.push(t_present(s.night_first_sample_rate,
        "first celestial sample after changing rate"));
    if s.night_first_sample_rate != () {
        f.push(t_true(s.night_first_sample_rate > 99000.0 &&
                      s.night_first_sample_rate < 101000.0,
            "changing CelestialTime rate must preserve the current sample and apply immediately"));
    }
    f.push(t_true(s.night_reached && s.night_power <= NIGHT_POWER_MAX_W,
        "solar panel incidence and power did not fall to zero below the local horizon"));
    if s.night_incidence != () && s.night_sun_up != () {
        f.push(t_true(s.night_incidence <= 0.0 && s.night_sun_up <= 0.0,
            "the night result must come from the shared below-horizon Sun vector"));
    }
    if s.night_clock_before != () && s.night_clock_after != () {
        let world_delta = s.night_clock_after.world_sim_s - s.night_clock_before.world_sim_s;
        let celestial_delta_s = (s.night_clock_after.celestial_epoch_jd -
            s.night_clock_before.celestial_epoch_jd) * 86400.0;
        let physical_tick_delta = (s.night_clock_after.sim_tick -
            s.night_clock_before.sim_tick).to_float() * s.night_clock_after.fixed_dt_s;
        f.push(t_true(s.night_clock_after.celestial_rate == 100000.0 &&
                      s.night_clock_after.celestial_parent == "world_time",
            "accelerated celestial time must remain a child of WorldTime"));
        f.push(t_true(s.night_clock_after.transport_rate == s.night_clock_before.transport_rate &&
                      s.night_clock_after.virtual_rate == s.night_clock_before.virtual_rate,
            "celestial acceleration must leave the physical transport unchanged"));
        f.push(t_true(world_delta >= CELESTIAL_NIGHT_WAIT_S - 0.1 &&
                      (physical_tick_delta - world_delta).abs() < 0.1,
            "physical time and fixed ticks must keep their ordinary cadence"));
        f.push(t_true(world_delta > 0.0 &&
                      celestial_delta_s / world_delta > 99000.0 &&
                      celestial_delta_s / world_delta < 101000.0,
            "Modelica solar inputs must follow the same 100000x CelestialTime sample"));
    }
    if s.island_drive_left_after_stop != () && s.island_drive_right_after_stop != () {
        f.push(t_true(s.island_drive_left_after_stop.abs() < IDLE_DRIVE_TOL &&
                      s.island_drive_right_after_stop.abs() < IDLE_DRIVE_TOL,
            "route safe stop left drive demand on the electrical island"));
    }

    print("[solar-cycle] SOC " + r4(s.soc_before_drive) + " -> " +
          r4(s.soc_after_drive) + " -> " + r4(s.soc_after_charge));
    print("[solar-cycle] yaw " + r4(s.yaw_before) + " -> " +
          r4(s.yaw_after_step) + ", hinge=" + r4(s.hinge_after_step));
    print("[solar-cycle] tracker target_mount_x=" +
          r4(get(s.tracker, "target_mount_x")));
    print("[solar-cycle] EarthTracker target_mount_xyz=" + r4(s.earth_direction_x) + "," +
          r4(s.earth_direction_y) + "," + r4(s.earth_direction_z));
    print("[solar-cycle] power " + r2(s.power_before) + "W -> " +
          r2(s.power_after_step) + "W, available=" + r2(s.available_before) +
          "W, motor peak=" + r2(s.peak_motor_power) + "W, motor current=" +
          r2(s.peak_motor_current) + "A, battery discharge/charge peak=" +
          r2(s.peak_battery_discharge_power) + "/" + r2(s.peak_battery_charge_power) + "W");
    print("[solar-cycle] loaded drive samples=" + s.loaded_drive_samples +
          ", charge observed=" + s.drive_charge_observed_samples +
          ", charge samples above tolerance=" + s.drive_charge_samples +
          ", net observed=" + s.drive_net_observed_samples +
          ", loaded-drive net-power max=" + r2(s.max_loaded_drive_net_power) + "W");
    print("[solar-cycle] post-stop electrical inputs left/right=" +
          r4(s.island_drive_left_after_stop) + "/" + r4(s.island_drive_right_after_stop));
    print("[solar-cycle] rover position before/after route=" +
          s.position_before_drive + " -> " + s.position_after_drive +
          ", motion=" + r4(vlen(vsub(s.position_after_drive, s.position_before_drive))) + "m");
    print("[solar-cycle] night panel power=" + r2(s.night_power) +
          "W, CelestialTime rate=" + r2(s.night_clock_after.celestial_rate) +
          "x, WorldTime elapsed=" + r2(s.night_clock_after.world_sim_s -
          s.night_clock_before.world_sim_s) + "s");
    report_verdict(f, "SOLAR ROVER ENERGY CYCLE", "SOLAR_ROVER_ENERGY_CYCLE");
}
