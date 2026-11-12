// @timing presentation
// A retained visual observer verifies quality changes during scene replacement.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["GetReadiness", "TerrainLodStatus"], required_inputs: [] }
}

fn request_replacement(depth, resolution, budget) {
    let restart = cmd("RestartScene", #{ reset_document: false });
    let quality = cmd("SetTerrainRenderingQuality", #{
        max_depth: depth, tile_resolution: resolution, tile_budget: budget,
    });
    restart != () && restart.ok == true && quality != () && quality.ok == true
}

fn on_start(me, params) {
    let status = query("TerrainLodStatus", #{});
    if status.contacts.len() == 0 {
        throw "restart-quality test requires a resident physical terrain";
    }
    this.source = status.contacts[0].source;
    this.phase = 1;
    this.started = clock_snapshot().wall_elapsed_s;
    this.checks = [t_true(request_replacement(6, 33, 64),
        "restart and constrained quality are admitted together")];
}

fn on_tick(me, params) {
    if this.phase == 3 { return; }
    if clock_snapshot().wall_elapsed_s - this.started > 90.0 {
        report_verdict([t_true(false, "terrain replacement did not settle before the deadline")],
            "TERRAIN RESTART QUALITY", "TERRAIN_RESTART_QUALITY");
        this.phase = 3;
        return;
    }
    let readiness = query("GetReadiness");
    if !readiness.ready || readiness.world_hold { return; }
    let status = query("TerrainLodStatus", #{});
    if status.contacts.len() == 0 || status.contacts[0].source == this.source
        || status.stream.wanted == 0 || status.stream.wanted != status.stream.resident
        || status.stream.pending != 0 { return; }
    let valid = true;
    for tile in status.contacts {
        valid = valid && tile.ready;
    }
    let views = 0;
    for view in status.camera_cover {
        if view.source == status.contacts[0].source {
            views += 1;
            valid = valid && view.ready && view.wanted > 0 && view.resident == view.wanted && view.drawn > 0;
        }
    }
    if !valid || views == 0 { return; }
    this.checks.push(t_true(valid && views > 0, "replacement physical support and camera covers are ready"));
    this.source = status.contacts[0].source;
    if this.phase == 1 {
        this.checks.push(t_true(status.config.max_depth.to_int() == 6,
            "constrained quality reaches the replacement scene"));
        this.checks.push(t_true(request_replacement(9, 65, 256),
            "second restart and restored quality are admitted together"));
        this.phase = 2;
        this.started = clock_snapshot().wall_elapsed_s;
        return;
    }
    this.checks.push(t_true(status.config.max_depth.to_int() == 9,
        "restored quality reaches the replacement scene"));
    report_verdict(this.checks, "TERRAIN RESTART QUALITY", "TERRAIN_RESTART_QUALITY");
    this.phase = 3;
}
