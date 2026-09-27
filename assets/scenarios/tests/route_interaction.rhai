// Windowed production gate for unpossessed route edits and the native
// route-point context menu while a rover is possessed.

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
        "ROUTE CONTEXT MENU DELETION", "ROUTE_CONTEXT_MENU_DELETION");
}

fn simulation_dependencies(me, ctx) {
    #{
       modelica_entities: [find_path(ROOT + "/Subject")],
       entity_reads: [], entity_writes: [],
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
    this.added_without_possession = ();
    this.route_children_before_edit = [];
    this.context_delete_path = ROUTE + "/P0";
    this.last_screen = ();
    this.stable_screen_samples = 0;
    this.scene_pointer_seen = false;
    this.scene_pointer_context = ();
}

fn on_event(me, evt, ctx) {
    if this.phase >= 13 && !this.done && evt.name == "scene.pointer" && evt.value != ()
        && evt.value["button"] == "secondary" {
        this.scene_pointer_seen = true;
        this.scene_pointer_context = evt.value;
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    this.phase_elapsed += dt();
    if this.elapsed > 30.0 {
        this.done = true;
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
        this.subject = find_path(ROOT + "/Subject");
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
            this.done = true;
            report(this, false,
                "route add failed when a waypoint was selected and the rover was not possessed");
            return;
        }
        if this.route_children_before_edit.contains(added.path) {
            this.done = true;
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
            this.done = true;
            report(this, false,
                "route delete failed when a waypoint was selected and the rover was not possessed");
            return;
        }
        this.phase = 8;
        this.phase_elapsed = 0.0;
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
            this.done = true;
            report(this, false, "could not establish the pre-existing rover selection");
            return;
        }
        this.phase = 8.5;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 8.5 {
        if !selected_only(this.subject) {
            if this.phase_elapsed > 3.0 {
                this.done = true;
                report(this, false, "the rover selection did not settle before possession");
            }
            return;
        }
        let possession = cmd("AcquireControl", #{ target: this.subject });
        if possession == () || possession.ok == false {
            this.done = true;
            report(this, false, "could not request rover possession before native route context input");
            return;
        }
        this.phase = 9;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 9 {
        if !is_controlled(this.subject) {
            if this.phase_elapsed > 3.0 {
                this.done = true;
                report(this, false,
                    "AcquireControl returned success but the rover never became locally controlled");
            }
            return;
        }
        let screen = viewport_position(this.marker);
        if screen == () || screen.len() != 2 {
            if this.phase_elapsed > 5.0 {
                this.done = true;
                report(this, false,
                    "rover is possessed, but viewport_position could not project the route marker");
            }
            return;
        }
        if this.last_screen != () {
            let dx = screen[0] - this.last_screen[0];
            let dy = screen[1] - this.last_screen[1];
            if (dx * dx + dy * dy).sqrt() <= 1.0 {
                this.stable_screen_samples += 1;
            } else {
                this.stable_screen_samples = 0;
            }
        }
        this.last_screen = screen;
        if this.stable_screen_samples < 3 {
            if this.phase_elapsed > 20.0 {
                this.done = true;
                report(this, false,
                    "rover is possessed and the marker projects, but its camera position never settles");
            }
            return;
        }
        this.screen = screen;
        input_pointer_move(this.screen[0], this.screen[1]);
        this.phase = 13;
        return;
    }

    if this.phase == 13 {
        input_pointer_press("secondary", this.screen[0], this.screen[1]);
        this.phase = 14;
        return;
    }

    if this.phase == 14 {
        input_pointer_release("secondary", this.screen[0], this.screen[1]);
        this.phase = 15;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 15 {
        if !this.scene_pointer_seen {
            if this.phase_elapsed < 2.0 { return; }
            this.done = true;
            report(this, false,
                "possessed secondary click produced no scene.pointer event; the click is being lost before route policy");
            return;
        }
        let context = this.scene_pointer_context;
        let intents = context["pointer_intents"];
        let policy = context["pointer_policy"];
        let paths = context["prim_paths"];
        let point_hit = context["hit_path"] == this.context_delete_path + "/Dome"
            || context["target_path"] == this.context_delete_path
            || (paths != () && paths.contains(this.context_delete_path + "/Dome"));
        if context["button"] != "secondary"
            || intents == () || !intents.contains("route.context")
            || policy == () || policy["right"] != "context"
            || !point_hit {
            this.done = true;
            report(this, false,
                "possessed scene.pointer arrived with the wrong route intent, pointer policy, or waypoint hit");
            return;
        }
        if !is_controlled(this.subject) {
            this.done = true;
            report(this, false, "rover possession was lost during the native context click");
            return;
        }
        if !selected_only(this.subject) {
            this.done = true;
            report(this, false, "the possessed right-click changed the existing rover selection");
            return;
        }
        this.phase = 16;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 16 {
        if this.phase_elapsed < 0.25 { return; }
        // The generic popup is anchored at the real secondary click. Its
        // second action is centered one row below the first and to its right.
        let x = this.screen[0] + 60.0;
        let y = this.screen[1] + 30.0;
        input_pointer_move(x, y);
        this.delete_x = x;
        this.delete_y = y;
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
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 19 {
        let deleted = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: this.context_delete_path,
        });
        if deleted == () || deleted.ok == false { return; }
        if deleted.active != false {
            if this.phase_elapsed < 2.0 { return; }
            this.done = true;
            report(this, false,
                "native possessed right-click reached scene.pointer with route.context, but its menu Delete action did not deactivate P0");
            return;
        }
        if !selected_only(this.subject) {
            this.done = true;
            report(this, false, "deleting a possessed route point changed the existing rover selection");
            return;
        }
        if !is_controlled(this.subject) {
            this.done = true;
            report(this, false,
                "deleting a route point through the native context menu dropped rover possession");
            return;
        }
        this.done = true;
        report(this, true,
            "unpossessed route edits work and native right-click deletes a waypoint while the rover is possessed");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report(this, false, "native route interaction stopped before its verdict");
    }
}
