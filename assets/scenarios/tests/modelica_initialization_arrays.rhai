// The standard USD scalar-port authoring contract cannot name indexed scalars.
// Exercise the shipped source policy directly with admitted aggregate facts.
import "lunco://scripting/policy/synth_acausal_network" as synth;

fn on_start(me, ctx) {
    let checks = [];
    let bindings = synth::initial_bindings([
        #{name: "u", dimensions: [2], values: ["p1.sample", "7.0"]},
        #{name: "matrix", dimensions: [2, 2], values: ["1.0", "2.0", "3.0", "4.0"]},
        #{name: "scalar", dimensions: [], values: ["true"]}
    ]);
    let source = synth::instance_declaration("Feedback", "p0", bindings);
    checks.push(t_true(source == "Feedback p0(u = {p1.sample, 7.0}, matrix = {{1.0, 2.0}, {3.0, 4.0}}, scalar = true)",
        "checked input groups emit whole array modifiers in exact scalar order"));
    let too_few = false;
    try { synth::initial_bindings([#{name: "u", dimensions: [2, 2], values: ["1.0"]}]); }
    catch (error) { too_few = true; }
    checks.push(t_true(too_few, "incomplete checked shape is rejected before source emission"));
    let too_many = false;
    try { synth::initial_bindings([#{name: "u", dimensions: [], values: ["1.0", "2.0"]}]); }
    catch (error) { too_many = true; }
    checks.push(t_true(too_many, "excess scalar values cannot silently change the input shape"));
    report_verdict(checks, "MODELICA INITIALIZATION ARRAYS", "MODELICA_INITIALIZATION_ARRAYS");
}
