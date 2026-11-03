// @timing presentation
// Observe the native support consumer alongside a source-backed vehicle model.
fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness", "CosimStatus", "QueryPhysicsState"], required_inputs: []}
}
fn on_start(me, params) {
    this.done = false;
    this.path = params.subject_path;
    this.minimum = params.minimum_support_contacts;
}
fn on_tick(me, params) {
    if this.done { return; }
    this.done = true;
    let ready = query("GetReadiness");
    let target = find_path(this.path);
    let body = query("QueryPhysicsState", #{id: target});
    let model_running = false;
    for row in query("CosimStatus", #{include_values: false}).entities {
        if row.name == this.path && row.status == "Running" && row.modelica_current_time > 0.0 {
            model_running = true;
        }
    }
    let valid_probes = body.support_contacts.len() >= this.minimum;
    for probe in body.support_contacts {
        valid_probes = valid_probes && probe.radius_m > 0.0 && probe.probe_length_m > 0.0;
    }
    let checks = [
        t_true(ready.ready && !ready.faulted && ready.pending_count == 0,
            "the source-backed vehicle reaches clear readiness"),
        t_true(model_running, "the source-backed vehicle model is advancing"),
        t_true(body.physics_state_ready && !body.physics_state_pending && !body.physics_initialization_pending,
            "native body admission is complete"),
        t_true(body.support_footprint_count >= this.minimum,
            "native wheel support geometry is published before reporting the vehicle ready"),
        t_true(body.support_sample_tick != () && body.support_sample_tick > 0,
            "the native support consumer publishes current contact samples"),
        t_true(valid_probes, "the support footprint contains real wheel probes")
    ];
    emit("NATIVE_VEHICLE_PROJECTION_EVIDENCE", #{subject: this.path,
        footprint_count: body.support_footprint_count, sample_tick: body.support_sample_tick});
    report_verdict(checks, "NATIVE VEHICLE PROJECTION", "NATIVE_VEHICLE_PROJECTION");
}
