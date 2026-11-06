// Render-owner admission is observable through ordinary typed reflection.
// Pixel equality and native GPU timing are separate acceptance gates.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [find_path(ctx.camera_path)],
      entity_writes: [], query_reads: [], required_inputs: []}
}
fn shadow_raster_checks(ctx) {
    let camera = find_path(ctx.camera_path);
    let count = get(camera, "DirectionalShadowRaster.cropped_cascades");
    let raster = get(camera, "DirectionalShadowRaster.rasterized_texels");
    let full = get(camera, "DirectionalShadowRaster.full_texels");
    let reason = get(camera, "DirectionalShadowRaster.reason");
    [camera >= 0, full > 0,
     if ctx.bounded { count > 0 && raster < full }
        else { count == 0 && raster == full }, reason == ctx.reason]
}
fn assert_shadow_raster(ctx) {
    let checks = shadow_raster_checks(ctx);
    for index in 0..checks.len {
        if !checks[index] { throw "shadow raster check failed: " + index; }
    }
    true
}
fn on_start(me, ctx) {
    let checks = shadow_raster_checks(ctx);
    report_verdict([
        t_true(checks[0], "camera must belong to the current scene"),
        t_true(checks[1], "camera must own native directional shadow cascades"),
        t_true(checks[2], "raster admission must match the current receiver contract"),
        t_true(checks[3], "render admission must expose its reason"),
    ], "SHADOW RECEIVER RASTER", "SHADOW_RECEIVER_RASTER");
}


// Native zero jitter leaves projection and image quality unchanged while the
// renderer conservatively declines receiver bounds for a temporal camera.
fn set_shadow_test_control(camera_path, full) {
    let camera = find_path(camera_path);
    if camera < 0 { throw "shadow-control camera missing"; }
    if full {
        add(camera, "TemporalJitter", #{offset: [0.0, 0.0]})
    } else {
        remove(camera, "TemporalJitter")
    }
}

// Move the live test camera without authoring its USD source, so each pixel
// comparison exercises a different receiver rectangle.
fn set_shadow_test_camera_x(camera_path, x) {
    let camera = find_path(camera_path);
    if camera < 0 { throw "shadow-control camera missing"; }
    if !set(camera, "Transform.translation.x", x) { throw "shadow-control camera move failed"; }
    true
}
