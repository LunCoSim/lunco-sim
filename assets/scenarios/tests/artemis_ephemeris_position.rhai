// Production projection contract for the Artemis scene's composed Orion.
// Run this against assets/scenes/celestial/artemis_2_review.usda.

const ORION = "/Artemis2Review/Artemis2/Orion";
const EARTH = "/Artemis2Review/SolarSystem/Earth";
const EARTH_ORBIT = "/Artemis2Review/SolarSystem/Earth/OrbitView";
const MOON_ORBIT = "/Artemis2Review/SolarSystem/Moon/OrbitView";
const SCENE_ROOT = "/Artemis2Review";

fn ephemeris_check(condition, message) {
    if condition { "" } else { message }
}

fn ephemeris_verdict(checks) {
    let failures = [];
    for check in checks {
        if check != "" { failures.push(check); }
    }
    if failures.is_empty() {
        print("TESTS_OK " + checks.len().to_string());
        print("ARTEMIS EPHEMERIS POSITION: PASS");
        emit("ARTEMIS_EPHEMERIS_POSITION", "PASS");
        return;
    }
    for failure in failures { print("FAIL: " + failure); }
    print("TESTS_FAIL " + failures.len().to_string() + "/" + checks.len().to_string());
    print("ARTEMIS EPHEMERIS POSITION: FAIL");
    emit("ARTEMIS_EPHEMERIS_POSITION", "FAIL");
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    let orion = find(ORION);
    let earth = find(EARTH);
    if orion < 0 || earth < 0 {
        if this.elapsed > 10.0 {
            this.done = true;
            ephemeris_verdict(["Artemis review scene did not project Orion and Earth"]);
        }
        return;
    }

    let authored = query("QueryUsdPrim", #{
        path: ORION,
        schemas: true,
        attrs: [
            "lunco:ephemeris:targetId",
            "lunco:ephemeris:referenceId",
            "lunco:spacecraft:hitRadiusKm",
        ],
    });
    let root = query("QueryUsdPrim", #{
        path: SCENE_ROOT,
        schemas: true,
        attrs: ["lunco:time:epochJd"],
    });
    let earth_orbit = query("QueryUsdPrim", #{
        path: EARTH_ORBIT,
        schemas: true,
        attrs: ["lunco:trajectory:trackedId", "lunco:trajectory:referenceId"],
    });
    let moon_orbit = query("QueryUsdPrim", #{
        path: MOON_ORBIT,
        schemas: true,
        attrs: ["lunco:trajectory:trackedId", "lunco:trajectory:referenceId"],
    });
    let clock = clock_snapshot();
    let entity = query("QueryEntity", #{id: orion});
    if authored == () || entity == () || authored.api_schemas == ()
        || authored.attrs == () || root == () || root.api_schemas == ()
        || root.attrs == () || clock == () || clock.mission_epoch0_jd == ()
        || earth_orbit == () || earth_orbit.api_schemas == ()
        || earth_orbit.attrs == () || moon_orbit == ()
        || moon_orbit.api_schemas == () || moon_orbit.attrs == () {
        if this.elapsed > 10.0 {
            this.done = true;
            ephemeris_verdict(["Orion composed metadata or entity identity was unavailable"]);
        }
        return;
    }

    let has_position_api = false;
    let has_spacecraft_api = false;
    let has_epoch_api = false;
    let has_earth_trajectory_api = false;
    let has_moon_trajectory_api = false;
    for schema in authored.api_schemas {
        if schema == "LunCoEphemerisPositionAPI" { has_position_api = true; }
        if schema == "LunCoSpacecraftAPI" { has_spacecraft_api = true; }
    }
    for schema in root.api_schemas {
        if schema == "LunCoEpochAPI" { has_epoch_api = true; }
    }
    for schema in earth_orbit.api_schemas {
        if schema == "LunCoTrajectoryViewAPI" { has_earth_trajectory_api = true; }
    }
    for schema in moon_orbit.api_schemas {
        if schema == "LunCoTrajectoryViewAPI" { has_moon_trajectory_api = true; }
    }

    let checks = [];
    checks.push(ephemeris_check(has_position_api,
        "Orion is missing the generic ephemeris-position schema"));
    checks.push(ephemeris_check(has_spacecraft_api,
        "Orion model lost its spacecraft interaction identity"));
    checks.push(ephemeris_check(has_epoch_api && root.attrs["lunco:time:epochJd"] == 2461132.58,
        "the scene must author its time inside the selected vector dataset coverage"));
    checks.push(ephemeris_check(clock.mission_epoch0_jd == 2461132.58,
        "the scene root epoch was not admitted to the simulation clock"));
    checks.push(ephemeris_check(has_earth_trajectory_api && has_moon_trajectory_api,
        "authored trajectory views must use only their generic schema"));
    let target_id = authored.attrs["lunco:ephemeris:targetId"];
    let reference_id = authored.attrs["lunco:ephemeris:referenceId"];
    let hit_radius_km = authored.attrs["lunco:spacecraft:hitRadiusKm"];
    checks.push(ephemeris_check(target_id != () && target_id != 0
        && reference_id != () && reference_id != 0,
        "scene must author valid ephemeris target and reference ids"));
    checks.push(ephemeris_check(entity.usd_prim_path == ORION,
        "projected Orion entity no longer identifies its composed USD prim"));
    checks.push(ephemeris_check(get(orion, "EphemerisPosition.target_id") == target_id
        && get(orion, "EphemerisPosition.reference_id") == reference_id,
        "the generic USD ephemeris binding did not project to ECS"));
    checks.push(ephemeris_check(hit_radius_km != () && hit_radius_km >= 0.0
        && get(orion, "Spacecraft.hit_radius_m") == hit_radius_km * 1000.0,
        "the model's authored spacecraft interaction radius did not project"));
    checks.push(ephemeris_check(get(earth, "EphemerisPosition.target_id") == ()
        && get(earth, "Spacecraft.hit_radius_m") == (),
        "an ordinary celestial body acquired spacecraft or ephemeris-position behavior"));

    this.done = true;
    ephemeris_verdict(checks);
}
