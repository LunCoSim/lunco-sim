// Production authored acceptance for the SysML/KerML neutral constraint IR.
// This runs inside the normal Rhai world bridge, so it exercises the same
// native SysmlModel methods that Twin policies use, including Rumoca lowering
// and the four-state provider evaluator.

import "lunco://scripting/tools/sysml_requirements" as sysml_requirements;

const VALID_PATH = "lunco://scripting/tests/fixtures/sysml_constraint_projection.sysml";
const INVALID_PATH = "lunco://scripting/tests/fixtures/sysml_constraint_ir_invalid.sysml";
const EVALUATION_PATH = "lunco://scripting/tests/fixtures/sysml_constraint_ir_evaluation.sysml";

fn finish(me, checks) {
    report_verdict(checks, "SYSML CONSTRAINT IR", "SYSML_CONSTRAINT_IR");
    me.done = true;
}

fn requirement_result_summary(report) {
    let summary = "verdict=" + report.verdict.to_string();
    if report.error != () && report.error != "" {
        return summary + " · " + report.error;
    }
    if report.non_pass_results != () && report.non_pass_results.len() > 0 {
        let result = report.non_pass_results[0];
        if type_of(result) == "map" {
            if result.error != () { summary += " · " + result.error.to_string(); }
            else if result.message != () { summary += " · " + result.message.to_string(); }
        } else if result != () {
            summary += " · " + result.to_string();
        }
    }
    summary
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["AnalyzeSysml", "InspectSysmlDocument", "ListOpenDocuments"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.document_phase = 0;
    this.document_id = ();
    this.document_generation = ();
    this.initial_document_ids = [];
    this.elapsed = 0.0;
}

fn on_tick(me, ctx) {
    if this.done { return; }

    this.elapsed += dt();
    if this.elapsed > 30.0 {
        finish(this, ["SysML document analysis test timed out in phase "
            + this.document_phase]);
        return;
    }

    let checks = [];
    if this.document_phase == 0 {
        let documents = query("ListOpenDocuments");
        if documents == () || documents.open_documents == () {
            finish(this, ["ListOpenDocuments did not return its documented result"]);
            return;
        }
        for document in documents.open_documents {
            this.initial_document_ids.push(document.doc_id);
        }
        cmd("NewDocument", #{ kind: "sysml" });
        this.document_phase = 1;
        return;
    }

    if this.document_phase == 1 {
        let documents = query("ListOpenDocuments");
        for document in documents.open_documents {
            if document.kind == "sysml"
                && !this.initial_document_ids.contains(document.doc_id) {
                this.document_id = document.doc_id;
            }
        }
        if this.document_id == () { return; }
        let before = query("InspectSysmlDocument", #{ doc_id: this.document_id });
        if before == () || before.generation == () {
            finish(this, ["the new SysML document could not be inspected"]);
            return;
        }
        let invalid_source = "part def Broken {";
        sysml_editor::replace_source(
            this.document_id, invalid_source, before.generation);
        let after = query("InspectSysmlDocument", #{ doc_id: this.document_id });
        if after == () || after.source != invalid_source
            || after.generation <= before.generation {
            finish(this, ["the SysML source edit did not commit its new generation"]);
            return;
        }
        let retry = sysml_editor::verify(this.document_id);
        checks.push(t_true(after.source == invalid_source
            && after.generation > before.generation,
            "a source edit commits immediately with a newer document generation"));
        checks.push(t_true(after.analysis_state == "pending"
            && after.analysis_generation == after.generation
            && after.diagnostics == () && after.semantic_errors == (),
            "an edit exposes pending analysis without fabricating a clean diagnostic result"));
        checks.push(t_true(retry.ok == false && retry.retryable == true
            && retry.stage == "analysis",
            "the Rhai editor reports unfinished analysis as retryable"));
        this.document_generation = after.generation;
        this.document_phase = 2;
        this.document_checks = checks;
        return;
    }

    if this.document_phase == 2 {
        let inspection = query("InspectSysmlDocument", #{ doc_id: this.document_id });
        if inspection == () || inspection.generation != this.document_generation {
            finish(this, ["analysis inspection lost the edited source generation"]);
            return;
        }
        if inspection.analysis_state == "pending" { return; }
        if inspection.analysis_state == "failed" {
            finish(this, ["SysML analysis failed: "
                + inspection.analysis_error.to_string()]);
            return;
        }
        if inspection.analysis_state != "ready" {
            finish(this, ["SysML analysis returned an unknown state"]);
            return;
        }
        let semantic = sysml_editor::verify(this.document_id);
        checks.push(t_true(inspection.analysis_state == "ready"
            && inspection.analysis_generation == this.document_generation
            && inspection.semantic_errors == true
            && inspection.diagnostics.len() > 0,
            "the committed analysis reports parser diagnostics for the exact edited generation"));
        checks.push(t_true(semantic.ok == false && semantic.stage == "semantic"
            && semantic.retryable == false,
            "the editor treats ready parser diagnostics as a terminal verification result"));
        for check in this.document_checks { checks.push(check); }
        cmd("CloseDocument", #{ doc_id: this.document_id });
        this.document_phase = 3;
    }

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
        && type_of(valid.constraint.fingerprint) == "u64",
        "valid IR preserves source identity, typed parameters, dependencies, expressions, and its native fingerprint"));

    let backend = model.modelica_constraint(
        "ConstraintProjection::CoincidentPointTranslation");
    let binding_names = [];
    for binding in backend.feature_bindings {
        binding_names.push(binding.qualified_name);
    }
    let backend_lowered = backend.ok == true && backend.ir.valid == true
        && backend.model_name.starts_with("SysmlConstraint_")
        && backend.source.contains("equation")
        && backend.source.contains("ISQBase::LengthValue")
        && binding_names.contains(
            "ConstraintProjection::CoincidentPointTranslation::movingPoint")
            && binding_names.contains(
                "ConstraintProjection::CoincidentPointTranslation::fixedPoint")
            && binding_names.contains(
                "ConstraintProjection::CoincidentPointTranslation::translation");
    checks.push(t_true(backend_lowered,
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
    let left = sum.constraint.parameters[0].feature;
    let right = sum.constraint.parameters[1].feature;
    let unavailable = evaluation_model.evaluate_constraint(
        sum_name, [], 1.0e-9, 1.0e-9);
    checks.push(t_true(sum.valid == true && sum.constraint.parameters.len() == 2
        && unavailable.verdict == "inconclusive"
        && unavailable.expression_results[0] == (),
        "missing provider observations are inconclusive, never false"));

    let observations = [
        #{path: [left], provider: "external", state: "value", value: 1.0},
        #{path: [right], provider: "external", state: "value", value: 2.0}
    ];
    let passed = evaluation_model.evaluate_constraint(
        sum_name, observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(passed.verdict == "pass"
        && passed.expression_results[0] == true,
        "typed provider observations produce a passing verdict"));

    observations[1].value = 1.0;
    let failed = evaluation_model.evaluate_constraint(
        sum_name, observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(failed.verdict == "fail"
        && failed.expression_results[0] == false,
        "a typed false relation produces a failure verdict"));

    let stale_observations = [
        #{path: [left], provider: "telemetry", state: "stale", detail: "sample expired"},
        #{path: [right], provider: "telemetry", state: "value", value: 2.0}
    ];
    let stale = evaluation_model.evaluate_constraint(
        sum_name, stale_observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(stale.verdict == "inconclusive" && stale.diagnostics.len() > 0,
        "stale telemetry is inconclusive and diagnostically visible"));

    let invalid_observations = [
        #{path: [left], provider: "usd", state: "invalid",
            detail: "non-finite stage value"},
        #{path: [right], provider: "usd", state: "value", value: 2.0}
    ];
    let invalid_result = evaluation_model.evaluate_constraint(
        sum_name, invalid_observations, 1.0e-9, 1.0e-9);
    checks.push(t_true(invalid_result.verdict == "error"
        && invalid_result.diagnostics.len() > 0,
        "provider-invalid observations are errors, not failed relations"));

    let requirement_path = "lunco://scripting/tests/fixtures/sysml_requirement_constraint.sysml";
    let analyzed_requirements = query("AnalyzeSysml", #{path: requirement_path,
        tables: ["requirements", "verifications"]});
    let requirement_source = if analyzed_requirements != ()
        && analyzed_requirements.analysis != () {
        #{ok: analyzed_requirements.ok, path: analyzed_requirements.path,
          errors: analyzed_requirements.errors,
          warnings: analyzed_requirements.warnings,
          requirement_records: analyzed_requirements.analysis.requirements,
          verification_records: analyzed_requirements.analysis.verifications,
          source_revision: analyzed_requirements.analysis.source_revision,
          source_files: analyzed_requirements.analysis.source_files}
    } else { #{} };
    checks.push(t_true(requirement_source.ok == true
        && requirement_source.source_revision != ()
        && requirement_source.requirement_records.len() == 2
        && requirement_source.verification_records.len() == 1,
        "the requirement evaluator received typed analysis facts: "
            + analyzed_requirements.to_string()));
    let constraint_name = "SysmlRequirementConstraint::PayloadWithinCapacity";
    let verification_name = "SysmlRequirementConstraint::VerifyPayloadCapacity";
    let pass_check = sysml_requirements::constraint_check("aggregate-pass",
        "payload", "payloadCapacity", verification_name, constraint_name,
        #{payloadKg: 625.0, capacityKg: 625.0}, "external", 0.0);
    let requirement_pass = sysml_requirements::evaluate(
        requirement_source, [pass_check]);
    checks.push(t_true(requirement_pass.verdict == "pass"
        && requirement_pass.pass_count == 1
        && requirement_pass.non_pass_count == 0,
        "the Rhai requirement evaluator aggregates a passing constraint as pass: "
            + requirement_result_summary(requirement_pass)));

    let fail_check = sysml_requirements::constraint_check("aggregate-fail",
        "payload", "payloadCapacity", verification_name, constraint_name,
        #{payloadKg: 625.001, capacityKg: 625.0}, "external", 0.0);
    let requirement_fail = sysml_requirements::evaluate(
        requirement_source, [fail_check]);
    checks.push(t_true(requirement_fail.verdict == "fail"
        && requirement_fail.failure_count == 1
        && requirement_fail.error_count == 0,
        "the Rhai requirement evaluator reports a false source predicate as fail: "
            + requirement_result_summary(requirement_fail)));

    let inconclusive_check = sysml_requirements::constraint_check("aggregate-inconclusive",
        "payload", "payloadCapacity", verification_name, constraint_name,
        #{payloadKg: 625.0}, "external", 0.0);
    let requirement_inconclusive = sysml_requirements::evaluate(
        requirement_source, [inconclusive_check]);
    checks.push(t_true(requirement_inconclusive.verdict == "inconclusive"
        && requirement_inconclusive.inconclusive_count == 1
        && requirement_inconclusive.failure_count == 0,
        "the Rhai requirement evaluator preserves missing provider data as inconclusive: "
            + requirement_result_summary(requirement_inconclusive)));

    let error_check = sysml_requirements::constraint_check("aggregate-error",
        "payload", "payloadCapacity", verification_name,
        "SysmlRequirementConstraint::UnrelatedCapacityCheck",
        #{payloadKg: 10.0, capacityKg: 625.0}, "external", 0.0);
    let requirement_error = sysml_requirements::evaluate(
        requirement_source, [error_check]);
    checks.push(t_true(requirement_error.verdict == "error"
        && requirement_error.error_count == 1
        && requirement_error.failure_count == 0,
        "the Rhai requirement evaluator distinguishes invalid verification setup from a failed predicate: "
            + requirement_result_summary(requirement_error)));

    checks.push(t_true(structured_verdict(requirement_pass) == "pass"
        && structured_verdict(requirement_fail) == "fail"
        && structured_verdict(requirement_inconclusive) == "inconclusive"
        && structured_verdict(requirement_error) == "error",
        "the authored verdict adapter preserves all four requirement result states"));
    checks.push(t_true(structured_verdict(#{verdict: "invalid", ok: true}) == "error"
        && sysml_requirements::_result_verdict(#{verdict: "invalid", ok: true}) == "error",
        "an invalid explicit verdict cannot be converted into a pass by its ok flag"));

    let mixed_error = sysml_requirements::evaluate(
        requirement_source, [fail_check, error_check]);
    checks.push(t_true(mixed_error.verdict == "error"
        && mixed_error.failure_count == 1
        && mixed_error.error_count == 1,
        "an evaluation error takes precedence in a mixed requirement result"));

    let empty_requirement = sysml_requirements::evaluate(requirement_source, []);
    checks.push(t_true(empty_requirement.verdict == "inconclusive"
        && empty_requirement.non_pass_count == 0,
        "an empty requirement evaluation is inconclusive rather than a vacuous pass"));

    let boolean_false = evaluation_model.evaluate_constraint(
        "ConstraintIrEvaluation::BooleanFalse", [], 1.0e-9, 1.0e-9);
    checks.push(t_true(boolean_false.verdict == "fail"
        && boolean_false.expression_results[0] == false,
        "a Boolean false body is a failure without external bindings"));

    finish(this, checks);
}
