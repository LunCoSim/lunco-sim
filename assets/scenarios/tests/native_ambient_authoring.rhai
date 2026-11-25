// Attach to the settled replacement fixture through RunScenarioAsset.
// Typed document operations establish the authored light contract.
const ROOT = "/World";
const SOURCE = ROOT + "/AmbientSource";
const FILL = ROOT + "/Environment/AmbientFill";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["QueryUsdPrim"], required_inputs: []}
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

fn light(params) {
    let result = cmd("SetEnvironmentLight", params);
    if result == () || result.id == () { throw("lighting command has no result: " + result.to_string()); }
    result.id
}

fn on_event(me, evt, ctx) {
    if evt.name == "usd.document.projected" && evt.value.doc_id == this.doc {
        this.projected = evt.value.generation;
    }
}

fn on_start(me, ctx) {
    this.phase = 0;
    this.doc = ();
    this.pending = ();
    this.projected = ();
    this.expected = "applied";
    this.membership = 0;
    this.cases = [
        #{label:"Define an active DomeLight", ops:[add(ROOT,"Switchable","DomeLight"),
            attr(ROOT+"/Switchable","inputs:intensity","5")], fill:9.0},
        #{label:"Retype the DomeLight as an Xform", ops:[add(ROOT,"Switchable","Xform")], fill:14.0},
        #{label:"Retype the Xform as a DomeLight", ops:[add(ROOT,"Switchable","DomeLight")], fill:9.0},
        #{label:"Disable the active DomeLight", ops:[#{SetActive:#{edit_target:"@root@",
            path:ROOT+"/Switchable", active:false}}], fill:14.0}
    ];
    this.start_tick = sim_tick();
    this.checks = [];
}

fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.start_tick > 900 { throw("native ambient authoring timed out in phase " + this.phase); }
    if this.doc == () {
        let root = query("QueryUsdPrim", #{path:ROOT});
        if root == () || root.ok == false || root.doc_id == () { return; }
        this.doc = root.doc_id;
        this.projected = usd_document_generation(this.doc);
    }
    if this.pending != () {
        let result = command_result(this.pending);
        if result.status == "pending" { return; }
        if result.status != this.expected || result.ok != (this.expected == "applied") {
            throw("unexpected command result in phase " + this.phase + ": " + result.to_string());
        }
        this.expected = "applied";
        this.pending = ();
    }
    if this.projected != usd_document_generation(this.doc) { return; }
    if this.phase == 0 {
        this.pending = edit(this.doc, "Author native ambient composition", [
            add("/", "OtherScene", "Xform"),
            add("/OtherScene", "ForeignDome", "DomeLight"),
            attr("/OtherScene/ForeignDome", "inputs:intensity", "999"),
            add("/OtherScene", "Environment", "LunCoEnvironment"),
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
        this.pending = light(#{intent:"Persistent", ambient_brightness: 32.0});
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        this.checks.push(t_true(fill_intensity(this.doc) == 14.0,
            "only active DomeLights under the selected mount contribute, including references, exposure and defaults"));
        this.before = usd_document_generation(this.doc);
        this.pending = light(#{ambient_brightness: 34.0});
        this.phase = 7;
        return;
    }
    if this.phase == 7 {
        this.checks.push(t_true(usd_document_generation(this.doc) == this.before && fill_intensity(this.doc) == 14.0,
            "default environment lighting changes live state without authoring USD"));
        this.pending = light(#{intent:"Persistent", ambient_brightness: 32.0});
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        this.checks.push(t_true(fill_intensity(this.doc) == 14.0,
            "repeated authoring excludes the current native runtime fill"));
        this.before = usd_document_generation(this.doc);
        this.expected = "rejected";
        this.pending = light(#{intent:"Persistent", ambient_brightness: 10.0});
        this.phase = 4;
        return;
    }
    if this.phase == 4 {
        this.checks.push(t_true(usd_document_generation(this.doc) == this.before
            && fill_intensity(this.doc) == 14.0,
            "an unattainable total is rejected and preserves the previous fill"));
        this.phase = 8;
        return;
    }
    if this.phase == 8 && this.membership < this.cases.len() {
        let step = this.cases[this.membership];
        this.pending = edit(this.doc, step.label, step.ops);
        this.phase = 9;
        return;
    }
    if this.phase == 9 {
        this.pending = light(#{intent:"Persistent", ambient_brightness:32.0});
        this.phase = 10;
        return;
    }
    if this.phase == 10 {
        let step = this.cases[this.membership];
        this.checks.push(t_true(fill_intensity(this.doc) == step.fill, step.label));
        this.membership += 1;
        this.phase = 8;
        return;
    }
    if this.phase == 8 {
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
        this.expected = "rejected";
        this.pending = light(#{intent:"Persistent", ambient_brightness: 40.0, illuminance: 7.0});
        this.phase = 6;
        return;
    }
    if this.phase == 6 {
        this.checks.push(t_true(usd_document_generation(this.doc) == this.before
            && fill_intensity(this.doc) == 14.0,
            "a connected ambient input rejects every authored lighting edit without a partial document mutation"));
        this.phase = 99;
        report_verdict(this.checks, "NATIVE AMBIENT AUTHORING", "NATIVE_AMBIENT_AUTHORING");
    }
}
