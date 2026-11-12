// @timing presentation
// Physical support and camera presentation are independent products. The driver
// exercises a real native-contact drive and supplies the current source/view IDs.
const TEST_KIND = "graphics";
fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [params.body_id], entity_writes: [],
      query_reads: ["TerrainLodStatus", "InspectUsdCurveView", "InspectVehicleTrail"], required_inputs: []}
}
fn on_start(me, params) {
    let status = query("TerrainLodStatus", #{});
    let physical_count = 0;
    let physical_ready = true;
    let lattice_valid = true;
    for contact in status.contacts {
        if contact.source == params.source_raw {
            physical_count += 1;
            physical_ready = physical_ready && contact.ready;
            lattice_valid = lattice_valid && contact.resolution > 1 && contact.min_wavelength_m > 0.0;
        }
    }
    let camera = ();
    for row in status.camera_cover {
        if row.camera == params.camera_raw && row.source == params.source_raw { camera = row; }
    }
    let camera_ready = camera != () && camera.ready && camera.wanted > 0
        && camera.resident == camera.wanted && camera.drawn > 0;
    let near_detail = camera != () && (abs(camera.clearance_m) >= 10.0
        || (camera.pinned && camera.depth == camera.effective_max_depth));
    let route = query("InspectUsdCurveView", #{doc_id: params.doc_id, entity_id: params.curve_id});
    let trail = query("InspectVehicleTrail", #{id: params.body_id});
    let tracks = 0;
    let tracks_valid = true;
    for lane in trail.lanes {
        if lane.sample_count.to_int() > 1 {
            tracks += 1;
            tracks_valid = tracks_valid && lane.projection == "terrain"
                && lane.published_revision != () && lane.error == ()
                && abs(lane.render_width_m-lane.wheel_width_m) < 0.000001;
        }
    }
    report_verdict([
        t_true(physical_count > 0, "physics publishes explicit support for the source"),
        t_true(physical_count > 0 && physical_ready, "physical support remains ready at this graphics quality"),
        t_true(physical_count > 0 && lattice_valid, "physical support reports its authored lattice"),
        t_true(camera != (), "the selected camera has its exact source/view demand"),
        t_true(camera_ready, "the selected view has a resident drawn cover"),
        t_true(near_detail, "a nearby camera retains its own finest geometry"),
        t_true(route.projection == "terrain_surface" && route.error == "",
            "the route is published as a ground-material annotation"),
        t_true(route.displayed_surface_binding_count != () && route.displayed_surface_binding_count > 0,
            "drawn terrain retains the route and wheel-mark texture"),
        t_true(!params.require_trails || (tracks > 0 && tracks_valid),
            "native wheel contacts publish terrain-projected marks at tire width after driving")
    ], "TERRAIN PHYSICS AND CAMERA ANNOTATIONS", "TERRAIN_PHYSICS_CAMERA_ANNOTATIONS");
}
