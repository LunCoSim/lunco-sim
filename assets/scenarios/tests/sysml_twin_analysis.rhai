// Production contract for Twin-scoped asynchronous SysML source-set analysis.
// The scene-test host opens this Twin and waits for its SysML dependency before
// evaluating the test program.

const PATH = "twin://sysml_async_analysis";
const SOURCE_PATH = "twin://sysml_async_analysis/analysis.sysml";

fn simulation_dependencies(me, ctx) {
    #{
        modelica_entities: [],
        required_inputs: [
            #{ owner: "sysml.twin-analysis", identity: twin_name() },
        ],
    }
}

let checks = [];
let analyzed = query("AnalyzeSysml", #{
    path: PATH,
    tables: ["requirements", "verifications"],
});
let usage = ();
let unrelated_usage = ();
let verification = ();
let requirement_coverage = ();
if analyzed != () && analyzed.analysis != () {
    for requirement in analyzed.analysis.requirements {
        if requirement.element.kind == "RequirementUsage"
            && requirement.element.qualified_name == "AsyncTwinAnalysis::powerBudget" {
            usage = requirement;
        }
        if requirement.element.kind == "RequirementUsage"
            && requirement.element.qualified_name == "AsyncTwinAnalysis::otherBudget" {
            unrelated_usage = requirement;
        }
    }
    for candidate in analyzed.analysis.verifications {
        if candidate.element.qualified_name == "AsyncTwinAnalysis::VerifyPowerBudget" {
            verification = candidate;
        }
    }
    requirement_coverage = sysml_requirement_coverage(
        analyzed.analysis.verifications);
}
checks.push(t_true(analyzed != () && analyzed.ok == true
    && analyzed.analysis != ()
    && analyzed.analysis.source_files.len() == 1
    && analyzed.analysis.source_files[0] == SOURCE_PATH
    && analyzed.analysis.source_revision != ()
    && usage != (),
    "the Twin query returns the committed source-set snapshot with its authored requirement usage"));
let covers_resolved_target = false;
let excludes_unrelated_usage = false;
if analyzed != () && analyzed.analysis != () && usage != ()
    && unrelated_usage != () && verification != ()
    && requirement_coverage != () {
    let coverage = requirement_coverage;
    if !verification.verified_requirements.is_empty() {
        covers_resolved_target = coverage.covers(
            verification.element.handle, verification.verified_requirements[0]);
    }
    excludes_unrelated_usage = !coverage.covers(
        verification.element.handle, unrelated_usage.element.handle);
}
checks.push(t_true(verification != () && verification.verifies.len() == 1
    && verification.verifies[0] == "powerBudget",
    "the verification projection retains its authored verify objective"));
checks.push(t_true(verification != () && verification.verified_requirements.len() == 1,
    "the verification projection publishes one resolved requirement handle"));
checks.push(t_true(covers_resolved_target,
    "verification coverage includes its resolved target handle"));
checks.push(t_true(excludes_unrelated_usage,
    "verification coverage excludes the other same-snapshot requirement"));

let unmounted = query("AnalyzeSysml", #{
    path: "twin://sysml_async_analysis_missing",
    tables: ["requirements"]});
checks.push(t_true(unmounted != () && unmounted.ok == false
    && unmounted.errors.len() > 0,
    "an unmounted Twin returns an explicit analysis error"));

report_verdict(checks, "SYSML TWIN ANALYSIS", "SYSML_TWIN_ANALYSIS");
