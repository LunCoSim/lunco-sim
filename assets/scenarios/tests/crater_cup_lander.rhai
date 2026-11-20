// @timing presentation
const LANDER = "/World/HopperLander";
const PLANT = "/World/RelayPlant";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness"], required_inputs: []}
}

fn require(result) {
    if !result.ok { throw result.to_string(); }
}

fn pilot(me) {
    this.lander = find_path(LANDER);
    this.plant = find_path(PLANT);
    require(cmd("AcquireControl", #{target:this.lander, bind_camera:true}));
}

fn send_controls(lander, held) {
    for intent in ["thrust", "forward", "left", "yaw_left"] {
        require(cmd("SimulateIntent", #{target:lander, intent:intent, held:held}));
    }
}

fn press_controls(me) {
    this.start_altitude = get(this.lander, "position_y");
    this.start_fuel = get(this.plant, "fuel_kg");
    send_controls(this.lander, true);
    this.last_input_time = clock_snapshot().world_sim_s;
}

fn capture_flight(me) {
    let plant = this.plant;
    let rise = get(this.lander, "position_y") - this.start_altitude;
    let burned = this.start_fuel - get(plant, "fuel_kg");
    this.checks = [
        t_true(owner_of(this.lander) == local_session_id(), "Kestrel retains local possession"),
        t_true(get(plant, "piloted") > 0.5 && get(plant, "fly") < 0.5, "Pilot input takes authority from auto-hop"),
        t_true(get(plant, "external_throttle") > 0.9 && get(plant, "thrust_n") > 2000.0,
            "The standard thrust intent drives Kestrel's engine"),
        t_true(get(plant, "pitch") < -0.9 && get(plant, "roll") > 0.9 && get(plant, "yaw") > 0.9,
            "The shared lander profile sends pitch, roll and yaw in the usual directions"),
        t_true(rise > 0.3 && burned > 0.05, "Kestrel rises " + rise + " m and burns " + burned + " kg"),
        t_true(get(plant, "transfer_kgs").abs() < 0.001, "The tender stays disconnected during manual flight")
    ];
    send_controls(this.lander, false);
    this.last_input_time = clock_snapshot().world_sim_s;
    require(cmd("ReleaseControlSource", #{source:find_path("/World/Avatar")}));
}

fn finish(me) {
    let p = this.plant;
    this.checks.push(t_true(get(p, "piloted") == 0.0 && owner_of(this.lander) != local_session_id(),
        "Releasing Kestrel clears pilot authority"));
    this.checks.push(t_true(get(p, "fly") > 0.5 || get(this.lander, "contact") > 0.5,
        "Automatic pad guidance resumes after release"));
    this.checks.push(t_true(query("GetReadiness").ready && clock_snapshot().clock_contract_ok,
        "The transfer leaves the shared simulation clock healthy"));
    report_verdict(this.checks, "CRATER CUP LANDER", "CRATER_CUP_LANDER");
}

fn task(me, ctx) {
    seq([wait_until(|m| query("GetReadiness").ready && get(find_path(LANDER), "contact") > 0.5
            && get(find_path(LANDER), "speed") < 0.22 && get(find_path(PLANT), "fly") < 0.5),
        once(Fn("pilot")), wait(0.15), once(Fn("press_controls")),
        wait_until(|m| clock_snapshot().world_sim_s - this.last_input_time > 1.15),
        once(Fn("capture_flight")),
        wait_until(|m| clock_snapshot().world_sim_s - this.last_input_time > 0.6), once(Fn("finish"))])
}
