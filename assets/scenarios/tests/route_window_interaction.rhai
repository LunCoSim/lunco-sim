// Windowed production gate driven with injected typed Bevy window events. It
// verifies app behavior after event creation, not OS mouse/keyboard delivery.
//
// Pointer targets are projected from the live viewport before each gesture.
// The scenario sends typed window events, then observes the public selection,
// USD, control, and authored-program surfaces a user would see.

const TEST_KIND = "editor";

import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

const ROUTE = "/Traverse/Route";
const SUBJECT = "/Traverse/Rover";
// The chassis is coincident with W0's transparent annotation in this Twin.
// Use a visible rover panel as the possession target so the gesture
// tests the same unambiguous hit a human gets when clicking the rover.
const ROVER_TARGET = "/Traverse/Rover/SolarPanel/PanelSurface";
const ADD_X = 1000.0;
const ADD_Y = 700.0;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path(SUBJECT)], entity_reads: [], entity_writes: [],
       query_reads: ["InspectSelection", "QueryUsdPrim", "QueryUsdPrims", "ReadExposures",
           "ReadPorts", "ResolveUsdTarget"], required_inputs: [] }
}

fn route_points(doc) {
    if doc < 0 { return (); }
    let result = waypoint_editor::route_positions(doc, ROUTE);
    if result == () || result.ok != true { return (); }
    result.paths
}

fn finish(state) {
    state.done = true;
    report_verdict(state.checks,
        "ROUTE WINDOWED INJECTED INTERACTION", "ROUTE_WINDOWED_INJECTED_INTERACTION");
    state
}

fn fail(state, message) {
    if state.done { return state; }
    state.failure = message;
    state.checks.push(t_true(false, message));
    finish(state)
}

fn send_key(key, state) {
    input_key(key, state);
}

fn port_active(id) {
    let value = port(id, "program_active");
    value != () && value > 0.5
}

fn guidance_is_live(id) {
    let value = port(id, "guidance_throttle");
    value != () && value.abs() > 0.01
}

fn autopilot_command_state(id) {
    let values = #{
        program_active: port(id, "program_active"),
        autopilot_enable: port(id, "autopilot_enable"),
        target_x: port(id, "autopilot_target_x"),
        target_z: port(id, "autopilot_target_z"),
        speed: port(id, "autopilot_speed"),
        radius: port(id, "autopilot_radius"),
        turn_only: port(id, "autopilot_turn_only"),
    };
    for name in ["program_active", "autopilot_enable", "target_x", "target_z",
        "speed", "radius", "turn_only"] {
        if values[name] == () { return (); }
    }
    values
}

fn pose_distance(left, right) {
    if left == () || right == () || left.len() != 3 || right.len() != 3 {
        return ();
    }
    let dx = left[0] - right[0];
    let dy = left[1] - right[1];
    let dz = left[2] - right[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn autopilot_feedback_matches_pose(id, pose) {
    if pose == () || pose.len() != 3 { return false; }
    let x = port(id, "autopilot_position_x");
    let z = port(id, "autopilot_position_z");
    x != () && z != () && (x - pose[0]).abs() < 1.0 && (z - pose[2]).abs() < 1.0
}

fn vessel_hud_visible() {
    let result = query("ReadExposures", #{ surface: "vessel-hud" });
    if result == () || result.surfaces == () { return (); }
    let surface = result.surfaces["vessel-hud"];
    if surface == () || surface.visible == () { return (); }
    surface.visible
}

fn on_start(me, ctx) {
    this.done = false;
    this.failure = "";
    this.checks = [];
    this.waiting_for = "the route fixture and rover entities";
    this.doc = -1;
    this.subject = -1;
    this.before = ();
    this.after_add = ();
    this.added = ();
    this.after_second_add = ();
    this.added_second = ();
    this.marker = -1;
    this.second_marker = -1;
    this.action_key = "KeyF";
    this.forward_key = input_binding("forward");
    this.route_was_active = false;
    this.started = false;
    this.rover_x = 0.0;
    this.rover_y = 0.0;
    this.last_rover_x = 0.0;
    this.last_rover_y = 0.0;
    this.has_last_rover_screen = false;
    this.stable_rover_samples = 0;
    this.add_x = ADD_X;
    this.add_y = ADD_Y;
    this.second_add_x = 0.0;
    this.second_add_y = 0.0;
    this.sky_x = ADD_X;
    this.sky_y = 80.0;
    this.point_x = 0.0;
    this.point_y = 0.0;
    this.last_point_x = 0.0;
    this.last_point_y = 0.0;
    this.has_last_point_screen = false;
    this.stable_point_samples = 0;
    this.delete_x = 0.0;
    this.delete_y = 0.0;
    this.before_add_pose = ();
    this.before_second_add_pose = ();
    this.autopilot_before_release = ();
    this.rover_pose_before_release = ();
    this.pre_release_motion_start = ();
    this.pre_release_motion_elapsed = 0.0;
    this.pre_release_speed = 0.0;
    this.pre_release_snapshot_ready = false;
    this.release_elapsed = 0.0;
    this.release_verified = false;
    this.input_frame_samples = 0;
}

fn test_is_active(me) { !this.done }

fn timeout(me) {
    if !this.done {
        this = fail(this, "windowed route interaction timed out while "
            + this.waiting_for + " after 45 simulation seconds");
    }
}

fn begin_input_frame_boundary(me) { this.input_frame_samples = 0; }
fn advance_input_frame_boundary(me) { this.input_frame_samples += 1; }
fn input_frame_boundary_reached(me) { this.input_frame_samples > 1 }

fn next_input_frame() {
    seq([
        once(Fn("begin_input_frame_boundary")),
        step(Fn("advance_input_frame_boundary"), Fn("input_frame_boundary_reached")),
    ])
}

fn route_fixture_ready(me) {
    if this.doc != -1 && this.subject >= 0 && this.before != () { return true; }
    let route = query("QueryUsdPrim", #{ path: ROUTE });
    let subject = find_path(SUBJECT);
    if route == () || route.ok == false || route.doc_id == () {
        this.waiting_for = "a live route query: " + route.to_string();
        return false;
    }
    if subject < 0 {
        this.waiting_for = "the rover entity under " + SUBJECT;
        return false;
    }
    let positions = waypoint_editor::route_positions(route.doc_id, ROUTE);
    if positions == () || positions.ok != true {
        this.waiting_for = "composed route positions: " + positions.to_string();
        if positions != () && positions.retryable == false {
            this = fail(this, "route document does not expose an authored route: "
                + positions.error);
        }
        return false;
    }
    if positions.paths.len() == 0 {
        this = fail(this, "the Summer Space School route has no active waypoint children");
        return false;
    }
    this.doc = route.doc_id;
    this.subject = subject;
    this.before = positions.paths;
    this.waiting_for = "stable rover projection";
    true
}

fn reset_rover_projection(me) {
    this.last_rover_x = 0.0;
    this.last_rover_y = 0.0;
    this.has_last_rover_screen = false;
    this.stable_rover_samples = 0;
}

fn sample_rover_projection(me) {
    if this.done { return; }
    let target = find_path(ROVER_TARGET);
    if target < 0 { return; }
    let screen = viewport_position(target);
    if screen == () || screen.len() != 2 { return; }
    if this.has_last_rover_screen {
        let dx = screen[0] - this.last_rover_x;
        let dy = screen[1] - this.last_rover_y;
        if (dx * dx + dy * dy).sqrt() <= 2.0 {
            this.stable_rover_samples += 1;
        } else {
            this.stable_rover_samples = 0;
        }
    }
    this.last_rover_x = screen[0];
    this.last_rover_y = screen[1];
    this.has_last_rover_screen = true;
    this.rover_x = screen[0];
    this.rover_y = screen[1];
}

fn rover_projection_stable(me) {
    this.waiting_for = "stable rover projection samples="
        + this.stable_rover_samples.to_string();
    this.done || this.stable_rover_samples >= 3
}
fn move_pointer_to_rover(me) { if !this.done { input_pointer_move(this.rover_x, this.rover_y); } }
fn press_rover(me) { if !this.done { input_pointer_press("primary", this.rover_x, this.rover_y); } }
fn release_rover(me) { if !this.done { input_pointer_release("primary", this.rover_x, this.rover_y); } }

fn rover_is_possessed(me) {
    this.waiting_for = "rover possession and visible HUD";
    !this.done && is_controlled(this.subject) && vessel_hud_visible() == true
}

fn record_rover_possession(me) {
    if this.done { return; }
    this.checks.push(t_true(true, "the windowed app click controls the rover"));
    this.checks.push(t_true(true, "the vessel HUD is visible while the rover is possessed"));
    this.route_was_active = port_active(this.subject);
    this.waiting_for = "route initial state branch, active=" + this.route_was_active.to_string();
}

fn route_was_active(me) { this.route_was_active }
fn route_was_inactive(me) { !this.route_was_active }
fn press_action_key(me) { if !this.done { send_key(this.action_key, "Pressed"); } }
fn release_action_key(me) { if !this.done { send_key(this.action_key, "Released"); } }
fn route_is_stopped(me) {
    this.waiting_for = "the initially active route to stop, program_active="
        + port(this.subject, "program_active").to_string();
    this.done || !port_active(this.subject)
}

fn autopilot_is_live(me) {
    let active = port(this.subject, "program_active");
    let guidance = port(this.subject, "guidance_throttle");
    let label = get_exposure("vessel-hud", "program_label");
    this.waiting_for = "route program active=" + active.to_string()
        + " guidance=" + guidance.to_string()
        + " hud=" + vessel_hud_visible().to_string()
        + " label=" + label.to_string();
    if this.done || !port_active(this.subject) || !guidance_is_live(this.subject)
        || vessel_hud_visible() != true {
        return false;
    }
    label == "AUTOPILOT ON"
}

fn record_autopilot_started(me) {
    if this.done { return; }
    this.started = true;
    this.checks.push(t_true(this.started, "the configured F input starts the route program"));
    this.checks.push(t_true(true, "the authored route program produces live guidance"));
    this.checks.push(t_true(true, "the vessel HUD reflects the active route state"));
    this.pre_release_motion_start = world_pos(this.subject);
    if this.pre_release_motion_start == () || this.pre_release_motion_start.len() != 3 {
        this = fail(this, "could not read rover pose before measuring route motion");
    }
}

fn sample_pre_release_motion(me) {
    if this.done || this.pre_release_snapshot_ready { return; }
    this.pre_release_motion_elapsed += dt();
    if this.pre_release_motion_elapsed < 0.25
        || !port_active(this.subject) || !guidance_is_live(this.subject) {
        return;
    }
    let current_pose = world_pos(this.subject);
    let distance = pose_distance(this.pre_release_motion_start, current_pose);
    if distance == () {
        this = fail(this, "could not read rover pose while route guidance was active");
        return;
    }
    this.pre_release_speed = distance / this.pre_release_motion_elapsed;
    this.rover_pose_before_release = current_pose;
    this.autopilot_before_release = autopilot_command_state(this.subject);
    if this.autopilot_before_release == () {
        this = fail(this, "could not snapshot active autopilot inputs before releasing possession");
        return;
    }
    this.pre_release_snapshot_ready = true;
}

fn pre_release_motion_ready(me) {
    this.done || this.pre_release_snapshot_ready
}

fn press_escape_to_release(me) {
    if this.done { return; }
    this.release_elapsed = 0.0;
    send_key("Escape", "Pressed");
}

fn release_escape(me) { if !this.done { send_key("Escape", "Released"); } }

fn observe_possession_release(me) {
    if this.done || this.release_verified { return; }
    this.release_elapsed += dt();
    if is_controlled(this.subject) || !port_active(this.subject)
        || !guidance_is_live(this.subject) || vessel_hud_visible() != false {
        if this.release_elapsed > 5.0 {
            this = fail(this, "possession release did not preserve active route guidance and hide the vessel HUD");
        }
        return;
    }
    let after = autopilot_command_state(this.subject);
    if after == () {
        this = fail(this, "autopilot command ports were unavailable after releasing possession");
        return;
    }
    for name in ["program_active", "autopilot_enable", "target_x", "target_z",
        "speed", "radius", "turn_only"] {
        let before = this.autopilot_before_release[name];
        if (before == () || after[name] == () || (before - after[name]).abs() > 1e-9) {
            this = fail(this, "releasing possession changed autopilot input " + name);
            return;
        }
    }
    let pose = world_pos(this.subject);
    let displacement = pose_distance(this.rover_pose_before_release, pose);
    let plausible_speed = this.pre_release_speed * 3.0 + 5.0;
    let plausible_travel = 2.0 + plausible_speed * this.release_elapsed;
    if displacement == () || displacement > plausible_travel {
        this = fail(this, "releasing possession teleported the rover by "
            + displacement.to_string() + " m in " + this.release_elapsed.to_string() + " s");
        return;
    }
    if !autopilot_feedback_matches_pose(this.subject, pose) {
        this = fail(this, "route guidance stopped receiving the rover's current position after release");
        return;
    }
    this.checks.push(t_true(true,
        "possession release leaves route command and target inputs unchanged"));
    this.checks.push(t_true(true,
        "rover pose stays continuous while route guidance continues after release"));
    this.checks.push(t_true(true,
        "route guidance keeps receiving the rover position after release"));
    this.checks.push(t_true(true,
        "releasing possession keeps the route active and hides the vessel HUD"));
    this.release_verified = true;
}

fn release_observation_complete(me) { this.done || this.release_verified }

fn press_forward_unpossessed(me) {
    if this.done { return; }
    if this.forward_key == () {
        this = fail(this, "the active input profile has no forward key binding");
        return;
    }
    let result = input_key_press(this.forward_key);
    if result == () || result.ok != true {
        this = fail(this, "could not press the configured forward key while unpossessed");
    }
}

fn unpossessed_forward_is_harmless(me) {
    if this.done { return true; }
    if is_controlled(this.subject) {
        this = fail(this, "the rover regained possession while processing an unpossessed forward input");
        return false;
    }
    if vessel_hud_visible() != false { return false; }
    if !port_active(this.subject) || !guidance_is_live(this.subject) {
        this = fail(this, "an unpossessed forward key stopped the rover route");
        return false;
    }
    this.checks.push(t_true(true,
        "the configured forward key cannot take control or stop autopilot while unpossessed"));
    true
}

fn release_forward_unpossessed(me) {
    if this.done { return; }
    let result = input_key_release(this.forward_key);
    if result == () || result.ok != true {
        this = fail(this, "could not release the configured forward key");
    }
}

fn send_unpossessed_intent(me) {
    if this.done { return; }
    let sent = emit("intent.edge", #{ intent: "forward", edge: "pressed", target_gid: me });
    if !sent { this = fail(this, "could not deliver the unrelated unpossessed intent edge"); }
}

fn unpossessed_intent_is_harmless(me) {
    if this.done { return true; }
    if is_controlled(this.subject) {
        this = fail(this, "the rover regained possession while processing an unrelated intent edge");
        return false;
    }
    if vessel_hud_visible() != false { return false; }
    if !port_active(this.subject) || !guidance_is_live(this.subject) {
        this = fail(this, "an unrelated unpossessed intent edge stopped the rover route");
        return false;
    }
    this.checks.push(t_true(true,
        "an unrelated unpossessed intent edge cannot stop the active rover route"));
    true
}

fn move_pointer_to_rover_for_repossession(me) {
    if !this.done { input_pointer_move(this.rover_x, this.rover_y); }
}

fn rover_is_repossessed(me) {
    !this.done && is_controlled(this.subject) && port_active(this.subject)
        && guidance_is_live(this.subject) && vessel_hud_visible() == true
        && get_exposure("vessel-hud", "program_label") == "AUTOPILOT ON"
}

fn record_repossession(me) {
    if this.done { return; }
    this.checks.push(t_true(true, "the rover can be repossessed while autopilot stays active"));
    this.checks.push(t_true(true, "repossessing restores the vessel HUD with active route state"));
}

fn route_is_stopped_with_hud(me) {
    if this.done || port_active(this.subject) || vessel_hud_visible() != true { return false; }
    get_exposure("vessel-hud", "program_label") == "AUTOPILOT OFF"
}

fn record_route_stopped(me) {
    if !this.done {
        this.checks.push(t_true(true, "the configured F input stops the route and leaves the rover safe"));
        this.checks.push(t_true(true, "the vessel HUD reflects the inactive route state"));
    }
}

fn press_alt(me) { if !this.done { input_key_press("AltLeft"); } }
fn release_alt(me) { if !this.done { input_key_release("AltLeft"); } }
fn move_pointer_to_sky(me) {
    if !this.done { input_pointer_move(this.sky_x, this.sky_y); }
}
fn press_sky(me) {
    if !this.done { input_pointer_press("primary", this.sky_x, this.sky_y); }
}
fn release_sky(me) {
    if !this.done { input_pointer_release("primary", this.sky_x, this.sky_y); }
}

fn sky_click_is_ignored(me) {
    if this.done { return true; }
    let points = route_points(this.doc);
    if points == () { return false; }
    if points.len() != this.before.len() {
        this = fail(this, "Alt-clicking the sky changed the authored route");
        return false;
    }
    this.checks.push(t_true(true, "Alt-clicking the sky does not append a route point"));
    true
}

fn prepare_first_terrain_add(me) {
    if this.done { return; }
    this.add_x = this.rover_x + 240.0;
    this.add_y = this.rover_y - 120.0;
    this.before_add_pose = world_pos(this.subject);
}

fn move_pointer_to_first_add(me) {
    if !this.done { input_pointer_move(this.add_x, this.add_y); }
}
fn press_first_add(me) {
    if !this.done { input_pointer_press("primary", this.add_x, this.add_y); }
}
fn release_first_add(me) {
    if !this.done { input_pointer_release("primary", this.add_x, this.add_y); }
}

fn inspect_added_point(state, me, previous_paths, expected_count, click_x, click_y,
    before_pose, label) {
    if state.done { return #{ state: state, ready: false }; }
    let paths = route_points(state.doc);
    if paths == () || paths.len() < expected_count {
        return #{ state: state, ready: false };
    }
    if paths.len() > expected_count {
        state = fail(state, label + " created an unexpected number of route points");
        return #{ state: state, ready: false };
    }
    let added = ();
    for path in paths {
        if !previous_paths.contains(path) { added = path; break; }
    }
    if added == () {
        state = fail(state, label + " did not create a distinct route point");
        return #{ state: state, ready: false };
    }
    let marker = find_path(added + "/Dome");
    let point = find_path(added);
    if marker < 0 || point < 0 || !track_entity_read(marker) {
        return #{ state: state, ready: false };
    }
    let marker_screen = viewport_position(marker);
    let point_screen = viewport_position(point);
    let marker_look = get(marker, "PbrLook");
    if marker_screen == () || marker_screen.len() != 2 || point_screen == ()
        || point_screen.len() != 2 || marker_look == () {
        return #{ state: state, ready: false };
    }
    let marker_dx = point_screen[0] - click_x;
    let marker_dy = point_screen[1] - click_y;
    if (marker_dx * marker_dx + marker_dy * marker_dy).sqrt() >= 64.0 {
        state = fail(state, label + " did not stay anchored at the terrain click");
        return #{ state: state, ready: false };
    }
    if marker_look.unlit != true || marker_look.no_shadow_cast != true
        || marker_look.alpha != "Blend" {
        state = fail(state, label + " marker lost its translucent unlit render intent");
        return #{ state: state, ready: false };
    }
    if before_pose != () {
        let after_pose = world_pos(state.subject);
        let displacement = pose_distance(before_pose, after_pose);
        if displacement == () { return #{ state: state, ready: false }; }
        if displacement >= 5.0 {
            state = fail(state, label + " teleported the rover by "
                + displacement.to_string() + " m");
            return #{ state: state, ready: false };
        }
    }
    let ribbon = query("QueryUsdPrim", #{
        path: ROUTE + "/RouteRibbon/Geometry",
        attrs: ["wrap", "curveVertexCounts", "points"],
    });
    if ribbon == () || ribbon.ok != true || ribbon.attrs == ()
        || ribbon.attrs["wrap"] == () || ribbon.attrs["curveVertexCounts"] == ()
        || ribbon.attrs["points"] == () {
        return #{ state: state, ready: false };
    }
    if ribbon.attrs["wrap"] != "nonperiodic"
        || ribbon.attrs["curveVertexCounts"].len() != 1
        || ribbon.attrs["curveVertexCounts"][0] != ribbon.attrs["points"].len() {
        state = fail(state, label + " did not leave the route ribbon as an open polyline");
        return #{ state: state, ready: false };
    }
    if label == "first terrain add" {
        state.after_add = paths;
        state.added = added;
        state.marker = marker;
        state.checks.push(t_true(true, "Alt-click on terrain appends one authored route point"));
        state.checks.push(t_true(true, "the added point has a live viewport projection"));
        state.checks.push(t_true(true, "the added point stays anchored at the terrain click"));
        state.checks.push(t_true(true, "the added marker uses the authored unlit translucent style"));
        state.checks.push(t_true(true, "adding a route point does not teleport the rover"));
        state.checks.push(t_true(true, "the route ribbon remains an open polyline"));
    } else {
        state.after_second_add = paths;
        state.added_second = added;
        state.second_marker = marker;
        state.checks.push(t_true(true, "a second Alt-click appends another authored route point"));
        state.checks.push(t_true(true, "the second waypoint has an independent viewport projection"));
        state.checks.push(t_true(true, "the second point stays anchored at its terrain click"));
        state.checks.push(t_true(true, "the second marker uses the authored unlit translucent style"));
        state.checks.push(t_true(true, "adding a second route point does not teleport the rover"));
        state.checks.push(t_true(true, "the second route update remains an open polyline"));
    }
    #{ state: state, ready: true }
}

fn first_terrain_point_ready(me) {
    let result = inspect_added_point(this, me, this.before, this.before.len() + 1,
        this.add_x, this.add_y, this.before_add_pose, "first terrain add");
    this = result.state;
    result.ready
}

fn prepare_second_terrain_add(me) {
    if this.done { return; }
    this.second_add_x = this.add_x - 160.0;
    this.second_add_y = this.add_y;
    this.before_second_add_pose = world_pos(this.subject);
}
fn move_pointer_to_second_add(me) {
    if !this.done { input_pointer_move(this.second_add_x, this.second_add_y); }
}
fn press_second_add(me) {
    if !this.done { input_pointer_press("primary", this.second_add_x, this.second_add_y); }
}
fn release_second_add(me) {
    if !this.done { input_pointer_release("primary", this.second_add_x, this.second_add_y); }
}
fn second_terrain_point_ready(me) {
    let result = inspect_added_point(this, me, this.after_add, this.before.len() + 2,
        this.second_add_x, this.second_add_y, this.before_second_add_pose,
        "second terrain add");
    this = result.state;
    result.ready
}

fn reset_point_projection(me) {
    this.last_point_x = 0.0;
    this.last_point_y = 0.0;
    this.has_last_point_screen = false;
    this.stable_point_samples = 0;
}
fn sample_point_projection(me) {
    if this.done { return; }
    if this.marker < 0 { return; }
    let screen = viewport_position(this.marker);
    if screen == () || screen.len() != 2 { return; }
    if this.has_last_point_screen {
        let dx = screen[0] - this.last_point_x;
        let dy = screen[1] - this.last_point_y;
        if (dx * dx + dy * dy).sqrt() <= 2.0 {
            this.stable_point_samples += 1;
        } else {
            this.stable_point_samples = 0;
        }
    }
    this.last_point_x = screen[0];
    this.last_point_y = screen[1];
    this.has_last_point_screen = true;
    this.point_x = screen[0];
    this.point_y = screen[1];
}
fn point_projection_stable(me) { this.done || this.stable_point_samples >= 3 }

fn move_pointer_to_route_point(me) {
    if !this.done { input_pointer_move(this.point_x, this.point_y); }
}
fn press_route_context(me) {
    if !this.done { input_pointer_press("secondary", this.point_x, this.point_y); }
}
fn release_route_context(me) {
    if !this.done { input_pointer_release("secondary", this.point_x, this.point_y); }
}
fn prepare_delete_menu_row(me) {
    if this.done { return; }
    this.delete_x = this.point_x + 60.0;
    this.delete_y = this.point_y + 30.0;
    input_pointer_move(this.delete_x, this.delete_y);
}
fn press_delete_menu_row(me) {
    if !this.done { input_pointer_press("primary", this.delete_x, this.delete_y); }
}
fn release_delete_menu_row(me) {
    if !this.done { input_pointer_release("primary", this.delete_x, this.delete_y); }
}
fn deleted_route_point_is_visible(me) {
    if this.done { return true; }
    let paths = route_points(this.doc);
    if paths == () { return false; }
    if paths.len() > this.before.len() + 1 {
        this = fail(this, "context-menu Delete left an unexpected number of route points");
        return false;
    }
    if paths.len() != this.before.len() + 1 || paths.contains(this.added)
        || !paths.contains(this.added_second) {
        return false;
    }
    this.checks.push(t_true(true,
        "the injected context-menu Delete removes the first authored point and retains the second"));
    true
}

fn finish_route_window_interaction(me) {
    if this.done { return; }
    this = finish(this);
}

fn possession_and_start_route_tree() {
    seq([
        wait_until(Fn("route_fixture_ready")),
        once(Fn("reset_rover_projection")),
        step(Fn("sample_rover_projection"), Fn("rover_projection_stable")),
        once(Fn("move_pointer_to_rover")),
        next_input_frame(),
        once(Fn("press_rover")),
        next_input_frame(),
        once(Fn("release_rover")),
        wait_until(Fn("rover_is_possessed")),
        once(Fn("record_rover_possession")),
        sel([
            seq([
                check(Fn("route_was_active")),
                once(Fn("press_action_key")),
                next_input_frame(),
                once(Fn("release_action_key")),
                wait_until(Fn("route_is_stopped")),
            ]),
            check(Fn("route_was_inactive")),
        ]),
        once(Fn("press_action_key")),
        next_input_frame(),
        once(Fn("release_action_key")),
        wait_until(Fn("autopilot_is_live")),
        once(Fn("record_autopilot_started")),
    ])
}

fn release_and_repossess_route_tree() {
    seq([
        step(Fn("sample_pre_release_motion"), Fn("pre_release_motion_ready")),
        once(Fn("press_escape_to_release")),
        next_input_frame(),
        once(Fn("release_escape")),
        step(Fn("observe_possession_release"), Fn("release_observation_complete")),
        once(Fn("press_forward_unpossessed")),
        next_input_frame(),
        wait_until(Fn("unpossessed_forward_is_harmless")),
        once(Fn("release_forward_unpossessed")),
        next_input_frame(),
        once(Fn("send_unpossessed_intent")),
        next_input_frame(),
        wait_until(Fn("unpossessed_intent_is_harmless")),
        once(Fn("reset_rover_projection")),
        step(Fn("sample_rover_projection"), Fn("rover_projection_stable")),
        once(Fn("move_pointer_to_rover_for_repossession")),
        next_input_frame(),
        once(Fn("press_rover")),
        next_input_frame(),
        once(Fn("release_rover")),
        wait_until(Fn("rover_is_repossessed")),
        once(Fn("record_repossession")),
        once(Fn("press_action_key")),
        next_input_frame(),
        once(Fn("release_action_key")),
        wait_until(Fn("route_is_stopped_with_hud")),
        once(Fn("record_route_stopped")),
    ])
}

fn add_one_route_point_tree() {
    seq([
        once(Fn("prepare_first_terrain_add")),
        once(Fn("press_alt")),
        next_input_frame(),
        once(Fn("move_pointer_to_first_add")),
        next_input_frame(),
        once(Fn("press_first_add")),
        next_input_frame(),
        once(Fn("release_first_add")),
        next_input_frame(),
        once(Fn("release_alt")),
        wait_until(Fn("first_terrain_point_ready")),
    ])
}

fn add_second_route_point_tree() {
    seq([
        once(Fn("prepare_second_terrain_add")),
        once(Fn("press_alt")),
        next_input_frame(),
        once(Fn("move_pointer_to_second_add")),
        next_input_frame(),
        once(Fn("press_second_add")),
        next_input_frame(),
        once(Fn("release_second_add")),
        next_input_frame(),
        once(Fn("release_alt")),
        wait_until(Fn("second_terrain_point_ready")),
    ])
}

fn add_route_points_tree() {
    seq([
        once(Fn("press_alt")),
        next_input_frame(),
        once(Fn("move_pointer_to_sky")),
        next_input_frame(),
        once(Fn("press_sky")),
        next_input_frame(),
        once(Fn("release_sky")),
        next_input_frame(),
        once(Fn("release_alt")),
        wait_until(Fn("sky_click_is_ignored")),
        add_one_route_point_tree(),
        add_second_route_point_tree(),
    ])
}

fn delete_route_point_tree() {
    seq([
        once(Fn("reset_point_projection")),
        step(Fn("sample_point_projection"), Fn("point_projection_stable")),
        once(Fn("move_pointer_to_route_point")),
        next_input_frame(),
        once(Fn("press_route_context")),
        next_input_frame(),
        once(Fn("release_route_context")),
        next_input_frame(),
        once(Fn("prepare_delete_menu_row")),
        next_input_frame(),
        once(Fn("press_delete_menu_row")),
        next_input_frame(),
        once(Fn("release_delete_menu_row")),
        wait_until(Fn("deleted_route_point_is_visible")),
        once(Fn("finish_route_window_interaction")),
    ])
}

fn route_window_interaction_tree() {
    seq([
        possession_and_start_route_tree(),
        release_and_repossess_route_tree(),
        add_route_points_tree(),
        delete_route_point_tree(),
    ])
}

fn task(me, ctx) {
    par_race([
        force_ok(reactive_seq([
            check(Fn("test_is_active")),
            route_window_interaction_tree(),
        ])),
        seq([
            wait(45.0),
            once(Fn("timeout")),
        ]),
    ])
}
