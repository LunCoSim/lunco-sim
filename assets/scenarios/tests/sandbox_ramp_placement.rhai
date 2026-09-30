// Two rover poses overlap a late-projected collider and must pause in place;
// the magenta rover's support probes reach 5 cm below the ground plane, yet it
// stays admitted and drives from its nominal rest pose.

const SETTLE_S = 2.0;
const DRIVE_S = 28.0;
const MIN_PROGRESS_M = 15.0;
const MIN_RISE_M = 1.0;
const RED_PATH = "/SandboxRampPlacement/Skid_Raycast_1";
const GREEN_PATH = "/SandboxRampPlacement/Skid_Raycast_2";
const FLAT_PATH = "/SandboxRampPlacement/Skid_Battery_Thermal_1";
const SUPPORT_PATH = "/SandboxRampPlacement/ZSupport";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path(FLAT_PATH)],
       entity_reads: [find_path(RED_PATH), find_path(GREEN_PATH), find_path(FLAT_PATH),
                      find_path(SUPPORT_PATH)],
       entity_writes: [find_path(FLAT_PATH)],
       query_reads: ["QueryPhysicsState", "QueryUsdPrim", "RuntimeDiagnostics"],
       required_inputs: [] }
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn initialization_result(id, path, expected_position, should_pause) {
    let state = query("QueryPhysicsState", #{ id: id });
    if !query_ok(state) || state.physics_initialization_pending != false {
        return [false, false];
    }
    if should_pause {
        let pose = query("QueryUsdPrim", #{ path: path, attrs: [] });
        let diagnostics = query("RuntimeDiagnostics", #{});
        let has_warning = false;
        if query_ok(diagnostics) {
            for finding in diagnostics.findings {
                if finding.code == "physics-initialization-paused"
                    && finding.severity == "warning" && finding.subject == path {
                    has_warning = true;
                }
            }
        }
        let stayed_authored = query_ok(pose)
            && pose.position_frame == "active_physics"
            && (pose.world_position[0] - expected_position[0]).abs() <= 0.001
            && (pose.world_position[1] - expected_position[1]).abs() <= 0.001
            && (pose.world_position[2] - expected_position[2]).abs() <= 0.001;
        let paused = state.physics_initialization_paused == true
            && state.physics_initialization_invalid == true
            && state.physics_object_paused == true
            && state.physics_state_pending == false
            && state.rigid_body_disabled == true
            && state.collider_disabled == true
            && state.physics_initialization_penetration_m > 0.0
            && stayed_authored && has_warning;
        return [true, paused];
    }
    let admitted = state.physics_initialization_paused == false
        && state.physics_initialization_invalid == false
        && state.physics_object_paused == false
        && state.physics_state_pending == false
        && state.rigid_body_disabled == false
        && state.collider_disabled == false
        && state.body_mode == "dynamic";
    [true, admitted]
}

fn verdict(pass, reason) {
    if pass {
        emit("SANDBOX_RAMP_PLACEMENT", "PASS");
    } else {
        print("SANDBOX_RAMP_PLACEMENT FAIL: " + reason);
        emit("SANDBOX_RAMP_PLACEMENT", "FAIL");
    }
}

fn on_start(me, ctx) {
    this.t = 0.0;
    this.done = false;
    this.red = find_path(RED_PATH);
    this.green = find_path(GREEN_PATH);
    this.flat = find_path(FLAT_PATH);
    this.support = find_path(SUPPORT_PATH);
    this.ramp = find_path("/SandboxRampPlacement/Ramp");
    this.start = ();
    if this.flat >= 0 { this.start = world_pos(this.flat); }
    this.min_y = 0.0;
    if this.start != () { this.min_y = this.start[1]; }
    this.red_checked = false;
    this.green_checked = false;
    this.flat_checked = false;
    this.red_paused_in_place = false;
    this.green_paused_in_place = false;
    this.flat_admitted = false;
    this.support_collider_ready = false;
    this.possession = ();
    this.drive_command = ();
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.t += dt();
    if this.red >= 0 && !this.red_checked {
        let result = initialization_result(this.red, RED_PATH, [1000.0, 5.0, -10.0], true);
        if result[0] { this.red_checked = true; this.red_paused_in_place = result[1]; }
    }
    if this.green >= 0 && !this.green_checked {
        let result = initialization_result(this.green, GREEN_PATH, [1000.0, 5.0, 10.0], true);
        if result[0] { this.green_checked = true; this.green_paused_in_place = result[1]; }
    }
    if this.flat >= 0 && !this.flat_checked {
        let result = initialization_result(this.flat, FLAT_PATH, (), false);
        if result[0] { this.flat_checked = true; this.flat_admitted = result[1]; }
    }
    if this.red < 0 || this.green < 0 || this.flat < 0 || this.support < 0
        || this.ramp < 0 || this.start == () {
        this.done = true;
        verdict(false, "the composed sandbox ramp fixture is missing a rover, ramp, or flat-rover pose");
        return;
    }
    let support_state = query("QueryPhysicsState", #{ id: this.support });
    if query_ok(support_state) {
        this.support_collider_ready = support_state.body_mode == "static"
            && support_state.collider_present == true;
    }
    if this.t < SETTLE_S { return; }
    if this.possession == () {
        this.possession = cmd("AcquireControl", #{ target: this.flat, bind_camera: false });
    }
    let command = drive(this.flat, 0.55, 0.0);
    if this.drive_command == () { this.drive_command = command; }
    let p = world_pos(this.flat);
    if p != () && p[1] < this.min_y { this.min_y = p[1]; }
    if this.t >= SETTLE_S + DRIVE_S {
        this.done = true;
        drive(this.flat, 0.0, 0.0);
        let red_final = initialization_result(this.red, RED_PATH, [1000.0, 5.0, -10.0], true);
        let green_final = initialization_result(this.green, GREEN_PATH, [1000.0, 5.0, 10.0], true);
        let flat_final = initialization_result(this.flat, FLAT_PATH, (), false);
        let flat_support = query("QueryPhysicsState", #{ id: this.flat });
        let flat_probe_overtravel = 0.0;
        if query_ok(flat_support) && flat_support.support_contacts.len() > 0 {
            let contact = flat_support.support_contacts[0];
            flat_probe_overtravel = -(this.start[1] + contact.probe_origin[1]
                - contact.probe_length_m);
        }
        let flat_shallow_overlap = (flat_probe_overtravel - 0.05).abs() <= 0.001;
        let support_prim = query("QueryUsdPrim", #{
            path: SUPPORT_PATH, attrs: ["xformOp:rotateXYZ"], collision_bounds: true,
        });
        let support_rotation = ();
        let support_bounds = ();
        if query_ok(support_prim) {
            support_rotation = support_prim.attrs["xformOp:rotateXYZ"];
            support_bounds = support_prim.collision_bounds;
        }
        let support_inclined = support_rotation != () && support_bounds != ()
            && support_bounds.fidelity == "exact"
            && (support_rotation[2] - 17.1887).abs() <= 0.001;
        let progress = 0.0;
        let rise = 0.0;
        if p != () { progress = p[0] - this.start[0]; rise = p[1] - this.start[1]; }
        print("[sandbox-ramp] flat start=" + this.start + " final=" + p
            + " progress_x=" + progress + " rise_y=" + rise
            + " red_paused=" + this.red_paused_in_place
            + " green_paused=" + this.green_paused_in_place
            + " support_inclined=" + support_inclined
            + " flat_probe_overtravel=" + flat_probe_overtravel
            + " flat_admitted=" + this.flat_admitted);
        report_verdict([
            t_true(this.possession != () && this.possession.ok == true,
                "the flat rover received control"),
            t_true(this.drive_command != () && this.drive_command.ok == true
                && this.drive_command.status == "applied",
                "the flat rover throttle command was applied"),
            t_true(this.red_checked && this.red_paused_in_place && red_final[0] && red_final[1],
                "the first rover inside the ramp stayed paused at its authored pose with a diagnostic"),
            t_true(this.green_checked && this.green_paused_in_place && green_final[0] && green_final[1],
                "the second rover inside the ramp stayed paused at its authored pose with a diagnostic"),
            t_true(this.support_collider_ready,
                "the late-projected USD support collider entered physics before overlap validation"),
            t_true(support_inclined,
                "the late-projected support collider retains its authored inclined geometry"),
            t_true(flat_shallow_overlap,
                "the normal magenta rover has 5 cm wheel-probe overtravel at nominal rest"),
            t_true(this.flat_checked && this.flat_admitted && flat_final[0] && flat_final[1],
                "the shallow ground overlap is admitted without pausing the normal rover"),
            t_true(p != () && progress > MIN_PROGRESS_M && rise > MIN_RISE_M,
                "the admitted flat rover climbed the authored ramp"),
        ], "SANDBOX RAMP PLACEMENT", "SANDBOX_RAMP_PLACEMENT");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        verdict(false, "the scenario stopped before both ramp pauses and the flat-rover traverse were observed");
    }
}
