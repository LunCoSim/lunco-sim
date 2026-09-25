// Production Rhai contract for requirement-linked neutral SysML constraints.
// The source owns the predicate; this check table supplies provider readings
// and the generic evaluator preserves the four-state IR verdict.

import "lunco://scripting/tools/sysml_requirements" as sysml_requirements;

const PATH = "lunco://scripting/tests/fixtures/sysml_requirement_constraint.sysml";
const CONSTRAINT = "SysmlRequirementConstraint::PayloadWithinCapacity";
const UNRELATED_CONSTRAINT = "SysmlRequirementConstraint::UnrelatedCapacityCheck";
const VERIFICATION = "SysmlRequirementConstraint::VerifyPayloadCapacity";

let checks = [];
let analyzed = query("AnalyzeSysml", #{path: PATH,
    tables: ["requirements", "verifications"]});
let source = if analyzed != () && analyzed.analysis != () {
    #{ok: analyzed.ok, path: analyzed.path, errors: analyzed.errors,
      warnings: analyzed.warnings,
      requirement_records: analyzed.analysis.requirements,
      verification_records: analyzed.analysis.verifications,
      source_revision: analyzed.analysis.source_revision,
      source_files: analyzed.analysis.source_files}
} else { #{} };
checks.push(t_true(analyzed != () && analyzed.ok == true
    && source.requirement_records.len() == 2
    && source.verification_records.len() == 1,
    "constraint fixture resolves through generic typed SysML facts"));

let linked_requirement = ();
for requirement in source.requirement_records {
    if requirement.qualified_name == "SysmlRequirementConstraint::payloadCapacity" {
        linked_requirement = requirement;
    }
}
checks.push(t_true(linked_requirement != ()
    && linked_requirement.constraints.len() == 1
    && linked_requirement.constraints[0].kind == "require"
    && linked_requirement.constraints[0].definition_name == CONSTRAINT,
    "analysis preserves the formal requirement-to-constraint membership"));

let pass_check = sysml_requirements::constraint_check("payload-at-limit",
    "payload", "payloadCapacity", "VerifyPayloadCapacity", CONSTRAINT,
    #{payloadKg: 625.0, capacityKg: 625.0}, "external", 0.0);
let passed = sysml_requirements::evaluate(source, [pass_check]);
let pass_result = if passed.results.len() == 1 { passed.results[0] } else { () };
checks.push(t_true(passed.ok == true && pass_result != ()
    && pass_result.verdict == "pass"
    && pass_result.requirement.ends_with("::payloadCapacity")
    && pass_result.verification == VERIFICATION
    && pass_result.required_constraint_definition == CONSTRAINT
    && pass_result.constraint_fingerprint.starts_with("0x"),
    "pass evidence retains canonical SysML identities and constraint fingerprint"));

let fail_check = sysml_requirements::constraint_check("payload-over-limit",
    "payload", "payloadCapacity", VERIFICATION, CONSTRAINT,
    #{payloadKg: 625.001, capacityKg: 625.0}, "external", 0.0);
let failed = sysml_requirements::evaluate(source, [fail_check]);
let fail_result = if failed.results.len() == 1 { failed.results[0] } else { () };
checks.push(t_true(failed.ok == false && fail_result != ()
    && fail_result.verdict == "fail" && fail_result.ok == false,
    "a false source constraint remains a fail verdict instead of a Rhai boolean assertion"));

let missing_check = sysml_requirements::constraint_check("payload-missing-limit",
    "payload", "payloadCapacity", VERIFICATION, CONSTRAINT,
    #{payloadKg: 10.0}, "external", 0.0);
let missing = sysml_requirements::evaluate(source, [missing_check]);
let missing_result = if missing.results.len() == 1 { missing.results[0] } else { () };
checks.push(t_true(missing.ok == false && missing_result != ()
    && missing_result.verdict == "inconclusive",
    "an absent provider value remains inconclusive and is not fabricated"));

let typo_check = sysml_requirements::constraint_check("payload-unknown-binding",
    "payload", "payloadCapacity", VERIFICATION, CONSTRAINT,
    #{paylodKg: 10.0, capacityKg: 625.0}, "external", 0.0);
let typo = sysml_requirements::evaluate(source, [typo_check]);
let typo_result = if typo.results.len() == 1 { typo.results[0] } else { () };
checks.push(t_true(typo.ok == false && typo_result != ()
    && typo_result.verdict == "error"
    && typo_result.error.contains("no SysML constraint parameter"),
    "an unknown binding key is a visible error"));

let unlinked_check = sysml_requirements::constraint_check("payload-unlinked-constraint",
    "payload", "payloadCapacity", VERIFICATION, UNRELATED_CONSTRAINT,
    #{payloadKg: 10.0, capacityKg: 625.0}, "external", 0.0);
let unlinked = sysml_requirements::evaluate(source, [unlinked_check]);
let unlinked_result = if unlinked.results.len() == 1 { unlinked.results[0] } else { () };
checks.push(t_true(unlinked.ok == false && unlinked_result != ()
    && unlinked_result.verdict == "error"
    && unlinked_result.error.contains("not a required constraint"),
    "a valid but unrelated constraint cannot satisfy a requirement"));

t_report(checks);
