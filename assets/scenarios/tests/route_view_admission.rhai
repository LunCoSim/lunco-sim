// Attach to the mounted fixture with RunScenarioAsset and params: #{ doc_id: owning_document }.
const ROOT = "/ModelicaRuntimeContracts";

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
    this = #{done:false, phase:0, ticks:0, doc:ctx.doc_id, waiting_for:"mounted route owner"};
    if this.doc == () {
        this = finish(this, [t_true(false, "route admission requires the mounted document ID")]);
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 600 { this = finish(this, [t_true(false, "route view admission timed out waiting for " + this.waiting_for)]); return; }
    if this.phase == 0 {
        if !command_available("SetSceneViewObject") {
            this = finish(this, [t_true(false, "render host has no keyed presentation owner")]); return;
        }
        let parent = query("QueryUsdPrim", #{doc_id:this.doc, path:ROOT});
        if parent == () || parent.ok == false || parent.doc_id == () {
            this = finish(this, [t_true(false, "fixture must be mounted before starting admission: " + parent.to_string())]); return;
        }
        this.generation = usd_document_generation(this.doc);
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        let submitted = waypoint_editor::update_ribbon_view(this.doc, ROOT,
            [[0.0, 0.0, 0.0], [0.0, 0.0, 5.0]], ());
        if submitted.ok != true { this = finish(this, [t_true(false, submitted.error)]); return; }
        let slot = waypoint_editor::route_view(this.doc, ROOT, "route.ribbon");
        if !slot.ok { this = finish(this, [t_true(false, slot.error)]); return; }
        this.waiting_for = "curve identity " + slot.to_string() + ", submission " + submitted.to_string();
        if slot.entity_id == () || submitted.pending == true { return; }
        this.curve = slot.entity_id;
        this.phase = 2;
        return;
    }
    let view = query("InspectUsdCurveView", #{ doc_id: this.doc, entity_id: this.curve });
    if view == () || view.ok == false || view.state == "failed" {
        this = finish(this, [t_true(false, "curve publication failed: " + view.to_string())]); return;
    }
    this.waiting_for = "curve publication " + view.to_string();
    if view.state == "pending" { return; }
    let object = waypoint_editor::route_view(this.doc, ROOT, "route.ribbon");
    let before = usd_document_generation(this.doc);
    let rejected = waypoint_editor::update_ribbon_view(this.doc, "/MissingRoute",
        [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]], ());
    let after = usd_document_generation(this.doc);
    this = finish(this, [
        t_true(view.state == "ready" && to_int(view.result_segment_count) == 1
            && view.applied_revision == view.requested_revision,
            "the mounted owner publishes the complete curve"),
        t_true(object.entity_id == this.curve && object.left == "pass_through" && object.right == "context"
            && this.generation == after, "first ribbon activation creates only a pickable runtime view"),
        t_true(rejected.ok == false && rejected.retryable == false && rejected.error != ""
            && before == after, "invalid view edits reject terminally without mutation"),
    ]);
}
