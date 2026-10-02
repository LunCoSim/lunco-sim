// Production GUI gate, attached to an addressable scene entity with RunScenarioAsset.
// params: #{ doc_id: document_id, route_path: "/scene/route", view_owner: program_entity_id }.
// Pause the explicit view writer while checking disposable publication; restore its pause state.
// Leaves authored route facts unchanged.
const TEST_KIND = "editor";
import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [ctx.view_owner], entity_writes: [ctx.view_owner],
       query_reads: ["QueryUsdPrim", "QueryUsdPrims", "ResolveUsdTarget", "InspectUsdCurveView", "ScriptInspect"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = true;
    this.view_owner = ();
    if ctx.view_owner == () { throw "view_owner is required for exclusive presentation inspection"; }
    if ctx.route_path == () || ctx.doc_id == () { throw "doc_id and route_path are required"; }
    let owner = query("QueryUsdPrim", #{ doc_id: ctx.doc_id, path: ctx.route_path });
    if owner == () || owner.doc_id == () { throw "route document is unavailable: " + owner.to_string(); }
    let route = waypoint_editor::route_positions(owner.doc_id, ctx.route_path);
    if route.ok != true || route.points.len() < 2 { throw "gate needs at least two authored route points"; }
    this.doc = owner.doc_id;
    this.route_path = ctx.route_path;
    this.points = route.points;
    this.paths = route.paths;
    this.root = find_path_in_document(this.doc, waypoint_editor::ribbon_path(this.route_path));
    this.curve = find_path_in_document(this.doc, waypoint_editor::ribbon_geometry_path(this.route_path));
    this.checks = [];
    this.phase = 0;
    this.ticks = 0;
    let writer = query("ScriptInspect", #{ target: ctx.view_owner });
    if writer == () || writer.scripted != true { throw "view_owner has no live scenario"; }
    this.owner_was_paused = writer.paused;
    let paused = cmd("SetScenarioPaused", #{ target: ctx.view_owner, paused: true });
    if paused.ok != true { throw "view_owner could not be paused"; }
    this.view_owner = ctx.view_owner;
    this.done = false;
}

fn restore_view_owner(state) {
    if state.view_owner != () {
        let restored = cmd("SetScenarioPaused", #{ target: state.view_owner, paused: state.owner_was_paused });
        if restored.ok != true { throw "view_owner pause state could not be restored"; }
        state.view_owner = ();
    }
    state
}

fn on_stop(me, ctx) {
    if this.view_owner != () { submit_view(this, this.points); }
    this = restore_view_owner(this);
}

fn inspect_view(state) {
    query("InspectUsdCurveView", #{ doc_id: state.doc, entity_id: state.curve })
}

fn submit_view(state, points) {
    cmd("UpdateUsdCurveView", #{ doc_id: state.doc, root_entity_id: state.root,
        curve_entity_id: state.curve, points: points, width_m: waypoint_editor::route_ribbon_width() })
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 3600 {
        this.checks.push(t_true(false, "surface annotation gate timed out"));
        submit_view(this, this.points);
        this = restore_view_owner(this);
        report_verdict(this.checks, "ROUTE SURFACE ANNOTATION", "ROUTE_SURFACE_ANNOTATION");
        this.done = true;
        return;
    }
    let view = inspect_view(this);
    if view == () || view.state == "pending" { return; }
    if this.phase == 0 {
        if view.state != "ready" || view.projection != "terrain_surface" { return; }
        this.checks.push(t_true(view.local_visibility == "hidden"
            && to_int(view.result_segment_count) == this.points.len() - 1
            && to_int(view.surface_binding_count) > 0,
            "sparse route is painted on terrain while its independent mesh is hidden"));
        // A five-kilometre closed path stays inside the supplied terrain while
        // exercising dense spatial subdivision and the GPU node-walk ABI.
        let centre = this.points[0];
        let outer = [centre[0] + 300.0, centre[1], centre[2] + 300.0];
        let long_path = [outer];
        for i in 0..2001 {
            let angle = to_float(i) * 6.283185307179586 / 100.0;
            long_path.push([centre[0] + 40.0 * cos(angle), centre[1], centre[2] + 40.0 * sin(angle)]);
        }
        long_path.push(outer);
        this.previous_revision = view.requested_revision;
        this.checks.push(t_true(submit_view(this, long_path).ok, "long-path presentation accepted"));
        this.phase = 4; return;
    }
    if this.phase == 4 {
        if to_int(view.requested_revision) <= to_int(this.previous_revision) || view.state == "pending" { return; }
        this.checks.push(t_true(view.state == "ready" && view.projection == "terrain_surface"
            && to_int(view.result_segment_count) == 2002 && view.applied_revision == view.requested_revision,
            "all five-kilometre path legs publish without density retirement"));
        this.long_ready_tick = this.ticks;
        print("[route-gate] long path published");
        this.phase = 5; return;
    }
    if this.phase == 5 {
        if this.ticks - this.long_ready_tick < 120 { return; }
        // A finite route outside the DEM must fail at its surface owner.
        let invalid = submit_view(this, [[1.0e12, 0.0, 1.0e12], [1.0e12 + 10.0, 0.0, 1.0e12]]);
        this.previous_revision = view.requested_revision;
        this.checks.push(t_true(invalid.ok == true, "negative fixture was admitted for bounded preparation"));
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        if to_int(view.requested_revision) <= to_int(this.previous_revision) { return; }
        this.checks.push(t_true(view.state == "failed" && view.local_visibility == "hidden"
            && view.error.contains("terrain coverage"),
            "missing surface coverage gives a terminal failure without fabricated geometry"));
        submit_view(this, this.points);
        this.previous_revision = view.requested_revision;
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        if to_int(view.requested_revision) <= to_int(this.previous_revision) { return; }
        if view.state != "ready" { return; }
        let current = waypoint_editor::route_positions(this.doc, this.route_path);
        this.checks.push(t_true(current.ok == true && current.paths == this.paths
            && current.points == this.points,
            "presentation preparation and failure leave authored waypoint topology and positions unchanged"));
        this.checks.push(t_true(view.projection == "terrain_surface"
            && to_int(view.result_segment_count) == this.points.len() - 1
            && view.applied_revision == view.requested_revision && to_int(view.surface_binding_count) > 0,
            "current surface publication restores the sparse route"));
        this = restore_view_owner(this);
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        let writer = query("ScriptInspect", #{ target: ctx.view_owner });
        this.checks.push(t_true(writer != () && writer.paused == this.owner_was_paused,
            "the explicit view writer retains its original pause state after the gate"));
        report_verdict(this.checks, "ROUTE SURFACE ANNOTATION", "ROUTE_SURFACE_ANNOTATION");
        this.done = true;
    }
}
