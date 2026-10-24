const TEST_KIND = "graphics";
const CAMERA = "/World/Camera";
const LIGHT = "/World/Bulb";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [find_path(CAMERA), find_path(LIGHT)],
       entity_writes: [find_path(CAMERA)], query_reads: [], required_inputs: [] }
}

fn on_start(me, ctx) {
    let camera = find_path(CAMERA);
    let light = find_path(LIGHT);
    report_verdict([
        t_true(get(light, "PointLight.shadow_maps_enabled") == true,
            "the authored point light must render shared shadows"),
        t_true(add(camera, "DepthPrepass"), "could not enable native depth prepass"),
        t_true(add(camera, "NormalPrepass"), "could not enable native normal prepass"),
        t_true(add(camera, "MotionVectorPrepass"), "could not enable native motion prepass"),
    ], "SHARED SHADOW ALLOCATION", "SHARED_SHADOW_ALLOCATION");
}
