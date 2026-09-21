// Windowed production gate for the native route-point context gesture.
// Synthetic input still traverses Bevy picking, the Rhai interaction router,
// the authored menu host, and the generic selection command.

const TEST_KIND = "editor";
const ROOT = "/RouteInteraction";
const ROUTE = ROOT + "/Route";

fn selected_only(entity) {
    let selection = query("InspectSelection", #{});
    selection != () && selection.ok != false
        && selection.primary == entity
        && selection.selected != () && selection.selected.len() == 1
        && selection.selected[0] == entity
}

fn report(me, passed, message) {
    me.done = true;
    report_verdict([t_true(passed, message)],
        "ROUTE POINTER INTERACTION", "ROUTE_POINTER_INTERACTION");
}

fn on_start(me, ctx) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.phase_elapsed = 0.0;
    this.done = false;
    this.subject = -1;
    this.point = -1;
    this.marker = -1;
    this.screen = ();
    this.secondary_pointer = ();
    this.scene_pointer_events = 0;
    this.last_event_name = "none";
}

fn on_event(me, event, ctx) {
    this.last_event_name = event.name;
    if event.name == "scene.pointer" {
        this.scene_pointer_events += 1;
        if event.value != () && event.value.button == "secondary" {
            this.secondary_pointer = event.value;
        }
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    this.phase_elapsed += dt();
    if this.elapsed > 30.0 {
        report(this, false, "native route context interaction timed out in phase " + this.phase);
        return;
    }

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
        let pointer_api = query("QueryUsdPrim", #{
            path: ROUTE + "/P0/Dome",
            attrs: ["lunco:interaction:right"],
            schemas: true,
        });
        this.subject = find(ROOT + "/Subject");
        this.point = find_path(ROUTE + "/P0");
        this.marker = find_path(ROUTE + "/P0/Dome");
        if route == () || route.ok == false || pointer_api == () || pointer_api.ok == false
            || pointer_api.api_schemas == ()
            || !pointer_api.api_schemas.contains("LunCoPointerInteractionAPI")
            || pointer_api.attrs == ()
            || pointer_api.attrs["lunco:interaction:right"] != "context"
            || this.subject < 0
            || this.point < 0 || this.marker < 0 {
            return;
        }
        let selected = cmd("SelectEntity", #{
            entity_id: this.subject,
            inspector_part_entity_id: 0,
            extend: false,
            toggle: false,
            remove_only: false,
        });
        if selected == () || selected.ok == false {
            report(this, false, "could not establish the pre-existing rover selection");
            return;
        }
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        if !selected_only(this.subject) { return; }
        this.screen = viewport_position(this.marker);
        if this.screen == () || this.screen.len() != 2 { return; }
        input_pointer_move(this.screen[0], this.screen[1]);
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        input_pointer_press("secondary", this.screen[0], this.screen[1]);
        this.phase = 3;
        return;
    }

    if this.phase == 3 {
        input_pointer_release("secondary", this.screen[0], this.screen[1]);
        this.phase = 4;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 4 {
        if this.phase_elapsed < 0.25 { return; }
        if this.secondary_pointer == () {
            report(this, false,
                "no secondary scene.pointer event arrived (observed "
                    + this.scene_pointer_events + "; last event was " + this.last_event_name
                    + "; target screen position was (" + this.screen[0].to_string()
                    + "," + this.screen[1].to_string() + ")");
            return;
        }
        if !this.secondary_pointer.pointer_intents.contains("route.context") {
            report(this, false, "the secondary pointer event did not resolve the route.context input binding");
            return;
        }
        if this.secondary_pointer.pointer_policy == () {
            report(this, false, "the secondary pointer event did not carry the hit prim's USD interaction policy");
            return;
        }
        if this.secondary_pointer.pointer_policy.right != "context" {
            report(this, false, "the hit prim's authored secondary interaction policy was not context");
            return;
        }
        if this.secondary_pointer.prim_paths == ()
            || !this.secondary_pointer.prim_paths.contains(ROUTE + "/P0") {
            report(this, false, "the secondary pointer event did not resolve route waypoint P0");
            return;
        }
        if !this.secondary_pointer.prim_paths.contains(ROUTE + "/P0/Dome") {
            report(this, false, "the secondary pointer event did not resolve the route marker hit prim");
            return;
        }
        if !selected_only(this.subject) {
            report(this, false,
                "right-clicking a waypoint changed selection or activated its gizmo");
            return;
        }
        this.phase = 5;
        return;
    }

    if this.phase == 5 {
        // The generic popup is anchored at the click. Its first action is
        // centered one row below the anchor and to its right.
        let x = this.screen[0] + 60.0;
        let y = this.screen[1] + 12.0;
        input_pointer_move(x, y);
        input_pointer_press("primary", x, y);
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        let x = this.screen[0] + 60.0;
        let y = this.screen[1] + 12.0;
        input_pointer_release("primary", x, y);
        this.phase = 7;
        return;
    }

    if this.phase == 7 {
        if !selected_only(this.point) { return; }
        report(this, true,
            "the waypoint stays unselected on right-click and selects only from its explicit menu action");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        report(this, false, "native route interaction stopped before its verdict");
    }
}
