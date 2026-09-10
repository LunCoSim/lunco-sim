// Negative production coverage: missing scene presentation and a disabled
// collider must remain observable as missing/disabled, not receive a runtime
// default that makes an invalid authored scene look healthy.

const ROOT = "/AuthoredRuntimeContractsNegative";

fn missing(record) {
    record == () || record.ok == false
}

fn on_start(me) {
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }

    let camera = query("QueryUsdPrim", #{ path: ROOT + "/VisualCamera" });
    let sun = query("QueryUsdPrim", #{ path: ROOT + "/Sun" });
    let body = query("QueryUsdPrim", #{
        path: ROOT + "/UncollidableBody",
        attrs: ["physics:collisionEnabled"],
    });
    let body_topology = query("QueryUsdPrim", #{
        path: ROOT + "/UncollidableBody",
        topology: true,
    });
    let disabled_part = ();
    if body_topology != () && body_topology.ok != false && body_topology.topology != () {
        for part in body_topology.topology.parts {
            if part.path == ROOT + "/UncollidableBody" { disabled_part = part; }
        }
    }
    let ray = query("Raycast", #{
        origin: [0.0, 10.0, 0.0],
        dir: [0.0, -1.0, 0.0],
        max: 20.0,
    });
    let unrepresentable_ray = query("Raycast", #{
        origin: [1e308, 10.0, 0.0],
        dir: [0.0, -1.0, 0.0],
        max: 20.0,
    });
    let unrepresentable_ground = query("GroundHeight", #{
        x: 1e308,
        z: 0.0,
        from: 10.0,
        max: 20.0,
    });

    this.done = true;
    report_verdict([
        t_true(missing(camera), "missing camera was fabricated by the runtime"),
        t_true(missing(sun), "missing light was fabricated by the runtime"),
        t_true(body != () && body.ok != false
            && body.attrs["physics:collisionEnabled"] == false,
            "disabled authored collision was not visible through QueryUsdPrim"),
        t_true(disabled_part != ()
            && disabled_part.collider == false
            && disabled_part.collision_enabled == false
            && disabled_part.bounds.frame == "canonical_stage",
            "topology hid the disabled collider fact or its geometry envelope"),
        t_true(ray != () && ray.hit == false,
            "raycast reported a collider for the disabled-only fixture"),
        t_true(unrepresentable_ray != () && unrepresentable_ray.ok != false
            && unrepresentable_ray.hit == false,
            "raycast admitted an origin the Avian spatial backend cannot represent"),
        t_true(unrepresentable_ground != () && unrepresentable_ground.ok != false
            && unrepresentable_ground.hit == false,
            "ground query admitted an origin the Avian spatial backend cannot represent"),
    ], "AUTHORED RUNTIME CONTRACTS NEGATIVE", "AUTHORED_RUNTIME_CONTRACTS_NEGATIVE");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["negative authored contract fixture stopped before its rejection observations"],
            "AUTHORED RUNTIME CONTRACTS NEGATIVE", "AUTHORED_RUNTIME_CONTRACTS_NEGATIVE");
    }
}
