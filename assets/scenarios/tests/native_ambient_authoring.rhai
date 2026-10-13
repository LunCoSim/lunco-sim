// Attach to the settled replacement fixture through RunScenarioAsset.
// Typed document operations establish the authored light contract.
const ROOT = "/World";
const SOURCE = ROOT + "/AmbientSource";
const FILL = ROOT + "/Environment/AmbientFill";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim"], required_inputs: []}
}

fn add(parent, name, kind) {
    #{AddPrim: #{edit_target: "@root@", parent_path: parent, name: name,
        type_name: kind, reference: (), reference_prim_path: ()}}
}

fn attr(path, name, value) {
    #{SetAttribute: #{edit_target: "@root@", path: path, name: name,
        type_name: "float", value: value}}
}

fn edit(doc, label, ops) {
    let result = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: label, ops: ops});
    if result == () || result.id == () { throw("light fixture edit was not admitted"); }
    result.id
}

fn fill_intensity(doc) {
    let prim = query("QueryUsdPrim", #{doc_id: doc, path: FILL, attrs: ["inputs:intensity"]});
    if prim == () || prim.ok == false { throw("native fill query failed: " + prim.to_string()); }
    prim.attrs["inputs:intensity"]
}

fn on_start(me, ctx) {
    this.phase = 0;
    this.doc = ();
    this.pending = ();
    this.wait_until = 0;
    this.start_tick = sim_tick();
    this.checks = [];
}

fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.start_tick > 900 { throw("native ambient authoring timed out in phase " + this.phase); }
    if this.doc == () {
        let docs = query("ListOpenDocuments", #{});
        for document in docs.open_documents {
            if document.active && document.kind == "usd" { this.doc = document.doc_id; }
        }
        if this.doc == () { return; }
    }
    if this.pending != () {
        let result = command_result(this.pending);
        if result.status == "pending" { return; }
        if result.status != "applied" || result.ok != true { throw("fixture edit failed: " + result.to_string()); }
        this.pending = ();
        this.wait_until = sim_tick() + 30;
    }
    if sim_tick() < this.wait_until { return; }
    if this.phase == 0 {
        this.pending = edit(this.doc, "Author native ambient composition", [
            add(ROOT, "AmbientSource", "Xform"),
            add(SOURCE, "SchemaDefault", "DomeLight"),
            add(SOURCE, "Authored", "DomeLight"),
            attr(SOURCE + "/Authored", "inputs:intensity", "4"),
            attr(SOURCE + "/Authored", "inputs:exposure", "1"),
            add(SOURCE, "Inactive", "DomeLight"),
            attr(SOURCE + "/Inactive", "inputs:intensity", "999"),
            #{SetActive: #{edit_target: "@root@", path: SOURCE + "/Inactive", active: false}},
            #{AddPrim: #{edit_target: "@root@", parent_path: ROOT, name: "ReferencedAmbient",
                type_name: "Xform", reference: "", reference_prim_path: SOURCE}}
        ]);
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        cmd("SetEnvironmentLight", #{ambient_brightness: 32.0});
        this.wait_until = sim_tick() + 30;
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        this.checks.push(t_true(fill_intensity(this.doc) == 14.0,
            "native references, schema defaults, exposure and inactive filtering contribute eighteen"));
        cmd("SetEnvironmentLight", #{ambient_brightness: 32.0});
        this.wait_until = sim_tick() + 30;
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        this.checks.push(t_true(fill_intensity(this.doc) == 14.0,
            "repeated authoring excludes the current native runtime fill"));
        cmd("SetEnvironmentLight", #{ambient_brightness: 10.0});
        this.wait_until = sim_tick() + 30;
        this.phase = 4;
        return;
    }
    if this.phase == 4 {
        this.checks.push(t_true(fill_intensity(this.doc) == 0.0,
            "an unattainable total authors a nonnegative fill"));
        this.pending = edit(this.doc, "Author unresolved ambient input", [
            attr(SOURCE + "/Authored", "outputs:drive", "4"),
            #{SetConnection: #{edit_target: "@root@", path: SOURCE + "/Authored",
                name: "inputs:intensity", type_name: "float",
                sources: [SOURCE + "/Authored.outputs:drive"]}}
        ]);
        this.phase = 5;
        return;
    }
    if this.phase == 5 {
        this.before = usd_document_generation(this.doc);
        cmd("SetEnvironmentLight", #{ambient_brightness: 40.0, illuminance: 7.0});
        this.wait_until = sim_tick() + 30;
        this.phase = 6;
        return;
    }
    if this.phase == 6 {
        this.checks.push(t_true(usd_document_generation(this.doc) == this.before
            && fill_intensity(this.doc) == 0.0,
            "a connected ambient input rejects every authored lighting edit without a partial document mutation"));
        this.phase = 99;
        report_verdict(this.checks, "NATIVE AMBIENT AUTHORING", "NATIVE_AMBIENT_AUTHORING");
    }
}
