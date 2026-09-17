// Headful acceptance for the native user path in a loaded Twin.
//
// The coordinates are supplied by the runner from a current screenshot. The
// scenario sends typed window events, then observes the same public selection,
// USD, control, and authored-program surfaces a user would see.

const ROUTE = "/Traverse/Route";
const SUBJECT = "/Traverse/Rover";
const ROVER_TARGET = "/Traverse/Rover/Chassis";
const ADD_X = 1000.0;
const ADD_Y = 700.0;

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

fn fail(message) {
    report_verdict([t_true(false, message)],
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

fn on_start(me, ctx) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.phase_elapsed = 0.0;
    this.done = false;
    this.checks = [];
    this.doc = -1;
    this.subject = -1;
    this.before = ();
    this.after_add = ();
    this.added = ();
    this.marker = -1;
    this.selection_ok = false;
    this.route_was_active = false;
    this.started = false;
    this.start_release_sent = false;
    this.stop_requested = false;
    this.menu_requested = false;
    this.action_key = "KeyF";
    this.rover_x = 0.0;
    this.rover_y = 0.0;
    this.last_rover_x = 0.0;
    this.last_rover_y = 0.0;
    this.stable_rover_samples = 0;
    this.add_x = ADD_X;
    this.add_y = ADD_Y;
    this.point_x = 0.0;
    this.point_y = 0.0;
    this.last_point_x = 0.0;
    this.last_point_y = 0.0;
    this.stable_point_samples = 0;
    this.delete_x = 0.0;
    this.delete_y = 0.0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    this.phase_elapsed += dt();
    if this.elapsed > 45.0 {
        this.done = true;
        fail("native route interaction timed out in phase " + this.phase);
        return;
    }
    if this.phase_elapsed < 0.25 { return; }
    this.phase_elapsed = 0.0;

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
        this.subject = find(SUBJECT);
        if route == () || route.ok == false || route.doc_id == () || this.subject < 0 {
            return;
        }
        this.doc = route.doc_id;
        this.before = route_points();
        if this.before == () || this.before.len() == 0 { return; }
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
        this.checks.push(t_true(true, "native primary click controls the rover"));
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
        // Stop before the spatial-edit half of the test. The attended camera
        // follows the controlled subject, so leaving the route running would
        // make a fixed screen coordinate cease to identify the point that was
        // just authored. This still proves both F transitions and makes the
        // subsequent pointer cycle deterministic.
        input_key_press(this.action_key);
        this.stop_requested = true;
        this.phase = 7;
        return;
    }

    if this.phase == 7 {
        input_key_release(this.action_key);
        this.phase = 8;
        return;
    }

    if this.phase == 8 {
        if port_active(this.subject) { return; }
        input_key_press("AltLeft");
        this.phase = 9;
        return;
    }

    if this.phase == 9 {
        input_pointer_move(this.add_x, this.add_y);
        this.phase = 10;
        return;
    }

    if this.phase == 10 {
        input_pointer_press("primary", this.add_x, this.add_y);
        this.phase = 11;
        return;
    }

    if this.phase == 11 {
        input_pointer_release("primary", this.add_x, this.add_y);
        this.phase = 12;
        return;
    }

    if this.phase == 12 {
        input_key_release("AltLeft");
        this.phase = 13;
        return;
    }

    if this.phase == 13 {
        this.after_add = route_points();
        if this.after_add == () || this.after_add.len() != this.before.len() + 1 { return; }
        for path in this.after_add {
            if !this.before.contains(path) { this.added = path; break; }
        }
        if this.added == () {
            this.done = true;
            fail("native Alt-click did not produce a distinct route point");
            return;
        }
        this.checks.push(t_true(true, "native Alt-click appends one authored route point"));
        this.marker = find_path(this.added + "/Dome");
        if this.marker < 0 { return; }
        this.stable_point_samples = 0;
        this.phase = 14;
        return;
    }

    if this.phase == 14 {
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
        // The generic popup is anchored at the pointer and its standard
        // frame/button padding places the row hit area to the right of the
        // anchor. Use the center of that generic row, not the marker itself.
        this.delete_x = this.point_x + 60.0;
        this.delete_y = this.point_y + 28.0;
        input_pointer_move(this.point_x, this.point_y);
        input_pointer_press("secondary", this.point_x, this.point_y);
        this.phase = 15;
        return;
    }

    if this.phase == 15 {
        input_pointer_release("secondary", this.point_x, this.point_y);
        this.phase = 16;
        return;
    }

    if this.phase == 16 {
        // The generic menu is anchored at the secondary-click position. The
        // runner supplies the current delete-row coordinate from its screenshot.
        input_pointer_move(this.delete_x, this.delete_y);
        this.phase = 17;
        return;
    }

    if this.phase == 17 {
        input_pointer_press("primary", this.delete_x, this.delete_y);
        this.phase = 18;
        return;
    }

    if this.phase == 18 {
        input_pointer_release("primary", this.delete_x, this.delete_y);
        this.phase = 19;
        return;
    }

    if this.phase == 19 {
        let after_delete = route_points();
        if after_delete == () || after_delete.len() != this.before.len()
            || after_delete.contains(this.added) {
            return;
        }
        this.checks.push(t_true(true,
            "native context-menu delete removes the authored route point"));
        if port_active(this.subject) {
            send_key(this.action_key, "Pressed");
            this.phase = 20;
        } else {
            this.done = true;
            finish(this.checks);
        }
        return;
    }

    if this.phase == 20 {
        send_key(this.action_key, "Released");
        this.phase = 21;
        return;
    }

    if this.phase == 21 {
        if port_active(this.subject) { return; }
        this.checks.push(t_true(true, "native F stops the route and leaves the subject safe"));
        this.done = true;
        finish(this.checks);
    }
}
