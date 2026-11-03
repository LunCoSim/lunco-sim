// @timing presentation
// Solver outputs on a mobility root have one producer, including before t=0.
const ROOT = "/World";

fn attr(path, name, type_name, value) {
    #{SetAttribute: #{edit_target: "@runtime@", path: path, name: name,
        type_name: type_name, value: value}}
}
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "CosimStatus", "GetReadiness", "BindingStatus", "ReadPorts", "QueryPhysicsState"],
      required_inputs: []}
}
fn on_start(me, ctx) {
    this.done = false; this.ticks = 0; this.program_requested = false;
    this.negative = ctx.negative == true;
    this.path = ROOT + if this.negative { "/InvalidInertia" } else { "/SolverVehicle" };
    let doc = ();
    for item in query("ListOpenDocuments").open_documents {
        if item.kind == "usd" && item.title == "replacement" { doc = item.doc_id; }
    }
    if doc == () { throw("vehicle output fixture document is missing"); }
    this.doc = doc;
    let ops = [
        #{AddPrim: #{edit_target: "@runtime@", parent_path: ROOT,
            name: if this.negative { "InvalidInertia" } else { "SolverVehicle" },
            type_name: "Cube", reference: (), reference_prim_path: ""}},
        #{SetApiSchemas: #{edit_target: "@runtime@", path: this.path,
            schemas: ["PhysicsRigidBodyAPI", "PhysicsMassAPI", "PhysxVehicleContextAPI"]}},
        attr(this.path, "physics:mass", "float", "10"),
        attr(this.path, "physics:diagonalInertia", "float3", "(1, 1, 1)"),
        attr(this.path, "outputs:sample", "double", "0")
    ];
    let result = cmd("ApplyUsdOps", #{doc_id: doc, ops: ops});
    if result.ok != true { throw(result.to_string()); }
}
fn install_program(me) {
    let ops = [
        #{SetApiSchemas: #{edit_target: "@runtime@", path: me.path,
            schemas: ["PhysicsRigidBodyAPI", "PhysicsMassAPI", "PhysxVehicleContextAPI", "LunCoProgramAPI"]}},
        attr(me.path, "info:implementationSource", "token", "\"sourceAsset\""),
        attr(me.path, "info:sourceAsset", "asset", "@lunco://models/tests/InitialExpressionParameter.mo@"),
        attr(me.path, "lunco:program:realtimeSafe", "bool", "true"),
        attr(me.path, "inputs:k", "double", if me.negative { "0" } else { "6" })
    ];
    for axis in ["xx", "yy", "zz"] {
        ops.push(#{SetConnection: #{edit_target: "@runtime@", path: me.path,
            name: "inputs:inertia_" + axis, type_name: "double",
            sources: [me.path + ".outputs:sample"]}});
    }
    let result = cmd("ApplyUsdOps", #{doc_id: me.doc, ops: ops});
    if result.ok != true { throw(result.to_string()); }
}
fn finish(me, checks) {
    me.done = true;
    report_verdict(checks, if me.negative { "VEHICLE INVALID INERTIA" } else { "VEHICLE PROGRAM OUTPUTS" },
        "VEHICLE_PROGRAM_OUTPUTS");
    me
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if !this.program_requested {
        let native = find_path(this.path);
        if native >= 0 {
            for port in query("ReadPorts", #{api_id: native, port_names: ["sample"]}).ports {
                if port.direction == "out" && port.metadata.source == "runtime producer" {
                    install_program(this);
                    this.program_requested = true;
                    return;
                }
            }
        }
        if this.ticks >= 1800 { this = finish(this, ["native output fixture did not publish"]); }
        return;
    }
    let ready = query("GetReadiness");
    let participant = ();
    for row in query("CosimStatus", #{include_values: true}).entities {
        if row.name == this.path { participant = row; }
    }
    if participant == () || (!ready.faulted && (this.negative || participant.modelica_current_time <= 0.0)) {
        if this.ticks < 1800 { return; }
        this = finish(this, ["vehicle output admission timed out"]); return;
    }
    let target = find_path(this.path);
    let solver_source = false;
    for port in query("ReadPorts", #{api_id: target, port_names: ["sample"]}).ports {
        if port.direction == "out" && port.metadata.source == "Co-simulation participant" {
            solver_source = true;
        }
    }
    let body = query("QueryPhysicsState", #{id: target});
    if this.negative {
        this = finish(this, [
            t_true(ready.application_available && ready.faulted, "invalid inertia leaves the app available and holds simulation"),
            t_true(ready.fault.kind == "cosim-connection-contract-invalid", "zero inertia retains the physics contract fault"),
            t_true(ready.fault.detail.contains("value 0")
                && ready.fault.detail.contains(this.path + ".sample -> " + this.path + ".inertia_"),
                "the diagnostic names the invalid sample and both endpoints"),
            t_true(solver_source, "the invalid sample is owned by the solver"),
            t_true(body.inertia_principal_kgm2 == [1.0, 1.0, 1.0], "rejected initial inertia never mutates the body")
        ]);
        return;
    }
    let edges = 0; let exact_sources = true;
    for edge in query("BindingStatus").connections {
        if edge.sink.usd_path == this.path && edge.sink_port.starts_with("inertia_") {
            edges += 1;
            exact_sources = exact_sources && edge.source.usd_path == this.path
                && edge.source_port == "sample" && edge.bound;
        }
    }
    let inertia_ok = body.inertia_principal_kgm2 != () && body.inertia_principal_kgm2.len() == 3;
    if inertia_ok {
        for value in body.inertia_principal_kgm2 { inertia_ok = inertia_ok && abs(value - 6.0) < 1.0e-5; }
    }
    this = finish(this, [
        t_true(ready.ready && !ready.faulted, "valid initial inertia admits simulation"),
        t_true(participant.outputs.sample == 6.0, "the checked Modelica observation owns the sample"),
        t_true(solver_source, "the selected source is the solver output"),
        t_true(edges == 3 && exact_sources, "all inertia wires retain the declared source identity"),
        t_true(inertia_ok, "physics receives the checked initial inertia"),
        t_true(participant.modelica_current_time > 0.0, "the admitted participant advances")
    ]);
}
