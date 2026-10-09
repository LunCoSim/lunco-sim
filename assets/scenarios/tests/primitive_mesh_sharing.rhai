// TEST-ONLY. Native mesh identity is reflected by the production visual owner.
// Equal authored headlight parts must share geometry; unlike dimensions must
// remain distinct even when the parts use the same primitive type.
const WAIT_S = 30.0;
const ROVERS = ["Ackermann_Physical_1", "Ackermann_Raycast_1", "Skid_Physical_1",
    "Skid_Raycast_1", "Skid_Raycast_2", "Skid_Battery_Thermal_1"];

fn simulation_dependencies(me, ctx) {
    let reads = [];
    for rover in ROVERS {
        for side in ["L", "R"] {
            for part in ["Lens", "Casing"] {
                reads.push(find_path("/SandboxScene/" + rover + "/Headlight_" + side + "/" + part));
            }
        }
    }
    #{ modelica_entities: [], entity_reads: reads, entity_writes: [],
       query_reads: ["QueryUsdPrim"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    let rows = [];
    for rover in ROVERS {
        let base = "/SandboxScene/" + rover + "/Headlight_";
        let row = [];
        for path in [base + "L/Lens", base + "R/Lens", base + "L/Casing", base + "R/Casing"] {
            let entity = find_path(path);
            let mesh = if entity == () { () } else { get(entity, "Mesh3d") };
            if mesh == () && this.elapsed < WAIT_S { return; }
            row.push(#{ path: path, entity: entity, mesh: mesh });
        }
        rows.push(row);
    }
    let failures = [];
    for row in rows {
        failures.push(t_true(row[0].mesh != () && row[0].mesh == row[1].mesh,
            row[0].path + " equal lenses did not share a native mesh"));
        failures.push(t_true(row[2].mesh != () && row[2].mesh == row[3].mesh,
            row[2].path + " equal casings did not share a native mesh"));
        let lens = query("QueryUsdPrim", #{ path: row[0].path, attrs: ["radius", "height"] });
        let casing = query("QueryUsdPrim", #{ path: row[2].path, attrs: ["radius", "height"] });
        failures.push(t_true(lens != () && lens.ok != false && casing != () && casing.ok != false
            && lens.attrs != casing.attrs && row[0].mesh != () && row[2].mesh != ()
            && row[0].mesh != row[2].mesh,
            row[0].path + " unlike authored dimensions were aliased"));
        for part in row {
            failures.push(t_true(part.entity != () && get(part.entity, "SharedMeshAsset") != (),
                part.path + " shared mesh ownership is missing"));
        }
    }
    report_verdict(failures, "PRIMITIVE MESH SHARING", "PRIMITIVE_MESH_SHARING");
    this.done = true;
}
