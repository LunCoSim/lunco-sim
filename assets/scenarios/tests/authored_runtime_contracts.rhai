// Positive production coverage for the authored scene/runtime boundary.
// USD is checked through QueryUsdPrim; collision and presentation are checked
// through the public runtime query surfaces.

const ROOT = "/AuthoredRuntimeContracts";

fn query_ok(record) {
    record != () && record.ok != false
}

fn on_start(me) {
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }

    let camera = query("QueryUsdPrim", #{
        path: ROOT + "/VisualCamera",
        attrs: ["focalLength", "xformOp:translate"],
    });
    let camera_track = query("QueryUsdPrim", #{
        path: ROOT + "/CameraTrack",
    });
    let sun = query("QueryUsdPrim", #{
        path: ROOT + "/Sun",
        attrs: ["inputs:intensity", "xformOp:rotateXYZ"],
    });
    let ground = query("QueryUsdPrim", #{
        path: ROOT + "/Ground",
        attrs: ["physics:collisionEnabled"],
    });
    let payload = query("QueryUsdPrim", #{
        path: ROOT + "/Payload",
        attrs: ["size", "physics:mass", "physics:collisionEnabled"],
    });
    let joint = query("QueryUsdPrim", #{
        path: ROOT + "/PayloadJoint",
        rels: ["physics:body0", "physics:body1"],
    });
    let ramp = query("QueryUsdPrim", #{
        path: ROOT + "/Ramp",
        attrs: ["xformOp:scale", "physics:collisionEnabled"],
    });
    let ray = query("Raycast", #{
        origin: [0.0, 10.0, 0.0],
        dir: [0.0, -1.0, 0.0],
        max: 20.0,
    });
    let ground_height = query("GroundHeight", #{
        x: 0.0,
        z: 0.0,
        from: 10.0,
        max: 20.0,
    });
    let checks = [];
    checks.push(t_true(query_ok(camera) && camera.type_name == "Camera"
        && camera.attrs["focalLength"] == 24.0
        && camera.attrs["xformOp:translate"] == [8.0, 5.0, 10.0],
        "authored camera is missing or its composed pose/focal length changed"));
    checks.push(t_true(query_ok(camera_track) && camera_track.type_name == "Scope"
        && camera.spawned == true,
        "authored camera track or projected camera is missing"));
    checks.push(t_true(query_ok(sun) && sun.type_name == "DistantLight"
        && sun.attrs["inputs:intensity"] == 128000.0,
        "authored scene light is missing or lost its composed physical intensity"));
    checks.push(t_true(query_ok(ground)
        && ground.attrs["physics:collisionEnabled"] == true,
        "authored ground collider is missing or disabled"));
    checks.push(t_true(query_ok(payload)
        && payload.attrs["size"] == 1.0
        && payload.attrs["physics:mass"] == 1.0
        && payload.attrs["physics:collisionEnabled"] == true
        && find(ROOT + "/Payload") >= 0,
        "composed payload geometry/collision did not reach the production world"));
    checks.push(t_true(query_ok(joint)
        && joint.relationships["physics:body0"] == [ROOT + "/Anchor"]
        && joint.relationships["physics:body1"] == [ROOT + "/Payload"],
        "authored payload joint does not bind the declared rigid bodies"));
    checks.push(t_true(query_ok(ramp)
        && ramp.attrs["xformOp:scale"] == [4.0, 1.0, 2.0]
        && ramp.attrs["physics:collisionEnabled"] == true,
        "authored clearance obstacle is missing or its collision shape changed"));
    checks.push(t_true(ray != () && ray.hit == true,
        "production raycast did not observe the authored ground collider"));
    checks.push(t_true(ground_height != () && ground_height.hit == true
        && ground_height.height.abs() < 0.1,
        "production ground-height query did not observe the authored surface"));

    let ray_hit = ray != () && ray.hit == true;
    let ground_hit = ground_height != () && ground_height.hit == true;
    print("[authored-runtime] camera_track_present=" + query_ok(camera_track)
        + " ray_hit=" + ray_hit + " ground_hit=" + ground_hit);
    this.done = true;
    report_verdict(checks, "AUTHORED RUNTIME CONTRACTS", "AUTHORED_RUNTIME_CONTRACTS");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["authored runtime contract fixture stopped before its public observations"],
            "AUTHORED RUNTIME CONTRACTS", "AUTHORED_RUNTIME_CONTRACTS");
    }
}
