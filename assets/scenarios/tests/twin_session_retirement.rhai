// Production document-retirement gate and live Twin-switch assertions.
// External API drivers supply the outgoing IDs to verify_editor_retirement.
const TEST_KIND = "headless";
import "lunco://scripting/tools/modelica_editor" as modelica_editor;

fn document_present(id) {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () {
        throw "ListOpenDocuments did not publish its document inventory";
    }
    for doc in docs.open_documents {
        if doc.doc_id.to_string() == id.to_string() { return true; }
    }
    false
}

fn retirement_checks(retired_ids, retained_ids) {
    if retired_ids.is_empty() { throw "retirement requires an outgoing document"; }
    let checks = [];
    for id in retired_ids {
        checks.push(t_true(!document_present(id),
            "outgoing document " + id.to_string() + " is retired"));
    }
    for id in retained_ids {
        checks.push(t_true(document_present(id),
            "current document " + id.to_string() + " remains open"));
    }
    checks
}

fn verify_editor_retirement(retired_ids, retained_ids) {
    let checks = retirement_checks(retired_ids, retained_ids);
    let viewport = query("InspectUsdViewport");
    if viewport == () || viewport.previews == () { throw "USD viewport inventory missing"; }
    for preview in viewport.previews {
        let retired = false;
        for id in retired_ids {
            if preview.doc_id.to_string() == id.to_string() { retired = true; }
        }
        checks.push(t_true(!retired, "preview belongs to a current document"));
    }
    report_verdict(checks, "Twin editor retirement", "TWIN_SESSION_RETIREMENT")
}

fn verify_session_preserved(ids) {
    if ids.is_empty() { throw "preservation requires an open document"; }
    let checks = [];
    for id in ids {
        checks.push(t_true(document_present(id), "rejected replacement preserves its document"));
    }
    report_verdict(checks, "Rejected Twin replacement", "TWIN_SESSION_PRESERVED")
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.checks = [];
    let created = modelica_editor::new_scratch(
        "model RetirementProbe Real x; equation x = 1; end RetirementProbe;",
        "RetirementProbe");
    if created == () || created.ok != true || created.doc_id == () {
        throw "Modelica scratch creation did not return its document identity";
    }
    this.doc = created.doc_id;
}

fn ready_to_close(me) { document_present(this.doc) }

fn close_probe(me) {
    this.checks.push(t_true(document_present(this.doc), "scratch document opened"));
    cmd("CloseDocument", #{ doc_id: this.doc });
}

fn probe_retired(me) { !document_present(this.doc) }

fn finish_probe(me) {
    this.checks.push(t_true(!document_present(this.doc),
        "generic CloseDocument retires the headless Modelica document"));
    // Closing a retired ID must be harmless and cannot recreate its registry.
    cmd("CloseDocument", #{ doc_id: this.doc });
    this.checks.push(t_true(!document_present(this.doc), "repeated close remains retired"));
    this.done = true;
    report_verdict(this.checks, "Twin document close", "TWIN_DOCUMENT_CLOSE");
}

fn timeout(me) {
    if this.done { return; }
    this.done = true;
    this.checks.push(t_true(false, "document lifecycle did not complete"));
    report_verdict(this.checks, "Twin document close", "TWIN_DOCUMENT_CLOSE");
}

fn task(me, ctx) {
    par_race([
        seq([wait_until(Fn("ready_to_close")), once(Fn("close_probe")),
             wait_until(Fn("probe_retired")), once(Fn("finish_probe"))]),
        seq([wait(10.0), once(Fn("timeout"))]),
    ])
}
