// @timing presentation
// The API harness supplies a mounted document and a mesh in that document.
fn on_start(me, ctx) {
    this.doc = ctx.doc_id;
    this.mesh = ctx.mesh;
    this.revision = ();
    this.done = false;
    let result = cmd("ApplyUsdOp", #{doc_id:this.doc, op:#{SetStageMetrics:#{
        edit_target:"@root@", metrics:#{meters_per_unit:1.01, up_axis:"Y"}
    }}});
    if !result.ok {
        this.done = true;
        report_verdict([result.to_string()], "USD VISUAL COMPLETION", "USD_VISUAL_COMPLETION");
    }
}

fn on_event(me, evt, ctx) {
    if this.done || !evt.name.starts_with("usd.") {return;}
    let event = evt.value;
    if event.doc_id != this.doc || event.projection_mode != "full_stage" {return;}
    if evt.name == "usd.document.projected" {
        this.revision = event.generation;
        return;
    }
    if evt.name != "usd.projection.visuals_ready" && evt.name != "usd.projection.visuals_failed" {return;}
    let mesh = -1;
    for document in query("ListOpenDocuments").open_documents {
        if document.kind == "usd" && document.doc_id.to_string() == this.doc.to_string() {
            mesh = find_path_in_document(document.doc_id, this.mesh);
            break;
        }
    }
    this.done = true;
    report_verdict([
        t_true(evt.name == "usd.projection.visuals_ready" && this.revision == event.generation,
            "visual completion follows publication of the same document revision"),
        t_true(mesh > 0 && get(mesh, "Mesh3d") != () && get(mesh, "UsdSceneProjectionFailed") == ()
            && get(mesh, "PendingPbrLook") == () && get(mesh, "FailedPbrLook") == (),
            "the affected mesh is available at visual completion")
    ], "USD VISUAL COMPLETION", "USD_VISUAL_COMPLETION");
}
