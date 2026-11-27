// @timing presentation
// Cross a normal window's retention boundary while observing the actual cover.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [params.camera_id],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn view_for(status, params) {
    for view in status.camera_cover {
        if view.camera == params.camera && view.source == params.source { return view; }
    }
    ()
}

fn contact_keys(status) {
    let keys = [];
    for row in status.contacts {
        if row.ready { keys.push([row.source, row.depth, row.x, row.z, row.resolution, row.min_wavelength_m]); }
    }
    keys
}

fn on_start(me, params) {
    let status = query("TerrainLodStatus", #{include_draw_cover: true});
    let view = view_for(status, params);
    if view == () || view.relief == () || !view.relief.current || !view.ready {
        throw "translation gate requires an admitted starting normal window";
    }
    this.eye = view.position_local_m;
    this.initial_page = view.relief;
    this.contacts = contact_keys(status);
    this.command_ok = cmd("SetCameraLookAt", #{camera: params.camera_id, eye: params.eye, target: params.target}).ok;
    this.cover_ok = true;
    this.page_ok = true;
    this.done = false;
    this.started = clock_snapshot().wall_elapsed_s;
}

fn on_tick(me, params) {
    if this.done == () || this.done { return; }
    let status = query("TerrainLodStatus", #{include_draw_cover: true});
    let view = view_for(status, params);
    let moved = false;
    let complete = false;
    if view != () {
        let dx = view.position_local_m[0] - this.eye[0];
        let dz = view.position_local_m[2] - this.eye[2];
        moved = dx * dx + dz * dz >= params.minimum_distance_m * params.minimum_distance_m;
        this.cover_ok = this.cover_ok && view.draw_cover != () && view.draw_cover.len() > 0;
        this.page_ok = this.page_ok && view.relief != ();
        complete = moved && view.ready && view.relief != () && view.relief.current
            && view.relief.center_m != this.initial_page.center_m;
    } else {
        this.cover_ok = false;
        this.page_ok = false;
    }
    let elapsed = clock_snapshot().wall_elapsed_s - this.started;
    if complete || elapsed > params.timeout_s || !this.command_ok {
        this.done = true;
        let contacts = contact_keys(status);
        let same_contacts = this.contacts.len() > 0 && contacts.len() == this.contacts.len();
        for contact in contacts { same_contacts = same_contacts && this.contacts.contains(contact); }
        report_verdict([
            t_true(this.command_ok && moved, "the camera crossed the normal-window retention boundary"),
            t_true(this.cover_ok, "terrain coverage remains available throughout translation"),
            t_true(this.page_ok, "the previous normal window remains available until replacement"),
            t_true(complete, "a new world-aligned normal window and current geometry publish"),
            t_true(same_contacts, "normal-window translation leaves physical support unchanged"),
            t_true(complete && view.relief.atlas == this.initial_page.atlas,
                "translation retains the same GPU atlas allocation"),
            t_true(complete && view.relief.chunks_reused > 0 && view.relief.chunks_updated < 64
                && view.relief.chunks_reused + view.relief.chunks_updated == 64,
                "translation reuses the overlapping world-aligned chunks"),
            t_true(complete && view.relief.uploaded_patch_bytes != () && view.relief.uploaded_patch_bytes > 0
                && view.relief.uploaded_patch_bytes < this.initial_page.prepared_bytes,
                "the render adapter uploads only the exposed mip regions")
        ], "TERRAIN RELIEF TRANSLATION", "TERRAIN_RELIEF_TRANSLATION");
    }
}
