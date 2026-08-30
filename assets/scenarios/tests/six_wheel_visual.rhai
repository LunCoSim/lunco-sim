// Graphics acceptance for the authored six-wheel rover presentation.
const TEST_KIND = "graphics";

fn on_start(me) {
    this.rover = find("/SixWheelVisual/Rover");
    this.done = false;
    this.t = 0.0;
    if this.rover >= 0 {
        let result = cmd("SetPorts", #{
            target: this.rover,
            writes: [["headlights", 1.0]],
        });
        this.accepted = result.ok;
    } else {
        this.accepted = false;
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    this.done = true;
    if this.accepted {
        report_verdict([], "SIX WHEEL VISUAL", "SIX_WHEEL_VISUAL");
    } else {
        report_verdict(["visual fixture could not command the rover headlights"],
                       "SIX WHEEL VISUAL", "SIX_WHEEL_VISUAL");
    }
}
