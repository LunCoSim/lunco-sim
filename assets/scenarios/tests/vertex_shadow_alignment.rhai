// Pixel probes check the visible vertex deformation against native Sun shadows.
// The graphics gate requires both positive probe colors and rejects any red.
const TEST_KIND = "graphics";
const CAMERA = "/VertexShadowAlignment/Camera";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [find_path(CAMERA)],
       entity_writes: [find_path(CAMERA)], query_reads: [], required_inputs: [] }
}

fn on_start(me, ctx) {
    let camera = find_path(CAMERA);
    report_verdict([
        t_true(add(camera, "DepthPrepass"), "could not enable native depth prepass"),
        t_true(add(camera, "NormalPrepass"), "could not enable native normal prepass"),
        t_true(add(camera, "MotionVectorPrepass"), "could not enable native motion prepass"),
    ], "VERTEX PREPASS VARIANTS", "VERTEX_PREPASS_VARIANTS");
}
