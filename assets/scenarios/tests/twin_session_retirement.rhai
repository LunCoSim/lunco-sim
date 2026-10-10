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

fn verify_twin_browser_retirement(retired_ids, retained_ids, replacement_root) {
    replacement_root.replace("\\", "/");
    let checks = retirement_checks(retired_ids, retained_ids);
    let twin = query("ListTwin");
    if twin == () || twin.open != true { throw "replacement Twin inventory missing"; }
    let browser_root = twin.root;
    browser_root.replace("\\", "/");
    checks.push(t_true(browser_root == replacement_root, "browser inventory belongs to the replacement Twin"));
    checks.push(t_true(!twin.files.is_empty(), "replacement publishes its indexed files"));
    for file in twin.files {
        let path = file.absolute_path;
        path.replace("\\", "/");
        checks.push(t_true(path.starts_with(replacement_root + "/"),
            "indexed file belongs to the replacement root"));
    }
    let docs = query("ListOpenDocuments");
    let current_owner = ();
    for doc in docs.open_documents {
        if doc.runtime_context.kind == "local_twin" {
            if current_owner == () { current_owner = doc.runtime_context.twin; }
            checks.push(t_true(doc.runtime_context.twin == current_owner,
                "current documents share the replacement Twin owner"));
        } else {
            // Workspace restoration also admits private editor buffers.
            checks.push(t_true(doc.runtime_context.kind == "application",
                "private restored buffer keeps its application owner"));
        }
    }
    checks.push(t_true(current_owner != (), "replacement publishes Twin-owned documents"));
    report_verdict(checks, "Twin browser inventory retirement", "TWIN_BROWSER_RETIREMENT")
}

fn verify_session_preserved(ids) {
    if ids.is_empty() { throw "preservation requires an open document"; }
    let checks = [];
    for id in ids {
        checks.push(t_true(document_present(id), "rejected replacement preserves its document"));
    }
    report_verdict(checks, "Rejected Twin replacement", "TWIN_SESSION_PRESERVED")
}

fn verify_additive_workspace(ids, active_root) {
    let checks = [];
    for id in ids {
        checks.push(t_true(document_present(id), "additive opening preserves the existing document"));
    }
    let twin = query("ListTwin");
    checks.push(t_true(twin.open == true && twin.root == active_root,
        "additive opening preserves the active Twin"));
    report_verdict(checks, "Additive Twin workspace", "TWIN_ADDITIVE_WORKSPACE")
}

fn verify_catalog_retirement(retired_paths) {
    if retired_paths.is_empty() { throw "catalog retirement requires an outgoing source"; }
    let metadata = query("ListUsdAssetMetadata");
    let palette = query("ListSpawnCatalog");
    let present = #{};
    for entry in metadata.entries { present[entry.path] = true; }
    for entry in palette.entries { present[entry.source.path] = true; }
    let checks = [];
    for retired in retired_paths {
        checks.push(t_true(!present.contains(retired), "retired catalog source is absent: " + retired));
    }
    report_verdict(checks, "Twin catalog retirement", "TWIN_CATALOG_RETIREMENT")
}

fn verify_spawn_catalog_source(spawnable_paths, non_spawnable_paths) {
    let authority = twin_name();
    if authority.is_empty() || spawnable_paths.is_empty() {
        throw "catalog source check requires a mounted Twin and a positive asset";
    }
    let palette = query("ListSpawnCatalog");
    let checks = [];
    for relative in spawnable_paths + non_spawnable_paths {
        let path = "twin://" + authority + "/" + relative;
        let found = false;
        for entry in palette.entries {
            if entry.source.path == path { found = true; }
        }
        checks.push(t_true(found == spawnable_paths.contains(relative),
            "catalog admission matches authored loadability for " + path));
    }
    report_verdict(checks, "Twin Spawn source context", "TWIN_SPAWN_SOURCE_CONTEXT")
}

fn sysml_registry_retired(id) {
    let inspection = query("InspectSysmlDocument", #{ doc_id: id });
    inspection != () && inspection.ok == false && inspection.status == "not_found"
        && inspection.error.contains("SysML document") && inspection.error.contains("is not open")
}

fn verify_sysml_retirement(ids) {
    if ids.is_empty() { throw "SysML retirement requires a document"; }
    let checks = [];
    for id in ids {
        checks.push(t_true(sysml_registry_retired(id), "SysML domain registry released its retired document"));
    }
    report_verdict(checks, "SysML registry retirement", "TWIN_SYSML_RETIREMENT")
}

// The active scripting name must address the current mount's semantic source
// set, including after the same root is reopened under a fresh authority.
fn verify_twin_source_scope(retired_authority) {
    let authority = twin_name();
    let current = query("AnalyzeSysml", #{path: "twin://" + authority, tables: []});
    let checks = [t_true(authority != "" && current != () && current.ok == true,
        "the active scripting authority resolves its mounted SysML source set")];
    if retired_authority != "" {
        let retired = query("AnalyzeSysml", #{path: "twin://" + retired_authority, tables: []});
        checks.push(t_true(authority != retired_authority,
            "reopening publishes a fresh scripting authority"));
        checks.push(t_true(retired != () && retired.ok == false
            && retired.errors != () && retired.errors.len() > 0
            && retired.errors[0].contains("not mounted"),
            "the retired source authority remains rejected"));
    }
    report_verdict(checks, "Twin scripting source scope", "TWIN_SCRIPT_SOURCE_SCOPE")
}

fn verify_modelica_retirement(ids) {
    if ids.is_empty() { throw "Modelica retirement requires a document"; }
    let checks = [];
    for id in ids {
        let result = query("ListCompileCandidates", #{ doc_id: id });
        checks.push(t_true(result.ok == false && result.status == "not_found"
            && result.error.contains("doc_id " + id.to_string() + " not in registry"),
            "closed Modelica document is absent from the Models registry"));
    }
    report_verdict(checks, "Twin Models registry retirement", "TWIN_MODELICA_RETIREMENT")
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments", "InspectSysmlDocument"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.checks = [];
    this.sysml = ();
    cmd("NewDocument", #{ kind: "sysml" });
    let created = modelica_editor::new_scratch(
        "model RetirementProbe Real x; equation x = 1; end RetirementProbe;",
        "RetirementProbe");
    if created == () || created.ok != true || created.doc_id == () {
        throw "Modelica scratch creation did not return its document identity";
    }
    this.doc = created.doc_id;
}

fn ready_to_close(me) {
    if !document_present(this.doc) { return false; }
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind == "sysml" { this.sysml = doc.doc_id; }
    }
    this.sysml != ()
}

fn close_probe(me) {
    this.checks.push(t_true(document_present(this.doc), "scratch document opened"));
    cmd("CloseDocument", #{ doc_id: this.doc });
    cmd("CloseDocument", #{ doc_id: this.sysml });
}

fn probe_retired(me) { !document_present(this.doc) && !document_present(this.sysml) }

fn finish_probe(me) {
    this.checks.push(t_true(!document_present(this.doc),
        "generic CloseDocument retires the headless Modelica document"));
    // Closing a retired ID must be harmless and cannot recreate its registry.
    cmd("CloseDocument", #{ doc_id: this.doc });
    cmd("CloseDocument", #{ doc_id: this.sysml });
    this.checks.push(t_true(!document_present(this.sysml),
        "generic CloseDocument retires dirty SysML documents and repeated close is harmless"));
    this.checks.push(t_true(sysml_registry_retired(this.sysml), "closed SysML document is absent from its domain registry"));
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
