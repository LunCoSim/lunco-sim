// ── Battery case geometry and mount regression ─────────────────────────────
//
// Runs against `scenes/tests/battery_mounts.usda`. USD owns the component
// dimensions and host transforms; this scenario only observes the composed
// result through the public QueryUsdPrim surface.

const EPS = 0.0001;
const WAIT_S = 30.0;

fn close_vec(actual, expected) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if (actual[i] - expected[i]).abs() > EPS {
            return false;
        }
    }
    true
}

fn battery_ok(root, expected_translate, expected_scale) {
    let path = "/BatteryMountsTest/" + root + "/Battery";
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: ["xformOp:translate", "xformOp:scale", "physics:mass"],
        children: true,
    });
    if record == () || record.ok == false || record.attrs == () || record.children == () {
        return false;
    }
    close_vec(record.attrs["xformOp:translate"], expected_translate)
        && close_vec(record.attrs["xformOp:scale"], expected_scale)
        && record.attrs["physics:mass"] == 50.0
        && record.children.contains(path + "/Body")
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.cases = [
        ["Skid", [0.0, 0.25, 0.0], [1.2, 0.2, 2.0]],
        ["SixWheel", [0.0, 0.25, 0.0], [1.2, 0.2, 2.0]],
        ["RockerBogie", [0.0, 0.35, 0.0], [1.0, 0.3, 1.5]],
        ["Rucheyok", [0.0, 2.0, 0.0], [4.0, 0.8, 6.0]],
    ];
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    let failures = [];
    let ready = true;
    for spec in this.cases {
        let path = "/BatteryMountsTest/" + spec[0] + "/Battery";
        let record = query("QueryUsdPrim", #{ path: path });
        if record == () || record.ok == false {
            ready = false;
        }
    }
    if !ready {
        if this.elapsed >= WAIT_S {
            this.done = true;
            report_verdict(["composed battery prims were not queryable before the production stage became ready"],
                           "BATTERY MOUNTS", "BATTERY_MOUNTS");
        }
        return;
    }

    for spec in this.cases {
        failures.push(t_true(battery_ok(spec[0], spec[1], spec[2]),
            "/BatteryMountsTest/" + spec[0] + "/Battery has the wrong composed case, mass, or mount"));
    }

    this.done = true;
    report_verdict(failures, "BATTERY MOUNTS", "BATTERY_MOUNTS");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["battery mount test stopped before composed geometry was observed"],
                       "BATTERY MOUNTS", "BATTERY_MOUNTS");
    }
}
