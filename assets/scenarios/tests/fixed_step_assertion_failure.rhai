// @timing presentation
// A fixed-step participant must retain its original parameter assertion failure.
import "lunco://scripting/tools/assembly_edit" as assembly;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "CosimStatus", "GetReadiness"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.ticks = 0; this.done = false;
    let doc = ();
    for item in query("ListOpenDocuments").open_documents {
        if item.kind == "usd" && item.title == "replacement" { doc = item.doc_id; }
    }
    if doc == () { throw("fixed-step assertion fixture document is missing"); }
    let result = assembly::attach_program(doc, #{edit_target: "@runtime@",
        host_path: "/World", name: "Rejected",
        source_asset: "lunco://models/LunCo/Mobility/RoverAutopilotGuidance.mo",
        inputs: [#{name: "drive_heading_tolerance", type_name: "double",
            default_value: -0.1, connection: ()}],
        outputs: [#{name: "throttle_cmd", type_name: "double", connections: []}],
        realtime_safe: true});
    if result.ok != true { throw(result.to_string()); }
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    let ready = query("GetReadiness");
    let rejected = ();
    for row in query("CosimStatus", #{include_values: false}).entities {
        if row.name == "/World/Rejected" { rejected = row; }
    }
    if rejected == () || !ready.faulted {
        if this.ticks < 1800 { return; }
        this.done = true;
        report_verdict(["invalid fixed-step parameter did not reach its terminal diagnostic"],
            "FIXED STEP ASSERTION FAILURE", "FIXED_STEP_ASSERTION_FAILURE");
        return;
    }
    this.done = true;
    let checks = [
        t_true(ready.application_available, "the failed model leaves the application available"),
        t_true(ready.fault.kind == "modelica-compile-failed", "startup failure has its typed runtime fault"),
        t_true(ready.fault.detail.contains("Rover guidance heading window must be in"),
            "the original parameter assertion explains the failure: " + ready.fault.detail),
        t_true(rejected.status == "Error" && rejected.modelica_error_present,
            "the participant retains its error status"),
        t_true(rejected.modelica_paused, "a failed participant remains stopped"),
        t_true(rejected.modelica_current_time == 0.0, "invalid initialization never advances model time")
    ];
    emit("FIXED_STEP_ASSERTION_FAILURE_EVIDENCE", #{fault: ready.fault, participant: rejected.name});
    report_verdict(checks, "FIXED STEP ASSERTION FAILURE", "FIXED_STEP_ASSERTION_FAILURE");
}
