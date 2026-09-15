// The shader schema is authored in WGSL, and the material binding is authored
// in USD. Keep this acceptance contract beside those assets: Rust owns only
// the generic ParamSchema mechanism and ValidateAsset exposes its result.
//
// This test deliberately checks the public validation surface rather than
// reading shader files from a Rust integration target. A missing field or an
// oversized uniform block therefore fails through the same boundary used by
// the editor and the production pre-flight command.

fn shader_report(path) {
    query("ValidateAsset", #{ path: path })
}

fn shader_param(report, name) {
    if report == () || report.info == () || report.info.shader_params == () {
        return ();
    }
    for field in report.info.shader_params {
        if field.name == name { return field; }
    }
    ()
}

fn has_param(report, name, type_name) {
    let field = shader_param(report, name);
    field != () && field.type == type_name
}

fn schema_checks(path, fields) {
    let report = shader_report(path);
    let failures = [];
    failures.push(t_true(report != (), path + " returned no ValidateAsset report"));
    failures.push(t_true(report != () && report.kind == "wgsl",
        path + " was not classified as WGSL"));
    failures.push(t_true(report != () && report.ok == true,
        path + " failed standalone WGSL schema validation"));
    failures.push(t_true(report != () && report.info.uniform_size != ()
        && report.info.uniform_size <= 256,
        path + " exceeds the 256-byte material uniform contract"));
    for field in fields {
        failures.push(t_true(has_param(report, field.name, field.type),
            path + " is missing reflected " + field.name + ": " + field.type));
    }
    failures
}

fn on_start(me) {
    this.done = false;
    this.elapsed = 0.0;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.1 { return; }

    let failures = [];
    for failure in schema_checks("lunco://shaders/blueprint.wgsl", [
        #{ name: "surface_color", type: "Vec3" },
        #{ name: "transition", type: "F32" },
        #{ name: "major_grid_spacing", type: "F32" },
        #{ name: "blueprint_origin", type: "Vec3" },
        #{ name: "blueprint_frame_origin", type: "Vec3" },
        #{ name: "blueprint_frame_rotation", type: "Vec4" },
    ]) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/solar_panel.wgsl", [
        #{ name: "seamless_u", type: "F32" },
        #{ name: "v_scale", type: "F32" },
    ]) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/terrain_layered.wgsl", [
        #{ name: "weight_albedo", type: "F32" },
        #{ name: "weight_mineral", type: "F32" },
        #{ name: "weight_rough", type: "F32" },
        #{ name: "weight_ao", type: "F32" },
        #{ name: "weight_normal", type: "F32" },
        #{ name: "map_texel_size_m", type: "F32" },
        #{ name: "derived_surface_on", type: "F32" },
        #{ name: "derived_normal_on", type: "F32" },
        #{ name: "authored_surface_on", type: "F32" },
        #{ name: "authored_normal_on", type: "F32" },
    ]) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/terrain_geomorph.wgsl", [
        #{ name: "weight_albedo", type: "F32" },
        #{ name: "map_texel_size_m", type: "F32" },
        #{ name: "derived_surface_on", type: "F32" },
        #{ name: "authored_surface_on", type: "F32" },
    ]) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/terrain_debug.wgsl", [
        #{ name: "mode", type: "F32" },
        #{ name: "opacity", type: "F32" },
        #{ name: "safe_rad", type: "F32" },
        #{ name: "cliff_rad", type: "F32" },
        #{ name: "lod", type: "F32" },
    ]) { failures.push(failure); }
    for path in [
        "lunco://shaders/regolith.wgsl",
        "lunco://shaders/terrain_shadow.wgsl",
    ] {
        let report = shader_report(path);
        failures.push(t_true(report != () && report.ok == true,
            path + " failed standalone WGSL schema validation"));
        failures.push(t_true(has_param(report, "shadow_cache_on", "F32"),
            path + " is missing shadow_cache_on"));
        failures.push(t_true(has_param(report, "horizon_march_steps", "F32"),
            path + " is missing horizon_march_steps"));
    }

    this.done = true;
    report_verdict(failures, "SHADER ASSET CONTRACTS", "SHADER_ASSET_CONTRACTS");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["shader asset contract stopped before validation"],
            "SHADER ASSET CONTRACTS", "SHADER_ASSET_CONTRACTS");
    }
}
