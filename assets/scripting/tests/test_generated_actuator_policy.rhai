// Contract tests for the shipped actuator-wrench policy.
//
// Geometry and actuator membership are Rust facts. The source shape, solver
// ownership, and presentation contract are policy decisions and are checked
// in Rhai so they can evolve without a Rust rebuild.

fn require(condition, message) {
    if !condition { throw message; }
}

fn test_generated_actuator_policy(net) {
    let result = synthesize(net);
    let source = result.source;

    require(source.contains("model " + net.model_name), "actuator root model is missing");
    require(source.contains("LunCo.Actuation.WrenchAllocator"),
        "the shared allocator class is missing");
    require(source.contains("wrench_matrix = ["), "wrench matrix is missing");
    require(source.contains("allocation_step = " + net.allocation_step.to_string()),
        "allocation step is missing");
    require(source.contains("Force allocation | " + net.actuator_count.to_string()),
        "actuator diagram title is missing");
    require(!source.contains("RcsValveAllocator"), "legacy allocator leaked into policy output");
    require(!source.contains("\\n"), "literal escaped newlines leaked into Modelica");

    require(result.source_roots.contains("LunCo"), "policy did not report its source root");
    require(source.contains(net.outputs[0] + " = allocator.command[1];"),
        "actuator command output is not wired");
    true
}
