// Generic public-surface assertions invoked by the production API driver.
fn restored(expected) {
    let checks = [];
    for intent in expected.keys() {
        checks.push(t_true(input_binding(intent) == expected[intent], "restored " + intent));
    }
    report_verdict(checks, "TWIN INPUT RESTORATION", "TWIN_INPUT_RESTORATION");
}
fn rejected(code, previous_tick) {
    let readiness = query("GetReadiness", #{});
    report_verdict([t_true(readiness.faulted && readiness.fault.kind == code,
        "invalid Twin presentation data reports its owner fault"),
        t_true(!readiness.ready && clock_snapshot().sim_tick == previous_tick, "invalid input or UI holds admission without crashing")],
        "TWIN PRESENTATION REJECTION", "TWIN_PRESENTATION_REJECTION");
}

fn preferences(expected) {
    report_verdict([t_true(get_setting("InputBindingsSettings.bindings") == expected,
        "Twin bindings never modify user preferences")], "TWIN INPUT PREFERENCES", "TWIN_INPUT_PREFERENCES");
}
