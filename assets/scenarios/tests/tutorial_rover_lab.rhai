//! Exercise the shipped workshop through its public actions and measured state.
//! The driver supplies learner input; only the lesson decides progression.
fn simulation_dependencies(me, ctx) {
    let rover = find_path("/RoverLab/Rover");
    #{modelica_entities: [rover], entity_reads: vessel_descendants(find_path("/RoverLab")),
      entity_writes: [rover, find_path("/RoverLab/BuildBlock")],
      query_reads: ["QueryPhysicsState", "SensorOccupants", "ListOpenDocuments", "QueryUsdPrim", "InspectUsdDocument"], required_inputs: []}
}
fn action(id) { emit("tutorial.action", "rover_lab:" + id); }
fn require(result) { if result == () || !result.ok { throw result.to_string(); } }
fn on_start(me, ctx) {
    this.rover = find_path("/RoverLab/Rover"); this.sensor = find_path("/RoverLab/Flag/Trigger");
    this.doc = assembly_edit::document_for_entity(this.rover);
    this.phase = 0; this.step = 0; this.time = 0.0; this.at = 0.0;
    this.samples = []; this.checks = []; this.done = false; this.ready_to_save = false; this.saved = false;
}
fn on_event(me, evt, ctx) {
    if evt.name == "ROVER_LAB_STEP" { this.step = evt.value.step; }
    else if evt.name == "ROVER_LAB_SAMPLE" { this.samples.push(evt.value); }
    else if evt.name == "ROVER_LAB_READY_TO_SAVE" { this.ready_to_save = true; }
    else if evt.name == "ROVER_LAB_SAVED" { this.saved = true; }
}
fn inside(rover, sensor) {
    for entrant in query("SensorOccupants", #{sensor: sensor}).entrants {
        let current = entrant;
        for depth in 0..32 {
            if current == rover { return true; }
            current = parent(current); if current == () || current < 0 { break; }
        }
    }
    false
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.time += dt();
    if this.time > 90.0 {
        this.done = true;
        this.checks.push(t_true(false, "workshop timed out in phase " + this.phase));
        report_verdict(this.checks, "Lunar Rover Lab", "TUTORIAL_ROVER_LAB"); return;
    }
    if this.phase == 0 {
        action("run"); action("check_stop"); action("explain_higher");
        emit("SENSOR_ENTER", this.rover);
        this.at = this.time; this.phase = 1; return;
    }
    if this.phase == 1 {
        if this.time - this.at < 0.25 { return; }
        this.checks.push(t_true(this.step == 1 && this.samples.len() == 0,
            "early run, stop, explanation and unrelated sensor events cannot skip driving"));
        require(cmd("ClaimControl", #{target: this.rover}));
        require(modelica_waypoint(this.rover, world_pos(find_path("/RoverLab/Flag")), 0.7, 0.7, 0.0));
        this.phase = 2; return;
    }
    if this.phase == 2 {
        if !inside(this.rover, this.sensor) { return; }
        require(brake(this.rover)); this.phase = 3; return;
    }
    if this.phase == 3 {
        let speed = vlen(query("QueryPhysicsState", #{id: this.rover}).linear_velocity_mps);
        if speed >= 0.15 { return; }
        action("check_stop"); this.phase = 4; return;
    }
    if this.phase == 4 {
        if this.step != 2 { return; }
        this.checks.push(t_true(true, "the controlled rover arrived and stopped at the exact waypoint"));
        action("check_signal"); this.at = this.time; this.phase = 5; return;
    }
    if this.phase == 5 {
        if this.time - this.at < 0.25 { return; }
        this.checks.push(t_true(this.step == 2, "signal assessment cannot pass before opening the graph"));
        action("graph"); this.at = this.time; this.phase = 51; return;
    }
    if this.phase == 51 {
        if this.time - this.at < 0.25 { return; }
        action("check_signal"); this.phase = 6; return;
    }
    if this.phase == 6 {
        if this.step != 3 { return; }
        this.checks.push(t_true(true, "the rover's recorded speed contains real motion"));
        action("run"); this.at = this.time; this.phase = 7; return;
    }
    if this.phase == 7 {
        if this.time - this.at < 0.25 { return; }
        this.checks.push(t_true(this.samples.len() == 0, "comparison requires a prediction"));
        action("predict_lower"); this.at = this.time; this.phase = 71; return;
    }
    if this.phase == 71 {
        if this.time - this.at < 0.25 { return; }
        action("run"); this.phase = 8; return;
    }
    if this.phase == 8 {
        if this.samples.len() < 2 { return; }
        let a = this.samples[0]; let b = this.samples[1];
        this.checks.push(t_true(a.trial == "baseline" && b.trial == "changed_demand"
            && a.speed_mps > 0.01 && b.speed_mps > 0.01 && abs(b.mass_kg - a.mass_kg) < 0.001,
            "both ordered trials move the real rover and mass remains unchanged"));
        let difference = b.speed_mps - a.speed_mps;
        this.observed = if abs(difference) < 0.02 { "similar" } else if difference < 0.0 { "lower" } else { "higher" };
        action(if this.observed == "higher" { "explain_lower" } else { "explain_higher" });
        this.at = this.time; this.phase = 9; return;
    }
    if this.phase == 9 {
        if this.time - this.at < 0.25 { return; }
        this.checks.push(t_true(this.step == 4 && !this.ready_to_save,
            "an explanation contrary to the measurements cannot pass"));
        action("explain_" + this.observed); this.phase = 10; return;
    }
    if this.phase == 10 {
        if this.step != 5 { return; }
        action("save"); this.at = this.time; this.phase = 11; return;
    }
    if this.phase == 11 {
        if this.time - this.at < 0.25 { return; }
        this.checks.push(t_true(!this.ready_to_save && !this.saved,
            "saving cannot pass before the authored block reaches its bay"));
        require(set_world_pos(find_path("/RoverLab/BuildBlock"), [9.0, 0.5, 3.0]));
        this.at = this.time; this.phase = 111; return;
    }
    if this.phase == 111 {
        if this.time - this.at < 0.25 { return; }
        this.checks.push(t_true(!this.ready_to_save,
            "a disposable runtime move cannot pass persistent block placement"));
        require(assembly_edit::transform(this.doc, "@root@", "/RoverLab/BuildBlock",
            [9.0, 0.5, 3.0], (), (), usd_document_generation(this.doc)));
        this.phase = 12; return;
    }
    if this.phase == 12 {
        if !this.ready_to_save { return; }
        let report = query("QueryUsdPrim", #{doc_id: this.doc, path: "/RoverLab/Results/Report",
            attrs: ["inputs:baseline_speed_mps", "inputs:changed_speed_mps"]});
        if report == () || report.doc_id == () { return; }
        let doc = query("InspectUsdDocument", #{doc_id: report.doc_id});
        this.checks.push(t_true(report.attrs["inputs:baseline_speed_mps"] == this.samples[0].speed_mps
            && report.attrs["inputs:changed_speed_mps"] == this.samples[1].speed_mps,
            "the participant document records the measured f64 results exactly"));
        this.checks.push(t_true(doc.dirty && !this.saved,
            "preparing a report does not claim a successful save"));
        this.done = true;
        report_verdict(this.checks, "Lunar Rover Lab", "TUTORIAL_ROVER_LAB");
    }
}
