// Production regression: an attached flight crosses BigSpace cell boundaries;
// fast free drops contact the same below-datum globe/collar seen by rendering.
const TEST_KIND = "headless";

fn launch_input(id, speed_mps) {
    let mass = get(id,"mass");
    if mass == () || mass <= 0.0 { return false; }
    // One fixed-step force input supplies the requested momentum increment
    // through the normal native force-port owner.
    cmd("SetPorts",#{target:id,writes:[["force_y",mass*speed_mps/dt()]]}).ok
}

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [find_path("/World/Flight"),
      find_path("/World/Companion"), find_path("/World/GlobeDrop"),
      find_path("/World/CollarDrop")], entity_writes: [find_path("/World/Flight"),
      find_path("/World/Companion"),find_path("/World/GlobeDrop"),find_path("/World/CollarDrop")],
      query_reads: ["QueryPhysicsState"], required_inputs: []}
}

fn on_start(me, ctx) {
    this.start = sim_tick();
    this.done = false;
    this.peak = -1.0e9;
    this.max_attachment_error = 0.0;
    this.crossed_up = false;
    this.crossed_down = false;
    this.min_cell_y = 1000000;
    this.max_cell_y = -1000000;
    this.initial_reported = false;
    this.drive_tick = -1;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    let flight = find_path("/World/Flight");
    let companion = find_path("/World/Companion");
    let globe = find_path("/World/GlobeDrop");
    let collar = find_path("/World/CollarDrop");
    let p = world_pos3(flight);
    let q = world_pos3(companion);
    let g = world_pos3(globe);
    let c = world_pos3(collar);
    if this.drive_tick < 0 {
        if sim_tick() - this.start < 3 { return; }
        // Apply one launch input after native frame admission, as a pilot
        // does. This fixture isolates flight/ground support from USD's
        // independent authored-initial-velocity projection contract.
        let applied = launch_input(flight,80.0) && launch_input(companion,80.0) &&
          launch_input(globe,-150.0) && launch_input(collar,-30.0);
        if !applied {
            this.done = true;
            report_verdict(["native launch force input was rejected"],
              "Planetary flight/surface", "PLANETARY_FLIGHT_SURFACE");
            return;
        }
        this.drive_tick = sim_tick();
        return;
    }
    this.peak = this.peak.max(p.y);
    this.max_attachment_error = this.max_attachment_error.max((vlen(q-p)-3.0).abs());
    let cell_y = get(flight, "CellCoord.y");
    if cell_y != () {
        this.min_cell_y = this.min_cell_y.min(cell_y);
        this.max_cell_y = this.max_cell_y.max(cell_y);
    }
    if p.y > 3100.0 { this.crossed_up = true; }
    if this.crossed_up && p.y < 1050.0 { this.crossed_down = true; }
    let elapsed = (sim_tick() - this.drive_tick).to_float() / 60.0;
    let gg = geolocation(globe);
    let cg = geolocation(collar);
    if !this.initial_reported && elapsed >= 0.05 {
        this.initial_reported = true;
        let fv = query("QueryPhysicsState",#{id:flight}).linear_velocity_mps;
        let gv = query("QueryPhysicsState",#{id:globe}).linear_velocity_mps;
        emit("PLANETARY_FLIGHT_SURFACE_EVIDENCE", #{stage:"first_native_steps",
          flight_velocity:fv, globe_velocity:gv,
          collar_velocity:query("QueryPhysicsState",#{id:collar}).linear_velocity_mps});
    }
    if gg == () || cg == () {
        this.done = true;
        report_verdict(["planetary height unavailable"], "Planetary flight/surface", "PLANETARY_FLIGHT_SURFACE");
        return;
    }
    if gg.height < -502.0 || cg.height < -502.0 {
        this.done = true;
        report_verdict(["body penetrated planetary surface: globe height=" + gg.height +
          " collar height=" + cg.height], "Planetary flight/surface", "PLANETARY_FLIGHT_SURFACE");
        return;
    }
    if elapsed < 135.0 { return; }
    this.done = true;
    let checks = [
        t_true(this.crossed_up && this.crossed_down,
          "attached flight passes 3.1 km and returns below the former ceiling"),
        t_true(this.max_cell_y >= 2 && this.min_cell_y < this.max_cell_y,
          "the production bridge actually transports the body between BigSpace cells"),
        t_true(this.max_attachment_error < 0.02,
          "cell changes preserve the native fixed attachment"),
        t_true(p.y > -499.1 && p.y < -498.5,
          "flight returns to the authored below-datum local ground"),
        t_true(gg.height > -499.1 && gg.height < -498.5,
          "fast drop contacts the curved globe four kilometres outside local ground"),
        t_true(cg.height > -499.1 && cg.height < -498.5,
          "drop at the terrain/globe transition contacts the shared collar"),
    ];
    let bodies = [];
    for id in [flight,companion,globe,collar] {
        let state = query("QueryPhysicsState", #{id:id});
        checks.push(t_true(state.physics_pose_seeded && state.physics_state_ready &&
          !state.physics_object_paused && !state.rigid_body_disabled,
          "native body stays admitted across flight and contact"));
        let v = state.linear_velocity_mps;
        let a = state.constant_linear_acceleration_mps2;
        if a == () {
            checks.push("native gravity direction unavailable for contact check");
        } else {
            let magnitude = (a[0]*a[0]+a[1]*a[1]+a[2]*a[2]).sqrt();
            if magnitude <= 0.0 {
                checks.push("native gravity direction has zero magnitude");
            } else {
                // A sphere may roll along the curved globe. Ground support
                // arrests motion through the surface, not tangential travel.
                let normal_speed = (v[0]*a[0]+v[1]*a[1]+v[2]*a[2]).abs()/magnitude;
                checks.push(t_true(normal_speed < 0.1,
                  "surface contact arrests normal motion: "+state.usd_prim_path));
                bodies.push(#{path:state.usd_prim_path,velocity:v,normal_speed:normal_speed});
            }
        }
    }
    emit("PLANETARY_FLIGHT_SURFACE_EVIDENCE", #{peak_m:this.peak,
      max_attachment_error_m:this.max_attachment_error,flight_y:p.y,
      globe_y:g.y,collar_y:c.y,globe_height:gg.height,collar_height:cg.height,
      min_cell_y:this.min_cell_y,max_cell_y:this.max_cell_y,bodies:bodies});
    report_verdict(checks, "Planetary flight/surface", "PLANETARY_FLIGHT_SURFACE");
}
