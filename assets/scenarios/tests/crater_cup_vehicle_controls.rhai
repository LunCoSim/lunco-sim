// @timing presentation
const AVATAR = "/World/Avatar";
const COMET = "/World/Comet";
const LANDER = "/World/HopperLander";
const PAD = "/World/PadCam";
const SCOUT = "/World/Scout";

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

fn prepare_upward_view(me) {
    this.avatar = find_path(AVATAR);
    this.comet = find_path(COMET);
    this.lander = find_path(LANDER);
    this.look_point = viewport_position(this.lander);
    if this.look_point == () { throw "Kestrel must be visible from the starting avatar"; }
    input_pointer_move(this.look_point[0], this.look_point[1]);
}

fn start_on_pad_camera(me) {
    input_pointer_release("secondary", this.look_point[0], this.look_point[1]);
    this.position = world_pos(this.avatar);
    this.heading = world_forward(this.avatar);
    this.checks = [];
    this.checks.push(t_true(this.heading[1] > 0.5, "The free avatar looks up before opening PadCam"));
    require(set_camera(PAD));
}

fn vehicle_view_ready(name) {
    let cards = cmd_data("InspectGuidedHud", #{}).cards;
    cards.len() > 0 && cards[0].hint.starts_with(name)
        && current_camera_name() != "NO ACTIVE CAMERA"
}

fn control_interval_elapsed(start, seconds) {
    clock_snapshot().world_sim_s - start > seconds
}

fn drive_comet(me) {
    this.checks.push(t_true(current_camera_name() == PAD, "The pad view is active before possession"));
    require(cmd("AcquireControl", #{target:this.comet, bind_camera:true}));
}

fn check_comet_view(me) {
    let view = current_camera_name();
    let rig = get(this.avatar, "SpringArmCamera");
    this.checks.push(t_true(is_controlled(this.comet) && view == AVATAR && rig != () && rig.attitude == "Heading",
        "Comet uses the avatar's heading-follow rig from the pad view, active camera " + view));
    send(this.comet, ["forward", "left"], true);
    this.input_time = clock_snapshot().world_sim_s;
}

fn check_comet_inputs(me) {
    let values = inputs(this.comet, ["throttle", "steer", "brake"]);
    this.checks.push(t_true(values.throttle == 1.0 && values.steer == -1.0 && values.brake == 0.0,
        "The standard car bindings drive throttle and steering: " + values));
    let inner_heading = get(this.comet, "heading_fl");
    let outer_heading = get(this.comet, "heading_fr");
    let chassis_forward = world_forward(this.comet);
    let inner_forward = world_forward(find_path(COMET + "/SteeringKnuckle_FL"));
    let alignment = chassis_forward[0] * inner_forward[0] + chassis_forward[2] * inner_forward[2];
    this.checks.push(t_true(inner_heading > 0.68 && outer_heading > 0.4
        && inner_heading > outer_heading && alignment < 0.84 && get(this.comet, "speed") > 0.15,
        "Comet physically reaches the tighter Ackermann angle while moving"));
    send(this.comet, ["forward", "left"], false);
    send(this.comet, ["brake"], true);
    this.input_time = clock_snapshot().world_sim_s;
}

fn check_comet_brake(me) {
    let values = inputs(this.comet, ["throttle", "steer", "brake"]);
    this.checks.push(t_true(values.throttle == 0.0 && values.steer == 0.0 && values.brake == 1.0,
        "The standard brake binds and released drive inputs return to neutral: " + values));
    send(this.comet, ["brake"], false);
    require(cmd("ReleaseControlSource", #{source:this.avatar}));
    this.input_time = clock_snapshot().world_sim_s;
}

fn start_lander_from_pad(me) {
    this.checks.push(t_true(world_forward(this.avatar)[1] > 0.5,
        "Releasing Comet restores the upward overview before the lander handoff"));
    require(set_camera(PAD));
}

fn take_lander(me) {
    require(cmd("AcquireControl", #{target:this.lander, bind_camera:true}));
}

fn check_lander_view(me) {
    let rig = get(this.avatar, "SpringArmCamera");
    this.checks.push(t_true(is_controlled(this.lander) && !is_controlled(this.comet)
        && current_camera_name() == AVATAR && rig != () && rig.attitude == "WorldLocked",
        "Kestrel uses the avatar's stabilized follow rig and owns local control"));
    this.checks.push(t_true(rig != () && rig.pitch < 0.0 && world_up(this.avatar)[1] > 0.9
        && world_pos(this.avatar)[1] > world_pos(this.lander)[1] + 1.0,
        "Kestrel's follow camera starts upright above the surface even after an upward avatar view"));
    send(this.lander, ["forward", "left", "yaw_left", "thrust"], true);
    this.input_time = clock_snapshot().world_sim_s;
}

fn check_lander_inputs(me) {
    let values = inputs(this.lander, ["pitch", "roll", "yaw", "external_throttle"]);
    this.checks.push(t_true(values.pitch == -1.0 && values.roll == 1.0
        && values.yaw == 1.0 && values.external_throttle == 1.0,
        "The standard lander bindings drive body attitude and thrust: " + values));
    send(this.lander, ["forward", "left", "yaw_left", "thrust"], false);
    require(cmd("ReleaseControlSource", #{source:this.avatar}));
    this.input_time = clock_snapshot().world_sim_s;
}

fn start_scout_pivot(me) {
    let p = world_pos(this.avatar);
    let h = world_forward(this.avatar);
    this.checks.push(t_true(!is_controlled(this.lander) && current_camera_name() == AVATAR
        && vlen(vsub(p, this.position)) < 0.01 && vlen(vsub(h, this.heading)) < 0.001,
        "Releasing the lander restores the original avatar position and view"));
    this.scout = find_path(SCOUT);
    require(cmd("AcquireControl", #{target:this.scout, bind_camera:true}));
}

fn steer_scout(me) {
    this.scout_heading = world_forward(this.scout);
    send(this.scout, ["left"], true);
    this.input_time = clock_snapshot().world_sim_s;
}

fn finish(me) {
    this.checks.push(t_true(get(this.scout, "angvel_y") > 0.19
        && vlen(vsub(world_forward(this.scout), this.scout_heading)) > 0.18
        && get(this.scout, "speed") < 0.1,
        "Scout pivots under steering alone with little chassis translation"));
    send(this.scout, ["left"], false);
    require(cmd("ReleaseControlSource", #{source:this.avatar}));
    this.checks.push(t_true(query("GetReadiness").ready && clock_snapshot().clock_contract_ok,
        "The simulation stays healthy through the control handoffs"));
    report_verdict(this.checks, "CRATER CUP VEHICLE CONTROLS", "CRATER_CUP_VEHICLE_CONTROLS");
}

fn task(me, ctx) {
    seq([wait_until(|m| query("GetReadiness").ready && current_camera_name() == AVATAR
            && cmd_data("InspectGuidedHud", #{}).cards.len() > 0),
        once(Fn("prepare_upward_view")), wait(0.1),
        once(|m| input_pointer_press("secondary", this.look_point[0], this.look_point[1])), wait(0.1),
        once(|m| input_mouse_motion(0.0, -130.0)), wait(0.15),
        once(Fn("start_on_pad_camera")), wait_until(|m| current_camera_name() == PAD),
        once(Fn("drive_comet")),
        wait_until(|m| vehicle_view_ready("Comet") && current_camera_name() != PAD),
        once(Fn("check_comet_view")),
        wait_until(|m| control_interval_elapsed(this.input_time, 0.45)), once(Fn("check_comet_inputs")),
        wait_until(|m| control_interval_elapsed(this.input_time, 0.15)), once(Fn("check_comet_brake")),
        wait_until(|m| control_interval_elapsed(this.input_time, 0.2)
            && !is_controlled(this.comet) && current_camera_name() == AVATAR
            && get(this.avatar, "SpringArmCamera") == ()),
        once(Fn("start_lander_from_pad")), wait_until(|m| current_camera_name() == PAD), once(Fn("take_lander")),
        wait_until(|m| vehicle_view_ready("Kestrel") && current_camera_name() == AVATAR),
        once(Fn("check_lander_view")),
        wait_until(|m| control_interval_elapsed(this.input_time, 0.2)), once(Fn("check_lander_inputs")),
        wait_until(|m| control_interval_elapsed(this.input_time, 0.2)
            && current_camera_name() == AVATAR), once(Fn("start_scout_pivot")),
        wait_until(|m| vehicle_view_ready("Scout")), once(Fn("steer_scout")),
        wait_until(|m| control_interval_elapsed(this.input_time, 1.4)), once(Fn("finish"))])
}
