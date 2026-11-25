// Cyan terrain agrees with the native light's horizon side; green sky has a
// zero angular radius. scripts/api/test_sun_material_precision.py drives small
// crossings and the authored zero/positive angle boundary through the live API.
const TEST_KIND = "graphics";

fn check_lighting_rejection(doc) {
    let before = usd_document_generation(doc);
    for intent in ["Interactive", "Persistent"] {
        let answer = cmd("SetEnvironmentLight", #{intent:intent, illuminance:-1.0, sun_color:[0.0,1.0,0.0]});
        if answer.status != "rejected" { throw "Invalid lighting request was accepted: " + answer; }
    }
    if usd_document_generation(doc) != before { throw "Rejected lighting changed the document"; }
    print("LIGHT_REJECTION_PASS");
}

fn light_sample() {
    let light = find_path("/World/Sun");
    let direction = world_forward(light);
    [get(light, "DirectionalLight.illuminance"), direction[0], direction[1], direction[2]]
}

fn light_matches(expected) {
    let actual = light_sample();
    for channel in 0..actual.len() {
        if abs(actual[channel] - expected[channel]) > 0.00001 { return false; }
    }
    true
}

fn live_channels_retained(child) {
    let light = find_path("/World/Sun");
    let color = get(light, "DirectionalLight.color.0");
    find_path("/World/Sun/AttachedScope") == child &&
        color.red == 0.25 && color.green == 0.5 && color.blue == 0.75 &&
        get(light, "DirectionalLight.shadow_maps_enabled") == false
}
