import "lunco://scripting/tools/asset_preflight" as preflight;

// The shader schema is authored in WGSL, and the material binding is authored
// in USD. Keep this acceptance contract beside those assets: Rust owns only
// the generic ParamSchema mechanism and ValidateAsset exposes its result.
//
// This test deliberately checks the public validation surface rather than
// reading shader files from a Rust integration target. A missing field or an
// oversized uniform block therefore fails through the same boundary used by
// the editor and the production pre-flight command.

fn shader_report(path, reports) {
    reports[path]
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

fn schema_checks(path, fields, reports) {
    let report = shader_report(path, reports);
    let failures = [];
    failures.push(t_true(report != (), path + " returned no ValidateAsset report"));
    failures.push(t_true(report != () && report.kind == "wgsl",
        path + " was not classified as WGSL"));
    failures.push(t_true(report != () && report.ok == true,
        path + " failed standalone WGSL schema validation"));
    // ValidateAsset exposes uniform_size as UInt; this bounded threshold keeps
    // the <= 256 result exact after explicit conversion and avoids mixed types.
    failures.push(t_true(report != () && report.info.uniform_size != ()
        && to_float(report.info.uniform_size) <= 256.0,
        path + " exceeds the 256-byte material uniform contract"));
    for field in fields {
        failures.push(t_true(has_param(report, field.name, field.type),
            path + " is missing reflected " + field.name + ": " + field.type));
    }
    failures
}

fn empty_usd_lint_facts(shader_interface) {
    let facts = #{};
    facts.bodies = [];
    facts.joints = [];
    facts.filtered_pairs = [];
    facts.collision_groups = [];
    facts.prims = [];
    facts.runtime_connections = [];
    facts.runtime_port_collisions = [];
    facts.runtime_joints = [];
    facts.control_bindings = [];
    facts.vehicle_parts = [];
    facts.unsupported_program_prims = [];
    facts.connector_programs = [];
    facts.collections = [];
    facts.network_roots = [];
    facts.drives = [];
    facts.gear_drives = [];
    facts.wheel_attachments = [];
    facts.invalid_wheel_attachments = [];
    facts.collision_enabled_without_api = [];
    facts.telemetry_declarations = [];
    facts.shader_interfaces = [shader_interface];
    let stage = #{};
    stage.meters_per_unit_authored = true;
    stage.fixed_hz = 60.0;
    stage.physics_substeps = 1;
    stage.substep_dt = 1.0 / 60.0;
    stage.default_prim = "";
    stage.root_spawnable = false;
    facts.stage = stage;
    facts
}

fn continuation_facts(expected, declared, actual, reflected, valid, shader) {
    let facts = #{};
    facts.expected_interface = expected;
    facts.declared_interface = declared;
    facts.actual_interface = actual;
    facts.source_reflected = reflected;
    facts.source_valid = valid;
    facts.shader_path = shader;
    facts
}

fn shader_interface_issue(expected) {
    let issue = #{};
    issue.subject = "/ShaderAssetContracts/Body/Shader";
    issue.shader_path = "lunco://shaders/blueprint.wgsl";
    issue.declared_interface = expected;
    issue.actual_interface = "";
    issue.source_reflected = true;
    issue.source_valid = true;
    issue.continuation_required = true;
    issue.expected_interface = expected;
    issue
}

fn shader_interface_policy_checks() {
    let expected = "lunco.lunar-surface-continuation.v1";
    let good_facts = continuation_facts(expected, expected, expected, true, true,
        "lunco://shaders/terrain_layered.wgsl");
    let good = invoke_hook("terrain.lunar_surface_continuation", [good_facts]);
    let bad_facts = continuation_facts(expected, expected, "", true, true,
        "lunco://shaders/blueprint.wgsl");
    let bad = invoke_hook("terrain.lunar_surface_continuation", [bad_facts]);
    let issue = shader_interface_issue(expected);
    let lint = invoke_hook("lint.usd", [empty_usd_lint_facts(issue)]);
    let lint_finding = false;
    if lint != () && lint.status == "ok" && lint.value != () {
        for finding in lint.value {
            if finding.rule == "usd-shader-interface-contract"
                && finding.subject == issue.subject {
                lint_finding = true;
            }
        }
    }
    [
        t_true(good != () && good.status == "ok"
            && good.value.action == "compose",
            "a reflected matching USD/WGSL interface did not compose"),
        t_true(bad != () && bad.status == "ok"
            && bad.value.action == "hold",
            "a reflected interface mismatch did not use the default hold policy"),
        t_true(lint != () && lint.status == "ok" && lint_finding,
            "USD lint did not report the required shader-interface mismatch"),
    ]
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ValidateAsset"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.preflight = preflight::create(["lunco://shaders/blueprint.wgsl", "lunco://shaders/solar_panel.wgsl", "lunco://shaders/terrain_layered.wgsl", "lunco://shaders/terrain_geomorph.wgsl", "lunco://shaders/terrain_debug.wgsl", "lunco://shaders/regolith.wgsl", "lunco://shaders/terrain_shadow.wgsl"]);
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.1 { return; }
    this.preflight = preflight::advance(this.preflight);
    if this.preflight.error != () || this.elapsed > 30.0 {
        this.done = true;
        report_verdict([if this.preflight.error != () { this.preflight.error } else { "preflight preparation timed out" }], "SHADER ASSET CONTRACTS", "SHADER_ASSET_CONTRACTS");
        return;
    }
    if !this.preflight.done { return; }

    let failures = [];
    for failure in schema_checks("lunco://shaders/blueprint.wgsl", [
        #{ name: "surface_color", type: "Vec3" },
        #{ name: "transition", type: "F32" },
        #{ name: "major_grid_spacing", type: "F32" },
        #{ name: "blueprint_origin", type: "Vec3" },
        #{ name: "blueprint_frame_origin", type: "Vec3" },
        #{ name: "blueprint_frame_rotation", type: "Vec4" },
        #{ name: "terrain_half_extent", type: "F32" },
        #{ name: "site_blend_widths_m", type: "Vec4" },
        #{ name: "site_weight_albedo", type: "F32" },
        #{ name: "site_weight_rough", type: "F32" },
        #{ name: "site_weight_ao", type: "F32" },
        #{ name: "site_base_color", type: "Vec4" },
        #{ name: "site_photometry", type: "Vec4" },
    ], this.preflight.reports) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/solar_panel.wgsl", [
        #{ name: "seamless_u", type: "F32" },
        #{ name: "v_scale", type: "F32" },
    ], this.preflight.reports) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/terrain_layered.wgsl", [
        #{ name: "micro_albedo", type: "F32" },
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
        #{ name: "terrain_geometry_on", type: "F32" },
    ], this.preflight.reports) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/terrain_geomorph.wgsl", [
        #{ name: "micro_albedo", type: "F32" },
        #{ name: "weight_albedo", type: "F32" },
        #{ name: "map_texel_size_m", type: "F32" },
        #{ name: "derived_surface_on", type: "F32" },
        #{ name: "authored_surface_on", type: "F32" },
        #{ name: "terrain_geometry_on", type: "F32" },
    ], this.preflight.reports) { failures.push(failure); }
    for failure in schema_checks("lunco://shaders/terrain_debug.wgsl", [
        #{ name: "mode", type: "F32" },
        #{ name: "opacity", type: "F32" },
        #{ name: "safe_rad", type: "F32" },
        #{ name: "cliff_rad", type: "F32" },
        #{ name: "lod", type: "F32" },
    ], this.preflight.reports) { failures.push(failure); }
    for path in [
        "lunco://shaders/regolith.wgsl",
        "lunco://shaders/terrain_shadow.wgsl",
    ] {
        let report = shader_report(path, this.preflight.reports);
        failures.push(t_true(report != () && report.ok == true,
            path + " failed standalone WGSL schema validation"));
        failures.push(t_true(has_param(report, "shadow_cache_on", "F32"),
            path + " is missing shadow_cache_on"));
        failures.push(t_true(has_param(report, "horizon_march_steps", "F32"),
            path + " is missing horizon_march_steps"));
    }
    for failure in shader_interface_policy_checks() { failures.push(failure); }

    this.done = true;
    report_verdict(failures, "SHADER ASSET CONTRACTS", "SHADER_ASSET_CONTRACTS");
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["shader asset contract stopped before validation"],
            "SHADER ASSET CONTRACTS", "SHADER_ASSET_CONTRACTS");
    }
}
