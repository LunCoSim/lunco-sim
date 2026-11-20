// Shot changes follow the same simulation clock as the USD keyframes.
fn checked(result) {
    if result == () || !result.ok { throw "Regolith haul: " + result; }
}

fn shot(camera, message) {
    checked(set_camera(camera));
    hint(message);
    true
}

fn on_start(me, ctx) {
    hud_perspectives([]);
    guided_hud_actions("tutorials","action",[
        #{id:"haul.replay",label:"Replay haul",enabled:true},
        #{id:"haul.cup",label:"Crater Cup",enabled:true}
    ]);
    shot("/World/Avatar", "Regolith haul · Approaching feeder");
}

fn task(me, ctx) {
    seq([
        wait(4.6),
        once(|m| shot("/World/LoadingCam", "Loading regolith")),
        wait(5.1),
        once(|m| shot("/World/Hauler/TravelCam", "Loaded · Factory bound")),
        wait(5.9),
        once(|m| shot("/World/DeliveryCam", "Factory intake · Unloading")),
        wait(5.0),
        once(|m| { hint("Delivery complete"); true })
    ])
}

fn on_event(me, evt, ctx) {
    if evt.name != "tutorial.action" { return; }
    switch evt.value {
        "haul.replay" => checked(cmd("RestartScene", #{})),
        "haul.cup" => checked(cmd("OpenTwinAsset", #{manifest_asset:"lunco://scenes/luncosim/twin.toml"})),
        _ => ()
    }
}
