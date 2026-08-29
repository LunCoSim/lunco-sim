// Production regression for the lander's semantic WASDQE control contract.
// The scenario drives the possessed production lander through SimulateIntent,
// then reads the same command ports consumed by the Modelica/Avian path. It
// does not duplicate RCS physics or alter the authored control profile.

const ROOT = "/LanderControlsTest";
const EPS = 0.0001;

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
        if port.name == name && port.direction == "In" {
            return port.value;
        }
    }
    ()
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.phase = 0;
    this.done = false;
    this.lander = find(ROOT + "/Lander");
    this.avatar = find(ROOT + "/Avatar");
    this.camera_follow = get(this.lander, "CameraFollow");

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
    }
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
        send("left", false, this.lander);
        send("yaw_left", false, this.lander);
        send("thrust", false, this.lander);
        this.phase = 4;
    } else {
        let released_pitch = input_port(this.lander, "pitch");
        let released_roll = input_port(this.lander, "roll");
        let released_yaw = input_port(this.lander, "yaw");
        let released_throttle = input_port(this.lander, "external_throttle");
        let profile_ok = this.camera_follow == "Orbit";
        let checks = [
            t_true(profile_ok, "lander controls do not select the stable orbit camera"),
            t_true(near(this.forward_pitch, -1.0), "forward did not produce body pitch -1"),
            t_true(near(this.opposite_pitch, 0.0), "opposite pitch inputs did not cancel"),
            t_true(near(this.roll, 1.0), "left did not produce body roll +1"),
            t_true(near(this.yaw, 1.0), "yaw_left did not produce body yaw +1"),
            t_true(near(this.throttle, 1.0), "thrust did not produce external throttle +1"),
            t_true(near(released_pitch, 0.0) && near(released_roll, 0.0)
                && near(released_yaw, 0.0) && near(released_throttle, 0.0),
                "release did not clear every lander command port"),
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
