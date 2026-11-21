// Drive both lanes against the same physics and scoring scripts shipped in the demo.
fn simulation_dependencies(me, ctx) {
    let scout = find_path("/World/Scout");
    let comet = find_path("/World/Comet");
    let line = find_path("/World/Works/PackingLine");
    #{modelica_entities: [scout, comet, line, find_path("/World/RelayPlant")], entity_reads: [scout,comet,line,find_path("/World/Judge/DropBall"),find_path("/World/Works/Shuttle"),find_path("/World/Works/Array"),find_path("/World/RelayPlant"),find_path("/World/Scout/Turret/Head/MastCam"),find_path("/World/Scout/TiltJoint"),find_path("/World/Scout/PanJoint")],
      entity_writes: [scout, comet, line], query_reads: ["QueryUsdPrims"], required_inputs: []}
}

fn on_start(me, ctx) {
    this.scout = find_path("/World/Scout");
    this.comet = find_path("/World/Comet");
    this.line = find_path("/World/Works/PackingLine");
    let steering = query("QueryUsdPrims", #{
        paths: ["/World/Comet/Drivetrain", "/World/Comet/Steering_FL_Hinge", "/World/Comet/Steering_FR_Hinge"],
        attrs: ["inputs:max_heading", "physics:upperLimit", "physics:lowerLimit"],
    });
    this.max_heading = steering.records[0].attrs["inputs:max_heading"];
    this.left_stop = steering.records[1].attrs["physics:upperLimit"] * PI() / 180.0;
    this.right_stop = steering.records[2].attrs["physics:lowerLimit"] * PI() / 180.0;
    let idle=cmd("SetPorts",#{target:this.line,writes:[["line_enabled",0.0]]});
    if !idle.ok {throw idle.error;}
    this.stage = 0;
    this.elapsed = 0.0;
    this.claimed = false;
    this.driving = false;
    this.crest = false;
    this.scores = [];
    this.times = [];
    this.lock = 0;
    this.done = false;
}

fn on_event(me, evt, ctx) {
    if evt.name == "crater_cup.sample_recovered" { this.scores.push(evt.value); }
    if evt.name == "crater_cup.finish_seconds" { this.times.push(evt.value); }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if !this.claimed && this.elapsed > 0.45 {
        this.claimed = true;
        let claim = cmd("AcquireControl", #{target:this.scout,bind_camera:false});
        if !claim.ok { throw claim.error; }
    }
    if !this.driving && this.elapsed > 1.0 {
        this.driving = true;
        for vehicle in [this.scout, this.comet] {
            let result = drive(vehicle, 1.0, 0.0);
            if !result.ok { throw result.error; }
        }
    }
    if this.stage == 0 && this.elapsed > 1.0 {
        this.flat = get(this.line, "solar_w");
        this.initial_tote = world_pos(find_path("/World/Works/Shuttle"));
        this.initial_mass = get(this.line, "packed_kg");
        this.camera0 = world_forward(find_path("/World/Scout/Turret/Head/MastCam"));
        let slew=cmd("SetPorts",#{target:this.scout,writes:[["azimuth",0.6],["elevation",0.4]]});
        if !slew.ok {throw slew.error;}
        let result = cmd("SetPorts", #{target:this.line,writes:[["target_tilt",0.907571211],["jog",1.0]]});
        if !result.ok { throw result.error; }
        this.stage = 1;
    }
    if this.stage == 1 && this.elapsed > 3.25 {
        this.max_w = get(this.line,"solar_w");
        this.camera1 = world_forward(find_path("/World/Scout/Turret/Head/MastCam"));
        this.production = get(this.line,"packed_kg");
        this.tilted = world_up(find_path("/World/Works/Array"))[1];
        this.tote_motion = (world_pos(find_path("/World/Works/Shuttle"))[0] - this.initial_tote[0]).abs();
        let result = cmd("SetPorts", #{target:this.line,writes:[["jog",0.0]]});
        if !result.ok { throw result.error; }
        this.stage = 2;
    }
    if this.stage == 2 && this.elapsed > 5.0 {
        this.stopped_at = get(this.line,"packed_kg");
        this.stage = 3;
    }
    // Both full-lock keyboard commands run after the timed jump.
    if this.elapsed>17 && this.lock==0 {
        let turn=drive(this.comet,0.0,-1.0);
        if !turn.ok {throw turn.error;}
        this.lock=1;
    }
    if this.elapsed>18 && this.lock==1 {
        this.left_fl=get(this.comet,"heading_fl");this.left_fr=get(this.comet,"heading_fr");
        let turn=drive(this.comet,0.0,1.0);
        if !turn.ok {throw turn.error;}
        this.lock=2;
    }
    if this.elapsed>19 && this.lock==2 {
        this.right_fl=get(this.comet,"heading_fl");this.right_fr=get(this.comet,"heading_fr");
        this.lock=3;
    }
    let car = world_pos(this.comet);
    if car != () && car[2] < -12.0 && car[1] > 2.0 { this.crest = true; }
    if this.stage == 3 && this.elapsed > 7.0 {
        this.held_mass = get(this.line,"packed_kg");
        this.idle_w = get(this.line,"line_w");
        let result = cmd("SetPorts", #{target:this.line,writes:[["line_enabled",1.0]]});
        if !result.ok { throw result.error; }
        this.stage = 4;
    }
    if this.elapsed < 31.0 { return; }

    let remaining = [find_path("/World/Sample1"),find_path("/World/Sample2"),find_path("/World/Sample3")];
    let valid_scores = this.scores.contains(remaining[0]);
    for sample in this.scores {
        let index = remaining.index_of(sample);
        if index < 0 { valid_scores = false; } else { remaining.remove(index); }
    }
    let top = world_pos(find_path("/World/Judge/DropBall"));
    report_verdict([
        t_true((this.camera1[0]-this.camera0[0]).abs()>0.15 && (this.camera1[1]-this.camera0[1]).abs()>0.1,
            "Scout steers its optical axis in azimuth and elevation "+this.camera0+" to "+this.camera1),
        t_true(this.left_fl>this.left_fr && this.right_fr<this.right_fl &&
            (this.left_fl-this.max_heading).abs()<0.001 && (this.right_fr+this.max_heading).abs()<0.001 &&
            this.left_fl < this.left_stop && this.right_fr > this.right_stop,
            "Comet reaches its authored full-lock heading inside the mechanical stops in both directions"),
        t_true(this.flat > 3480 && this.flat < 3700 && this.max_w > 4250 && this.max_w <= 4320.1 && this.tilted < 0.72,
            "aiming the physical array raises terminal output from " + this.flat + " to " + this.max_w + " W, up.y=" + this.tilted),
        t_true(this.initial_mass == 0.0 && this.production > 1.8 && this.tote_motion > 0.25,
            "manual jog packs material and advances a tote, displacement " + this.tote_motion),
        t_true((this.held_mass - this.stopped_at).abs() < 0.001 && this.idle_w < 0.001,
            "releasing the jog control holds delivered mass while the array stays live"),
        t_true((get(this.line,"packed_kg") - 24.0 - get(find_path("/World/RelayPlant"),"samples_kg")).abs() < 0.03 && get(this.line,"line_w") < 0.1,
            "a full hopper delivers twelve 2 kg packs and shuts off the load"),
        t_true(top != () && top[1] > 4.20 && top[1] < 4.40,
            "dropped test ball settles on the sieve lid at " + top),
        t_true(this.crest, "Comet clears the launch ramp"),
        t_true(valid_scores, "the first sample scores, each ball counts once, and the following rover cannot score"),
        t_true(this.times.len() == 1 && this.times[0] > 0.0 && this.times[0] < 6.0, "the two timing beams record a finish time"),
    ], "CRATER CUP", "CRATER_CUP");
    this.done = true;
}
