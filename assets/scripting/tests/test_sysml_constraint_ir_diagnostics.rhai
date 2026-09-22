// Production Rhai contract for invalid neutral-IR input.
// A malformed or unsupported constraint must remain source-diagnosable and
// must never be presented as an executable Modelica obligation.

let valid_path = "lunco://scripting/tests/fixtures/sysml_constraint_projection.sysml";
let invalid_path = "lunco://scripting/tests/fixtures/sysml_constraint_ir_invalid.sysml";
let checks = [];

let valid = sysml_constraint_ir(valid_path,
    "ConstraintProjection::CoincidentPointTranslation");
checks.push(t_true(valid.valid == true && valid.status == "valid"
    && valid.constraint.parameters.len() == 3
    && valid.constraint.expressions.len() == 1
    && valid.constraint.dependencies.len() == 3
    && valid.constraint.fingerprint.starts_with("0x"),
    "a valid constraint exposes typed parameters, dependencies, expressions, and a stable fingerprint"));

let modelica = sysml_modelica_constraint(valid_path,
    "ConstraintProjection::CoincidentPointTranslation");
checks.push(t_true(modelica.ok == true
    && modelica.ir.valid == true
    && modelica.model_name.starts_with("SysmlConstraint_")
    && modelica.source.contains("equation")
    && modelica.source.contains("Real movingPoint[3]"),
    "a valid neutral constraint crosses the Rumoca-owned Modelica syntax boundary"));

let invalid = sysml_constraint_ir(invalid_path, "ConstraintIrDiagnostics::TypeMismatch");
checks.push(t_true(invalid.valid == false && invalid.status == "invalid"
    && invalid.constraint != ()
    && invalid.diagnostics.len() > 0,
    "an operator/type mismatch remains an invalid source-linked IR result"));

let rejected = sysml_modelica_constraint(invalid_path, "ConstraintIrDiagnostics::TypeMismatch");
checks.push(t_true(rejected.ok == false
    && rejected.ir.valid == false
    && rejected.error != ()
    && rejected.source == (),
    "invalid IR is rejected before Modelica generation and cannot become executable source"));

let missing = sysml_constraint_ir(valid_path, "ConstraintProjection::DoesNotExist");
checks.push(t_true(missing.valid == false && missing.status == "not_found"
    && missing.constraint == () && missing.diagnostics.len() == 1,
    "missing qualified identities produce an explicit not-found diagnostic"));

t_report(checks);
