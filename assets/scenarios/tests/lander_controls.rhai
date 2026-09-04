// Production regression for the lander's semantic WASDQE control contract.
// The scenario drives the possessed production lander through SimulateIntent,
// then reads the same command ports consumed by the Modelica/Avian path. It
// does not duplicate RCS physics or alter the authored control profile.

const ROOT = "/LanderControlsTest";
const EPS = 0.0001;
const GRAVITY_Y = -1.62;
const GRAVITY_TOLERANCE = 0.05;

fn near(actual, expected) {
    actual != () && (actual - expected).abs() < EPS
}

fn send(intent, held, target) {
    let result = cmd("SimulateIntent", #{
        intent: intent,
        held: held,
        target: target,
    });
    result.ok
}

// A production lander has more than one component contributing ports with
// overlapping names. The root command surface is the first matching inbound
// port, which is also the endpoint that ControlBinding writes.
fn input_port(target, name) {
    let record = query("ReadPorts", #{ api_id: target });
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name && port.direction == "in" {
            return port.value;
        }
    }
    ()
}

fn output_port(target, name) {
    let record = query("ReadPorts", #{ api_id: target });
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name && port.direction == "out" {
            return port.value;
        }
    }
    ()
}

fn component_present(target, component) {
    get(target, component) != ()
}

fn has_no_render_camera(audit) {
    if audit == () || audit.candidates == () { return false; }
    for candidate in audit.candidates {
        if candidate.render_camera { return false; }
    }
    true
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.phase = 0;
    this.done = false;
    this.lander = find(ROOT + "/Lander");
    this.avatar = find(ROOT + "/Avatar");
    this.environment = find(ROOT + "/Lander/Environment");
    this.camera_follow = get(this.lander, "CameraFollow");
    this.orbit_entered = false;
    this.returned_to_follow = false;
    this.gravity_y_in_orbit = ();
    this.velocity_y_in_orbit = ();
    this.acceleration_valid_in_orbit = ();
    this.headless_without_render_camera = has_no_render_camera(
        query("SceneCameraAudit", #{}));

    if this.lander < 0 || this.avatar < 0 {
        this.done = true;
        report_verdict(["production lander or avatar did not compose"],
                       "LANDER CONTROLS", "LANDER_CONTROLS");
        return;
    }
    let possessed = cmd("PossessVessel", #{ target: this.lander });
    if !possessed.ok {
        this.done = true;
        report_verdict(["production lander could not be possessed: " + possessed.error],
                       "LANDER CONTROLS", "LANDER_CONTROLS");
        return;
    }

    // Camera behavior is a presentation transaction. Enter the real orbit rig
    // while keeping the same possession and physics participant, then exercise
    // the existing control surface through that mode. This catches accidental
    // camera→control coupling without adding a second physics path.
    let focused = cmd("FocusTarget", #{ target: this.lander });
    this.orbit_entered = focused.ok && component_present(this.avatar, "OrbitCamera");
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.25 { return; }
    this.elapsed = 0.0;

    if this.phase == 0 {
        if !send("forward", true, this.lander) {
            this.done = true;
            report_verdict(["forward intent was rejected"], "LANDER CONTROLS", "LANDER_CONTROLS");
            return;
        }
        this.phase = 1;
    } else if this.phase == 1 {
        this.forward_pitch = input_port(this.lander, "pitch");
        send("backward", true, this.lander);
        this.phase = 2;
    } else if this.phase == 2 {
        this.opposite_pitch = input_port(this.lander, "pitch");
        send("forward", false, this.lander);
        send("backward", false, this.lander);
        send("left", true, this.lander);
        send("yaw_left", true, this.lander);
        send("thrust", true, this.lander);
        this.phase = 3;
    } else if this.phase == 3 {
        this.roll = input_port(this.lander, "roll");
        this.yaw = input_port(this.lander, "yaw");
        this.throttle = input_port(this.lander, "external_throttle");
        this.gravity_y_in_orbit = output_port(this.environment, "gravity_y");
        this.velocity_y_in_orbit = output_port(this.lander, "velocity_y");
        this.acceleration_valid_in_orbit = output_port(this.lander, "acceleration_valid");
        send("left", false, this.lander);
        send("yaw_left", false, this.lander);
        send("thrust", false, this.lander);
        this.phase = 4;
    } else {
        let released_pitch = input_port(this.lander, "pitch");
        let released_roll = input_port(this.lander, "roll");
        let released_yaw = input_port(this.lander, "yaw");
        let released_throttle = input_port(this.lander, "external_throttle");
        let returned = cmd("ReturnFromOrbit", #{ target: this.avatar });
        this.returned_to_follow = returned.ok && component_present(this.avatar, "SpringArmCamera")
            && !component_present(this.avatar, "OrbitCamera");
        let pose = query("QueryEntity", #{ id: this.lander });
        let gravity_y = output_port(this.environment, "gravity_y");
        let acceleration_valid = output_port(this.lander, "acceleration_valid");
        let profile_ok = this.camera_follow == "Orbit";
        let checks = [
            t_true(profile_ok, "lander controls do not select the stable orbit camera"),
            t_true(this.orbit_entered,
                "FocusTarget did not enter the exclusive OrbitCamera mode"),
            t_true(this.headless_without_render_camera,
                "headless physics run unexpectedly depends on a render camera"),
            t_true(near(this.forward_pitch, -1.0), "forward did not produce body pitch -1"),
            t_true(near(this.opposite_pitch, 0.0), "opposite pitch inputs did not cancel"),
            t_true(near(this.roll, 1.0), "left did not produce body roll +1"),
            t_true(near(this.yaw, 1.0), "yaw_left did not produce body yaw +1"),
            t_true(near(this.throttle, 1.0), "thrust did not produce external throttle +1"),
            t_true(near(released_pitch, 0.0) && near(released_roll, 0.0)
                && near(released_yaw, 0.0) && near(released_throttle, 0.0),
                "release did not clear every lander command port"),
            t_true(this.gravity_y_in_orbit != ()
                && (this.gravity_y_in_orbit - GRAVITY_Y).abs() <= GRAVITY_TOLERANCE,
                "gravity telemetry disappeared or changed while OrbitCamera was active"),
            t_present(this.velocity_y_in_orbit,
                "vehicle velocity telemetry disappeared while OrbitCamera was active"),
            t_true(this.acceleration_valid_in_orbit != ()
                && this.acceleration_valid_in_orbit >= 1.0,
                "IMU validity telemetry disappeared while OrbitCamera was active"),
            t_true(this.returned_to_follow,
                "ReturnFromOrbit did not restore the possessed follow camera"),
            t_true(pose != () && pose.position_frame == "active_physics",
                "camera mode transition removed the lander from the active physics frame"),
            t_true(gravity_y != () && (gravity_y - GRAVITY_Y).abs() <= GRAVITY_TOLERANCE,
                "gravity telemetry was not live after returning from orbit"),
            t_true(acceleration_valid != () && acceleration_valid >= 1.0,
                "IMU validity telemetry was not live after returning from orbit"),
        ];
        this.done = true;
        report_verdict(checks, "LANDER CONTROLS", "LANDER_CONTROLS");
    }
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["lander control regression stopped before command ports were observed"],
                       "LANDER CONTROLS", "LANDER_CONTROLS");
    }
}
