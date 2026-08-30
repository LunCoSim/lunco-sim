// Graphics acceptance for the composed battery mount presentation.
const TEST_KIND = "graphics";

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    this.done = true;
    report_verdict([], "BATTERY MOUNTS VISUAL", "BATTERY_MOUNTS_VISUAL");
}
