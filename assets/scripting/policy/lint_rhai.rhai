// Rhai lint policy — authored rules over `{ path, kind, source, ... }`.
//
// Production Rhai is orchestration: events, commands, and mission sequencing.
// Continuous numerical work belongs in Modelica or a native signal/control
// substrate. Test scenarios may use on_tick because advancing and observing the
// simulation is precisely what those scripts test.

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

    let message = "declares on_tick outside assets/scenarios/tests — production Rhai ";
    message += "must be event-driven; move continuous math/control to Modelica or ";
    message += "native signal code, and express propagation with authored connections.";
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
