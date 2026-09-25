// Production Rhai contract for the neutral IR evaluator.
// Provider state is explicit: missing/stale is inconclusive, provider-invalid
// is an error, and a typed false relation is the only failure verdict.

let path = "lunco://scripting/tests/fixtures/sysml_constraint_ir_evaluation.sysml";
let checks = [];
let name = "ConstraintIrEvaluation::SumEquals";
let ir = sysml_constraint_ir(path, name);
let left = ir.constraint.parameters[0].feature;
let right = ir.constraint.parameters[1].feature;

checks.push(t_true(ir.valid == true && ir.status == "valid"
    && ir.constraint.parameters.len() == 2,
    "the evaluator test uses a source-pinned IR result and typed constraint parameters"));

let unavailable = sysml_evaluate_constraint(path, name, [], 1.0e-9, 1.0e-9);
checks.push(t_true(unavailable.verdict == "inconclusive"
    && unavailable.expression_results[0] == (),
    "missing provider observations are inconclusive, never false"));

let pass_observations = [
    #{path: [left], provider: "external", state: "value", value: 1.0},
    #{path: [right], provider: "external", state: "value", value: 2.0}
];
let passed = sysml_evaluate_constraint(path, name, pass_observations, 1.0e-9, 1.0e-9);
checks.push(t_true(passed.verdict == "pass"
    && passed.expression_results[0] == true,
    "typed provider values produce a passing constraint verdict"));

pass_observations[1].value = 1.0;
let failed = sysml_evaluate_constraint(path, name, pass_observations, 1.0e-9, 1.0e-9);
checks.push(t_true(failed.verdict == "fail"
    && failed.expression_results[0] == false,
    "a typed false Boolean relation produces a failure verdict"));

let stale_observations = [
    #{path: [left], provider: "telemetry", state: "stale", detail: "sample expired"},
    #{path: [right], provider: "telemetry", state: "value", value: 2.0}
];
let stale = sysml_evaluate_constraint(path, name, stale_observations, 1.0e-9, 1.0e-9);
checks.push(t_true(stale.verdict == "inconclusive"
    && stale.diagnostics.len() > 0,
    "stale telemetry is inconclusive and remains diagnostically visible"));

let invalid_observations = [
    #{path: [left], provider: "usd", state: "invalid", detail: "non-finite stage value"},
    #{path: [right], provider: "usd", state: "value", value: 2.0}
];
let invalid = sysml_evaluate_constraint(path, name, invalid_observations, 1.0e-9, 1.0e-9);
checks.push(t_true(invalid.verdict == "error"
    && invalid.diagnostics.len() > 0,
    "provider-invalid observations are errors, never failed engineering relations"));

let boolean_false = sysml_evaluate_constraint(
    path, "ConstraintIrEvaluation::BooleanFalse", #{}, 1.0e-9, 1.0e-9);
checks.push(t_true(boolean_false.verdict == "fail"
    && boolean_false.expression_results[0] == false,
    "a body that evaluates to false is a failure even without external bindings"));

t_report(checks);
