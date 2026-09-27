// Windowed production gate for unpossessed route edits, route-point selection
// and move preview, and the native context menu while a rover is possessed.

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

fn positions_differ(left, right) {
    if left == () || right == () || left.len() != 3 || right.len() != 3 {
        return false;
    }
    let dx = left[0] - right[0];
    let dy = left[1] - right[1];
    let dz = left[2] - right[2];
    (dx * dx + dy * dy + dz * dz).sqrt() > 0.1
}

fn report(me, passed, message) {
    me.done = true;
    report_verdict([t_true(passed, message)],
        "ROUTE NATIVE WAYPOINT EDITING", "ROUTE_NATIVE_WAYPOINT_EDITING");
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
    this.primary_pointer_context = ();
}

fn on_event(me, evt, ctx) {
    if !this.done && evt.name == "scene.pointer" && evt.value != () {
        if this.phase >= 13 && evt.value["button"] == "secondary" {
            this.scene_pointer_seen = true;
            this.scene_pointer_context = evt.value;
        }
        if this.phase >= 22 && evt.value["button"] == "primary" {
            this.primary_pointer_context = evt.value;
        }
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
        let ribbon_api = query("QueryUsdPrim", #{
            path: ROUTE + "/RouteRibbon/Geometry",
            attrs: ["lunco:interaction:right"],
            schemas: true,
        });
        this.subject = find_path(ROOT + "/Subject");
        this.point = find_path(ROUTE + "/P0");
        this.marker = find_path(ROUTE + "/P0/Dome");
        if route == () || route.ok == false || pointer_api == () || pointer_api.ok == false
            || ribbon_api == () || ribbon_api.ok == false
            || pointer_api.api_schemas == ()
            || !pointer_api.api_schemas.contains("LunCoPointerInteractionAPI")
            || pointer_api.attrs == ()
            || pointer_api.attrs["lunco:interaction:right"] != "context"
            || ribbon_api.api_schemas == ()
            || !ribbon_api.api_schemas.contains("LunCoPointerInteractionAPI")
            || ribbon_api.attrs == ()
            || ribbon_api.attrs["lunco:interaction:right"] != "context"
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
        // The route menu is anchored at P0; its first row is the explicit
        // selection action. Click that UI row so the popup closes through its
        // real dispatch path before testing placement input.
        this.menu_x = this.screen[0] + 58.0;
        this.menu_y = this.screen[1] + 16.0;
        input_pointer_move(this.menu_x, this.menu_y);
        this.phase = 17;
        return;
    }

    if this.phase == 17 {
        input_pointer_press("primary", this.menu_x, this.menu_y);
        this.phase = 18;
        return;
    }

    if this.phase == 18 {
        input_pointer_release("primary", this.menu_x, this.menu_y);
        this.phase = 19;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 19 {
        if !selected_only(this.point) {
            if this.phase_elapsed < 3.0 { return; }
            this.done = true;
            report(this, false,
                "the explicit Select route point menu action did not select P0");
            return;
        }
        if !is_controlled(this.subject) {
            this.done = true;
            report(this, false, "selecting a route point released rover possession");
            return;
        }
        let second_marker = viewport_position(find_path(ROUTE + "/P1/Dome"));
        if second_marker == () || second_marker.len() != 2 {
            this.done = true;
            report(this, false,
                "could not project a second route point for the native move-preview check");
            return;
        }
        input_pointer_move(second_marker[0], second_marker[1]);
        this.phase = 20;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 20 {
        let ghost = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE + "/MovePreview",
            attrs: ["lunco:interaction:left", "lunco:interaction:right", "xformOp:translate"],
            schemas: true,
        });
        if ghost == () || ghost.ok == false || ghost.attrs == () || ghost.api_schemas == ()
            || !ghost.api_schemas.contains("LunCoPointerInteractionAPI")
            || ghost.attrs["lunco:interaction:left"] != "pass_through"
            || ghost.attrs["lunco:interaction:right"] != "pass_through"
            || ghost.attrs["xformOp:translate"] == () {
            if this.phase_elapsed < 3.0 { return; }
            this.done = true;
            report(this, false,
                "hovering another scene surface after selecting P0 did not create its view-layer move ghost");
            return;
        }
        let point = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: this.context_delete_path,
        });
        if point == () || point.ok == false || point.world_position == () {
            if this.phase_elapsed < 3.0 { return; }
            this.done = true;
            report(this, false, "could not read P0's authored position before native placement");
            return;
        }
        this.point_position_before_move = point.world_position;
        if ghost.world_position == () {
            this.done = true;
            report(this, false, "the route move ghost has no composed world position");
            return;
        }
        this.first_ghost_position = ghost.world_position;
        // Move away from the existing P1 marker to make the live preview
        // independently visible and prove that subsequent pointer motion
        // updates its authored view transform.
        this.move_x = this.screen[0] + 460.0;
        this.move_y = this.screen[1];
        input_pointer_move(this.move_x, this.move_y);
        this.phase = 21;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 21 {
        let ghost = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE + "/MovePreview",
            attrs: ["xformOp:translate"],
        });
        if ghost == () || ghost.ok == false || ghost.attrs == ()
            || ghost.attrs["xformOp:translate"] == () || ghost.world_position == ()
            || !positions_differ(ghost.world_position, this.first_ghost_position) {
            if this.phase_elapsed < 3.0 { return; }
            this.done = true;
            report(this, false, "the move ghost did not follow pointer movement across the scene");
            return;
        }
        this.phase = 22;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 22 {
        input_pointer_press("primary", this.move_x, this.move_y);
        this.phase = 23;
        return;
    }

    if this.phase == 23 {
        input_pointer_release("primary", this.move_x, this.move_y);
        this.phase = 24;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 24 {
        let moved_point = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: this.context_delete_path,
        });
        if moved_point == () || moved_point.ok == false || moved_point.world_position == ()
            || !positions_differ(
                moved_point.world_position,
                this.point_position_before_move,
            ) {
            if this.phase_elapsed < 2.0 { return; }
            this.done = true;
            report(this, false, "a primary scene click did not move selected waypoint P0");
            return;
        }
        let clicked_point = if this.primary_pointer_context == () { () }
            else { point_values(this.primary_pointer_context["world_position"]) };
        if clicked_point == () {
            if this.phase_elapsed < 2.0 { return; }
            this.done = true;
            report(this, false, "the native placement click had no active-physics hit point");
            return;
        }
        if positions_differ(moved_point.world_position, clicked_point) {
            this.done = true;
            report(this, false,
                "the committed waypoint position did not match the primary-click hit point");
            return;
        }
        if !is_controlled(this.subject) {
            this.done = true;
            report(this, false,
                "moving a route point through the native scene click dropped rover possession");
            return;
        }
        if !selected_only(this.point) {
            this.done = true;
            report(this, false, "moving P0 changed the existing waypoint selection");
            return;
        }
        this.screen = viewport_position(find_path(ROUTE + "/P0/Dome"));
        if this.screen == () || this.screen.len() != 2 {
            if this.phase_elapsed < 2.0 { return; }
            this.done = true;
            report(this, false, "could not project moved P0 for its final context-menu check");
            return;
        }
        this.scene_pointer_seen = false;
        input_pointer_move(this.screen[0], this.screen[1]);
        this.phase = 25;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 25 {
        input_pointer_press("secondary", this.screen[0], this.screen[1]);
        this.phase = 26;
        return;
    }

    if this.phase == 26 {
        input_pointer_release("secondary", this.screen[0], this.screen[1]);
        this.phase = 27;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 27 {
        if !this.scene_pointer_seen {
            if this.phase_elapsed < 2.0 { return; }
            this.done = true;
            report(this, false, "right-clicking moved P0 produced no scene.pointer event");
            return;
        }
        let context = this.scene_pointer_context;
        let paths = context["prim_paths"];
        let point_hit = context["hit_path"] == this.context_delete_path + "/Dome"
            || context["target_path"] == this.context_delete_path
            || (paths != () && paths.contains(this.context_delete_path + "/Dome"));
        if !point_hit || context["pointer_policy"] == ()
            || context.pointer_policy["right"] != "context" {
            this.done = true;
            report(this, false, "moved P0 did not retain its authored context target");
            return;
        }
        this.delete_x = this.screen[0] + 60.0;
        this.delete_y = this.screen[1] + 79.0;
        input_pointer_move(this.delete_x, this.delete_y);
        this.phase = 28;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 28 {
        // Let the foreground egui menu paint and register its bounds before
        // delivering the row click through the native window input path.
        if this.phase_elapsed < 0.25 { return; }
        input_pointer_press("primary", this.delete_x, this.delete_y);
        this.phase = 29;
        return;
    }

    if this.phase == 29 {
        input_pointer_release("primary", this.delete_x, this.delete_y);
        this.phase = 32;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 32 {
        let deleted = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: this.context_delete_path,
        });
        if deleted == () || deleted.ok == false { return; }
        if deleted.active != false {
            if this.phase_elapsed < 2.0 { return; }
            this.done = true;
            report(this, false,
                "native possessed route context did not deactivate P0 after its move");
            return;
        }
        if !is_controlled(this.subject) {
            this.done = true;
            report(this, false,
                "deleting a moved route point through the native context menu dropped rover possession");
            return;
        }
        this.done = true;
        report(this, true,
            "unpossessed edit works; possessed native selection, preview, click-to-move, right-click, and delete all preserve route and rover state");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report(this, false, "native route interaction stopped before its verdict");
    }
}
