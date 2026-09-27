// Windowed production gate for unpossessed route edits and the native
// route-point context menu. It opens the menu through route policy and sends
// the Delete row through the production editor input path.

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
    this.added_without_possession = ();
    this.route_children_before_edit = [];
    this.context_delete_path = ROUTE + "/P0";
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
        let opened = waypoint_editor::on_context(#{
            doc_id: this.doc,
            scene_root_path: ROOT,
            route_path: ROUTE,
            selected_path: this.context_delete_path,
            target_path: this.context_delete_path + "/Dome",
            hit_path: this.context_delete_path + "/Dome",
            prim_paths: [this.context_delete_path + "/Dome", this.context_delete_path],
            screen_position: this.screen,
        });
        if opened == () || opened.ok != true || opened.menu != true {
            report(this, false, "the route context policy did not open its native menu");
            return;
        }
        if !selected_only(this.subject) {
            report(this, false,
                "opening the route context menu changed selection or activated its gizmo");
            return;
        }
        this.phase = 5;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 5 {
        if this.phase_elapsed < 0.25 { return; }
        // The generic popup is anchored at the click. Its second action is
        // centered one row below the first and to its right.
        let x = this.screen[0] + 60.0;
        let y = this.screen[1] + 30.0;
        input_pointer_move(x, y);
        input_pointer_press("primary", x, y);
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        let x = this.screen[0] + 60.0;
        let y = this.screen[1] + 30.0;
        input_pointer_release("primary", x, y);
        this.phase = 50;
        this.phase_elapsed = 0.0;
        return;
    }

    if this.phase == 50 {
        let deleted = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: this.context_delete_path,
        });
        if deleted == () || deleted.ok == false { return; }
        if deleted.active != false {
            if this.phase_elapsed < 3.0 { return; }
            report(this, false,
                "the native context-menu Delete action did not deactivate P0 without rover possession");
            return;
        }
        if !selected_only(this.subject) {
            report(this, false,
                "deleting an unpossessed route point changed the existing rover selection");
            return;
        }
        let possessed_context = #{
            doc_id: this.doc,
            scene_root_path: ROOT,
            controlled_path: ROOT + "/Subject",
            selected_path: ROOT + "/Subject",
            target_path: ROUTE + "/P1",
            hit_path: ROUTE + "/P1/Dome",
            prim_paths: [ROUTE + "/P1/Dome", ROUTE + "/P1", ROUTE, ROOT],
            button: "secondary",
            pointer_intents: ["route.context"],
            pointer_policy: #{ right: "context" },
            screen_position: this.screen,
        };
        let possessed_route = waypoint_editor::route_edit_path(possessed_context);
        if possessed_route != ROUTE {
            report(this, false,
                "a possessed route context did not resolve the clicked waypoint's route");
            return;
        }
        report(this, true,
            "native menu deletion works without possession and possessed route context follows its clicked point");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        report(this, false, "native route interaction stopped before its verdict");
    }
}
