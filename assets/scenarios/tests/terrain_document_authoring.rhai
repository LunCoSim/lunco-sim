// Production terrain authoring uses the current native document composition.
// The fixture is authored through typed USD operations in the owned test session.
const TEST_KIND = "headless";
const ROOT = "/TerrainRocksTest";
const TERRAIN = ROOT + "/Terrain";
const SOURCE = ROOT + "/AuthoringSource";
const SCENE_FILE = "terrain_document_authoring.usdc";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim", "ResolveUsdTarget", "SyncUsdDocument"],
      required_inputs: []}
}

fn issue(command, params) {
    let result = cmd(command, params);
    if result == () || result.id == () {
        throw(command + " was not admitted: " + result.to_string());
    }
    result.id
}

fn applied(id) {
    let result = command_result(id);
    if result.status == "pending" { return false; }
    if result.status != "applied" || result.ok != true {
        throw("terrain authoring command failed: " + result.to_string());
    }
    true
}

fn add(parent, name) {
    #{AddPrim: #{edit_target: "@root@", parent_path: parent, name: name,
        type_name: "Xform", reference: (), reference_prim_path: ()}}
}

fn attr(path, name, type_name, value) {
    #{SetAttribute: #{edit_target: "@root@", path: path, name: name,
        type_name: type_name, value: value}}
}

fn exists(doc, path) {
    let target = query("ResolveUsdTarget", #{doc_id: doc, path: path, edit_target: "@runtime@"});
    if target == () || target.ok == false { throw("native target query failed: " + target.to_string()); }
    target.status == "resolved"
}

fn on_start(me, ctx) {
    this.doc = ();
    this.phase = 0;
    this.command_id = ();
    this.wait_until = 0;
    this.started_tick = sim_tick();
    this.checks = [];
}

fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started_tick > 1200 {
        let status = if this.command_id == () { "no command" } else { command_result(this.command_id).to_string() };
        throw("terrain document authoring timed out in phase " + this.phase
            + " for doc " + this.doc + "; command " + this.command_id
            + ": " + status);
    }
    if this.doc == () {
        for document in query("ListOpenDocuments", #{}).open_documents {
            if document.origin != () && document.origin.path.ends_with(SCENE_FILE) {
                this.doc = document.doc_id;
            }
        }
        if this.doc == () { return; }
    }
    if this.command_id != () && this.phase != 6 && this.phase != 8 {
        if !applied(this.command_id) { return; }
        this.command_id = ();
        this.wait_until = sim_tick() + 10;
    }
    if sim_tick() < this.wait_until { return; }

    if this.phase == 0 {
        let operations = [add(ROOT, "AuthoringSource"), add(SOURCE, "edit_90"),
            add(SOURCE, "rock_91"), add(SOURCE, "Craters"), add(SOURCE, "RemoteRocks"),
            #{SetActive: #{edit_target: "@root@", path: SOURCE + "/Craters", active: false}},
            attr(SOURCE + "/RemoteRocks", "lunco:layer", "token", "\"rocks\""),
            attr(SOURCE + "/RemoteRocks", "lunco:layer:density", "float", "0"),
            #{RemovePrim: #{edit_target: "@root@", path: TERRAIN + "/Rocks"}},
            #{SetReferenceArcs: #{edit_target: "@root@", path: TERRAIN,
                references: [#{asset_path: "", prim_path: SOURCE}], list_op: "Prepend"}}];
        this.command_id = issue("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (),
            label: "Author native terrain reference fixture", ops: operations});
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        this.checks.push(t_true(exists(this.doc, TERRAIN + "/rock_91")
            && exists(this.doc, TERRAIN + "/RemoteRocks"),
            "native internal reference contributes terrain children"));
        this.command_id = issue("PlaceRock", #{x: 0.0, z: 0.0, size: 1.0, seed: 42});
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        let rock = query("QueryUsdPrim", #{doc_id: this.doc, path: TERRAIN + "/rock_92",
            attrs: ["lunco:layer", "lunco:layer:x", "lunco:layer:z", "lunco:layer:size", "lunco:layer:seed"]});
        this.checks.push(t_true(rock.ok != false && rock.attrs["lunco:layer"] == "rock"
            && rock.attrs["lunco:layer:size"] == 1.0 && rock.attrs["lunco:layer:seed"] == 42,
            "one complete rock group reserves referenced child names"));
        this.command_id = issue("BrushTerrain", #{x: 0.0, z: 0.0, radius: 2.0, amplitude: 0.1, id: ""});
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        this.checks.push(t_true(exists(this.doc, TERRAIN + "/edit_93"),
            "brush naming reads the current runtime document overlay"));
        this.command_id = issue("UpdateObstacleFieldSpec", #{spec: #{seed: 42,
            region_half_extent: 5.0, grid_resolution: 17, pattern: "Uniform",
            craters: #{enabled: true, density: 0.0, size: #{min: 1.0, max: 2.0, mode: 1.5, sigma: 0.0},
                depth_ratio: 0.4, rim_height_ratio: 0.18},
            rocks: #{enabled: true, density: 0.0, size: #{min: 0.5, max: 2.0, mode: 1.0, sigma: 0.0},
                dynamic_fraction: 0.0}}});
        this.phase = 4;
        return;
    }
    if this.phase == 4 {
        let remote = query("QueryUsdPrim", #{doc_id: this.doc, path: TERRAIN + "/RemoteRocks",
            attrs: ["lunco:layer:density", "lunco:layer:sizeMode"]});
        this.checks.push(t_true(exists(this.doc, TERRAIN + "/Craters_1")
            && remote.ok != false && remote.attrs["lunco:layer:sizeMode"] == 1.0,
            "obstacle authoring reserves inactive referenced names and edits the referenced layer"));
        this.command_id = issue("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (),
            label: "Author exhausted terrain name boundary", ops: [add(SOURCE, "edit_18446744073709551615")]});
        this.phase = 5;
        return;
    }
    if this.phase == 5 {
        this.before = usd_document_generation(this.doc);
        this.command_id = issue("PlaceRock", #{x: 0.0, z: 0.0, size: 1.0, seed: 42});
        this.phase = 6;
        return;
    }
    if this.phase == 6 {
        let result = command_result(this.command_id);
        if result.status == "pending" { return; }
        let after = usd_document_generation(this.doc);
        this.checks.push(t_true(result.status == "rejected" && result.ok == false
            && result.error.contains("counter exhausted") && this.before == after,
            "exhausted authored names reject without a partial edit or crash"));
        this.before = after;
        this.command_id = issue("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (),
            label: "Reject an internal reference without its target", ops: [
                #{AddPrim: #{edit_target: "@runtime@", parent_path: ROOT, name: "InvalidInternalReference",
                    type_name: "Xform", reference: "", reference_prim_path: ()}}]});
        this.phase = 8;
        return;
    }
    if this.phase == 8 {
        let result = command_result(this.command_id);
        if result.status == "pending" { return; }
        this.checks.push(t_true(result.status == "rejected" && result.ok == false
            && result.error.contains("internal reference requires an explicit prim target")
            && usd_document_generation(this.doc) == this.before
            && !exists(this.doc, ROOT + "/InvalidInternalReference"),
            "an ambiguous internal reference rejects atomically"));
        this.phase = 99;
        report_verdict(this.checks, "TERRAIN DOCUMENT AUTHORING", "TERRAIN_DOCUMENT_AUTHORING");
    }
}
