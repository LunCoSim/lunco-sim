// Contract tests for the shipped acausal-network policy.
//
// The Rust host supplies composed USD facts and only invokes this function.
// Policy decisions stay testable here, so changing the visual layout or the
// generated equations does not require a Rust test rewrite.

fn require(condition, message) {
    if !condition { throw message; }
}

fn test_generated_acausal_policy(net) {
    let result = synthesize(net);
    let source = result.source;

    require(source.contains("model " + net.model_name), "root model is missing");
    require(source.contains("connect("), "acausal topology is missing");
    require(source.contains("LunCo.Electrical.Battery"), "battery class is missing");
    require(source.contains("LunCo.Electrical.DCMotor"), "motor class is missing");
    require(source.contains("drive_left"), "boundary input equation is missing");
    require(!source.contains("%name"), "opaque generated instance labels leaked into the UI");
    require(source.contains("Generated network"), "root diagram title is missing");
    require(source.contains("Generated unit"), "unit diagram title is missing");
    require(!source.contains("COMPOSED TOPOLOGY"), "poster graphics leaked into Modelica");
    require(!source.contains("\\n"), "literal escaped newlines leaked into Modelica");

    require(result.source_roots.contains("LunCo"), "policy did not report its source root");
    require(result.units.len > 0, "policy emitted no generated unit");
    require(result.layout.units.len == result.units.len, "unit layout is incomplete");
    require(result.layout.members.len == net.components.len, "member layout is incomplete");
    require(result.member_output_aliases.len == net.member_outputs.len,
        "member output promotion is incomplete");
    true
}
