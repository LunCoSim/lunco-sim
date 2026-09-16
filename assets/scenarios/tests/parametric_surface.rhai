// Asset-level acceptance for the authored USD parametric-surface contract.
// The scene references the shipped NurbsPatch prims directly; this observer
// checks their composed schema and parameters through the public USD query.

fn query_ok(record) {
    record != () && record.ok != false && record.attrs != () && record.api_schemas != ()
}

fn has_schema(record, name) {
    query_ok(record) && record.api_schemas.contains(name)
}

fn on_start(me, ctx) {
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }

    let reflector = query("QueryUsdPrim", #{
        path: "/ParametricSurfaceTest/AntennaReflector",
        attrs: [
            "lunco:lathe:profile", "lunco:lathe:apexRadius",
            "lunco:lathe:rimRadius", "lunco:lathe:focalLength", "vVertexCount",
        ],
        schemas: true,
    });
    let nozzle = query("QueryUsdPrim", #{
        path: "/ParametricSurfaceTest/LanderNozzle",
        attrs: [
            "lunco:lathe:profile", "lunco:lathe:throatRadius",
            "lunco:lathe:exitRadius", "lunco:lathe:length",
            "lunco:lathe:contour", "vVertexCount",
        ],
        schemas: true,
    });

    let checks = [];
    checks.push(t_true(query_ok(reflector) && reflector.type_name == "NurbsPatch"
        && has_schema(reflector, "LunCoLatheAPI")
        && reflector.attrs["lunco:lathe:profile"] == "paraboloid"
        && reflector.attrs["lunco:lathe:apexRadius"] == 0.02
        && reflector.attrs["lunco:lathe:rimRadius"] == 0.58
        && reflector.attrs["lunco:lathe:focalLength"] == 0.35
        && reflector.attrs["vVertexCount"] == 4,
        "shipped antenna reflector lost its composed LunCoLatheAPI contract"));
    checks.push(t_true(query_ok(nozzle) && nozzle.type_name == "NurbsPatch"
        && has_schema(nozzle, "LunCoLatheAPI")
        && nozzle.attrs["lunco:lathe:profile"] == "bell"
        && nozzle.attrs["lunco:lathe:throatRadius"] == 0.35
        && nozzle.attrs["lunco:lathe:exitRadius"] == 1.35
        && nozzle.attrs["lunco:lathe:length"] == 1.90
        && nozzle.attrs["lunco:lathe:contour"] == 0.55
        && nozzle.attrs["vVertexCount"] == 4,
        "shipped lander nozzle lost its composed LunCoLatheAPI contract"));

    this.done = true;
    report_verdict(checks, "PARAMETRIC SURFACE", "PARAMETRIC_SURFACE");
}
