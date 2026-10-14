// Attach to the settled replacement fixture through RunScenarioAsset.
const ROOT = "/World/NativeLightSchemaDefaults";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "InspectUsdDocument"], required_inputs: []}
}
fn add(parent, name, kind) {
    #{AddPrim: #{edit_target: "@root@", parent_path: parent, name: name,
        type_name: kind, reference: (), reference_prim_path: ()}}
}
fn attr(path, name, kind, value) {
    #{SetAttribute: #{edit_target: "@root@", path: path, name: name,
        type_name: kind, value: value}}
}
fn on_start(me, ctx) {
    this.doc = ();
    this.pending = ();
    this.phase = 0;
    this.wait_until = 0;
    this.started = sim_tick();
    this.checks = [];
}
fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started > 600 { throw("native light defaults timed out"); }
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
        if this.phase == 1 {
            if result.ok != true || result.status != "applied" { throw("light setup failed: " + result.to_string()); }
            this.wait_until = sim_tick() + 30;
        } else {
            this.checks.push(t_true(result.ok == false,
                "malformed light input produces a terminal command rejection"));
            this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.generation,
                "rejected light input does not mutate authored state"));
        }
        this.pending = ();
    }
    if sim_tick() < this.wait_until { return; }
    if this.phase == 0 {
        let queued = cmd("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (), label: "Native light schema defaults", ops: [
            add("/World", "NativeLightSchemaDefaults", "Xform"),
            add(ROOT, "Sun", "DistantLight"),
            attr(ROOT + "/Sun", "inputs:normalize", "bool", "true"),
            add(ROOT, "Bulb", "SphereLight"),
            add(ROOT, "Panel", "RectLight"),
            add(ROOT, "Authored", "SphereLight"),
            attr(ROOT + "/Authored", "inputs:intensity", "float", "23"),
            attr(ROOT + "/Authored", "inputs:exposure", "float", "2"),
            attr(ROOT + "/Authored", "inputs:radius", "float", "2"),
            attr(ROOT + "/Authored", "inputs:normalize", "bool", "true"),
            attr(ROOT + "/Authored", "inputs:color", "color3f", "(0.25, 0.5, 0.75)"),
            add(ROOT, "SizedPanel", "RectLight"),
            attr(ROOT + "/SizedPanel", "inputs:width", "float", "2"),
            attr(ROOT + "/SizedPanel", "inputs:height", "float", "3"),
            add(ROOT, "Warm", "SphereLight"),
            attr(ROOT + "/Warm", "inputs:enableColorTemperature", "bool", "true"),
            attr(ROOT + "/Warm", "inputs:colorTemperature", "float", "2000")
        ]});
        if queued == () || queued.id == () { throw("light setup was not admitted"); }
        this.pending = queued.id;
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        let sun = find(ROOT + "/Sun");
        let bulb = find(ROOT + "/Bulb");
        let panel = find(ROOT + "/Panel");
        let authored = find(ROOT + "/Authored");
        let sized = find(ROOT + "/SizedPanel");
        let warm = find(ROOT + "/Warm");
        if sun < 0 || bulb < 0 || panel < 0 || authored < 0 || sized < 0 || warm < 0 { return; }
        for id in [sun, bulb, panel, authored, sized, warm] { track_entity_read(id); }
        this.checks.push(t_true(get(sun, "DirectionalLight.illuminance") == 50000.0,
            "omitted normalized DistantLight intensity resolves from the native schema"));
        this.checks.push(t_true(get(bulb, "PointLight.intensity") == 1.0,
            "omitted SphereLight intensity resolves from the native schema"));
        this.checks.push(t_true(get(panel, "RectLight.intensity") == 1.0,
            "omitted RectLight intensity resolves from the native schema"));
        this.checks.push(t_true(get(authored, "PointLight.intensity") == 92.0,
            "authored intensity and exposure remain authoritative"));
        this.checks.push(t_true(get(bulb, "PointLight.radius") == 0.5,
            "omitted SphereLight radius resolves from the native schema"));
        this.checks.push(t_true(get(panel, "RectLight.width") == 1.0 && get(panel, "RectLight.height") == 1.0,
            "omitted RectLight dimensions resolve from the native schema"));
        this.checks.push(t_true(get(authored, "PointLight.radius") == 2.0,
            "authored radius and normalization preserve emitted power"));
        this.checks.push(t_true(get(sized, "RectLight.intensity") == 6.0,
            "native omitted normalization preserves rectangular area scaling"));
        let white = get(bulb, "PointLight.color.0");
        let tinted = get(authored, "PointLight.color.0");
        this.checks.push(t_true(white != () && tinted != () &&
            white.red == 1.0 && white.green == 1.0 && white.blue == 1.0 &&
            tinted.red == 0.25 && tinted.green == 0.5 && tinted.blue == 0.75,
            "native omitted and authored light colour reach the renderer"));
        let temperature = get(warm, "PointLight.color.0");
        this.checks.push(t_true(temperature != () && temperature.red == 1.0 && temperature.blue < 0.1,
            "native enabled temperature colours the light"));
        this.generation = assembly_edit::describe(this.doc).generation;
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: this.generation,
            op: attr(ROOT + "/Authored", "inputs:intensity", "float", "invalid-light-value")});
        if queued == () || queued.id == () { throw("negative light input was not admitted"); }
        this.pending = queued.id;
        this.phase = 2;
        return;
    }
    report_verdict(this.checks, "NATIVE LIGHT DEFAULTS", "NATIVE_LIGHT_DEFAULTS");
    this.phase = 99;
}
