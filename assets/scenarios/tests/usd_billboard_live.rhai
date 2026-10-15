const TEST_KIND = "headless";
const ROOT = "/ModelicaRuntimeContracts";
const TARGET = ROOT + "/Label";
const CHILD = TARGET + "/Child";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [find_path(TARGET), find_path(CHILD)],
       entity_writes: [find_path(CHILD)],
       query_reads: ["QueryUsdPrim"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this = #{ phase: 0, ticks: 0, done: false, doc: (), child: () };
}

fn finish(checks) {
    report_verdict(checks, "USD BILLBOARD LIVE", "USD_BILLBOARD_LIVE");
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 180 {
        this.done = true; finish([t_true(false, "billboard projection timed out")]); return;
    }
    let node = find_path(TARGET);
    let child = find_path(CHILD);
    if node < 0 || child < 0 { return; }
    if this.phase == 0 {
        let root = query("QueryUsdPrim", #{path: ROOT});
        this.doc = root.doc_id;
        this.child = child;
        track_entity_read(node);
        track_entity_read(child);
        track_entity_write(child);
        if set(child, "Transform.translation.x", 7.0) != true {
            this.done = true; finish([t_true(false, "the runtime child probe must be writable")]); return;
        }
        let applied = cmd("ApplyUsdTransientOps", #{doc_id: this.doc, ops: [
            #{SetAttribute: #{edit_target: "@view@", path: TARGET,
                name: "lunco:billboard", type_name: "bool", value: "true"}},
            #{SetAttribute: #{edit_target: "@view@", path: TARGET,
                name: "lunco:billboard:text", type_name: "string", value: "Updated"}},
        ]});
        if applied.ok != true { this.done = true; finish([t_true(false, applied.error)]); return; }
        this.phase = 1; return;
    }
    let prim = query("QueryUsdPrim", #{doc_id: this.doc, path: TARGET,
        attrs: ["lunco:billboard", "lunco:billboard:text"]});
    if prim == () || prim.ok == false || prim.attrs == () { return; }
    if child != this.child {
        this.done = true;
        finish([t_true(false, "a billboard edit must preserve the projected subtree")]); return;
    }
    track_entity_read(child);
    track_entity_read(node);
    if get(child, "Transform.translation.x") != 7.0 {
        this.done = true;
        finish([t_true(false, "a billboard edit must preserve live child state")]); return;
    }
    let label = get(node, "UsdBillboard");
    if this.phase == 1 {
        if label == () { return; }
        if label.template != "Updated" {
            this.done = true;
            finish([t_true(false, "the live billboard component must consume its authored text")]); return;
        }
        let applied = cmd("ApplyUsdTransientOps", #{doc_id: this.doc, ops: [
            #{SetAttribute: #{edit_target: "@view@", path: TARGET,
                name: "lunco:billboard", type_name: "bool", value: "false"}},
        ]});
        if applied.ok != true { this.done = true; finish([t_true(false, applied.error)]); return; }
        this.phase = 2; return;
    }
    if prim.attrs["lunco:billboard"] != false { return; }
    let generation = usd_document_generation(this.doc);
    let rejected = cmd("ApplyUsdTransientOps", #{doc_id: this.doc, ops: [
        #{SetAttribute: #{edit_target: "@view@", path: TARGET,
            name: "lunco:billboard", type_name: "bool", value: "invalid"}},
    ]});
    this.done = true;
    finish([
        t_true(child == this.child, "label changes preserve live child identities"),
        t_true(label == (), "disabling the label retires its component"),
        t_true(prim.attrs["lunco:billboard:text"] == "Updated",
            "disabling presentation retains authored text"),
        t_true(rejected.ok == false && rejected.error != ""
            && usd_document_generation(this.doc) == generation,
            "invalid label input rejects without publishing a generation"),
    ]);
}
