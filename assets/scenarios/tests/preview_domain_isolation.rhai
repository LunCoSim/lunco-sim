// TEST-ONLY: preview ownership must not admit runtime Modelica networks.
const TEST_KIND = "graphics";
const ROOT = "/PreviewDomainIsolation";
const DORMANT = "/DormantPreviewNetwork";

fn on_start(me) {
    this.phase = 0; this.elapsed = 0.0; this.observed = 0.0;
    this.done = false; this.doc = -1; this.fork = -1;
    this.ids = []; this.checks = []; this.leaked = false;
}
fn ready(preview, doc) {
    let generation = assembly_edit::describe(doc).generation;
    for p in assembly_edit::viewport().previews {
        if p.preview == preview && p.doc == doc && p.projection_ready
            && p.projected_generation == generation { return true; }
    }
    false
}
fn preview_has_solver() {
    for d in query("ListOpenDocuments").open_documents {
        if d.kind == "modelica" && d.title.contains("DormantPreviewNetwork") { return true; }
    }
    let entity = find(DORMANT + "/Network");
    entity >= 0 && get(entity, "voltage") != ()
}
fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 40.0 {
        this.checks.push(t_true(false, "preview isolation timeout phase " + this.phase));
        this.done = true;
        report_verdict(this.checks, "PREVIEW DOMAIN ISOLATION", "PREVIEW_DOMAIN_ISOLATION");
        return;
    }
    if this.phase == 0 {
        let live = find(ROOT + "/Network");
        if live < 0 || get(live, "voltage") == () { return; }
        this.live = live;
        this.checks.push(t_true(get(live, "voltage") > 20.0, "live electrical network solves"));
        for d in query("ListOpenDocuments").open_documents {
            this.ids.push(d.doc_id);
            if d.kind == "usd" {
                let prim = assembly_edit::inspect(d.doc_id, ROOT).prim;
                if prim != () && prim.exists { this.doc = d.doc_id; }
            }
        }
        if this.doc < 0 { return; }
        assembly_ui::open_session(15501, this.doc, "@root@");
        assembly_edit::fork_document(this.doc, "Dormant network preview");
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        if !ready(15501, this.doc) { return; }
        for d in query("ListOpenDocuments").open_documents {
            if d.kind == "usd" && !this.ids.contains(d.doc_id) { this.fork = d.doc_id; }
        }
        if this.fork < 0 { return; }
        let g = assembly_edit::describe(this.fork).generation;
        let ack = assembly_edit::batch(this.fork, "Distinct preview-only network",
            [#{ MovePrim: #{ edit_target: "@root@", from_path: ROOT, to_path: DORMANT } }], g);
        if ack.ok == false { this.elapsed = 41.0; return; }
        assembly_ui::open_session(15502, this.fork, "@root@");
        this.phase = 2;
        return;
    }
    if this.phase == 2 || this.phase == 3 {
        if !ready(15502, this.fork) { return; }
        this.leaked = this.leaked || preview_has_solver();
        this.observed += dt();
        if this.observed < 3.0 { return; }
        this.checks.push(t_true(!this.leaked, "preview phase " + this.phase + " has no generated solver or output"));
        this.checks.push(t_true(get(this.live, "voltage") != () && get(this.live, "voltage") > 20.0,
            "preview edits preserve the live solver"));
        if this.phase == 2 {
            let g = assembly_edit::describe(this.fork).generation;
            let ack = assembly_edit::batch(this.fork, "Invalid preview connection remains inert",
                [#{ SetConnection: #{ edit_target: "@root@", path: DORMANT + "/Network/Panel",
                    name: "inputs:sun_mount_x", type_name: "float", sources: ["/Absent.outputs:sun_x"] } }], g);
            this.checks.push(t_true(ack.ok != false, "invalid network fact can be authored for inspection"));
            this.observed = 0.0; this.leaked = false; this.phase = 3;
            return;
        }
        let inspected = assembly_edit::inspect(this.fork, DORMANT + "/Network/Panel");
        this.checks.push(t_true(inspected.prim.exists, "invalid preview remains inspectable"));
        assembly_edit::preview_close(15502);
        assembly_edit::close_document(this.fork);
        assembly_edit::preview_close(15501);
        this.done = true;
        report_verdict(this.checks, "PREVIEW DOMAIN ISOLATION", "PREVIEW_DOMAIN_ISOLATION");
    }
}
