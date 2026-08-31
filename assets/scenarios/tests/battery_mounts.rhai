// ── Battery envelope and solar-clearance regression ─────────────────────────
//
// Runs against `scenes/tests/battery_mounts.usda`. USD owns the component
// dimensions and host transforms; this scenario observes the composed result
// through the public QueryUsdPrim and world-pose surfaces. The check is about
// physical placement: the case and its collision shape must fit the chassis,
// remain below any deck, and leave the authored solar-panel envelope clear.

const EPS = 0.0001;
const MIN_SOLAR_CLEARANCE = 0.1;
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

fn query_ok(record) {
    record != () && record.ok != false
}

fn battery_ok(spec) {
    let root = spec[0];
    let root_path = "/BatteryMountsTest/" + root;
    let path = root_path + "/Battery";
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: ["xformOp:translate", "xformOp:scale", "physics:mass", "info:sourceAsset"],
        children: true,
    });
    let body = query("QueryUsdPrim", #{
        path: path + "/Body",
        attrs: ["size", "physics:collisionEnabled"],
    });
    let chassis = query("QueryUsdPrim", #{
        path: root_path + "/" + spec[3],
        attrs: ["xformOp:translate", "xformOp:scale", "size"],
    });
    let solar = query("QueryUsdPrim", #{
        path: root_path + "/SolarPanel",
        attrs: ["xformOp:translate", "xformOp:scale", "info:sourceAsset"],
        children: true,
    });
    let frame = query("QueryUsdPrim", #{
        path: root_path + "/SolarPanel/PanelFrame",
        attrs: ["xformOp:translate", "xformOp:scale"],
    });
    if !query_ok(record) || !query_ok(body) || !query_ok(chassis)
        || !query_ok(solar) || !query_ok(frame)
        || record.attrs == () || record.children == ()
        || body.attrs == () || chassis.attrs == ()
        || solar.attrs == () || solar.children == () || frame.attrs == () {
        return false;
    }

    let battery = find(path);
    let body_entity = find(root_path + "/" + spec[3]);
    let solar_entity = find(root_path + "/SolarPanel");
    let vessel = find(root_path);
    if battery < 0 || body_entity < 0 || solar_entity < 0 || vessel < 0 {
        return false;
    }
    let battery_pos = world_pos(battery);
    let body_pos = world_pos(body_entity);
    let solar_pos = world_pos(solar_entity);
    let electrical_surface = get(vessel, "soc") != ()
        && get(vessel, "solar_power") != ();
    let battery_scale = record.attrs["xformOp:scale"];
    let body_scale = chassis.attrs["xformOp:scale"];
    let body_half = [0.5 * body_scale[0], 0.5 * body_scale[1], 0.5 * body_scale[2]];
    let battery_half = [0.5 * battery_scale[0], 0.5 * battery_scale[1], 0.5 * battery_scale[2]];

    let inside_body = true;
    for i in 0..3 {
        inside_body = inside_body
            && (battery_pos[i] - body_pos[i]).abs() + battery_half[i]
                <= body_half[i] + EPS;
    }

    let frame_translate = frame.attrs["xformOp:translate"];
    let frame_scale = frame.attrs["xformOp:scale"];
    let panel_bottom = solar_pos[1]
        + solar.attrs["xformOp:scale"][1]
            * (frame_translate[1] - 0.5 * frame_scale[1]);
    let battery_top = battery_pos[1] + battery_half[1];
    let solar_clear = panel_bottom - battery_top >= MIN_SOLAR_CLEARANCE;

    let deck_clear = if spec[4] == "" {
        true
    } else {
        let deck = query("QueryUsdPrim", #{
            path: root_path + "/" + spec[4],
            attrs: ["xformOp:translate", "xformOp:scale"],
        });
        if !query_ok(deck) || deck.attrs == () {
            false
        } else {
            let deck_translate = deck.attrs["xformOp:translate"];
            let deck_scale = deck.attrs["xformOp:scale"];
            let deck_entity = find(root_path + "/" + spec[4]);
            if deck_entity < 0 {
                false
            } else {
                let deck_pos = world_pos(deck_entity);
                battery_top <= deck_pos[1] - 0.5 * deck_scale[1] + EPS
            }
        }
    };

    print("[battery] " + root + " battery=" + battery_pos
        + " body=" + body_pos + " solar=" + solar_pos
        + " inside=" + inside_body + " solar_clear=" + solar_clear
        + " deck_clear=" + deck_clear + " electrical=" + electrical_surface);

    close_vec(record.attrs["xformOp:translate"], spec[1])
        && close_vec(battery_scale, spec[2])
        && record.attrs["physics:mass"] == 50.0
        && record.attrs["info:sourceAsset"] != ()
        && record.children.contains(path + "/Body")
        && body.attrs["size"] == 1.0
        && body.attrs["physics:collisionEnabled"] == true
        && chassis.attrs["size"] == 1.0
        && solar.attrs["info:sourceAsset"] != ()
        && solar.children.contains(root_path + "/SolarPanel/PanelFrame")
        && solar.children.contains(root_path + "/SolarPanel/PanelSurface")
        && inside_body && solar_clear && deck_clear && electrical_surface
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.cases = [
        ["Skid", [0.0, 0.0, 0.0], [1.2, 0.2, 2.0], "Chassis", ""],
        ["SixWheel", [0.0, 0.0, 0.0], [1.2, 0.2, 2.0], "Chassis", ""],
        ["RockerBogie", [0.0, 0.0, 0.0], [0.8, 0.3, 1.2], "Body", "SolarDeck"],
        ["Rucheyok", [0.0, 2.0, 0.0], [4.0, 0.8, 6.0], "Chassis", ""],
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
        if !query_ok(record) {
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
        failures.push(t_true(battery_ok(spec),
            "/BatteryMountsTest/" + spec[0]
                + " battery does not fit its body or clear the solar hardware"));
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
