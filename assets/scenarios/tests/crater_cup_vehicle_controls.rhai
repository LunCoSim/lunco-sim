// @timing presentation
const AVATAR = "/World/Avatar";
const COMET = "/World/Comet";
const LANDER = "/World/HopperLander";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness", "ReadPorts"], required_inputs: []}
}

fn require(result) {
    if !result.ok { throw result.to_string(); }
}

fn send(target, intents, held) {
    for intent in intents {
        require(cmd("SimulateIntent", #{target:target, intent:intent, held:held}));
    }
}

fn inputs(target, names) {
    let found = #{};
    for port in query("ReadPorts", #{api_id:target, port_names:names}).ports {
        if port.direction == "in" && port.metadata.source == "input surface" {
            found[port.name] = port.value;
        }
    }
    found
}

fn start_on_pad_camera(me) {
    this.avatar = find_path(AVATAR);
    this.comet = find_path(COMET);
    this.lander = find_path(LANDER);
    this.position = world_pos(this.avatar);
    this.heading = world_forward(this.avatar);
    this.checks = [];
    require(set_camera("/World/PadCam"));
}

fn drive_comet(me) {
    this.checks.push(t_true(current_camera_name() == "/World/PadCam", "The pad view is active before possession"));
    require(cmd("AcquireControl", #{target:this.comet, bind_camera:true}));
}

fn check_comet_view(me) {
    let view = current_camera_name();
    let rig = get(this.avatar, "SpringArmCamera");
    this.checks.push(t_true(is_controlled(this.comet) && view == AVATAR && rig != () && rig.attitude == "Heading",
        "Comet uses the avatar's heading-follow rig from the pad view, active camera " + view));
    send(this.comet, ["forward", "left"], true);
}

fn check_comet_inputs(me) {
    let values = inputs(this.comet, ["throttle", "steer", "brake"]);
    this.checks.push(t_true(values.throttle == 1.0 && values.steer == -1.0 && values.brake == 0.0,
        "The standard car bindings drive throttle and steering: " + values));
    this.checks.push(t_true(get(this.comet, "heading_fl") > 0.3 && get(this.comet, "speed") > 0.15,
        "Comet's wheels steer and move the chassis"));
    send(this.comet, ["forward", "left"], false);
    send(this.comet, ["brake"], true);
}

fn check_comet_brake(me) {
    let values = inputs(this.comet, ["throttle", "steer", "brake"]);
    this.checks.push(t_true(values.throttle == 0.0 && values.steer == 0.0 && values.brake == 1.0,
        "The standard brake binds and released drive inputs return to neutral: " + values));
    send(this.comet, ["brake"], false);
    require(cmd("AcquireControl", #{target:this.lander, bind_camera:true}));
}

fn check_lander_view(me) {
    let rig = get(this.avatar, "SpringArmCamera");
    this.checks.push(t_true(is_controlled(this.lander) && !is_controlled(this.comet)
        && current_camera_name() == AVATAR && rig != () && rig.attitude == "WorldLocked",
        "Kestrel uses the avatar's stabilized follow rig and releases Comet"));
    send(this.lander, ["forward", "left", "yaw_left", "thrust"], true);
}

fn check_lander_inputs(me) {
    let values = inputs(this.lander, ["pitch", "roll", "yaw", "external_throttle"]);
    this.checks.push(t_true(values.pitch == -1.0 && values.roll == 1.0
        && values.yaw == 1.0 && values.external_throttle == 1.0,
        "The standard lander bindings drive body attitude and thrust: " + values));
    send(this.lander, ["forward", "left", "yaw_left", "thrust"], false);
    require(cmd("ReleaseControlSource", #{source:this.avatar}));
}

fn finish(me) {
    let p = world_pos(this.avatar);
    let h = world_forward(this.avatar);
    this.checks.push(t_true(!is_controlled(this.lander) && current_camera_name() == AVATAR
        && vlen(vsub(p, this.position)) < 0.01 && vlen(vsub(h, this.heading)) < 0.001,
        "Releasing the lander restores the original avatar position and view"));
    this.checks.push(t_true(query("GetReadiness").ready && clock_snapshot().clock_contract_ok,
        "The simulation stays healthy through both control handoffs"));
    report_verdict(this.checks, "CRATER CUP VEHICLE CONTROLS", "CRATER_CUP_VEHICLE_CONTROLS");
}

fn task(me, ctx) {
    seq([wait_until(|m| query("GetReadiness").ready && current_camera_name() == AVATAR),
        once(Fn("start_on_pad_camera")), wait(0.35), once(Fn("drive_comet")),
        wait(0.45), once(Fn("check_comet_view")), wait(0.8), once(Fn("check_comet_inputs")),
        wait(0.5), once(Fn("check_comet_brake")),
        wait(0.45), once(Fn("check_lander_view")), wait(0.65), once(Fn("check_lander_inputs")),
        wait(0.45), once(Fn("finish"))])
}
