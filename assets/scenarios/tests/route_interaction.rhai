// Windowed production gate for unpossessed route edits and the native
// route-point context gesture. Synthetic input still traverses Bevy picking,
// the Rhai interaction router, the authored menu host, and generic selection.

const TEST_KIND = "editor";
const ROOT = "/RouteInteraction";
const ROUTE = ROOT + "/Route";

import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

fn selected_only(entity) {
    if entity < 0 { return false; }
    let expected = entity.to_string();
    let selection = query("InspectSelection", #{});
    selection != () && selection.ok != false
        && selection.primary.to_string() == expected
        && selection.selected != () && selection.selected.len() == 1
        && selection.selected[0].to_string() == expected
}

fn report(me, passed, message) {
    me.done = true;
    report_verdict([t_true(passed, message)],
        "ROUTE POINTER INTERACTION", "ROUTE_POINTER_INTERACTION");
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["InspectSelection", "QueryUsdPrim", "ResolveUsdTarget"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.phase_elapsed = 0.0;
    this.done = false;
    this.doc = -1;
    this.subject = -1;
    this.point = -1;
    this.marker = -1;
    this.screen = ();
    this.secondary_pointer = ();
    this.scene_pointer_events = 0;
    this.last_event_name = "none";
    this.added_without_possession = ();
    this.route_children_before_edit = [];
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
            || this.point < 0 || this.marker < 0
            || route.children == () || !route.children.contains(ROUTE + "/P2") {
            return;
        }
        this.doc = route.doc_id;
        this.route_children_before_edit = route.children;
        let add = #{
            button: "primary",
            doc_id: this.doc,
            scene_root_path: ROOT,
            route_path: (),
            selected_path: ROUTE + "/P0",
            target_path: ROUTE + "/P0",
            hit_path: ROUTE + "/P0",
            prim_paths: [ROUTE + "/P0"],
            pointer_intents: ["route.add_point"],
            modifiers: #{ alt: true, shift: false, ctrl: false },
            surface_world_position: point3([4.0, 0.0, -16.0], "active_physics"),
        };
        let added = waypoint_editor::route_pointer_action(add);
        if added == () || added.ok != true || added.path == () {
            report(this, false,
                "route add failed when a waypoint was selected and the rover was not possessed");
            return;
        }
        if this.route_children_before_edit.contains(added.path) {
            report(this, false,
                "route add reused a waypoint path that existed before the unpossessed edit");
            return;
        }
        this.added_without_possession = added.path;
        let delete_context = #{
            doc_id: this.doc,
            selected_path: this.added_without_possession,
        };
        let deleted = waypoint_editor::on_delete(delete_context);
        if deleted == () || deleted.ok != true
            || deleted.path != this.added_without_possession {
            report(this, false,
                "route delete failed when a waypoint was selected and the rover was not possessed");
            return;
        }
        this.phase = 8;
        return;
    }

    if this.phase == 8 {
        let route_after_delete = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE,
            children: true,
        });
        if route_after_delete == () || route_after_delete.ok == false
            || route_after_delete.children == ()
            || route_after_delete.children.contains(this.added_without_possession) {
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
            "the waypoint context gesture selects explicitly after unpossessed route add/delete succeeded");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        report(this, false, "native route interaction stopped before its verdict");
    }
}
