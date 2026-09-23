// Production authored acceptance for the SysML/KerML neutral constraint IR.
// This runs inside the normal Rhai world bridge, so it exercises the same
// native SysmlModel methods that Twin policies use, including Rumoca lowering
// and the four-state provider evaluator.

const VALID_PATH = "lunco://scripting/tests/fixtures/sysml_constraint_projection.sysml";
const INVALID_PATH = "lunco://scripting/tests/fixtures/sysml_constraint_ir_invalid.sysml";
const EVALUATION_PATH = "lunco://scripting/tests/fixtures/sysml_constraint_ir_evaluation.sysml";

fn finish(me, checks) {
    report_verdict(checks, "SYSML CONSTRAINT IR", "SYSML_CONSTRAINT_IR");
    me.done = true;
}

fn on_start(me, ctx) {
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }

    let checks = [];
    let model = sysml_model(VALID_PATH);
    let invalid_model = sysml_model(INVALID_PATH);
    let evaluation_model = sysml_model(EVALUATION_PATH);
    if type_of(model) != "SysmlModel"
        || type_of(invalid_model) != "SysmlModel"
        || type_of(evaluation_model) != "SysmlModel" {
        let detail = "source fixtures did not produce native SysmlModel sessions ("
            + type_of(model) + ", " + type_of(invalid_model) + ", "
            + type_of(evaluation_model) + ")";
        if type_of(model) == "map" && model.error != () {
            detail += ": " + model.error;
        }
        finish(this, [detail]);
        return;
    }

    let valid = model.constraint_ir(
        "ConstraintProjection::CoincidentPointTranslation");
    checks.push(t_true(model.path == VALID_PATH
        && model.source_revision != ()
        && valid.valid == true && valid.status == "valid"
        && valid.constraint.parameters.len() == 3
        && valid.constraint.expressions.len() == 1
        && valid.constraint.dependencies.len() == 3
        && valid.constraint.fingerprint.starts_with("0x"),
        "valid IR preserves source identity, typed parameters, dependencies, expressions, and fingerprint"));

    let backend = model.modelica_constraint(
        "ConstraintProjection::CoincidentPointTranslation");
    checks.push(t_true(backend.ok == true && backend.ir.valid == true
        && backend.model_name.starts_with("SysmlConstraint_")
        && backend.source.contains("equation")
        && backend.source.contains("Real movingPoint[3]"),
        "valid IR lowers through the Rumoca-owned Modelica syntax boundary"));

    let invalid = invalid_model.constraint_ir("ConstraintIrDiagnostics::TypeMismatch");
    checks.push(t_true(invalid.valid == false && invalid.status == "invalid"
        && invalid.constraint != () && invalid.diagnostics.len() > 0,
        "operator/type mismatch remains a source-diagnosable invalid IR result"));
    let rejected = invalid_model.modelica_constraint("ConstraintIrDiagnostics::TypeMismatch");
    checks.push(t_true(rejected.ok == false && rejected.ir.valid == false
        && rejected.error != () && rejected.source == (),
        "invalid IR is rejected before Modelica generation"));
    let missing = model.constraint_ir("ConstraintProjection::DoesNotExist");
    checks.push(t_true(missing.valid == false && missing.status == "not_found"
        && missing.constraint == () && missing.diagnostics.len() == 1,
        "missing qualified identities produce an explicit diagnostic"));

    let sum_name = "ConstraintIrEvaluation::SumEquals";
    let sum = evaluation_model.constraint_ir(sum_name);
    let left = sum.constraint.parameters[0].qualified_name;
    let right = sum.constraint.parameters[1].qualified_name;
    let unavailable = evaluation_model.evaluate_constraint(
        sum_name, #{}, 1.0e-9, 1.0e-9);
    checks.push(t_true(sum.valid == true && sum.constraint.parameters.len() == 2
        && unavailable.verdict == "inconclusive"
        && unavailable.expression_results[0] == (),
        "missing provider observations are inconclusive, never false"));

    let observations = #{};
    observations[left] = #{provider: "external", state: "value", value: 1.0};
    observations[right] = #{provider: "external", state: "value", value: 2.0};
    let passed = evaluation_model.evaluate_constraint(
        sum_name, observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(passed.verdict == "pass"
        && passed.expression_results[0] == true,
        "typed provider observations produce a passing verdict"));

    observations[right].value = 1.0;
    let failed = evaluation_model.evaluate_constraint(
        sum_name, observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(failed.verdict == "fail"
        && failed.expression_results[0] == false,
        "a typed false relation produces a failure verdict"));

    let stale_observations = #{};
    stale_observations[left] = #{provider: "telemetry", state: "stale", detail: "sample expired"};
    stale_observations[right] = #{provider: "telemetry", state: "value", value: 2.0};
    let stale = evaluation_model.evaluate_constraint(
        sum_name, stale_observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(stale.verdict == "inconclusive" && stale.diagnostics.len() > 0,
        "stale telemetry is inconclusive and diagnostically visible"));

    let invalid_observations = #{};
    invalid_observations[left] = #{provider: "usd", state: "invalid",
        detail: "non-finite stage value"};
    invalid_observations[right] = #{provider: "usd", state: "value", value: 2.0};
    let invalid_result = evaluation_model.evaluate_constraint(
        sum_name, invalid_observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(invalid_result.verdict == "error"
        && invalid_result.diagnostics.len() > 0,
        "provider-invalid observations are errors, not failed relations"));

    let boolean_false = evaluation_model.evaluate_constraint(
        "ConstraintIrEvaluation::BooleanFalse", #{}, 1.0e-9, 1.0e-9);
    checks.push(t_true(boolean_false.verdict == "fail"
        && boolean_false.expression_results[0] == false,
        "a Boolean false body is a failure without external bindings"));

    finish(this, checks);
}
