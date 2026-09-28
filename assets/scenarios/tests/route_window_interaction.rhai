// Headful acceptance for the native user path in a loaded Twin.
//
// Pointer targets are projected from the live viewport before each gesture.
// The scenario sends typed window events, then observes the public selection,
// USD, control, and authored-program surfaces a user would see.

const TEST_KIND = "editor";

import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

const ROUTE = "/Traverse/Route";
const SUBJECT = "/Traverse/Rover";
// The chassis is coincident with W0's transparent annotation in this Twin.
// Use a visible rover panel as the native possession target so the gesture
// tests the same unambiguous hit a human gets when clicking the rover.
const ROVER_TARGET = "/Traverse/Rover/SolarPanel/PanelSurface";
const ADD_X = 1000.0;
const ADD_Y = 700.0;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path(SUBJECT)], entity_reads: [], entity_writes: [],
       query_reads: ["InspectSelection", "QueryUsdPrim", "ReadExposures", "ReadPorts",
           "ResolveUsdTarget"], required_inputs: [] }
}

fn route_points() {
    let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
    if route == () || route.ok == false || route.children == () { return (); }
    let points = [];
    for path in route.children {
        if waypoint_editor::is_route_point(route.doc_id, path) {
            let record = query("QueryUsdPrim", #{ path: path });
            if record != () && record.ok != false && record.active == true {
                points.push(path);
            }
        }
    }
    points
}

fn finish(checks) {
    report_verdict(checks, "NATIVE ROUTE WINDOW INTERACTION", "NATIVE_ROUTE_WINDOW_INTERACTION");
}

fn fail(checks, message) {
    checks.push(t_true(false, message));
    report_verdict(checks,
        "NATIVE ROUTE WINDOW INTERACTION", "NATIVE_ROUTE_WINDOW_INTERACTION");
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
    this.phase = 0;
    this.elapsed = 0.0;
    this.phase_elapsed = 0.0;
    this.done = false;
    this.checks = [];
    this.phase_detail = "scene setup pending";
    this.doc = -1;
    this.subject = -1;
    this.before = ();
    this.after_add = ();
    this.added = ();
    this.after_second_add = ();
    this.added_second = ();
    this.marker = -1;
    this.second_marker = -1;
    this.before_add_pose = ();
    this.before_second_add_pose = ();
    this.selection_ok = false;
    this.route_was_active = false;
    this.started = false;
    this.start_release_sent = false;
    this.stop_requested = false;
    this.menu_requested = false;
    this.action_key = "KeyF";
    this.forward_key = input_binding("forward");
    this.rover_x = 0.0;
    this.rover_y = 0.0;
    this.last_rover_x = 0.0;
    this.last_rover_y = 0.0;
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
    this.stable_point_samples = 0;
    this.delete_x = 0.0;
    this.delete_y = 0.0;
    this.autopilot_before_release = ();
    this.rover_pose_before_release = ();
    this.pre_release_motion_start = ();
    this.pre_release_motion_elapsed = 0.0;
    this.pre_release_speed = 0.0;
    this.release_elapsed = 0.0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    this.phase_elapsed += dt();
    if this.phase == 60 || this.phase == 61 {
        this.release_elapsed += dt();
    }
    if this.phase == 66 {
        this.pre_release_motion_elapsed += dt();
    }
    if this.elapsed > 45.0 {
        this.done = true;
        fail(this.checks, "native route interaction timed out in phase " + this.phase + ": "
            + this.phase_detail);
        return;
    }
    if this.phase_elapsed < 0.25 { return; }
    this.phase_elapsed = 0.0;

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
        this.subject = find_path(SUBJECT);
        if route == () || route.ok == false || route.doc_id == () || this.subject < 0 {
            this.phase_detail = "route query or rover projection is unavailable: route="
                + route.to_string() + " subject=" + this.subject.to_string();
            return;
        }
        this.doc = route.doc_id;
        this.before = route_points();
        if this.before == () || this.before.len() == 0 {
            let point_details = [];
            for path in route.children {
                let point = waypoint_editor::is_route_point(route.doc_id, path);
                let record = query("QueryUsdPrim", #{ path: path });
                let active = record != () && record.ok != false && record.active == true;
                point_details.push(path + "{route_point=" + point.to_string()
                    + ",active=" + active.to_string() + "}");
            }
            this.phase_detail = "route has no active waypoint children doc="
                + route.doc_id.to_string() + " children=" + point_details.to_string();
            return;
        }
        let rover_screen = viewport_position(find_path(ROVER_TARGET));
        if rover_screen == () || rover_screen.len() != 2 {
            this.phase_detail = "rover panel has no viewport projection: "
                + rover_screen.to_string();
            return;
        }
        if this.last_rover_x != 0.0 {
            let dx = rover_screen[0] - this.last_rover_x;
            let dy = rover_screen[1] - this.last_rover_y;
            if (dx * dx + dy * dy).sqrt() <= 2.0 {
                this.stable_rover_samples += 1;
            } else {
                this.stable_rover_samples = 0;
            }
        }
        this.last_rover_x = rover_screen[0];
        this.last_rover_y = rover_screen[1];
        this.phase_detail = "rover screen samples=" + this.stable_rover_samples.to_string()
            + " at " + rover_screen.to_string();
        if this.stable_rover_samples < 3 { return; }
        this.rover_x = rover_screen[0];
        this.rover_y = rover_screen[1];
        input_pointer_move(this.rover_x, this.rover_y);
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        input_pointer_press("primary", this.rover_x, this.rover_y);
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        input_pointer_release("primary", this.rover_x, this.rover_y);
        this.phase = 3;
        return;
    }

    if this.phase == 3 {
        // In the attended viewport a plain click is the possession gesture:
        // the avatar owns the clicked vessel, while editor selection remains
        // a separate modified gesture. Test the authoritative control state
        // that the F action consumes rather than conflating the two.
        this.selection_ok = is_controlled(this.subject);
        if !this.selection_ok { return; }
        if vessel_hud_visible() != true { return; }
        this.checks.push(t_true(true, "native primary click controls the rover"));
        this.checks.push(t_true(true, "the vessel HUD is visible while the rover is possessed"));
        this.route_was_active = port_active(this.subject);
        if this.route_was_active {
            send_key(this.action_key, "Pressed");
            this.stop_requested = true;
            this.phase = 4;
        } else {
            send_key(this.action_key, "Pressed");
            this.start_release_sent = false;
            this.phase = 5;
        }
        return;
    }

    if this.phase == 4 {
        send_key(this.action_key, "Released");
        this.phase = 5;
        return;
    }

    if this.phase == 5 {
        if this.stop_requested {
            if port_active(this.subject) { return; }
            this.stop_requested = false;
            send_key(this.action_key, "Pressed");
            this.start_release_sent = false;
            this.phase = 6;
            return;
        }
        send_key(this.action_key, "Released");
        this.started = true;
        this.start_release_sent = true;
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        if !port_active(this.subject) { return; }
        if !this.start_release_sent {
            send_key(this.action_key, "Released");
            this.start_release_sent = true;
            return;
        }
        // `program_active` is the accepted command state; the Modelica
        // output is updated on its worker step. Wait for that authoritative
        // continuous result instead of sampling the previous zero at the
        // same lifecycle boundary.
        if !guidance_is_live(this.subject) { return; }
        this.checks.push(t_true(this.started,
            "native F sends the configured action edge"));
        this.checks.push(t_true(true,
            "the authored route program produces a live guidance command"));
        let active_label = get_exposure("vessel-hud", "program_label");
        if active_label == () || active_label != "AUTOPILOT ON" { return; }
        if vessel_hud_visible() != true { return; }
        this.checks.push(t_true(true,
            "the vessel HUD reflects the authoritative active program state"));
        this.pre_release_motion_start = world_pos(this.subject);
        if this.pre_release_motion_start == () {
            this.done = true;
            fail(this.checks, "could not read rover pose before measuring route motion");
            return;
        }
        this.phase = 66;
        this.phase_elapsed = 0.0;
        this.pre_release_motion_elapsed = 0.0;
        return;
    }

    if this.phase == 66 {
        if !port_active(this.subject) || !guidance_is_live(this.subject) { return; }
        let current_pose = world_pos(this.subject);
        let distance = pose_distance(this.pre_release_motion_start, current_pose);
        if distance == () || this.pre_release_motion_elapsed <= 0.0 { return; }
        this.pre_release_speed = distance / this.pre_release_motion_elapsed;
        this.rover_pose_before_release = current_pose;
        this.autopilot_before_release = autopilot_command_state(this.subject);
        if this.autopilot_before_release == () {
            this.done = true;
            fail(this.checks, "could not snapshot active autopilot inputs before release");
            return;
        }
        // Possession and route execution have separate lifecycles. Releasing
        // the local avatar must hide its HUD while the route keeps guiding the
        // rover; a plain click must then reacquire that same rover.
        this.release_elapsed = 0.0;
        this.phase_elapsed = 0.0;
        send_key("Escape", "Pressed");
        this.phase = 60;
        return;
    }

    if this.phase == 60 {
        send_key("Escape", "Released");
        this.phase = 61;
        return;
    }

    if this.phase == 61 {
        if is_controlled(this.subject) || !port_active(this.subject)
            || !guidance_is_live(this.subject) {
            return;
        }
        if vessel_hud_visible() != false { return; }
        let autopilot_after_release = autopilot_command_state(this.subject);
        if autopilot_after_release == () {
            this.done = true;
            fail(this.checks, "autopilot command ports were unavailable after releasing possession");
            return;
        }
        for name in ["program_active", "autopilot_enable", "target_x", "target_z",
            "speed", "radius", "turn_only"] {
            let before = this.autopilot_before_release[name];
            let after = autopilot_after_release[name];
            if (before - after).abs() > 1e-9 {
                this.done = true;
                fail(this.checks, "releasing possession changed autopilot input " + name + " from "
                    + before.to_string() + " to " + after.to_string());
                return;
            }
        }
        this.checks.push(t_true(true,
            "releasing possession leaves the active route command and target inputs unchanged"));
        let rover_pose_after_release = world_pos(this.subject);
        let release_displacement = pose_distance(
            this.rover_pose_before_release,
            rover_pose_after_release,
        );
        let plausible_speed = this.pre_release_speed * 3.0 + 5.0;
        let plausible_travel = 2.0 + plausible_speed * this.release_elapsed;
        if release_displacement == () || release_displacement > plausible_travel {
            this.done = true;
            fail(this.checks, "releasing possession teleported the rover by "
                + release_displacement.to_string() + " m in "
                + this.release_elapsed.to_string() + " s");
            return;
        }
        this.checks.push(t_true(true,
            "rover pose remains continuous while the route runs through possession release"));
        if !autopilot_feedback_matches_pose(this.subject, rover_pose_after_release) {
            this.done = true;
            fail(this.checks, "releasing possession cleared the rover position feedback used by route guidance");
            return;
        }
        this.checks.push(t_true(true,
            "route guidance keeps receiving the rover's current position after release"));
        this.checks.push(t_true(true,
            "releasing possession keeps the route active and hides the vessel HUD"));
        if this.forward_key == () {
            this.done = true;
            fail(this.checks, "the active input profile has no forward key binding");
            return;
        }
        let pressed = input_key_press(this.forward_key);
        if pressed == () || pressed.ok != true {
            this.done = true;
            fail(this.checks, "could not press the configured forward key while unpossessed: "
                + pressed.to_string());
            return;
        }
        this.phase_detail = "checking route after forward key input while unpossessed";
        this.phase = 67;
        return;
    }

    if this.phase == 67 {
        if is_controlled(this.subject) || vessel_hud_visible() != false { return; }
        if !port_active(this.subject) || !guidance_is_live(this.subject) {
            this.done = true;
            fail(this.checks,
                "an unpossessed producer's forward input stopped the rover autopilot");
            return;
        }
        this.checks.push(t_true(true,
            "pressing the configured forward key while unpossessed cannot stop rover autopilot"));
        let released = input_key_release(this.forward_key);
        if released == () || released.ok != true {
            this.done = true;
            fail(this.checks, "could not release the configured forward key: "
                + released.to_string());
            return;
        }
        let input = emit("intent.edge", #{
            intent: "forward",
            edge: "pressed",
            target_gid: me,
        });
        if !input {
            this.done = true;
            fail(this.checks, "could not deliver the unrelated unpossessed intent edge");
            return;
        }
        this.phase_detail = "checking route after an unrelated unpossessed intent edge";
        this.phase = 68;
        return;
    }

    if this.phase == 68 {
        if is_controlled(this.subject) || vessel_hud_visible() != false { return; }
        if !port_active(this.subject) || !guidance_is_live(this.subject) {
            this.done = true;
            fail(this.checks,
                "an unrelated unpossessed intent edge stopped the rover autopilot");
            return;
        }
        this.checks.push(t_true(true,
            "an unrelated unpossessed intent edge cannot stop the active rover route"));
        let rover_screen = viewport_position(find_path(ROVER_TARGET));
        if rover_screen == () || rover_screen.len() != 2 { return; }
        this.rover_x = rover_screen[0];
        this.rover_y = rover_screen[1];
        input_pointer_move(this.rover_x, this.rover_y);
        this.phase = 62;
        return;
    }

    if this.phase == 62 {
        input_pointer_press("primary", this.rover_x, this.rover_y);
        this.phase = 63;
        return;
    }

    if this.phase == 63 {
        input_pointer_release("primary", this.rover_x, this.rover_y);
        this.phase = 64;
        return;
    }

    if this.phase == 64 {
        this.phase_detail = "waiting for rover repossession after the unpossessed-input checks";
        if !is_controlled(this.subject) || !port_active(this.subject)
            || !guidance_is_live(this.subject) || vessel_hud_visible() != true {
            return;
        }
        let active_label = get_exposure("vessel-hud", "program_label");
        if active_label == () || active_label != "AUTOPILOT ON" { return; }
        this.checks.push(t_true(true,
            "the rover can be repossessed while autopilot remains active"));
        this.checks.push(t_true(true,
            "repossessing restores the vessel HUD with the active route state"));
        input_key_press(this.action_key);
        this.phase = 65;
        return;
    }

    if this.phase == 65 {
        input_key_release(this.action_key);
        this.phase = 8;
        return;
    }

    if this.phase == 7 {
        input_key_release(this.action_key);
        this.phase = 8;
        return;
    }

    if this.phase == 8 {
        if port_active(this.subject) { return; }
        // Possession changes the camera binding. Re-project the target after
        // that handoff and wait for the follow camera to settle; reusing the
        // pre-possession screen coordinate would click a different surface.
        let rover_screen = viewport_position(find_path(ROVER_TARGET));
        if rover_screen == () || rover_screen.len() != 2 { return; }
        if this.last_rover_x != 0.0 {
            let dx = rover_screen[0] - this.last_rover_x;
            let dy = rover_screen[1] - this.last_rover_y;
            if (dx * dx + dy * dy).sqrt() <= 2.0 {
                this.stable_rover_samples += 1;
            } else {
                this.stable_rover_samples = 0;
            }
        }
        this.last_rover_x = rover_screen[0];
        this.last_rover_y = rover_screen[1];
        if this.stable_rover_samples < 3 { return; }
        this.rover_x = rover_screen[0];
        this.rover_y = rover_screen[1];
        this.before_add_pose = world_pos(this.subject);
        input_key_press("AltLeft");
        this.phase = 9;
        return;
    }

    if this.phase == 9 {
        // A terrain-only authoring gesture must consume a sky click without
        // creating a point or showing an error notification.
        input_pointer_move(this.sky_x, this.sky_y);
        this.phase = 10;
        return;
    }

    if this.phase == 10 {
        input_pointer_press("primary", this.sky_x, this.sky_y);
        this.phase = 11;
        return;
    }

    if this.phase == 11 {
        input_pointer_release("primary", this.sky_x, this.sky_y);
        this.phase = 12;
        return;
    }

    if this.phase == 12 {
        let after_sky = route_points();
        if after_sky == () || after_sky.len() != this.before.len() {
            return;
        }
        this.checks.push(t_true(true,
            "native Alt-click on the sky does not append a route point"));
        // Keep the valid terrain gesture close to the controlled rover so the
        // newly composed marker is observable in the same camera view.
        // The panel is the rover's visible hit target. Move well clear of it,
        // but remain below the horizon so the ray must land on terrain.
        this.add_x = this.rover_x + 240.0;
        this.add_y = this.rover_y - 120.0;
        input_pointer_move(this.add_x, this.add_y);
        this.phase = 13;
        return;
    }

    if this.phase == 13 {
        input_pointer_press("primary", this.add_x, this.add_y);
        this.phase = 14;
        return;
    }

    if this.phase == 14 {
        input_pointer_release("primary", this.add_x, this.add_y);
        this.phase = 15;
        return;
    }

    if this.phase == 15 {
        input_key_release("AltLeft");
        this.phase = 16;
        return;
    }

    if this.phase == 16 {
        this.after_add = route_points();
        if this.after_add == () || this.after_add.len() != this.before.len() + 1 { return; }
        for path in this.after_add {
            if !this.before.contains(path) { this.added = path; break; }
        }
        if this.added == () {
            this.done = true;
            fail(this.checks, "native Alt-click did not produce a distinct route point");
            return;
        }
        this.checks.push(t_true(true, "native Alt-click appends one authored route point"));
        this.marker = find_path(this.added + "/Dome");
        if this.marker < 0 { return; }
        if !track_entity_read(this.marker) { return; }
        let marker_screen = viewport_position(this.marker);
        if marker_screen == () || marker_screen.len() != 2 { return; }
        this.checks.push(t_true(true,
            "the newly authored waypoint has a live viewport projection"));
        let marker_root_screen = viewport_position(find_path(this.added));
        if marker_root_screen == () || marker_root_screen.len() != 2 { return; }
        let marker_dx = marker_root_screen[0] - this.add_x;
        let marker_dy = marker_root_screen[1] - this.add_y;
        this.checks.push(t_true((marker_dx * marker_dx + marker_dy * marker_dy).sqrt() < 64.0,
            "the waypoint stays anchored at the terrain click"));
        let marker_look = get(this.marker, "PbrLook");
        this.checks.push(t_true(marker_look != ()
            && marker_look.unlit == true
            && marker_look.no_shadow_cast == true
            && marker_look.alpha == "Blend",
            "the waypoint uses the authored translucent, unlit, shadowless render intent"));
        if this.before_add_pose != () {
            let after_add_pose = world_pos(this.subject);
            if after_add_pose == () || after_add_pose.len() != 3 { return; }
            let dx = after_add_pose[0] - this.before_add_pose[0];
            let dy = after_add_pose[1] - this.before_add_pose[1];
            let dz = after_add_pose[2] - this.before_add_pose[2];
            this.checks.push(t_true((dx * dx + dy * dy + dz * dz).sqrt() < 5.0,
                "adding a waypoint does not teleport the rover"));
        }
        let ribbon = query("QueryUsdPrim", #{
            path: ROUTE + "/RouteRibbon/Geometry",
            attrs: ["wrap", "curveVertexCounts", "points"],
        });
        this.checks.push(t_true(ribbon != () && ribbon.ok != false
            && ribbon.attrs != ()
            && ribbon.attrs["wrap"] == "nonperiodic"
            && ribbon.attrs["curveVertexCounts"] != ()
            && ribbon.attrs["curveVertexCounts"].len() == 1
            && ribbon.attrs["curveVertexCounts"][0] == ribbon.attrs["points"].len(),
            "the route ribbon remains an open polyline with no closing leg"));
        // Exercise the consecutive-add case as well. The second marker must
        // be projected independently; a ribbon update is not evidence that
        // its referenced sphere reached the live renderer.
        this.second_add_x = this.add_x - 160.0;
        this.second_add_y = this.add_y;
        this.before_second_add_pose = world_pos(this.subject);
        input_key_press("AltLeft");
        this.phase = 26;
        return;
    }

    if this.phase == 26 {
        input_pointer_move(this.second_add_x, this.second_add_y);
        this.phase = 27;
        return;
    }

    if this.phase == 27 {
        input_pointer_press("primary", this.second_add_x, this.second_add_y);
        this.phase = 28;
        return;
    }

    if this.phase == 28 {
        input_pointer_release("primary", this.second_add_x, this.second_add_y);
        this.phase = 29;
        return;
    }

    if this.phase == 29 {
        input_key_release("AltLeft");
        this.phase = 30;
        return;
    }

    if this.phase == 30 {
        this.after_second_add = route_points();
        if this.after_second_add == ()
            || this.after_second_add.len() != this.before.len() + 2 {
            return;
        }
        for path in this.after_second_add {
            if !this.after_add.contains(path) { this.added_second = path; break; }
        }
        if this.added_second == () {
            this.done = true;
            fail(this.checks, "the second native Alt-click did not produce a distinct route point");
            return;
        }
        this.checks.push(t_true(true,
            "a second native Alt-click appends another authored route point"));
        this.second_marker = find_path(this.added_second + "/Dome");
        if this.second_marker < 0 { return; }
        if !track_entity_read(this.second_marker) { return; }
        let second_marker_screen = viewport_position(this.second_marker);
        if second_marker_screen == () || second_marker_screen.len() != 2 { return; }
        this.checks.push(t_true(true,
            "the second waypoint has an independent live viewport projection"));
        let second_root_screen = viewport_position(find_path(this.added_second));
        if second_root_screen == () || second_root_screen.len() != 2 { return; }
        let second_dx = second_root_screen[0] - this.second_add_x;
        let second_dy = second_root_screen[1] - this.second_add_y;
        this.checks.push(t_true((second_dx * second_dx + second_dy * second_dy).sqrt() < 64.0,
            "the second waypoint stays anchored at its terrain click"));
        let second_marker_look = get(this.second_marker, "PbrLook");
        this.checks.push(t_true(second_marker_look != ()
            && second_marker_look.unlit == true
            && second_marker_look.no_shadow_cast == true
            && second_marker_look.alpha == "Blend",
            "the second waypoint uses the authored translucent render intent"));
        if this.before_second_add_pose != () {
            let after_second_add_pose = world_pos(this.subject);
            if after_second_add_pose == () || after_second_add_pose.len() != 3 { return; }
            let dx = after_second_add_pose[0] - this.before_second_add_pose[0];
            let dy = after_second_add_pose[1] - this.before_second_add_pose[1];
            let dz = after_second_add_pose[2] - this.before_second_add_pose[2];
            this.checks.push(t_true((dx * dx + dy * dy + dz * dz).sqrt() < 5.0,
                "adding a second waypoint does not teleport the rover"));
        }
        let second_ribbon = query("QueryUsdPrim", #{
            path: ROUTE + "/RouteRibbon/Geometry",
            attrs: ["wrap", "curveVertexCounts", "points"],
        });
        this.checks.push(t_true(second_ribbon != () && second_ribbon.ok != false
            && second_ribbon.attrs != ()
            && second_ribbon.attrs["wrap"] == "nonperiodic"
            && second_ribbon.attrs["curveVertexCounts"] != ()
            && second_ribbon.attrs["curveVertexCounts"].len() == 1
            && second_ribbon.attrs["curveVertexCounts"][0]
                == second_ribbon.attrs["points"].len(),
            "the second route update remains an open polyline"));
        // Repeat the release after the projected point is observable. This
        // gives the native input fan-out its own frame before the secondary
        // click, so a modifier cannot accidentally suppress route.context.
        input_key_release("AltLeft");
        this.stable_point_samples = 0;
        this.phase = 17;
        return;
    }

    if this.phase == 17 {
        // The possessed camera continues to settle after stopping the route.
        // Re-project immediately before the secondary click so the event is
        // addressed to the current rendered marker, not a stale screen point.
        let current_screen = viewport_position(this.marker);
        if current_screen == () || current_screen.len() != 2 { return; }
        if this.last_point_x != 0.0 {
            let dx = current_screen[0] - this.last_point_x;
            let dy = current_screen[1] - this.last_point_y;
            if (dx * dx + dy * dy).sqrt() <= 2.0 {
                this.stable_point_samples += 1;
            } else {
                this.stable_point_samples = 0;
            }
        }
        this.last_point_x = current_screen[0];
        this.last_point_y = current_screen[1];
        if this.stable_point_samples < 3 { return; }
        this.point_x = current_screen[0];
        this.point_y = current_screen[1];
        input_key_release("AltLeft");
        this.phase = 18;
        return;
    }

    if this.phase == 18 {
        // The generic popup is anchored at the pointer and its standard
        // frame/button padding places the row hit area to the right of the
        // anchor. Use the center of that generic row, not the marker itself.
        this.delete_x = this.point_x + 60.0;
        // The generic popup's second row is centered about 30 logical pixels
        // below its anchor. Keep this as a UI coordinate for the production
        // input path; the route policy still owns the action identity.
        this.delete_y = this.point_y + 30.0;
        input_pointer_move(this.point_x, this.point_y);
        input_pointer_press("secondary", this.point_x, this.point_y);
        this.phase = 19;
        return;
    }

    if this.phase == 19 {
        input_pointer_release("secondary", this.point_x, this.point_y);
        this.phase = 20;
        return;
    }

    if this.phase == 20 {
        // The generic menu is anchored at the secondary-click position. The
        // second authored row is the explicit Delete route point action.
        input_pointer_move(this.delete_x, this.delete_y);
        this.phase = 21;
        return;
    }

    if this.phase == 21 {
        input_pointer_press("primary", this.delete_x, this.delete_y);
        this.phase = 22;
        return;
    }

    if this.phase == 22 {
        input_pointer_release("primary", this.delete_x, this.delete_y);
        this.phase = 23;
        return;
    }

    if this.phase == 23 {
        let after_delete = route_points();
        // The second point is intentionally retained: this acceptance path
        // deletes the first newly-added point while proving that a consecutive
        // point remains authored and visible.
        if after_delete == () || after_delete.len() != this.before.len() + 1
            || after_delete.contains(this.added) {
            return;
        }
        this.checks.push(t_true(true,
            "native context-menu delete removes the authored route point"));
        if port_active(this.subject) {
            send_key(this.action_key, "Pressed");
            this.phase = 24;
        } else {
            this.done = true;
            finish(this.checks);
        }
        return;
    }

    if this.phase == 24 {
        send_key(this.action_key, "Released");
        this.phase = 25;
        return;
    }

    if this.phase == 25 {
        if port_active(this.subject) { return; }
        this.checks.push(t_true(true, "native F stops the route and leaves the subject safe"));
        let inactive_label = get_exposure("vessel-hud", "program_label");
        if inactive_label == () || inactive_label != "AUTOPILOT OFF" { return; }
        this.checks.push(t_true(true,
            "the vessel HUD reflects the authoritative inactive program state"));
        this.done = true;
        finish(this.checks);
    }
}
