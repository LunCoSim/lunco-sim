// ── Solar rover: tracked sun, generated power island, BT traverse, recharge ─
//
// Runs against `scenes/tests/solar_rover_energy_cycle.usda`.
//
// This is the one end-to-end scenario for the authored stack:
//
//   environment provider -> SunTracker.mo -> hinge -> articulated panel
//   panel + battery + motors -> generated Modelica electrical island
//   BT.CPP patrol -> rover movement / motor demand
//   Rhai -> bounded phase policy and machine-readable verdict
//
// The scene starts with a deliberately small but real battery and panel. While
// the BT patrol drives, motor load exceeds solar generation and SOC falls. Once
// Rhai disengages the patrol, the still-lit panel charges the same battery back
// up. No script writes SOC, panel power, a Sun vector, or a motor demand.

const PORT_WAIT_S = 30.0;
const TRACK_S = 12.0;
const DRIVE_S = 5.0;
const CHARGE_S = 12.0;
const SUN_STEP = 1.0;
const YAW_DELTA_MIN = 0.2;
const HINGE_TOL = 0.18;
const SOC_DROP_MIN = 0.01;
const SOC_RECOVER_MIN = 0.005;
const MOTION_MIN_M = 0.25;
const IDLE_DRIVE_TOL = 0.001;

// Generated electrical-island ports are named from their member paths. This
// reads the Modelica result, not an authored USD output declaration.
fn island_port(member, field) {
    "SolarRoverTest_x2f_SolarRover_x2f_" + member + "." + field
}

fn island_value(elec, member, field) {
    get(elec, island_port(member, field))
}

fn have_ports(s) {
    island_value(s.elec, "Battery", "soc_out") != () &&
    island_value(s.elec, "YawHead_x2f_SolarPanel", "power_out") != () &&
    island_value(s.elec, "YawHead_x2f_SolarPanel", "cos_incidence") != () &&
    island_value(s.elec, "Motor__FL", "electrical_power") != () &&
    get(s.tracker, "yaw") != () && get(s.hinge, "angle") != ()
}

fn on_start(me) {
    this.t = 0.0;
    this.phase_started = 0.0;
    this.phase = "await_ports";
    this.done = false;

    this.rover = find("/SolarRoverTest/SolarRover");
    this.elec = find("/SolarRoverTest/SolarRover/Electrical");
    this.tracker = find("/SolarRoverTest/SolarRover/SunTrackerController");
    this.hinge = find("/SolarRoverTest/SolarRover/Hinge");

    this.soc_before_drive = ();
    this.soc_after_drive = ();
    this.soc_after_charge = ();
    this.power_before = ();
    this.power_after_step = ();
    this.incidence_before = ();
    this.incidence_after_step = ();
    this.yaw_before = ();
    this.yaw_after_step = ();
    this.hinge_after_step = ();
    this.position_before_drive = ();
    this.position_after_drive = ();
    this.island_drive_left_after_stop = ();
    this.island_drive_right_after_stop = ();
    this.peak_motor_power = 0.0;

    print("[solar-cycle] ── solar rover energy cycle ─────────────────────");
}

fn finish_failure(s, message) {
    s.done = true;
    fail_fast(message, "SOLAR ROVER ENERGY CYCLE", "SOLAR_ROVER_ENERGY_CYCLE");
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    let phase_t = this.t - this.phase_started;

    if this.rover < 0 || this.elec < 0 || this.tracker < 0 || this.hinge < 0 {
        finish_failure(this, "solar rover test prim missing (rover=" + this.rover +
            " electrical=" + this.elec + " tracker=" + this.tracker +
            " hinge=" + this.hinge + ") — wrong composed scene?");
        return;
    }

    if this.phase == "await_ports" {
        if !have_ports(this) {
            if this.t >= PORT_WAIT_S {
                finish_failure(this, "generated electrical island or SunTracker ports never appeared " +
                    "within " + PORT_WAIT_S + "s");
            }
            return;
        }

        this.power_before = island_value(this.elec, "YawHead_x2f_SolarPanel", "power_out");
        this.incidence_before = island_value(this.elec, "YawHead_x2f_SolarPanel", "cos_incidence");
        this.yaw_before = get(this.tracker, "yaw");
        this.phase = "track_sun";
        this.phase_started = this.t;
        cmd("SetEnvironmentLight", #{ sun_yaw: SUN_STEP });
        print("[solar-cycle] ports live; stepping canonical environment sun");
        return;
    }

    if this.phase == "track_sun" {
        if phase_t < TRACK_S { return; }
        this.power_after_step = island_value(this.elec, "YawHead_x2f_SolarPanel", "power_out");
        this.incidence_after_step = island_value(this.elec, "YawHead_x2f_SolarPanel", "cos_incidence");
        this.yaw_after_step = get(this.tracker, "yaw");
        this.hinge_after_step = get(this.hinge, "angle");
        this.soc_before_drive = island_value(this.elec, "Battery", "soc_out");
        this.position_before_drive = world_pos(this.rover);
        cmd("EngageAutopilot", #{ vessel: this.rover });
        this.phase = "drive";
        this.phase_started = this.t;
        print("[solar-cycle] tracker settled; engaging authored BT patrol");
        return;
    }

    if this.phase == "drive" {
        let motor = island_value(this.elec, "Motor__FL", "electrical_power");
        if motor != () && motor > this.peak_motor_power {
            this.peak_motor_power = motor;
        }
        if phase_t < DRIVE_S { return; }
        this.soc_after_drive = island_value(this.elec, "Battery", "soc_out");
        this.position_after_drive = world_pos(this.rover);
        cmd("DisengageAutopilot", #{ vessel: this.rover });
        this.phase = "charge";
        this.phase_started = this.t;
        print("[solar-cycle] patrol stopped; observing idle solar recharge");
        return;
    }

    if this.phase == "charge" {
        // Read the USD-forwarded actuator outputs after the lifecycle boundary,
        // not merely the logical throttle request. This makes a failed safe-stop
        // distinguishable from an idle electrical-model defect.
        this.island_drive_left_after_stop = get(this.elec, "drive_left");
        this.island_drive_right_after_stop = get(this.elec, "drive_right");
        if phase_t < CHARGE_S { return; }
        this.soc_after_charge = island_value(this.elec, "Battery", "soc_out");
        this.done = true;
        verdict(this);
    }
}

fn verdict(s) {
    let f = [];
    f.push(t_present(s.soc_before_drive, "battery SOC before drive"));
    f.push(t_present(s.soc_after_drive, "battery SOC after drive"));
    f.push(t_present(s.soc_after_charge, "battery SOC after charge"));
    f.push(t_present(s.power_before, "panel power before sun step"));
    f.push(t_present(s.power_after_step, "panel power after sun step"));
    f.push(t_present(s.incidence_before, "panel incidence before sun step"));
    f.push(t_present(s.incidence_after_step, "panel incidence after sun step"));
    f.push(t_present(s.yaw_after_step, "SunTracker yaw"));
    f.push(t_present(s.hinge_after_step, "panel hinge angle"));
    f.push(t_present(s.position_before_drive, "rover position before patrol"));
    f.push(t_present(s.position_after_drive, "rover position after patrol"));

    if s.yaw_before != () && s.yaw_after_step != () {
        f.push(t_true((s.yaw_after_step - s.yaw_before).abs() > YAW_DELTA_MIN,
            "SunTracker yaw did not respond to the environment sun step"));
    }
    if s.hinge_after_step != () && s.yaw_after_step != () {
        f.push(t_true((s.hinge_after_step - s.yaw_after_step).abs() < HINGE_TOL,
            "hinge angle does not follow SunTracker yaw"));
    }
    if s.power_after_step != () {
        f.push(t_true(s.power_after_step > 0.0,
            "solar panel reports no generated power after the environment sun step"));
    }
    if s.incidence_before != () && s.incidence_after_step != () {
        f.push(t_true((s.incidence_after_step - s.incidence_before).abs() > 0.02,
            "panel incidence did not respond to the environment sun step"));
    }
    if s.position_before_drive != () && s.position_after_drive != () {
        f.push(t_true(vlen(vsub(s.position_after_drive, s.position_before_drive)) > MOTION_MIN_M,
            "BT patrol did not move the rover while engaged"));
    }
    f.push(t_true(s.peak_motor_power > 0.0,
        "generated motor model reported no electrical draw under BT patrol"));
    if s.soc_before_drive != () && s.soc_after_drive != () {
        f.push(t_true(s.soc_before_drive - s.soc_after_drive > SOC_DROP_MIN,
            "battery SOC did not fall under motor load"));
    }
    if s.soc_after_drive != () && s.soc_after_charge != () {
        f.push(t_true(s.soc_after_charge - s.soc_after_drive > SOC_RECOVER_MIN,
            "battery SOC did not recover after the patrol stopped"));
    }
    if s.island_drive_left_after_stop != () && s.island_drive_right_after_stop != () {
        f.push(t_true(s.island_drive_left_after_stop.abs() < IDLE_DRIVE_TOL &&
                      s.island_drive_right_after_stop.abs() < IDLE_DRIVE_TOL,
            "disengaged autopilot left drive demand on the electrical island"));
    }

    print("[solar-cycle] SOC " + r4(s.soc_before_drive) + " -> " +
          r4(s.soc_after_drive) + " -> " + r4(s.soc_after_charge));
    print("[solar-cycle] yaw " + r4(s.yaw_before) + " -> " +
          r4(s.yaw_after_step) + ", hinge=" + r4(s.hinge_after_step));
    print("[solar-cycle] tracker sun_mount_x=" +
          r4(get(s.tracker, "sun_mount_x")));
    print("[solar-cycle] power " + r2(s.power_before) + "W -> " +
          r2(s.power_after_step) + "W, motor peak=" + r2(s.peak_motor_power) + "W");
    print("[solar-cycle] post-stop electrical inputs left/right=" +
          r4(s.island_drive_left_after_stop) + "/" + r4(s.island_drive_right_after_stop));
    report_verdict(f, "SOLAR ROVER ENERGY CYCLE", "SOLAR_ROVER_ENERGY_CYCLE");
}
