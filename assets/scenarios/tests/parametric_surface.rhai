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

    // The registered lathe dimension fields are USD float; compare their
    // native values within one micrometre instead of requiring f64 literals
    // to have the same representation as promoted f32 authored values.
    let checks = [];
    checks.push(t_true(query_ok(reflector) && reflector.type_name == "NurbsPatch"
        && has_schema(reflector, "LunCoLatheAPI")
        && reflector.attrs["lunco:lathe:profile"] == "paraboloid"
        && near(reflector.attrs["lunco:lathe:apexRadius"], 0.02, 0.000001)
        && near(reflector.attrs["lunco:lathe:rimRadius"], 0.58, 0.000001)
        && near(reflector.attrs["lunco:lathe:focalLength"], 0.35, 0.000001)
        && reflector.attrs["vVertexCount"] == 4,
        "shipped antenna reflector lost its composed LunCoLatheAPI contract"));
    checks.push(t_true(query_ok(nozzle) && nozzle.type_name == "NurbsPatch"
        && has_schema(nozzle, "LunCoLatheAPI")
        && nozzle.attrs["lunco:lathe:profile"] == "bell"
        && near(nozzle.attrs["lunco:lathe:throatRadius"], 0.35, 0.000001)
        && near(nozzle.attrs["lunco:lathe:exitRadius"], 1.35, 0.000001)
        && near(nozzle.attrs["lunco:lathe:length"], 1.90, 0.000001)
        && near(nozzle.attrs["lunco:lathe:contour"], 0.55, 0.000001)
        && nozzle.attrs["vVertexCount"] == 4,
        "shipped lander nozzle lost its composed LunCoLatheAPI contract"));

    this.done = true;
    report_verdict(checks, "PARAMETRIC SURFACE", "PARAMETRIC_SURFACE");
}
