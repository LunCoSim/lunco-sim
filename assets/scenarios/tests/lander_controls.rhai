// Production regression for the lander's semantic WASDQE control contract.
// The scenario drives the possessed production lander through SimulateIntent,
// then reads the same command ports consumed by the Modelica/Avian path. It
// does not duplicate RCS physics or alter the authored control profile.
import "lunco://scripting/policy/runtime_ui_surface.rhai" as runtime_ui_policy;

const ROOT = "/LanderControlsTest";
const EPS = 0.0001;
const GRAVITY_Y = -1.62;
const GRAVITY_TOLERANCE = 0.05;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path(ROOT + "/Lander")],
       entity_reads: [find_path(ROOT + "/Avatar"), find_path(ROOT + "/Lander/Environment")], entity_writes: [],
       query_reads: ["SceneCameraAudit", "ReadExposures"], required_inputs: [] }
}

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
    let record = query("ReadPorts", #{ api_id: target, port_names: [name] });
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name && port.direction == "in" {
            return port.value;
        }
    }
    ()
}

fn output_port(target, name) {
    let record = query("ReadPorts", #{ api_id: target, port_names: [name] });
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

fn vehicle_control_hud() {
    let result = query("ReadExposures", #{surface: "vehicle-control-left"});
    if result == () || result.surfaces == () { return (); }
    result.surfaces["vehicle-control-left"]
}

fn has_no_render_camera(audit) {
    if audit == () || audit.candidates == () { return false; }
    for candidate in audit.candidates {
        if candidate.render_camera { return false; }
    }
    true
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.hud_wait_elapsed = 0.0;
    this.phase = 0;
    this.done = false;
    this.hud_phase = "await_hidden";
    this.return_transition_seen = false;
    this.original_return_transition_duration =
        get_setting("CameraInputSettings.orbit_return_transition_duration_s");
    let duration_update = cmd("SetCameraInput", #{ orbit_return_transition_duration_s: 1.2 });
    this.return_transition_configured = this.original_return_transition_duration != ()
        && duration_update.ok
        && near(get_setting("CameraInputSettings.orbit_return_transition_duration_s"), 1.2);
    this.lander = find_path(ROOT + "/Lander");
    this.avatar = find_path(ROOT + "/Avatar");
    this.environment = find_path(ROOT + "/Lander/Environment");
    this.camera_follow = get(this.lander, "CameraFollow");
    this.orbit_entered = false;
    this.returned_to_follow = false;
    this.gravity_y_in_orbit = ();
    this.velocity_y_in_orbit = ();
    this.acceleration_valid_in_orbit = ();
    this.headless_without_render_camera = has_no_render_camera(
        query("SceneCameraAudit", #{}));
    this.hud_visibility_checks = [
        t_true(!runtime_ui_policy::runtime_ui_should_be_visible(#{
            surface_id: "vehicle-control-left", available: true,
            visibility_mode: "possessed", control_owner: "none",
            status: "compiling", simulation_progress: [#{owner: "SceneLifecycle"}],
        }), "an unpossessed HUD stays hidden during startup preparation"),
        t_true(!runtime_ui_policy::runtime_ui_should_be_visible(#{
            surface_id: "vehicle-control-left", available: true,
            visibility_mode: "possessed", control_owner: "remote",
            status: "running", simulation_progress: [#{owner: "ModelicaPreparation"}],
        }), "remote control does not reveal the local HUD during preparation"),
        t_true(runtime_ui_policy::runtime_ui_should_be_visible(#{
            surface_id: "vehicle-control-left", available: true,
            visibility_mode: "possessed", control_owner: "local",
            status: "compiling", simulation_progress: [#{owner: "TerrainPreparation"}],
        }), "local possession keeps its HUD visible during preparation"),
        t_true(runtime_ui_policy::runtime_ui_should_be_visible(#{
            surface_id: "vehicle-control-left", available: true,
            visibility_mode: "possessed", control_owner: "local",
            status: "running", simulation_progress: [],
        }), "local possession keeps its HUD visible after preparation"),
        t_true(runtime_ui_policy::runtime_ui_should_be_visible(#{
            surface_id: "vehicle-control-left", available: true,
            visibility_mode: "always", control_owner: "none",
            status: "compiling", simulation_progress: [#{owner: "SceneReferences"}],
        }), "an available always-mode HUD remains visible during preparation"),
        t_true(!runtime_ui_policy::runtime_ui_should_be_visible(#{
            surface_id: "vehicle-control-left", available: false,
            visibility_mode: "always", control_owner: "none",
            status: "unavailable", simulation_progress: [],
        }), "always mode does not reveal a missing subject"),
    ];

    if this.lander < 0 || this.avatar < 0 {
        this.done = true;
        this.hud_visibility_checks.push(t_true(false, "production lander or avatar did not compose"));
        report_verdict(this.hud_visibility_checks,
                       "LANDER CONTROLS", "LANDER_CONTROLS");
        return;
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.hud_wait_elapsed += dt();
    let hud = vehicle_control_hud();
    if this.hud_phase == "await_hidden" {
        if this.hud_wait_elapsed > 30.0 {
            this.done = true;
            this.hud_visibility_checks.push(t_true(false,
                "the unpossessed HUD did not publish its initial hidden state"));
            report_verdict(this.hud_visibility_checks, "LANDER CONTROLS", "LANDER_CONTROLS");
            return;
        }
        if hud == () || hud.state == "pending" { return; }
        if hud.state != "ready" || hud.visible {
            this.done = true;
            this.hud_visibility_checks.push(t_true(false,
                "the composed unpossessed lander HUD must be ready and hidden"));
            report_verdict(this.hud_visibility_checks, "LANDER CONTROLS", "LANDER_CONTROLS");
            return;
        }
        this.hud_visibility_checks.push(t_true(!hud.visible,
            "the composed lander HUD is hidden before local possession"));
        let possessed = cmd("AcquireControl", #{ target: this.lander });
        if !possessed.ok {
            this.done = true;
            this.hud_visibility_checks.push(t_true(false,
                "production lander could not be possessed: " + possessed.error));
            report_verdict(this.hud_visibility_checks, "LANDER CONTROLS", "LANDER_CONTROLS");
            return;
        }
        this.hud_wait_elapsed = 0.0;
        this.hud_phase = "await_visible";
        return;
    }
    if this.hud_phase == "await_visible" {
        if this.hud_wait_elapsed > 30.0 {
            this.done = true;
            this.hud_visibility_checks.push(t_true(false,
                "local possession did not publish the lander HUD as visible"));
            report_verdict(this.hud_visibility_checks, "LANDER CONTROLS", "LANDER_CONTROLS");
            return;
        }
        if hud == () || hud.state == "pending" { return; }
        if hud.state != "ready" || !hud.visible { return; }
        this.hud_visibility_checks.push(t_true(hud.visible,
            "local possession reveals the composed lander HUD"));
        // Camera behavior is a presentation transaction. Enter the real orbit
        // rig while keeping the same possession and physics participant, then
        // exercise controls through that mode.
        let focused = cmd("FocusTarget", #{ target: this.lander });
        this.focus_requested = focused.ok;
        this.hud_phase = "running";
        return;
    }
    this.elapsed += dt();
    if this.elapsed < 0.25 { return; }
    this.elapsed = 0.0;

    if this.phase == 0 {
        // Commands are queued transactions. Observe realization on a later
        // tick rather than treating an acknowledgement as immediate state.
        this.orbit_entered = this.focus_requested && component_present(this.avatar, "OrbitCamera")
            && !component_present(this.avatar, "SpringArmCamera");
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
    } else if this.phase == 4 {
        let returned = cmd("ReturnFromOrbit", #{ camera: this.avatar });
        this.return_requested = returned.ok;
        this.phase = 5;
    } else if this.phase == 5 {
        this.return_transition_seen = component_present(this.avatar, "CameraPoseLock")
            && !component_present(this.avatar, "SpringArmCamera");
        this.phase = 6;
    } else if this.phase == 6 {
        if component_present(this.avatar, "CameraPoseLock") { return; }
        this.phase = 7;
    } else {
        let released_pitch = input_port(this.lander, "pitch");
        let released_roll = input_port(this.lander, "roll");
        let released_yaw = input_port(this.lander, "yaw");
        let released_throttle = input_port(this.lander, "external_throttle");
        this.returned_to_follow = this.return_requested && component_present(this.avatar, "SpringArmCamera")
            && !component_present(this.avatar, "OrbitCamera");
        let pose = query("QueryEntity", #{ id: this.lander });
        let gravity_y = output_port(this.environment, "gravity_y");
        let acceleration_valid = output_port(this.lander, "acceleration_valid");
        let profile_ok = this.camera_follow == "Orbit";
        let checks = this.hud_visibility_checks + [
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
            t_true(this.return_transition_configured,
                "orbit return transition duration could not be set for the production check"),
            t_true(this.return_transition_seen,
                "ReturnFromOrbit did not hold the camera in its animated transition"),
            t_true(pose != () && pose.position_frame == "active_physics",
                "camera mode transition removed the lander from the active physics frame"),
            t_true(gravity_y != () && (gravity_y - GRAVITY_Y).abs() <= GRAVITY_TOLERANCE,
                "gravity telemetry was not live after returning from orbit"),
            t_true(acceleration_valid != () && acceleration_valid >= 1.0,
                "IMU validity telemetry was not live after returning from orbit"),
        ];
        cmd("SetCameraInput", #{
            orbit_return_transition_duration_s: this.original_return_transition_duration
        });
        this.done = true;
        report_verdict(checks, "LANDER CONTROLS", "LANDER_CONTROLS");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        if this.original_return_transition_duration != () {
            cmd("SetCameraInput", #{
                orbit_return_transition_duration_s: this.original_return_transition_duration
            });
        }
        this.done = true;
        report_verdict(["lander control regression stopped before command ports were observed"],
                       "LANDER CONTROLS", "LANDER_CONTROLS");
    }
}
