// Policy-separation acceptance: structural validation reports the missing
// requirement subject, while policy-neutral analysis still exposes the
// source-backed requirement facts to independent Rhai policies.

import "lunco://scripting/tools/asset_preflight" as preflight;

const TEST_KIND = "headless";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["AnalyzeSysml", "ValidateSysml"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.done = false; this.elapsed = 0.0; this.preflight = preflight::requests();
    this.warm_index = 0;
    this.warm_requests = [
        #{query_name: "ValidateSysml", params: #{path: "lunco://scripting/tests/fixtures/sysml_policy_isolation.sysml"}},
        #{query_name: "AnalyzeSysml", params: #{path: "lunco://scripting/tests/fixtures/sysml_policy_isolation.sysml", tables: ["requirements"]}}
    ];
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.preflight.error != () || this.elapsed > 30.0 {
        this.done = true;
        report_verdict([if this.preflight.error != () { this.preflight.error } else { "SysML file query preparation timed out" }], "SYSML POLICY ISOLATION", "SYSML_POLICY_ISOLATION");
        return;
    }
    if this.warm_index < this.warm_requests.len() {
        let request = this.warm_requests[this.warm_index];
        let prepared = preflight::read(request.query_name, request.params, this.preflight);
        this.preflight = prepared.state;
        if prepared.ready { this.warm_index += 1; }
        return;
    }
    let path = "lunco://scripting/tests/fixtures/sysml_policy_isolation.sysml";
    let checks = [];

    let prepared_validation = preflight::read("ValidateSysml", #{path: path}, this.preflight);
    this.preflight = prepared_validation.state;
    if !prepared_validation.ready { return; }
    let validation = prepared_validation.report;
    checks.push(t_true(validation != () && validation.ok == false,
        "source validation applies the structural SysML quality policy"));

    let prepared_analysis = preflight::read("AnalyzeSysml", #{path: path, tables: ["requirements"]}, this.preflight);
    this.preflight = prepared_analysis.state;
    if !prepared_analysis.ready { return; }
    let analysis = prepared_analysis.report;
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
    this.done = true;
    report_verdict(checks, "SYSML POLICY ISOLATION", "SYSML_POLICY_ISOLATION");
}
fn on_stop(me, ctx) {
    if !this.done { this.done = true; report_verdict(["SysML file query scenario stopped before validation"], "SYSML POLICY ISOLATION", "SYSML_POLICY_ISOLATION"); }
}
