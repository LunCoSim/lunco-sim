// Production visual gate. Attach to a disposable scenario owner with
// RunScenarioAsset, params #{ rover_id: stable_vehicle_id }. Uses live pose
// commands without saving source documents. Tests physics-driven presentation.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [ctx.rover_id], entity_reads: [ctx.rover_id],
       entity_writes: [ctx.rover_id], query_reads: [], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.rover = ctx.rover_id;
    this.phase = "settle";
    this.t = 0.0;
    this.total = 0.0;
    this.done = false;
    this.checks = [];
    this.previous = ();
    this.continuous = false;
    this.publication_continuous = true;
    this.claimed = false;
    this.start = world_pos(this.rover);
    this.rotation = world_rotation(this.rover);
    if this.start == () || this.rotation == () { throw "trail gate requires a live vehicle pose"; }
}

fn inspect_trail(state) { query("InspectVehicleTrail", #{ id: state.rover }); }

fn end_points(trail) {
    let points = [];
    for lane in trail.lanes {
        points.push([lane.wheel_id, lane.sample_count, lane.strokes]);
    }
    points
}

fn airborne(trail) {
    if trail == () || trail.lanes.len() == 0 { return false; }
    for lane in trail.lanes { if lane.contacting { return false; } }
    true
}

fn observe_continuity(current, previous) {
    if previous == () { return false; }
    for lane in current.lanes {
        for old in previous.lanes {
            if old.wheel_id != lane.wheel_id || old.strokes.len() == 0 || lane.strokes.len() == 0 { continue; }
            let a = old.strokes[old.strokes.len() - 1];
            let b = lane.strokes[lane.strokes.len() - 1];
            let distance = vlen(vsub(a.end, b.end));
            if a.id == b.id && a.count == b.count && distance > 0.001 && distance < 0.5 { return true; }
        }
    }
    false
}

fn retained_publication(current, previous) {
    if previous == () { return true; }
    for old in previous.lanes {
        if old.published_revision == () { continue; }
        for lane in current.lanes {
            if lane.wheel_id == old.wheel_id
                && (lane.published_revision == () || lane.error != ()) { return false; }
        }
    }
    true
}

fn finish(state) {
    if state.claimed { brake(state.rover); release_control(state.rover); }
    state.claimed = false;
    state.done = true;
    report_verdict(state.checks, "VEHICLE TRAIL CONTACT", "VEHICLE_TRAIL_CONTACT");
    state
}

fn on_stop(me, ctx) {
    if this.claimed { brake(this.rover); release_control(this.rover); }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.t += dt(); this.total += dt();
    if this.total > 75.0 {
        this.checks.push(t_true(false, "trail gate timed out in " + this.phase));
        this = finish(this); return;
    }
    let trail = inspect_trail(this);
    if this.phase == "settle" {
        if this.t < 2.0 || trail == () || trail.lanes.len() == 0 { return; }
        this.claimed = cmd("AcquireControl", #{ target: this.rover, bind_camera: false }).ok;
        this.checks.push(t_true(this.claimed, "could not acquire test vehicle control"));
        this.checks.push(t_true(drive(this.rover, 0.35, 0.0).ok, "ground drive command rejected"));
        this.start = world_pos(this.rover);
        this.phase = "drive"; this.t = 0.0; return;
    }
    if this.phase == "drive" {
        if observe_continuity(trail, this.previous) { this.continuous = true; }
        this.publication_continuous = this.publication_continuous
            && retained_publication(trail, this.previous);
        this.previous = trail;
        if this.t < 30.0 { return; }
        let measured = 0;
        let widths_ok = true;
        let published = true;
        for lane in trail.lanes {
            if to_int(lane.sample_count) >= 2 && lane.contacting {
                measured += 1;
                widths_ok = widths_ok && lane.render_width_m != () && lane.wheel_width_m != ()
                    && (lane.render_width_m - lane.wheel_width_m).abs() < 1.0e-6;
                published = published && lane.projection == "terrain" && lane.published_revision != () && lane.error == ();
            }
        }
        this.checks.push(t_true(measured > 0, "no grounded wheel trails were measured"));
        this.checks.push(t_true(widths_ok, "rendered track width differs from the physical tire width"));
        this.checks.push(t_true(published, "ground tracks did not publish on terrain"));
        this.checks.push(t_true(this.continuous, "no moving endpoint was observed below history spacing"));
        this.checks.push(t_true(this.publication_continuous,
            "displayed ground-track publication is retained throughout the drive"));
        this.checks.push(t_moved(vlen(vsub(world_pos(this.rover), this.start)), 20.0, "grounded vehicle travel past waypoint"));
        this.ground = world_pos(this.rover);
        this.rotation = world_rotation(this.rover);
        this.before_flight = trail;
        brake(this.rover);
        this.checks.push(t_true(set_world_pos(this.rover, vadd(this.ground, [0.0, 10.0, 0.0])).ok, "airborne stimulus rejected"));
        print("[trail-gate] airborne stimulus");
        this.phase = "air"; this.t = 0.0; return;
    }
    if this.phase == "air" {
        if this.t < 0.5 { return; }
        if this.previous != () && this.t < 1.0 { this.air_snapshot = end_points(trail); this.previous = (); }
        if this.t < 1.5 { return; }
        this.checks.push(t_true(airborne(trail), "airborne wheels retained trail contact"));
        this.checks.push(t_true(end_points(trail) == this.air_snapshot, "airborne motion extended ground history"));
        let q = this.rotation;
        let inverted = [-q[1], q[0], q[3], -q[2]];
        this.checks.push(t_true(set_world_pose(this.rover, vadd(this.ground, [4.0, -0.5, 0.0]), inverted).ok, "inverted stimulus rejected"));
        print("[trail-gate] inverted landing stimulus");
        this.phase = "roof"; this.t = 0.0; return;
    }
    if this.phase == "roof" {
        if this.t < 1.0 { return; }
        if this.previous == () { this.roof_snapshot = end_points(trail); this.previous = trail; }
        if this.t < 3.0 { return; }
        let up = world_up(this.rover);
        this.checks.push(t_true(up != () && up[1] < -0.5, "inverted stimulus did not leave the vehicle on its back"));
        this.checks.push(t_true(airborne(trail) && end_points(trail) == this.roof_snapshot,
            "roof motion extended a wheel trail"));
        this.checks.push(t_true(set_world_pose(this.rover, vadd(this.ground, [8.0, 0.5, 0.0]), this.rotation).ok, "upright landing stimulus rejected"));
        drive(this.rover, 0.35, 0.0);
        print("[trail-gate] upright landing stimulus");
        this.phase = "landing"; this.t = 0.0; return;
    }
    if this.phase == "landing" {
        if this.t < 4.0 { return; }
        let disconnected = 0;
        for lane in trail.lanes {
            for old in this.before_flight.lanes {
                if lane.wheel_id != old.wheel_id || !lane.contacting || lane.strokes.len() == 0 || old.strokes.len() == 0 { continue; }
                let last = lane.strokes[lane.strokes.len() - 1];
                let before = old.strokes[old.strokes.len() - 1];
                if last.id != before.id && to_int(last.count) >= 2 { disconnected += 1; }
            }
        }
        this.checks.push(t_true(disconnected > 0, "landing did not start a disconnected grounded stroke"));
        this = finish(this);
    }
}
