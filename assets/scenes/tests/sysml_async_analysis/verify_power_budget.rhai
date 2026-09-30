// Deterministic fixture observation for the source-defined rover power limit.
// This demonstrates SysML requirement evaluation; it is not live simulation telemetry.
import "lunco://scripting/tools/sysml_requirements" as sysml_requirements;
import "lunco://scripting/tools/sysml_requirement_checks" as sysml_requirement_checks;
import "lunco://scripting/tools/engineering_units" as engineering_units;

const REQUIREMENT = "AsyncTwinAnalysis::powerBudget";
const VERIFICATION = "AsyncTwinAnalysis::VerifyPowerBudget";
const CRITERION = "AsyncTwinAnalysis::MinimumOutputPower";
const LIMIT = "AsyncTwinAnalysis::PowerBudget::minimumPower";

fn simulation_dependencies(me, ctx) {
    #{
        modelica_entities: [],
        entity_reads: [], entity_writes: [],
        query_reads: ["AnalyzeSysml", "ReadActiveTwinContract"],
        required_inputs: [
            #{ owner: "sysml.twin-analysis", identity: twin_name() },
        ],
    }
}

fn source_failure(message, actual) {
    let check = sysml_requirement_checks::preflight(
        "power-budget-source", false, message, actual, "typed 45 W minimum and 52 W observation");
    sysml_requirement_checks::finish_preflight(
        [check], "Power budget verification", "POWER_BUDGET",
        twin_name(), "AsyncTwinAnalysis");
}

// Telemetry accepts the shared HookValue map/array/scalar ABI. Preserve native
// quantities during evaluation, then lower them to explicit evidence records
// only at this report boundary.
fn report_evidence(value) {
    let kind = type_of(value);
    if kind == "SysmlElementHandle" {
        return #{element_id: value.element_id,
            source_revision: value.source_revision,
            source_fingerprint: value.source_fingerprint};
    }
    if kind == "SysmlTypeRef" {
        return #{qualified_name: value.qualified_name};
    }
    if kind == "Quantity" {
        return #{kind: "quantity", value: value.value, unit: value.unit,
            dimension: value.dimension,
            conversion_is_exact: value.conversion_is_exact};
    }
    if kind == "SysmlQuantityLiteral" {
        let measurement_reference = value.measurement_reference;
        return #{kind: "quantity", value: value.value,
            unit: value.unit_symbol, quantity_kind: report_evidence(value.kind),
            measurement_reference: if measurement_reference == () { () }
                else { measurement_reference.qualified_name },
            resolved_unit: measurement_reference != ()
                && measurement_reference.engineering_unit != ()};
    }
    if kind == "array" {
        let result = [];
        for item in value { result.push(report_evidence(item)); }
        return result;
    }
    if kind == "map" {
        let result = #{};
        for key in value.keys() { result[key] = report_evidence(value[key]); }
        return result;
    }
    value
}

fn finish_requirement(source, check, delivered_power, minimum_power) {
    let requirement_handle = ();
    for record in source.requirement_records {
        if record.element.qualified_name == REQUIREMENT {
            requirement_handle = record.element.handle;
            break;
        }
    }
    let provenance = sysml_requirements::provenance_for_requirement(
        source, requirement_handle);
    if provenance.ok != true {
        source_failure("Requirement source evidence is unavailable",
            #{error: provenance.error});
        return;
    }

    let evaluation = sysml_requirements::evaluate(source, [check]);
    if evaluation.results == () || evaluation.results.len() != 1 {
        source_failure("The requirement evaluator returned no unique result",
            #{result_count: if evaluation.results == () { 0 } else { evaluation.results.len() }});
        return;
    }
    let outcome = evaluation.results[0];
    let constraint_diagnostics = [];
    if outcome.diagnostics != () {
        for diagnostic in outcome.diagnostics {
            constraint_diagnostics.push(#{severity: diagnostic.severity,
                code: diagnostic.code, message: diagnostic.message});
        }
    }
    let actual = #{deliveredPower: report_evidence(delivered_power),
        minimumPower: report_evidence(minimum_power)};
    let result = #{id: outcome.id, component: outcome.component,
        requirement: outcome.requirement, verification: outcome.verification,
        kind: outcome.kind, constraint: CRITERION, verdict: outcome.verdict,
        error: outcome.error, actual: actual,
        expression_results: outcome.expression_results,
        constraint_diagnostics: constraint_diagnostics,
        expected: #{minimumPower: report_evidence(minimum_power)},
        source_revision: outcome.source_revision, tolerance: outcome.tolerance};
    let failures = [];
    let non_pass_results = [];
    if outcome.verdict == "fail" { failures.push(result); }
    if outcome.verdict != "pass" { non_pass_results.push(result); }
    let report = #{ok: evaluation.ok, verdict: evaluation.verdict,
        results: [result], failures: failures, non_pass_results: non_pass_results,
        check_count: evaluation.check_count,
        requirement_count: evaluation.requirement_count,
        requirement_names: evaluation.requirement_names,
        requirement_summary: evaluation.requirement_summary,
        pass_count: evaluation.pass_count, failure_count: evaluation.failure_count,
        inconclusive_count: evaluation.inconclusive_count,
        error_count: evaluation.error_count, unverified_count: evaluation.unverified_count,
        non_pass_count: evaluation.non_pass_count,
        verification: evaluation.verification,
        source_revision: evaluation.source_revision, source_files: evaluation.source_files};
    report.source_evidence = [report_evidence(provenance.evidence)];
    report.source_evidence_failures = [];
    report.metrics = #{clock: clock_snapshot(), root: twin_name(),
        component_package: "AsyncTwinAnalysis",
        verification: VERIFICATION, separate_component_document: true};
    report_structured_verdict(report, "Power budget verification", "POWER_BUDGET");
}

fn on_start(me, ctx) {
    let source = sysml_requirements::source_with_selection(
        [LIMIT], [REQUIREMENT], [VERIFICATION]);
    if source == () || source.ok != true {
        source_failure("Twin SysML requirement source is unavailable", source);
        return;
    }

    let minimum_power = sysml_requirements::native_value(source, LIMIT);
    let delivered_power = engineering_units::quantity_from_ucum(52.0, "W");
    if minimum_power == () || delivered_power == () {
        source_failure("The source minimum or typed power observation is unavailable",
            #{minimum_power: minimum_power, delivered_power: delivered_power});
        return;
    }

    let check = sysml_requirements::required_constraint_check(
        "nominal-power-output", "rover", REQUIREMENT, VERIFICATION, CRITERION,
        #{deliveredPower: delivered_power, minimumPower: minimum_power},
        "derived", 0.0);
    finish_requirement(source, check, delivered_power, minimum_power);
}
