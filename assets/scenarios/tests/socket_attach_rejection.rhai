// Production negative coverage for the typed socket attachment contract.
const ROOT = "/SocketAttachRejection";
const HOST = ROOT + "/Chassis";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments", "QueryUsdPrim", "InspectUsdDocument"], required_inputs: [] }
}
fn on_start(me, ctx) {
    this.doc = (); this.request = (); this.index = 0;
    this.elapsed = 0.0; this.done = false; this.checks = [];
    this.cases = [
        #{socket: "occupied", name: "SecondProbe", joint: "constraint_second_probe", diagnostic: "already occupied"},
        #{socket: "incompatible", name: "IncompatibleProbe", joint: "constraint_incompatible_probe", diagnostic: "socket accepts"},
        #{socket: "wrong_joint", name: "WrongJointProbe", joint: "constraint_wrong_joint_probe", diagnostic: "requires joint"},
    ];
}
fn fail(me, message) {
    me.done = true;
    report_verdict([message], "SOCKET ATTACH REJECTION", "SOCKET_ATTACH_REJECTION");
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 10.0 { fail(this, "socket rejection did not reach its terminal result"); return; }
    if this.doc == () {
        for item in query("ListOpenDocuments").open_documents {
            if item.kind == "usd" {
                let root = query("QueryUsdPrim", #{doc_id: item.doc_id, path: ROOT});
                if root != () && root.ok != false { this.doc = item.doc_id; }
            }
        }
        if this.doc == () { return; }
        this.generation = query("InspectUsdDocument", #{doc_id: this.doc}).generation;
    }
    if this.index == this.cases.len() {
        this.done = true;
        report_verdict(this.checks, "SOCKET ATTACH REJECTION", "SOCKET_ATTACH_REJECTION");
        return;
    }
    let test = this.cases[this.index];
    if this.request == () {
        this.request = cmd("AttachComponent", #{doc_id: this.doc, spec: #{
            edit_target: "@runtime@", socket_path: HOST + "/Interfaces/" + test.socket,
            host_path: HOST, name: test.name, joint_name: test.joint,
            asset: "lunco://components/mounting/demo_probe.usda",
            placement: [0.0, 0.0, 0.0], rotate_deg: [0.0, 0.0, 0.0], joint: "Fixed",
        }});
        return;
    }
    let result = this.request;
    if result.status == "pending" { result = command_result(result.id); }
    if result == () || result.status == "pending" { return; }
    let rejected = result.ok == false && result.status == "rejected"
        && result.error != () && result.error.contains(test.diagnostic);
    if !rejected { fail(this, test.socket + " did not return its owning diagnostic: " + result.to_string()); return; }
    let absent = query("QueryUsdPrim", #{doc_id: this.doc, path: HOST + "/" + test.name});
    this.checks.push(t_true(rejected, test.socket + " returns a terminal socket-contract rejection"));
    this.checks.push(t_true(query("InspectUsdDocument", #{doc_id: this.doc}).generation == this.generation,
        test.socket + " preserves the document generation"));
    this.checks.push(t_true(absent != () && absent.ok == false && find_path(HOST + "/" + test.name) < 0,
        test.socket + " cannot create an authored or projected component"));
    this.index += 1;
    this.request = ();
}
