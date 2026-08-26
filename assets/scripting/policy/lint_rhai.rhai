// Rhai lint policy — authored rules over `{ path, kind, source, ... }`.
//
// Production Rhai owns discrete policy and orchestration. on_tick is supported
// as an exceptional fixed-step hook for sampled observers and discrete control;
// continuous numerical work remains in Modelica or a native signal substrate.

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

    let message = "declares on_tick outside assets/scenarios/tests — allowed only as an ";
    message += "exceptional fixed-step observer or discrete controller; keep continuous ";
    message += "equations/control in Modelica and ordinary sequencing in task/events.";
    out.push(finding(
        "production-rhai-on-tick",
        "warning",
        facts.path,
        message
    ));
    out
}

fn lint_rhai(facts) {
    production_on_tick(facts)
}
