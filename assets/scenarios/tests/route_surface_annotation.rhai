// Production GUI gate, attached to an addressable scene entity with RunScenarioAsset.
// params: #{ doc_id: document_id, route_path: "/scene/route" }.
// Mutates only its disposable view.
const TEST_KIND = "editor";
import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["QueryUsdPrim", "QueryUsdPrims", "ResolveUsdTarget", "InspectUsdCurveView"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = true;
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
    this.done = false;
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
        report_verdict(this.checks, "ROUTE SURFACE ANNOTATION", "ROUTE_SURFACE_ANNOTATION");
        this.done = true;
    }
}
