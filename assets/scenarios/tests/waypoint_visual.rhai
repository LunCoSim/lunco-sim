// Graphics acceptance for the reusable waypoint marker presentation.
const TEST_KIND = "graphics";

fn on_start(me) {
    this.rover = find("/WaypointVisual/Rover");
    this.marker = find("/WaypointVisual/Waypoint");
    this.done = false;
    this.t = 0.0;
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    this.done = true;
    let checks = [];
    checks.push(t_true(this.rover >= 0,
        "six-wheel rover is missing from the waypoint review scene"));
    checks.push(t_true(this.marker >= 0,
        "reusable waypoint marker is missing from the review scene"));
    report_verdict(checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
}
