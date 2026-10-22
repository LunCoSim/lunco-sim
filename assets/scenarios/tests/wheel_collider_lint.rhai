// Exercise the public lint surface over real composed and admitted colliders.
const ROOT = "/WheelColliderLint";
const MESH = ROOT + "/RoverPhysical/Suspension_FL/Wheel_FL";

fn simulation_dependencies(me, ctx) {
    let physical = find_path(ROOT + "/RoverPhysical");
    let raycast = find_path(ROOT + "/RoverRaycast");
    let reads = vessel_descendants(physical);
    for entity in vessel_descendants(raycast) { reads.push(entity); }
    #{ modelica_entities: [physical, raycast], entity_reads: reads,
       entity_writes: [], query_reads: ["GetDiagnostics", "GetReadiness"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.phase = "start";
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }
    this.elapsed += dt();
    if this.phase == "start" {
        cmd("RunLint", #{ domain: "usd" });
        this.phase = "wait";
        return;
    }
    let report = query("GetDiagnostics", #{ scope: "loaded_stages" });
    if report == () || !report.complete {
        if this.elapsed < 10.0 { return; }
        this.phase = "done";
        fail_fast("Wheel collider lint did not produce a terminal report",
            "WHEEL COLLIDER LINT", "WHEEL_COLLIDER_LINT");
        return;
    }
    this.phase = "done";
    let warnings = [];
    for diagnostic in report.diagnostics {
        if diagnostic.code == "wheel-mesh-rolling-contact" {
            warnings.push(diagnostic);
        }
    }
    let physical = vessel_wheel_summary(find_path(ROOT + "/RoverPhysical"));
    let raycast = vessel_wheel_summary(find_path(ROOT + "/RoverRaycast"));
    let ready = query("GetReadiness", #{});
    let checks = [
        t_true(warnings.len() == 1,
            "only the enabled mesh wheel collider receives the rolling-contact warning; analytic wheels, visual meshes and chassis geometry do not"),
        t_true(physical.physical == 4 && raycast.raycast == 4,
            "mesh collision remains supported alongside analytic and raycast wheels"),
        t_true(ready.ready && !ready.world_hold && !ready.faulted,
            "the mesh warning does not reject or hold simulation admission"),
    ];
    if warnings.len() == 1 {
        checks.push(t_true(warnings[0].subject == MESH,
            "warning identifies the actual collision prim"));
        checks.push(t_true(warnings[0].severity == "warning"
            && warnings[0].message.contains("analytic Cylinder")
            && warnings[0].message.contains("raycast"),
            "warning explains the instability risk and supported alternatives"));
    }
    report_verdict(checks, "WHEEL COLLIDER LINT", "WHEEL_COLLIDER_LINT");
}
