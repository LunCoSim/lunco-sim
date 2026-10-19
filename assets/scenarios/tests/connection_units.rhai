// @timing presentation
// The document owns native metre and kilogram endpoints. This observer changes
// only their authored connection and checks the owning admission boundary.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetPort", "GetReadiness", "GetBrokenConnections"], required_inputs: []}
}
fn require(result) { if result.ok != true { throw(result.to_string()); } }
fn on_start(me, ctx) {
    this.phase = 0; this.done = false; this.checks = []; this.doc = ();
    for document in query("ListOpenDocuments").open_documents {
        if document.kind == "usd" && document.title == "connection_units" {
            this.doc = document.doc_id;
        }
    }
    if this.doc == () { throw("the connection fixture document is missing"); }
}
fn on_tick(me, ctx) {
    if this.done { return; }
    if this.phase == 0 {
        let source = find_path("/World/Source"); let target = find_path("/World/Target");
        if source < 0 || target < 0 { return; }
        let y = query("GetPort", #{api_id: target, name: "position_y"});
        if y.ok == false || y.value != 2.0 { return; }
        this.checks.push(t_true(y.value == 2.0, "compatible native length ports exchange their initial physical sample"));
        require(cmd("StepSimulation", #{hold: true}));
        let operation = #{SetConnection: #{edit_target: "@runtime@", path: "/World/Target",
            name: "inputs:mass", type_name: "double", sources: ["/World/Source.outputs:position_y"]}};
        require(cmd("ApplyUsdOps", #{doc_id: this.doc, ops: [operation]}));
        this.phase = 1; return;
    }
    let ready = query("GetReadiness");
    if !ready.faulted { return; }
    let mass = query("GetPort", #{api_id: find_path("/World/Target"), name: "mass"});
    let wiring = query("GetBrokenConnections");
    this.checks += [
        t_true(ready.fault.kind == "cosim-connection-contract-invalid", "incompatible units fail at the connection owner"),
        t_true(ready.fault.detail.contains("unit conversion"), "the owner identifies the incompatible unit contract"),
        t_true(mass.value == 5.0, "rejected metre-to-kilogram wiring preserves the native mass"),
        t_true(wiring.runtime_fault.kind == ready.fault.kind, "the public wiring report exposes the same structured failure")
    ];
    this.done = true;
    report_verdict(this.checks, "CONNECTION UNITS", "CONNECTION_UNITS");
}
