const ROOT = "/ModelicaRuntimeContracts/CurveOwner";
const GEOMETRY = ROOT + "/RouteRibbon/Geometry";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["QueryUsdPrim", "InspectUsdCurveView"], required_inputs: [] }
}

fn finish(state, checks) {
    report_verdict(checks, "ROUTE VIEW ADMISSION", "ROUTE_VIEW_ADMISSION");
    state.done = true;
    state
}

fn on_start(me, ctx) {
    this = #{ done: false, phase: 0, ticks: 0, doc: () };
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 600 { this = finish(this, [t_true(false, "route view admission timed out")]); return; }
    if this.phase == 0 {
        let parent = query("QueryUsdPrim", #{ path: "/ModelicaRuntimeContracts" });
        if parent == () || parent.ok == false { return; }
        this.doc = parent.doc_id;
        let created = cmd("ApplyUsdTransientOps", #{ doc_id: this.doc,
            ops: [#{ AddPrim: #{ edit_target: "@view@",
                parent_path: "/ModelicaRuntimeContracts", name: "CurveOwner", type_name: "Xform",
                reference: "lunco://markers/route_point.usda", reference_prim_path: "" }}] });
        if created.ok != true { this = finish(this, [t_true(false, "reference owner creation failed")]); return; }
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        let seed = waypoint_editor::update_ribbon_view(this.doc, ROOT,
            [[0.0, 0.0, 0.0], [0.0, 0.0, 5.0]]);
        if seed.ok != true { this = finish(this, [t_true(false, seed.error)]); return; }
        let curve = find_path(GEOMETRY);
        if curve < 0 || seed.pending == true { return; }
        this.curve = curve;
        this.phase = 2;
        return;
    }
    let view = query("InspectUsdCurveView", #{ doc_id: this.doc, entity_id: this.curve });
    if view == () || view.ok == false || view.state == "pending" { return; }
    let prim = query("QueryUsdPrim", #{ doc_id: this.doc, path: GEOMETRY,
        attrs: ["lunco:interaction:left", "lunco:interaction:right"], schemas: true });
    let before = usd_document_generation(this.doc);
    let rejected = waypoint_editor::update_ribbon_view(this.doc, "/MissingRoute",
        [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let after = usd_document_generation(this.doc);
    this = finish(this, [
        t_true(view.state == "ready" && to_int(view.result_segment_count) == 1
            && view.applied_revision == view.requested_revision,
            "the reference owner admits geometry before the curve view publishes"),
        t_true(prim != () && prim.ok != false && prim.type_name == "BasisCurves"
            && prim.api_schemas.contains("LunCoPointerInteractionAPI")
            && prim.attrs["lunco:interaction:left"] == "pass_through"
            && prim.attrs["lunco:interaction:right"] == "context",
            "document-scoped referenced child facts preserve the marker's interaction contract"),
        t_true(rejected.ok == false && rejected.retryable == false && rejected.error != ""
            && before == after, "invalid view edits reject terminally without mutation"),
    ]);
}
