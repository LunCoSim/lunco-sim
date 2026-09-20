// Policy-separation acceptance: structural validation reports the missing
// requirement subject, while policy-neutral analysis still exposes the
// source-backed requirement facts to independent Rhai policies.

let path = "assets/scripting/tests/fixtures/sysml_policy_isolation.sysml";
let checks = [];

let validation = query("ValidateSysml", #{path: path});
checks.push(t_true(validation != () && validation.ok == false,
    "source validation applies the structural SysML quality policy"));

let analysis = query("AnalyzeSysml", #{path: path, tables: ["requirements"]});
checks.push(t_true(analysis != () && analysis.ok == true
    && analysis.analysis.requirements.len() == 2,
    "typed source analysis remains available independently of structural lint findings"));

let usage_found = false;
for requirement in analysis.analysis.requirements {
    if requirement.element.kind == "RequirementUsage"
        && requirement.element.qualified_name == "PolicyIsolation::massBudget" {
        usage_found = true;
    }
}
checks.push(t_true(usage_found,
    "the independent analysis policy receives the source-backed requirement usage"));
t_report(checks);
