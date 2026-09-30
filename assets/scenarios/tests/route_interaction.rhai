// Windowed production gate driven through InjectWindowInput. It covers
// unpossessed route edits, possessed route-point selection, cursor-tracking move
// preview, and the context menu. It does not verify OS mouse delivery.

const TEST_KIND = "editor";
const ROOT = "/RouteInteraction";
const ROUTE = ROOT + "/Route";

import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

fn selected_only(doc, path) {
    let editor = query("InspectUsdSelection", #{});
    if editor != () && editor.ok != false
        && editor.focused == true && editor.doc_id == doc {
        return editor.primary != () && editor.primary.path == path
            && editor.selected != () && editor.selected.len() == 1
            && editor.selected[0].path == path;
    }
    let entity = find_path_in_document(doc, path);
    if entity < 0 { return false; }
    let selection = query("InspectSelection", #{});
    selection != () && selection.ok != false
        && selection.primary.to_string() == entity.to_string()
        && selection.selected != () && selection.selected.len() == 1
        && selection.selected[0].to_string() == entity.to_string()
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

fn distance_between(left, right) {
    if left == () || right == () || left.len() != 3 || right.len() != 3 {
        return ();
    }
    let dx = left[0] - right[0];
    let dy = left[1] - right[1];
    let dz = left[2] - right[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn document_generation(doc) {
    usd_document_generation(doc)
}

fn inspect_route_curve_view(doc) {
    let entity = find_path_in_document(doc, ROUTE + "/RouteRibbon/Geometry");
    if entity < 0 { return (); }
    query("InspectUsdCurveView", #{ doc_id: doc, entity_id: entity })
}

fn begin_curve_view_expectation(state, point_count, visibility, needs_mesh, label) {
    let view = inspect_route_curve_view(state.doc);
    if view == () || view.ok == false || view.state != "ready"
        || view.completed_revision != view.requested_revision
        || view.applied_revision != view.requested_revision {
        state.failure = "cannot start " + label
            + " before the previous route ribbon result is applied";
        return #{ state: state, ok: false };
    }
    state.curve_edit_baseline = view.requested_revision;
    state.curve_expected_point_count = point_count;
    state.curve_expected_visibility = visibility;
    state.curve_expected_mesh = needs_mesh;
    state.curve_edit_label = label;
    #{ state: state, ok: true }
}

fn route_curve_view_is_ready_after_edit_state(state) {
    if state.failure != "" { return #{ state: state, ready: true }; }
    let view = inspect_route_curve_view(state.doc);
    if view == () || view.ok == false { return #{ state: state, ready: false }; }
    if view.state == "failed" {
        state.failure = "route ribbon build failed after " + state.curve_edit_label
            + ": " + view.error;
        return #{ state: state, ready: true };
    }
    if view.state != "ready"
        || view.completed_revision != view.requested_revision
        || view.applied_revision != view.requested_revision
        || view.requested_revision <= state.curve_edit_baseline {
        return #{ state: state, ready: false };
    }
    let route = waypoint_editor::route_positions(state.doc, ROUTE);
    if route == () || route.ok != true
        || route.paths.len() != state.curve_expected_point_count {
        return #{ state: state, ready: false };
    }
    if view.local_visibility != state.curve_expected_visibility
        || (state.curve_expected_mesh && to_int(view.result_segment_count) < 1)
        || (!state.curve_expected_mesh && to_int(view.result_segment_count) != 0) {
        state.failure = "route ribbon result after " + state.curve_edit_label
            + " disagrees with the route: local visibility=" + view.local_visibility
            + ", segments=" + view.result_segment_count.to_string();
        return #{ state: state, ready: true };
    }
    #{ state: state, ready: true }
}

fn route_curve_view_is_ready_after_edit(me) {
    let result = route_curve_view_is_ready_after_edit_state(this);
    this = result.state;
    result.ready
}

fn record_route_curve_view_edit(me) {
    this.checks.push(t_true(this.failure == "",
        this.curve_edit_label + " applies the current route mesh revision and visibility"));
}

fn delete_last_route_point_for_view_test(state, next_count, visibility, needs_mesh, label) {
    let expectation = begin_curve_view_expectation(
        state, next_count, visibility, needs_mesh, label,
    );
    state = expectation.state;
    if !expectation.ok { return state; }
    let route = waypoint_editor::route_positions(state.doc, ROUTE);
    if route == () || route.ok != true
        || route.paths.len() != next_count + 1 || route.paths.len() == 0 {
        state.failure = "route topology is not ready for " + label;
        return state;
    }
    let target = route.paths[route.paths.len() - 1];
    let deleted = waypoint_editor::on_delete(#{
        doc_id: state.doc, route_path: ROUTE, selected_path: target,
    });
    if deleted == () || deleted.ok != true || deleted.path != target {
        state.failure = label + " was rejected: "
            + if deleted == () { "no command result" } else { deleted.error };
        return state;
    }
    state.waiting_for = label + " route and ribbon projection";
    state
}

fn add_route_point_for_view_test(state, position, next_count, visibility, needs_mesh, label) {
    let expectation = begin_curve_view_expectation(
        state, next_count, visibility, needs_mesh, label,
    );
    state = expectation.state;
    if !expectation.ok { return state; }
    let point = point3(position, "active_physics");
    let added = waypoint_editor::on_click(#{
        button: "primary",
        doc_id: state.doc,
        scene_root_path: ROOT,
        route_path: ROUTE,
        surface_world_position: point,
    });
    if added == () || added.ok != true || added.path == () {
        state.failure = label + " was rejected: "
            + if added == () { "no command result" } else { added.error };
        return state;
    }
    state.waiting_for = label + " route and ribbon projection";
    state
}

fn delete_to_three_route_points(me) {
    this = delete_last_route_point_for_view_test(this, 3, "inherited", true,
        "reducing the route to three points");
}

fn delete_to_two_route_points(me) {
    this = delete_last_route_point_for_view_test(this, 2, "inherited", true,
        "reducing the route to two points");
}

fn delete_to_one_route_point(me) {
    this = delete_last_route_point_for_view_test(this, 1, "hidden", false,
        "reducing the route to one point");
}

fn delete_to_empty_route(me) {
    this = delete_last_route_point_for_view_test(this, 0, "hidden", false,
        "emptying the route");
}

fn add_one_route_point(me) {
    this = add_route_point_for_view_test(this, [12.0, 1.0, -16.0], 1, "hidden", false,
        "restoring one route point");
}

fn add_second_route_point(me) {
    this = add_route_point_for_view_test(this, [18.0, 1.0, -16.0], 2, "inherited", true,
        "restoring two route points");
}

fn live_render_translation(entity) {
    if entity < 0 { return (); }
    try {
        let translation = get(entity, "Transform.translation");
        [translation.x, translation.y, translation.z]
    } catch (error) { () }
}

fn report(me, passed, message) {
    me.done = true;
    me.checks.push(t_true(passed, message));
    report_verdict(me.checks,
        "ROUTE WINDOWED INJECTED WAYPOINT EDITING", "ROUTE_WINDOWED_INJECTED_WAYPOINT_EDITING");
}

fn simulation_dependencies(me, ctx) {
    let subject = find_path(ROOT + "/Subject");
    #{ modelica_entities: [subject], entity_reads: [subject], entity_writes: [],
       query_reads: ["InspectSelection", "InspectUsdCurveView", "InspectUsdSelection", "ListOpenDocuments", "QueryUsdPrim", "QueryUsdPrims", "ResolveUsdTarget"],
       required_inputs: [] }
}

fn test_state_is_valid(me) { this.failure == "" }

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
    let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
    let ready = route != () && route.ok != false && route.doc_id != ()
        && route.children != () && route.children.contains(ROUTE + "/P2");
    if ready { this.doc = route.doc_id; }
    ready
}

fn route_curve_view_is_ready_at_fixture(me) {
    if this.failure != "" { return true; }
    let view = inspect_route_curve_view(this.doc);
    if view == () || view.ok == false { return false; }
    if view.state == "failed" {
        this.failure = "initial route ribbon build failed: " + view.error;
        return true;
    }
    view.state == "ready"
        && view.completed_revision == view.requested_revision
        && view.applied_revision == view.requested_revision
        && view.local_visibility == "inherited"
        && to_int(view.result_segment_count) >= 1
}

fn prepare_route_and_add_unpossessed(me) {
    let initial = query("QueryUsdPrim", #{ path: ROUTE, children: true });
    if initial == () || initial.ok == false || initial.doc_id == () {
        this.failure = "the route document did not become available";
        return;
    }
    this.doc = initial.doc_id;

    let route = query("QueryUsdPrim", #{
        doc_id: this.doc, path: ROUTE, children: true,
    });
    let unused = query("ResolveUsdTarget", #{
        doc_id: this.doc, path: ROUTE + "/P3", edit_target: "@runtime@",
    });
    let pointer_api = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE + "/P0/Dome",
        attrs: ["lunco:interaction:right"], schemas: true,
    });
    let ribbon_api = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE + "/RouteRibbon/Geometry",
        attrs: ["lunco:interaction:right", "curveVertexCounts"], schemas: true,
    });
    this.subject = find_path_in_document(this.doc, ROOT + "/Subject");
    this.point = find_path_in_document(this.doc, ROUTE + "/P0");
    this.marker = find_path_in_document(this.doc, ROUTE + "/P0/Dome");
    let ribbon_entity = find_path_in_document(
        this.doc, ROUTE + "/RouteRibbon/Geometry",
    );
    if unused == () || unused.ok == false
        || route == () || route.ok == false
        || pointer_api == () || pointer_api.ok == false
        || ribbon_api == () || ribbon_api.ok == false
        || pointer_api.api_schemas == ()
        || !pointer_api.api_schemas.contains("LunCoPointerInteractionAPI")
        || pointer_api.attrs == ()
        || pointer_api.attrs["lunco:interaction:right"] != "context"
        || ribbon_api.api_schemas == ()
        || !ribbon_api.api_schemas.contains("LunCoPointerInteractionAPI")
        || ribbon_api.attrs == ()
        || ribbon_api.attrs["curveVertexCounts"] == ()
        || ribbon_api.attrs["lunco:interaction:right"] != "context"
        || this.subject < 0 || this.point < 0 || this.marker < 0
        || route.children == () || !route.children.contains(ROUTE + "/P2")
        || route.doc_id != this.doc {
        this.failure = "route fixture identities or pointer schemas did not compose";
        return;
    }
    if ribbon_entity < 0 {
        this.failure = "route ribbon geometry has no live view identity";
        return;
    }
    let wrong_view_owner = query("InspectUsdCurveView", #{
        doc_id: this.doc + this.doc,
        entity_id: ribbon_entity,
    });
    this.checks.push(t_true(wrong_view_owner != () && wrong_view_owner.ok == false,
        "curve-view inspection rejects a mismatched document identity"));
    if unused.status != "missing" || unused.under_arc != true
        || unused.composed_exists != false {
        this.failure = "ResolveUsdTarget did not report an absent referenced child as missing";
        return;
    }

    this.route_children_before_edit = route.children;
    this.generation_before_add = document_generation(this.doc);
    this.ribbon_seed_count = ribbon_api.attrs["curveVertexCounts"][0];
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
    let expectation = begin_curve_view_expectation(
        this, 4, "inherited", true, "the unpossessed route add",
    );
    this = expectation.state;
    if !expectation.ok { return; }
    let added = waypoint_editor::route_pointer_action(add);
    if added == () || added.ok != true || added.path == () {
        this.failure = "route add failed when no rover was possessed";
        return;
    }
    if this.route_children_before_edit.contains(added.path) {
        this.failure = "route add reused a waypoint path that existed before the unpossessed edit";
        return;
    }
    this.generation_after_add = document_generation(this.doc);
    this.added_without_possession = added.path;
    this.waiting_for = "the unpossessed waypoint trigger projection";
}

fn unpossessed_waypoint_ready(me) {
    if this.added_without_possession == () { return false; }
    let trigger = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: this.added_without_possession + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    let ready = trigger != () && trigger.ok != false && trigger.attrs != ()
        && trigger.attrs["lunco:triggerZone"] != ();
    if !ready { return false; }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    if !ribbon.ready { return false; }
    let curve = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE + "/RouteRibbon/Geometry",
        attrs: ["curveVertexCounts"],
    });
    let generation = document_generation(this.doc);
    this.checks.push(t_true(to_int(this.generation_after_add)
        == to_int(this.generation_before_add) + 3
        && generation == this.generation_after_add,
        "adding one point applies its three USD ops once with no follow-up document projection"));
    this.checks.push(t_true(curve != () && curve.ok != false && curve.attrs != ()
        && curve.attrs["curveVertexCounts"] != ()
        && curve.attrs["curveVertexCounts"][0] == this.ribbon_seed_count,
        "route ribbon geometry stays outside the edited USD document"));
    true
}

fn delete_unpossessed_waypoint(me) {
    this.generation_before_delete = document_generation(this.doc);
    let expectation = begin_curve_view_expectation(
        this, 3, "inherited", true, "the unpossessed route delete",
    );
    this = expectation.state;
    if !expectation.ok { return; }
    let deleted = waypoint_editor::on_delete(#{
        doc_id: this.doc,
        selected_path: this.added_without_possession,
    });
    if deleted == () || deleted.ok != true
        || deleted.path != this.added_without_possession {
        this.failure = "route delete failed when no rover was possessed";
        return;
    }
    this.generation_after_delete = document_generation(this.doc);
    this.waiting_for = "the unpossessed waypoint deletion projection";
}

fn unpossessed_waypoint_deleted(me) {
    let route = query("QueryUsdPrim", #{
        doc_id: this.doc, path: ROUTE, children: true,
    });
    let deleted = route != () && route.ok != false && route.children != ()
        && !route.children.contains(this.added_without_possession);
    if !deleted { return false; }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    if !ribbon.ready { return false; }
    let generation = document_generation(this.doc);
    this.checks.push(t_true(to_int(this.generation_after_delete)
        == to_int(this.generation_before_delete) + 1
        && generation == this.generation_after_delete,
        "deleting one point projects one authored change without a ribbon projection"));
    true
}

fn select_rover(me) {
    let editor = query("InspectUsdSelection", #{});
    let selected = if editor != () && editor.ok != false
        && editor.focused == true && editor.doc_id == this.doc {
        cmd("SelectUsdPrim", #{
            preview: editor.preview,
            path: ROOT + "/Subject",
            extend: false,
            toggle: false,
        })
    } else {
        cmd("SelectEntity", #{
            entity_id: this.subject,
            inspector_part_entity_id: 0,
            extend: false,
            toggle: false,
            remove_only: false,
        })
    };
    if selected == () || selected.ok == false {
        this.failure = "could not establish the pre-existing rover selection";
    }
    this.waiting_for = "the rover selection";
}

fn rover_is_selected(me) { selected_only(this.doc, ROOT + "/Subject") }

fn possess_rover(me) {
    let possession = cmd("AcquireControl", #{ target: this.subject });
    if possession == () || possession.ok == false {
        this.failure = "could not request rover possession before injected route context input";
    }
    this.waiting_for = "rover possession";
}

fn rover_is_possessed(me) { is_controlled(this.subject) }

fn refresh_route_marker_screen(state) {
    if state.marker < 0 {
        state.screen = ();
        return #{ state: state, available: false };
    }
    let screen = viewport_position(state.marker);
    if screen == () || screen.len() != 2 {
        state.screen = ();
        return #{ state: state, available: false };
    }
    state.screen = screen;
    #{ state: state, available: true }
}

fn observe_route_marker_projection(me) {
    if !is_controlled(this.subject) {
        this.failure = "rover possession was lost before injected context input";
        return;
    }
    let refreshed = refresh_route_marker_screen(this);
    this = refreshed.state;
    if !refreshed.available {
        this.last_screen = ();
        this.stable_screen_samples = 0;
        return;
    }
    if this.last_screen != () {
        let dx = this.screen[0] - this.last_screen[0];
        let dy = this.screen[1] - this.last_screen[1];
        if (dx * dx + dy * dy).sqrt() <= 1.0 {
            this.stable_screen_samples += 1;
        } else {
            this.stable_screen_samples = 0;
        }
    }
    this.last_screen = this.screen;
}

fn marker_projection_stable_or_failed(me) {
    this.failure != "" || this.stable_screen_samples >= 3
}

fn move_pointer_to_p0(me) {
    let refreshed = refresh_route_marker_screen(this);
    this = refreshed.state;
    if !refreshed.available {
        this.failure = "P0 has no current viewport position for the context gesture";
        return;
    }
    input_pointer_move(this.screen[0], this.screen[1]);
    this.waiting_for = "the first injected waypoint context event";
}

fn press_secondary(me) {
    let refreshed = refresh_route_marker_screen(this);
    this = refreshed.state;
    if !refreshed.available {
        this.failure = "P0 has no current viewport position for the context press";
        return;
    }
    input_pointer_press("secondary", this.screen[0], this.screen[1]);
}

fn release_secondary(me) {
    input_pointer_release("secondary", this.screen[0], this.screen[1]);
}

fn first_secondary_context_arrived(me) { this.secondary_contexts.len() >= 1 }

fn context_hits_point(context, point) {
    if context == () { return false; }
    let paths = context["prim_paths"];
    context["hit_path"] == point + "/Dome"
        || context["target_path"] == point
        || (paths != () && paths.contains(point + "/Dome"))
}

fn route_context_is_valid(context, point) {
    if context == () { return false; }
    let intents = context["pointer_intents"];
    let policy = context["pointer_policy"];
    context["button"] == "secondary"
        && intents != () && intents.contains("route.context")
        && policy != () && policy["right"] == "context"
        && context_hits_point(context, point)
}

fn first_context_is_valid(me) {
    let context = this.secondary_contexts[this.secondary_contexts.len() - 1];
    let valid = route_context_is_valid(context, this.context_delete_path);
    if !valid {
        this.failure = "possessed secondary click projected P0 to " + this.screen.to_string()
            + " but hit `" + context["hit_path"].to_string() + "` at "
            + context["screen_position"].to_string() + " (target `"
            + context["target_path"].to_string() + "`, world "
            + context["world_position"].to_string() + ")";
    }
    valid
}

fn check_possession_and_selection(me) {
    if !is_controlled(this.subject) {
        this.failure = "rover possession was lost during the injected context click";
    } else if !selected_only(this.doc, ROOT + "/Subject") {
        this.failure = "the possessed right-click changed the existing rover selection";
    }
}

fn prepare_select_menu_row(me) {
    this.menu_x = this.screen[0] + 58.0;
    this.menu_y = this.screen[1] + 16.0;
    input_pointer_move(this.menu_x, this.menu_y);
}

fn press_menu_primary(me) {
    input_pointer_press("primary", this.menu_x, this.menu_y);
}

fn release_menu_primary(me) {
    input_pointer_release("primary", this.menu_x, this.menu_y);
}

fn p0_is_selected(me) { selected_only(this.doc, ROUTE + "/P0") }

fn point_selection_preserved_possession(me) {
    if !is_controlled(this.subject) {
        this.failure = "selecting a route point released rover possession";
    } else if !selected_only(this.doc, ROUTE + "/P0") {
        this.failure = "the explicit Select route point menu action did not select P0";
    }
}

fn move_pointer_to_p1(me) {
    let second_marker = viewport_position(
        find_path_in_document(this.doc, ROUTE + "/P1/Dome"),
    );
    if second_marker == () || second_marker.len() != 2 {
        this.failure = "could not project a second route point for the injected move-mode check";
        return;
    }
    input_pointer_move(second_marker[0], second_marker[1]);
}

fn selection_did_not_arm_move(me) {
    let ghost = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview",
        edit_target: "@view@",
    });
    ghost != () && ghost.ok != false && ghost.authored_here != true
}

fn prepare_second_context(me) {
    let refreshed = refresh_route_marker_screen(this);
    this = refreshed.state;
    if !refreshed.available {
        this.failure = "selected P0 has no current viewport position for its context gesture";
        return;
    }
    input_pointer_move(this.screen[0], this.screen[1]);
    this.waiting_for = "the second injected waypoint context event";
}

fn second_secondary_context_arrived(me) { this.secondary_contexts.len() >= 2 }

fn second_context_is_valid(me) {
    let valid = route_context_is_valid(
        this.secondary_contexts[this.secondary_contexts.len() - 1],
        this.context_delete_path,
    );
    if !valid { this.failure = "selected P0 did not retain its authored context target"; }
    valid
}

fn prepare_move_menu_row(me) {
    this.menu_x = this.screen[0] + 58.0;
    this.menu_y = this.screen[1] + 37.0;
    input_pointer_move(this.menu_x, this.menu_y);
}

fn move_preview_is_armed(me) {
    let target = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview/Target_P0",
        edit_target: "@view@",
    });
    let preview = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview",
        edit_target: "@view@",
        authored_children: true,
    });
    let ghost = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview",
        attrs: ["lunco:interaction:left", "lunco:interaction:right", "xformOp:translate"],
        schemas: true,
    });
    let preview_entity = find_path_in_document(this.doc, ROUTE + "/MovePreview");
    let target_is_direct_child = false;
    if preview != () && preview.ok != false && preview.authored_children != () {
        for child in preview.authored_children {
            if child == ROUTE + "/MovePreview/Target_P0" {
                target_is_direct_child = true;
            }
        }
    }
    target != () && target.ok != false && target.authored_here == true
        && target_is_direct_child
        && ghost != () && ghost.ok != false && ghost.attrs != () && ghost.api_schemas != ()
        && ghost.api_schemas.contains("LunCoPointerInteractionAPI")
        && ghost.attrs["lunco:interaction:left"] == "pass_through"
        && ghost.attrs["lunco:interaction:right"] == "pass_through"
        && ghost.attrs["xformOp:translate"] != ()
        && preview_entity >= 0
}

fn prepare_cursor_trace(me) {
    if !selected_only(this.doc, ROUTE + "/P0") {
        this.failure = "the Move route point menu action did not select P0";
        return;
    }
    if !is_controlled(this.subject) {
        this.failure = "starting a route move released rover possession";
        return;
    }
    let ghost = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview",
        attrs: ["lunco:interaction:left", "lunco:interaction:right", "xformOp:translate"],
        schemas: true,
    });
    let target = query("ResolveUsdTarget", #{
        doc_id: this.doc,
        path: ROUTE + "/MovePreview/Target_P0",
        edit_target: "@view@",
    });
    if ghost == () || ghost.ok == false || ghost.attrs == () || ghost.api_schemas == ()
        || !ghost.api_schemas.contains("LunCoPointerInteractionAPI")
        || ghost.attrs["lunco:interaction:left"] != "pass_through"
        || ghost.attrs["lunco:interaction:right"] != "pass_through"
        || ghost.attrs["xformOp:translate"] == ()
        || target == () || target.ok == false || target.authored_here != true {
        this.failure = "Move did not arm a pass-through preview for P0";
        return;
    }
    let point = query("QueryUsdPrim", #{ doc_id: this.doc, path: this.context_delete_path });
    if point == () || point.ok == false || point.world_position == () {
        this.failure = "could not read P0 position before injected placement";
        return;
    }
    this.point_position_before_move = point.world_position;
    this.move_preview_entity = find_path_in_document(this.doc, ROUTE + "/MovePreview");
    if this.move_preview_entity < 0
        || !track_entity_read(this.move_preview_entity) {
        this.failure = "the projected move ghost has no declared live entity read";
        return;
    }
    let ghost_position = live_render_translation(this.move_preview_entity);
    if ghost_position == () {
        this.failure = "the declared move ghost has no live render transform (entity "
            + this.move_preview_entity.to_string() + ")";
        return;
    }
    this.move_preview_generation = usd_document_generation(this.doc);
    this.last_ghost_position = ghost_position;
    this.cursor_sample_seen = false;
    // Keep the moved marker and its complete context menu inside the viewport.
    this.move_x = this.screen[0] + 240.0;
    this.move_y = this.screen[1];
    for offset in [20.0, 40.0, 60.0, 80.0, 100.0, 120.0,
        140.0, 160.0, 180.0, 200.0, 220.0, 240.0] {
        input_pointer_move(this.screen[0] + offset, this.move_y);
    }
    this.waiting_for = "the coalesced move-preview cursor sample";
}

fn observe_ghost_cursor_sample(me) {
    let ghost_position = live_render_translation(this.move_preview_entity);
    if ghost_position == () {
        this.failure = "the live move ghost lost its render transform during pointer movement";
        return;
    }
    if positions_differ(ghost_position, this.last_ghost_position) {
        this.cursor_sample_seen = true;
    }
}

fn ghost_followed_cursor_or_failed(me) {
    this.failure != "" || this.cursor_sample_seen
}

fn preview_document_generation_is_stable(me) {
    if usd_document_generation(this.doc) != this.move_preview_generation {
        this.failure = "cursor preview movement changed the USD document generation";
        return false;
    }
    true
}

fn press_placement(me) {
    let expectation = begin_curve_view_expectation(
        this, 3, "inherited", true, "the route-point move",
    );
    this = expectation.state;
    if !expectation.ok { return; }
    input_pointer_press("primary", this.move_x, this.move_y);
}
fn release_placement(me) { input_pointer_release("primary", this.move_x, this.move_y); }

fn observe_moved_waypoint(me) {
    let moved = query("QueryUsdPrim", #{
        doc_id: this.doc, path: this.context_delete_path,
    });
    if moved != () && moved.ok != false && moved.world_position != ()
        && positions_differ(moved.world_position, this.point_position_before_move) {
        this.moved_point = moved;
    }
}

fn waypoint_was_moved(me) { this.moved_point != () }

fn moved_waypoint_matches_placement_click(me) {
    if this.primary_contexts.len() == 0 {
        this.failure = "the injected placement click produced no scene.pointer context";
        return false;
    }
    let context = this.primary_contexts[this.primary_contexts.len() - 1];
    if context["selected_path"] != ROUTE + "/P0" {
        this.failure = "the terrain placement context lost the selected waypoint identity";
        return false;
    }
    let clicked_position = point_values(context["world_position"]);
    if clicked_position == () {
        this.failure = "the injected placement click had no active-physics hit point";
        return false;
    }
    if positions_differ(this.moved_point.world_position, clicked_position) {
        this.failure = "the committed waypoint position did not match the placement click";
        return false;
    }
    true
}

fn placement_preserved_rover_and_selection(me) {
    if !is_controlled(this.subject) {
        this.failure = "moving P0 through the injected scene click dropped rover possession";
    } else if !selected_only(this.doc, ROUTE + "/P0") {
        this.failure = "moving P0 changed the selected waypoint";
    }
}

fn move_pointer_to_moved_p0(me) {
    let refreshed = refresh_route_marker_screen(this);
    this = refreshed.state;
    if !refreshed.available {
        this.failure = "moved P0 has no current viewport position for its context gesture";
        return;
    }
    input_pointer_move(this.screen[0], this.screen[1]);
    this.waiting_for = "the post-move injected waypoint context event";
}

fn prepare_moved_marker_projection(me) {
    this.marker = find_path_in_document(this.doc, ROUTE + "/P0/Dome");
    this.screen = ();
    this.last_screen = ();
    this.stable_screen_samples = 0;
    if this.marker < 0 {
        this.failure = "moved P0 has no live marker for its context-menu check";
    }
}

fn third_secondary_context_arrived(me) { this.secondary_contexts.len() >= 3 }

fn third_context_is_valid(me) {
    let context = this.secondary_contexts[this.secondary_contexts.len() - 1];
    let valid = route_context_is_valid(context, this.context_delete_path);
    if !valid {
        this.failure = "moved P0 context hit `"
            + context["hit_path"].to_string()
            + "` at " + context["screen_position"].to_string()
            + "; expected `" + this.context_delete_path + "` at "
            + this.screen.to_string();
    }
    valid
}

fn prepare_delete_menu_row(me) {
    // P0 is an endpoint in the three-point fixture, so its menu contains
    // Select, Move, Add before, Add after, and Delete in that order.
    // The placement keeps the whole menu inside the viewport at its anchor.
    this.menu_x = this.screen[0] + 20.0;
    this.menu_y = this.screen[1] + 16.0 + 4.0 * 21.0;
    let expectation = begin_curve_view_expectation(
        this, 2, "inherited", true, "deleting the moved route point",
    );
    this = expectation.state;
    if !expectation.ok { return; }
    input_pointer_move(this.menu_x, this.menu_y);
}

fn moved_waypoint_is_inactive(me) {
    let deleted = query("QueryUsdPrim", #{
        doc_id: this.doc, path: this.context_delete_path,
    });
    if deleted == () || deleted.ok == false || deleted.active != false { return false; }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    ribbon.ready
}

fn delete_preserved_rover(me) {
    if !is_controlled(this.subject) {
        this.failure = "deleting a moved route point through the context menu dropped rover possession";
    }
}

fn insert_after_p1(me) {
    let expectation = begin_curve_view_expectation(this, 3, "inherited", true, "Add point after");
    this = expectation.state;
    if !expectation.ok { return; }
    let inserted = waypoint_editor::on_insert_after(#{
        doc_id: this.doc,
        route_path: ROUTE,
        selected_path: ROUTE + "/P1",
        target_path: ROUTE + "/P1",
        hit_path: ROUTE + "/P1",
        prim_paths: [ROUTE + "/P1"],
    });
    if inserted == () || inserted.ok != true || inserted.path != ROUTE + "/P3" {
        this.failure = "Add point after failed: " + inserted.to_string();
        return;
    }
    this.inserted_after = inserted.path;
    this.waiting_for = "the Add point after route order projection";
}

fn after_insertion_is_projected(me) {
    let read = waypoint_editor::route_positions(this.doc, ROUTE);
    if read == () || read.ok != true || read.paths == ()
        || !read.paths.contains(this.inserted_after) { return false; }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    ribbon.ready
}

fn after_insertion_order_is_valid(me) {
    let read = waypoint_editor::route_positions(this.doc, ROUTE);
    if read == () || read.ok != true || read.paths.len() != 3
        || read.paths[0] != ROUTE + "/P1"
        || read.paths[1] != this.inserted_after
        || read.paths[2] != ROUTE + "/P2" {
        this.failure = "Add point after produced unexpected route order: "
            + if read == () { "<no route read>" } else { read.paths.to_string() };
        return false;
    }
    this.checks.push(t_true(true,
        "Add point after orders the new direct point after its target"));
    true
}

fn delete_p2_for_inactive_order_check(me) {
    let expectation = begin_curve_view_expectation(
        this, 2, "inherited", true, "deleting inactive-order fixture point P2",
    );
    this = expectation.state;
    if !expectation.ok { return; }
    let deleted = waypoint_editor::on_delete(#{
        doc_id: this.doc, selected_path: ROUTE + "/P2",
    });
    if deleted == () || deleted.ok != true || deleted.path != ROUTE + "/P2" {
        this.failure = "could not deactivate P2 before testing order against inactive points";
        return;
    }
    this.waiting_for = "the inactive P2 route projection";
}

fn p2_is_absent_from_active_route(me) {
    let read = waypoint_editor::route_positions(this.doc, ROUTE);
    if read == () || read.ok != true || read.paths == ()
        || read.paths.contains(ROUTE + "/P2")
        || !read.paths.contains(this.inserted_after) { return false; }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    ribbon.ready
}

fn insert_before_p1(me) {
    let expectation = begin_curve_view_expectation(this, 3, "inherited", true, "Add point before");
    this = expectation.state;
    if !expectation.ok { return; }
    let inserted = waypoint_editor::on_insert_before(#{
        doc_id: this.doc,
        route_path: ROUTE,
        selected_path: ROUTE + "/P1",
        target_path: ROUTE + "/P1",
        hit_path: ROUTE + "/P1",
        prim_paths: [ROUTE + "/P1"],
    });
    if inserted == () || inserted.ok != true || inserted.path != ROUTE + "/P4" {
        this.failure = "Add point before failed after P2 deactivation: " + inserted.to_string();
        return;
    }
    this.inserted_before = inserted.path;
    this.waiting_for = "the Add point before route order projection";
}

fn before_insertion_is_projected(me) {
    let read = waypoint_editor::route_positions(this.doc, ROUTE);
    if read == () || read.ok != true || read.paths == ()
        || !read.paths.contains(this.inserted_before)
        || read.paths.contains(ROUTE + "/P2") { return false; }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    ribbon.ready
}

fn before_insertion_order_is_valid(me) {
    let read = waypoint_editor::route_positions(this.doc, ROUTE);
    if read == () || read.ok != true || read.paths.len() != 3
        || read.paths[0] != this.inserted_before
        || read.paths[1] != ROUTE + "/P1"
        || read.paths[2] != this.inserted_after {
        this.failure = "Add point before included an inactive point or misordered children: "
            + if read == () { "<no route read>" } else { read.paths.to_string() };
        return false;
    }
    this.checks.push(t_true(true,
        "Add point before orders only the remaining composed points after P2 is deactivated"));
    true
}

fn insert_after_final_point(me) {
    let expectation = begin_curve_view_expectation(this, 4, "inherited", true, "endpoint insertion");
    this = expectation.state;
    if !expectation.ok { return; }
    let inserted = waypoint_editor::on_insert_after(#{
        doc_id: this.doc,
        route_path: ROUTE,
        selected_path: this.inserted_after,
        target_path: this.inserted_after,
        hit_path: this.inserted_after,
        prim_paths: [this.inserted_after],
    });
    if inserted == () || inserted.ok != true {
        this.failure = "Add point after the final route point failed: " + inserted.to_string();
        return;
    }
    this.inserted_endpoint = inserted.path;
    this.waiting_for = "the endpoint insertion projection";
}

fn endpoint_is_projected(me) {
    let read = waypoint_editor::route_positions(this.doc, ROUTE);
    if read == () || read.ok != true || read.paths == ()
        || !read.paths.contains(this.inserted_endpoint) { return false; }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    ribbon.ready
}

fn finish_route_interaction(me) {
    let read = waypoint_editor::route_positions(this.doc, ROUTE);
    if read == () || read.ok != true || read.paths.len() != 4
        || read.paths[0] != this.inserted_before
        || read.paths[1] != ROUTE + "/P1"
        || read.paths[2] != this.inserted_after
        || read.paths[3] != this.inserted_endpoint {
        this.failure = "endpoint insertion produced unexpected route order: "
            + if read == () { "<no route read>" } else { read.paths.to_string() };
        return;
    }
    let extension = distance_between(
        read.points[read.points.len() - 2],
        read.points[read.points.len() - 1],
    );
    if extension == () || (extension - 5.0).abs() > 0.01 {
        this.failure = "endpoint insertion did not extend exactly five meters: "
            + extension.to_string();
        return;
    }
    this.checks.push(t_true(true,
        "endpoint insertion appends a route point exactly five meters outward"));
    this.waiting_for = "rover release before generic waypoint deletion";
}

fn release_rover_before_generic_delete(me) {
    let released = release_control_claim(this.subject);
    if released == () || released.ok == false {
        this.failure = "could not release rover control before generic waypoint deletion";
    }
}

fn rover_is_released(me) { !is_controlled(this.subject) }

fn open_different_active_document(me) {
    let opened = cmd("NewDocument", #{ kind: "usd" });
    if opened == () || opened.ok == false {
        this.failure = "could not create another active USD document before generic waypoint deletion";
        return;
    }
    this.waiting_for = "a different active USD document";
}

fn different_active_document_is_ready(me) {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.active_doc_id == () || docs.active_doc_id == this.doc {
        return false;
    }
    this.active_document_id = docs.active_doc_id;
    true
}

fn delete_waypoint_with_generic_command(me) {
    if this.active_document_id == () || this.active_document_id == this.doc {
        this.failure = "generic deletion requires a different active USD document";
        return;
    }
    let target = find_path_in_document(this.doc, this.inserted_after);
    if target < 0 {
        this.failure = "generic deletion target has no live waypoint entity";
        return;
    }
    this.generic_delete_generation_before = document_generation(this.doc);
    let expectation = begin_curve_view_expectation(
        this, 3, "inherited", true, "generic waypoint deletion",
    );
    this = expectation.state;
    if !expectation.ok { return; }
    let deleted = cmd("DeleteEntity", #{ target: target });
    if deleted == () || deleted.ok == false {
        this.failure = "generic DeleteEntity rejected the unpossessed route point: "
            + if deleted == () { "no command result" } else { deleted.error };
        return;
    }
    this.waiting_for = "generic waypoint deletion in its owning Twin document";
}

fn generic_delete_is_projected(me) {
    let route = waypoint_editor::route_positions(this.doc, ROUTE);
    if route == () || route.ok != true || route.paths.contains(this.inserted_after)
        || find_path_in_document(this.doc, this.inserted_after) >= 0 {
        return false;
    }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    if !ribbon.ready { return false; }
    let docs = query("ListOpenDocuments");
    if docs == () || docs.active_doc_id != this.active_document_id {
        this.failure = "generic DeleteEntity changed the active editor document";
        return true;
    }
    this.generic_delete_generation_after = document_generation(this.doc);
    if to_int(this.generic_delete_generation_after)
        != to_int(this.generic_delete_generation_before) + 1 {
        this.failure = "generic DeleteEntity did not author exactly one change in the stage-owning document";
        return true;
    }
    this.checks.push(t_true(true,
        "generic DeleteEntity removes an unpossessed route point from its owning Twin while another USD document is active"));
    true
}

fn undo_generic_waypoint_delete(me) {
    let expectation = begin_curve_view_expectation(
        this, 4, "inherited", true, "undoing generic waypoint deletion",
    );
    this = expectation.state;
    if !expectation.ok { return; }
    let undone = cmd("UndoDocument", #{ doc_id: this.doc });
    if undone == () || undone.ok == false {
        this.failure = "the owning Twin document rejected undo after generic DeleteEntity";
        return;
    }
    this.waiting_for = "the owning Twin document restoring its generically deleted point";
}

fn generic_delete_undo_is_projected(me) {
    let route = waypoint_editor::route_positions(this.doc, ROUTE);
    if route == () || route.ok != true || !route.paths.contains(this.inserted_after)
        || find_path_in_document(this.doc, this.inserted_after) < 0 {
        return false;
    }
    let ribbon = route_curve_view_is_ready_after_edit_state(this);
    this = ribbon.state;
    if !ribbon.ready { return false; }
    let docs = query("ListOpenDocuments");
    if docs == () || docs.active_doc_id != this.active_document_id {
        this.failure = "undo changed the active editor document";
        return true;
    }
    this.checks.push(t_true(true,
        "undo restores the route point through the same owning Twin document"));
    true
}

fn complete_route_curve_view_checks(me) {
    report(this, true,
        "unpossessed edit, possessed context selection, explicit Move, cursor-following preview, placement, ordered insertion, generic delete/undo, and applied ribbon states for zero, one, and multiple points passed");
}

fn task(me, ctx) {
    seq([
        wait_until(Fn("route_fixture_ready")),
        wait_until(Fn("route_curve_view_is_ready_at_fixture")),
        check(Fn("test_state_is_valid")),
        once(Fn("prepare_route_and_add_unpossessed")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("unpossessed_waypoint_ready")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("delete_unpossessed_waypoint")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("unpossessed_waypoint_deleted")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("select_rover")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("rover_is_selected")),
        once(Fn("possess_rover")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("rover_is_possessed")),
        step(Fn("observe_route_marker_projection"), Fn("marker_projection_stable_or_failed")),
        check(Fn("test_state_is_valid")),
        once(Fn("move_pointer_to_p0")),
        next_input_frame(),
        once(Fn("press_secondary")),
        next_input_frame(),
        once(Fn("release_secondary")),
        wait_until(Fn("first_secondary_context_arrived")),
        check(Fn("first_context_is_valid")),
        once(Fn("check_possession_and_selection")),
        check(Fn("test_state_is_valid")),
        next_input_frame(),
        once(Fn("prepare_select_menu_row")),
        next_input_frame(),
        once(Fn("press_menu_primary")),
        next_input_frame(),
        once(Fn("release_menu_primary")),
        wait_until(Fn("p0_is_selected")),
        once(Fn("point_selection_preserved_possession")),
        check(Fn("test_state_is_valid")),
        once(Fn("move_pointer_to_p1")),
        next_input_frame(),
        check(Fn("selection_did_not_arm_move")),
        once(Fn("prepare_second_context")),
        next_input_frame(),
        once(Fn("press_secondary")),
        next_input_frame(),
        once(Fn("release_secondary")),
        wait_until(Fn("second_secondary_context_arrived")),
        check(Fn("second_context_is_valid")),
        once(Fn("prepare_move_menu_row")),
        next_input_frame(),
        once(Fn("press_menu_primary")),
        next_input_frame(),
        once(Fn("release_menu_primary")),
        wait_until(Fn("move_preview_is_armed")),
        once(Fn("prepare_cursor_trace")),
        check(Fn("test_state_is_valid")),
        step(Fn("observe_ghost_cursor_sample"), Fn("ghost_followed_cursor_or_failed")),
        check(Fn("test_state_is_valid")),
        check(Fn("preview_document_generation_is_stable")),
        once(Fn("press_placement")),
        next_input_frame(),
        once(Fn("release_placement")),
        step(Fn("observe_moved_waypoint"), Fn("waypoint_was_moved")),
        check(Fn("moved_waypoint_matches_placement_click")),
        wait_until(Fn("route_curve_view_is_ready_after_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("placement_preserved_rover_and_selection")),
        check(Fn("test_state_is_valid")),
        once(Fn("prepare_moved_marker_projection")),
        step(Fn("observe_route_marker_projection"), Fn("marker_projection_stable_or_failed")),
        check(Fn("test_state_is_valid")),
        once(Fn("move_pointer_to_moved_p0")),
        check(Fn("test_state_is_valid")),
        next_input_frame(),
        once(Fn("press_secondary")),
        next_input_frame(),
        once(Fn("release_secondary")),
        wait_until(Fn("third_secondary_context_arrived")),
        check(Fn("third_context_is_valid")),
        once(Fn("prepare_delete_menu_row")),
        next_input_frame(),
        once(Fn("press_menu_primary")),
        next_input_frame(),
        once(Fn("release_menu_primary")),
        wait_until(Fn("moved_waypoint_is_inactive")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("delete_preserved_rover")),
        check(Fn("test_state_is_valid")),
        once(Fn("insert_after_p1")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("after_insertion_is_projected")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        check(Fn("after_insertion_order_is_valid")),
        check(Fn("test_state_is_valid")),
        once(Fn("delete_p2_for_inactive_order_check")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("p2_is_absent_from_active_route")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("insert_before_p1")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("before_insertion_is_projected")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        check(Fn("before_insertion_order_is_valid")),
        check(Fn("test_state_is_valid")),
        once(Fn("insert_after_final_point")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("endpoint_is_projected")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("finish_route_interaction")),
        check(Fn("test_state_is_valid")),
        once(Fn("release_rover_before_generic_delete")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("rover_is_released")),
        once(Fn("open_different_active_document")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("different_active_document_is_ready")),
        once(Fn("delete_waypoint_with_generic_command")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("generic_delete_is_projected")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("undo_generic_waypoint_delete")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("generic_delete_undo_is_projected")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("delete_to_three_route_points")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_curve_view_is_ready_after_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("delete_to_two_route_points")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_curve_view_is_ready_after_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("delete_to_one_route_point")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_curve_view_is_ready_after_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("delete_to_empty_route")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_curve_view_is_ready_after_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("add_one_route_point")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_curve_view_is_ready_after_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("add_second_route_point")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_curve_view_is_ready_after_edit")),
        check(Fn("test_state_is_valid")),
        once(Fn("record_route_curve_view_edit")),
        once(Fn("complete_route_curve_view_checks")),
        check(Fn("test_state_is_valid")),
    ])
}

fn on_start(me, ctx) {
    this.done = false;
    this.checks = [];
    this.failure = "";
    this.waiting_for = "the route fixture";
    this.doc = -1;
    this.subject = -1;
    this.point = -1;
    this.marker = -1;
    this.screen = ();
    this.last_screen = ();
    this.stable_screen_samples = 0;
    this.added_without_possession = ();
    this.route_children_before_edit = [];
    this.context_delete_path = ROUTE + "/P0";
    this.secondary_contexts = [];
    this.primary_contexts = [];
    this.move_preview_entity = -1;
    this.move_preview_generation = -1;
    this.last_ghost_position = ();
    this.cursor_sample_seen = false;
    this.moved_point = ();
    this.inserted_after = ();
    this.inserted_before = ();
    this.inserted_endpoint = ();
    this.active_document_id = ();
    this.generic_delete_generation_before = ();
    this.generic_delete_generation_after = ();
    this.curve_edit_baseline = 0;
    this.curve_expected_point_count = 0;
    this.curve_expected_visibility = "";
    this.curve_expected_mesh = false;
    this.curve_edit_label = "";
    this.input_frame_samples = 0;
    subscribe("scene.pointer");
    subscribe("TASK_FAILED");
}

fn on_event(me, evt, ctx) {
    if this.done { return; }
    if evt.name == "scene.pointer" && evt.value != () {
        if evt.value["button"] == "secondary" {
            this.secondary_contexts.push(evt.value);
        } else if evt.value["button"] == "primary" {
            this.primary_contexts.push(evt.value);
        }
        return;
    }
    if evt.name == "TASK_FAILED" {
        let message = if this.failure == "" {
            "the route interaction behavior tree failed while waiting for " + this.waiting_for
        } else { this.failure };
        report(this, false, message);
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        report(this, false,
            "injected route interaction stopped before its behavior tree produced a verdict");
    }
}
