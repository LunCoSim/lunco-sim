// Attach to sun_material_precision; the GPU host checks green/cyan and no red.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [find_path("/World/Camera")],
      entity_writes: [find_path("/World/Camera")],
      query_reads: ["QueryUsdPrim"], required_inputs: []}
}

fn on_start(me, ctx) {
    let doc = query("QueryUsdPrim", #{path: "/World/Sun"}).doc_id;
    let result = cmd("ApplyUsdTransientOps", #{doc_id: doc,
        label: "Directional response GPU controls", ops: [
        #{SetAttribute: #{edit_target: "@view@", path: "/World/Looks/Ground/Shader",
            name: "info:wesl:sourceAsset", type_name: "asset",
            value: "@lunco://shaders/tests/directional_light_response.wesl@"}},
        #{AddPrim: #{edit_target: "@view@", parent_path: "/World", name: "Fill",
            type_name: "DistantLight", reference: ()}},
        #{SetAttribute: #{edit_target: "@view@", path: "/World/Fill",
            name: "inputs:intensity", type_name: "float", value: "64000"}},
        #{SetAttribute: #{edit_target: "@view@", path: "/World/Fill",
            name: "inputs:angle", type_name: "float", value: "0"}},
        #{SetAttribute: #{edit_target: "@view@", path: "/World/Fill",
            name: "xformOp:rotateXYZ", type_name: "double3", value: "(-40, -40, 0)"}},
        #{SetAttribute: #{edit_target: "@view@", path: "/World/Fill",
            name: "xformOpOrder", type_name: "token[]", value: "[\"xformOp:rotateXYZ\"]"}},
    ]});
    let camera = find_path("/World/Camera");
    this.setup_ok = result.ok;
    this.depth_ok = add(camera, "DepthPrepass");
    this.normal_ok = add(camera, "NormalPrepass");
    this.waiting = true;
}

fn on_tick(me, ctx) {
    if !this.waiting { return; }
    let fill = find_path("/World/Fill");
    if fill < 0 { return; }
    track_entity_write(fill);
    // A preview-scoped light remains visible on the default render layer but
    // is excluded from structural selection of the scene's sole Sun.
    let scoped = add(fill, "RenderLayers");
    this.waiting = false;
    report_verdict([
        t_true(this.setup_ok, "typed transient material/light setup must succeed"),
        t_true(this.depth_ok, "native depth prepass must be enabled"),
        t_true(this.normal_ok, "native normal prepass must be enabled"),
        t_true(scoped, "the fill must retain its native preview-light scope"),
    ], "DIRECTIONAL RESPONSE SETUP", "DIRECTIONAL_RESPONSE_SETUP");
}
