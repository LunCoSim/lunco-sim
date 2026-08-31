// Graphics acceptance for the composed battery mount presentation. The
// geometry and clearance verdict lives in battery_mounts.rhai; this companion
// confirms that the canonical battery and solar-panel children are both
// active in the rendered composition used for review captures.
const TEST_KIND = "graphics";
const ROOT = "/BatteryMountsVisual/";

fn query_ok(record) {
    record != () && record.ok != false
}

fn visible_rover_ok(name) {
    let battery_path = ROOT + name + "/Battery";
    let solar_path = ROOT + name + "/SolarPanel";
    let battery = query("QueryUsdPrim", #{
        path: battery_path,
        attrs: ["xformOp:translate", "xformOp:scale", "info:sourceAsset"],
        children: true,
    });
    let solar = query("QueryUsdPrim", #{
        path: solar_path,
        attrs: ["xformOp:translate", "xformOp:scale", "info:sourceAsset"],
        children: true,
    });
    query_ok(battery) && query_ok(solar)
        && battery.spawned == true && solar.spawned == true
        && battery.attrs["info:sourceAsset"] != ()
        && solar.attrs["info:sourceAsset"] != ()
        && battery.children.contains(battery_path + "/Body")
        && solar.children.contains(solar_path + "/PanelSurface")
}

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    this.done = true;
    let failures = [];
    for name in ["Skid", "SixWheel", "RockerBogie"] {
        failures.push(t_true(visible_rover_ok(name),
            ROOT + name + " is missing its canonical battery or solar-panel geometry"));
    }
    report_verdict(failures, "BATTERY MOUNTS VISUAL", "BATTERY_MOUNTS_VISUAL");
}
