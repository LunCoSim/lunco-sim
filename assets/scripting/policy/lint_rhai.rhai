// Rhai lint policy — authored rules over `{ path, kind, source, ... }`.
//
// Production Rhai owns scenario policy and orchestration through task/events.
// on_tick is reserved for authored test scenarios that sample state and publish
// a bounded verdict; continuous numerical work remains in Modelica or a native
// signal substrate.

fn finding(rule, severity, subject, message) {
    let f = #{};
    f.rule = rule;
    f.severity = severity;
    f.subject = subject;
    f.message = message;
    f
}

fn is_test_scenario(path) {
    path.contains("assets/scenarios/tests/")
        || path.contains("scenarios/tests/")
}

fn is_lint_policy(path) {
    path.contains("assets/scripting/policy/")
        || path.contains("scripting/policy/")
}

fn production_on_tick(facts) {
    let out = [];
    if is_test_scenario(facts.path) || is_lint_policy(facts.path) { return out; }
    if !facts.source.contains("fn on_tick(") { return out; }

    let message = "declares on_tick outside assets/scenarios/tests — production scenarios must use task/events; reserve on_tick for bounded test verdicts.";
    out.push(finding(
        "production-rhai-on-tick",
        "error",
        facts.path,
        message
    ));
    out
}

fn build_profile_gate(facts) {
    let out = [];
    if is_lint_policy(facts.path) { return out; }
    if !facts.source.contains("is_debug(")
        && !facts.source.contains("debug_assertions") {
        return out;
    }

    out.push(finding(
        "rhai-build-profile-gate",
        "error",
        facts.path,
        "Rhai behavior must not branch on the Rust build profile; use the authored audience/runtime contract instead."
    ));
    out
}

fn lint_rhai(facts) {
    let out = production_on_tick(facts);
    for finding in build_profile_gate(facts) {
        out.push(finding);
    }
    out
}
