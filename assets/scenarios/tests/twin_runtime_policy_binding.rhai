// Exercise inline policy ownership on a real Twin-owned scenario host.
fn on_start(me, ctx) {
    let result = bind_policy("camera.default_presentation", "decide",
        "fn decide(facts) { \"none\" }");
    if !result.ok { throw "Twin policy binding failed: " + result.error; }
}

fn on_stop(me, ctx) {
    let result = bind_policy("camera.default_presentation", "decide",
        "fn decide(facts) { \"first\" }");
    if result.ok { throw "retired Twin callback recreated a policy binding"; }
}
