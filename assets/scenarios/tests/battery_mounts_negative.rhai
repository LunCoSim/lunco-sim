// Negative acceptance for the battery envelope check. This fixture overrides
// only the host mount, so a test that checks component identity without
// checking composed placement cannot pass it.

const ROOT = "/BatteryMountsNegative/SixWheel";
const EPS = 0.0001;

fn query_ok(record) {
    record != () && record.ok != false
}

fn on_start(me) {
    this.done = false;
    this.elapsed = 0.0;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.5 { return; }

    let battery = query("QueryUsdPrim", #{
        path: ROOT + "/Battery",
        attrs: ["xformOp:translate", "xformOp:scale"],
    });
    let chassis = query("QueryUsdPrim", #{
        path: ROOT + "/Chassis",
        attrs: ["xformOp:scale"],
    });
    let body = find(ROOT + "/Chassis");
    let battery_entity = find(ROOT + "/Battery");
    if !query_ok(battery) || !query_ok(chassis) || body < 0 || battery_entity < 0 {
        this.done = true;
        report_verdict(["negative battery fixture was not queryable"],
                       "BATTERY MOUNTS NEGATIVE", "BATTERY_MOUNTS_NEGATIVE");
        return;
    }

    let battery_pos = world_pos(battery_entity);
    let body_pos = world_pos(body);
    let battery_half_y = 0.5 * battery.attrs["xformOp:scale"][1];
    let body_half_y = 0.5 * chassis.attrs["xformOp:scale"][1];
    let inside_body = (battery_pos[1] - body_pos[1]).abs() + battery_half_y
        <= body_half_y + EPS;

    this.done = true;
    report_verdict([
        t_true(!inside_body,
               "an out-of-envelope battery mount was accepted")
    ], "BATTERY MOUNTS NEGATIVE", "BATTERY_MOUNTS_NEGATIVE");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["negative battery fixture stopped before the rejection check"],
                       "BATTERY MOUNTS NEGATIVE", "BATTERY_MOUNTS_NEGATIVE");
    }
}
