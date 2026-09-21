// Production acceptance for the application-authored physics escape policy.
// One body begins outside the static world; a second body proves that physics
// continues while the escaped object's articulated island is disabled.

fn fail(message) {
    report_verdict([message], "ESCAPE CONTAINMENT", "ESCAPE_CONTAINMENT");
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.escapee = find("/EscapeContainmentTest/Escapee");
    this.control = find("/EscapeContainmentTest/ControlBody");
    if this.escapee < 0 || this.control < 0 {
        this.done = true;
        fail("escape fixture bodies did not compose");
        return;
    }
    this.escapee_start = world_pos(this.escapee);
    this.control_start = world_pos(this.control);
    if this.escapee_start == () || this.control_start == () {
        this.done = true;
        fail("fixture body poses were unavailable at scenario start");
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 2.0 { return; }

    let escapee_end = world_pos(this.escapee);
    let control_end = world_pos(this.control);
    let checks = [
        t_present(escapee_end, "escaped object's pose remains inspectable"),
        t_present(control_end, "control body's pose remains inspectable"),
    ];
    if escapee_end != () && this.escapee_start != () {
        checks.push(t_true(
            (escapee_end[1] - this.escapee_start[1]).abs() < 0.1,
            "default Rhai policy did not pause the escaped object"));
    }
    if control_end != () && this.control_start != () {
        checks.push(t_true(
            this.control_start[1] - control_end[1] > 5.0,
            "physics stopped globally instead of continuing for the control body"));
        checks.push(t_true(control_end[1] > 0.3 && control_end[1] < 0.8,
            "control body did not settle on the static ground"));
    }
    if escapee_end != () && control_end != () {
        print("[escape] escaped_y=" + r2(escapee_end[1]) +
            " control_y=" + r2(control_end[1]));
    }
    this.done = true;
    report_verdict(checks, "ESCAPE CONTAINMENT", "ESCAPE_CONTAINMENT");
}
