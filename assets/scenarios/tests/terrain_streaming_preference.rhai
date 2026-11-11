// Runtime checks for the persisted global terrain event preference.
const TEST_KIND = "headless";

fn default_verdict(me) {
    report_verdict([
        t_true(get_setting("TerrainSettings.show_streaming_events") == false,
            "terrain streaming messages are off by default")
    ], "TERRAIN STREAMING PREFERENCE", "TERRAIN_STREAMING_PREFERENCE");
}

fn task(me, ctx) { seq([once(Fn("default_verdict"))]) }

fn verify_and_set(initial, desired) {
    let field = "TerrainSettings.show_streaming_events";
    if get_setting(field) != initial { throw "wrong initial terrain event preference"; }
    if !set_setting(field, desired) { throw "cannot update terrain event preference"; }
    if get_setting(field) != desired { throw "updated terrain event preference is missing"; }
    print("PREFERENCE_OK");
}
