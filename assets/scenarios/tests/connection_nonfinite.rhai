// @timing presentation
// Finite producer samples can overflow during fan-in. The simulation retains
// native doubles; only the public diagnostic classifies a rejected IEEE value.
fn nonfinite_require(result) {
    if result.ok != true { throw(result.to_string()); }
}
fn nonfinite_input(name, value) {
    // These USD ports must represent the finite f64 range under test.
    #{name: name, type_name: "double", default_value: value, connection: ()}
}
fn nonfinite_output(name) {
    #{name: name, type_name: "double", connections: []}
}
fn nonfinite_prepare(doc, direction) {
    for name in ["LargeA", "LargeB"] {
        nonfinite_require(assembly_edit::attach_program(doc, #{
            edit_target: "@runtime@", host_path: "/World", name: name,
            source_asset: "lunco://models/tests/ConnectionWriteTransaction.mo",
            realtime_safe: false,
            inputs: [nonfinite_input("initialMass", 0.0),
                     nonfinite_input("massRate", -direction * 1.0e308)],
            outputs: [nonfinite_output("mass"), nonfinite_output("comX"),
                      nonfinite_output("sampleTime")]}));
    }
    nonfinite_require(assembly_edit::attach_program(doc, #{
        edit_target: "@runtime@", host_path: "/World", name: "Sink",
        source_asset: "lunco://models/tests/InitialFeedback.mo", realtime_safe: false,
        inputs: [nonfinite_input("feedback", 0.0), nonfinite_input("clockFeedback", 0.0),
                 nonfinite_input("gain", 1.0), nonfinite_input("bias", 0.0)],
        outputs: [nonfinite_output("sample"), nonfinite_output("solverTime"),
                  nonfinite_output("clockSeen")]}));
    nonfinite_require(cmd("ApplyUsdOps", #{doc_id: doc, ops: [#{SetConnection: #{
        edit_target: "@runtime@", path: "/World/Sink", name: "inputs:feedback",
        type_name: "double", sources: ["/World/LargeA.outputs:mass", "/World/LargeB.outputs:mass"]}}]}));
    print("NONFINITE_PREPARED");
}
fn nonfinite_port(path, name) {
    query("GetPort", #{api_id: find_path(path), name: name}).value
}
fn nonfinite_snapshot() {
    #{clock: clock_snapshot(), native_steps: query("PhysicsPerformance").step_number,
      source_time: nonfinite_port("/World/LargeA", "sampleTime"),
      feedback: nonfinite_port("/World/Sink", "feedback")}
}
fn nonfinite_verify(direction, before) {
    let ready = query("GetReadiness");
    let wiring = query("GetBrokenConnections");
    let current = nonfinite_snapshot();
    let a = nonfinite_port("/World/LargeA", "mass");
    let b = nonfinite_port("/World/LargeB", "mass");
    let rejected = ();
    for broken in wiring.broken {
        if broken.port == "feedback" { rejected = broken.dropped_value; }
    }
    let classified = type_of(rejected) == "map";
    let kind = if classified { rejected.kind } else { "unclassified" };
    // Communication samples use the integer master's quantum. Native Duration
    // diagnostics report their nanosecond rounding, not this sampling lattice.
    let expected = 2.0 * (direction * 1.0e308
        * (current.source_time - current.clock.simulation_dt_s));
    print("CONNECTION_NONFINITE_STATE " + #{source_a: a, source_b: b,
        source_time: current.source_time, simulation_dt_s: current.clock.simulation_dt_s,
        feedback: current.feedback, expected: expected});
    let failures = [
        t_true(ready.faulted && ready.fault.kind == "cosim-input-write-rejected",
            "overflow fails at the live input owner"),
        t_true(abs(a) < 1.0e308 && abs(b) < 1.0e308 && a == b,
            "both admitted producer samples remain finite"),
        t_true(abs(current.feedback) < 1.7976931348623157e308,
            "the rejected sum never replaces the finite input"),
        t_true(abs((current.feedback - expected) / expected) < 1.0e-12,
            "the target retains the last accepted communication sample"),
        t_true(classified && kind == (if direction > 0.0 { "positive_infinity" } else { "negative_infinity" }),
            "the diagnostic classifies the rejected infinity"),
        t_true(classified && rejected.value == (),
            "a non-finite diagnostic has no fabricated finite payload"),
        t_true(current.clock.sim_tick == before.clock.sim_tick
            && current.native_steps == before.native_steps,
            "master and native physics remain stopped after rejection"),
        t_true(current.source_time == before.source_time && current.feedback == before.feedback,
            "Modelica and port state remain stopped after rejection")
    ];
    report_verdict(failures, "CONNECTION NONFINITE", "CONNECTION_NONFINITE");
}
fn on_start(me, ctx) {
    if ctx.direction == () || abs(ctx.direction) != 1.0 {
        throw "the overflow fixture requires direction +1 or -1";
    }
    this.direction = ctx.direction;
    this.before = nonfinite_snapshot();
    this.frames = 0;
    this.done = false;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.frames += 1;
    if this.frames < 3 { return; }
    this.done = true;
    nonfinite_verify(this.direction, this.before);
}
