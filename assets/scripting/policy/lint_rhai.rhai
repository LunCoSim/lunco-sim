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

fn production_on_tick(facts) {
    let out = [];
    if is_test_scenario(facts.path) { return out; }
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

fn lint_rhai(facts) {
    production_on_tick(facts)
}
