// Each truck owns a berth-to-berth route. Pumps and augers run only at rest.
fn simulation_dependencies(me,ctx) {
    let members = [find_path("/World/RelayPlant"), find_path("/World/Miner"),
                   find_path("/World/Tailings"), find_path("/World/Tanker")];
    let reads = members + [find_path("/World/HopperLander")];
    for berth in ["MineFace","OreIntake","ResidueBay","SpoilTip","LoadPoint","ServicePoint"] {
        reads.push(find_path("/World/"+berth));
    }
    #{modelica_entities:members, entity_reads:reads,
      entity_writes:members, query_reads:[], required_inputs:[]}
}
fn accept(result) {
    if result == () || !result.ok { throw "Base dispatch: " + result; }
}
fn berth_distance(vehicle, station) {
    let a = world_pos(vehicle);
    let b = world_pos(station);
    let dx = a[0]-b[0];
    let dz = a[2]-b[2];
    (dx*dx+dz*dz).sqrt()
}
fn route(vehicle, source, sink, load, unload, cargo) {
    #{vehicle:find_path("/World/"+vehicle), source:find_path("/World/"+source),
      sink:find_path("/World/"+sink), load:load, unload:unload, cargo:cargo,
      at_source:true, travelling:false, manual:false, valve:""}
}
fn drive_leg() {
    let destination = if this.at_source {this.source} else {this.sink};
    let throttle = if this.at_source {-0.84} else {0.95};
    accept(modelica_waypoint(this.vehicle,world_pos(destination),throttle,0.28,0.0));
    this.travelling = true;
}
fn set_valve(plant, valve) {
    if this.valve == valve { return; }
    let loading = if valve == "load" {1.0} else {0.0};
    let unloading = if valve == "unload" {1.0} else {0.0};
    accept(cmd("SetPorts",#{target:plant,writes:[[this.load,loading],[this.unload,unloading]]}));
    this.valve = valve;
}
fn service_route(plant, leave_source, leave_sink) {
    let manual = is_controlled(this.vehicle);
    if manual != this.manual {
        this.manual = manual;
        if manual {
            this.set_valve(plant,"");
            accept(cmd("SetPorts",#{target:this.vehicle,writes:[
                ["autopilot_enable",0.0],["program_active",0.0],["brake",0.0]]}));
        } else { this.drive_leg(); }
    }
    if manual { return; }
    let destination = if this.at_source {this.source} else {this.sink};
    let dist = berth_distance(this.vehicle,destination);
    if this.travelling {
        if dist>1.0 { return; }
        accept(modelica_waypoint_stop(this.vehicle));
        this.travelling = false;
    }
    if dist>1.38 || get(this.vehicle,"speed")>0.27 {
        this.set_valve(plant,"");
        return;
    }
    if (this.at_source && leave_source) || (!this.at_source && leave_sink) {
        this.set_valve(plant,"");
        if !this.at_source { emit("polar.delivery",this.cargo); }
        this.at_source = !this.at_source;
        this.drive_leg();
    } else {
        this.set_valve(plant,if this.at_source {"load"} else {"unload"});
    }
}
fn flight(phase) {
    this.phase = phase;
    let climb = phase == "Liftoff" || phase == "Return";
    let fly = phase == "Approach" || climb;
    accept(cmd("SetPorts",#{target:this.plant,writes:[["fly",if fly {1.0} else {0.0}],
        ["climb",if climb {1.0} else {0.0}]]}));
    emit("crater_relay.phase",phase);
}
fn on_start(me,ctx) {
    this.plant = find_path("/World/RelayPlant");
    this.lander = find_path("/World/HopperLander");
    this.mine = route("Miner","MineFace","OreIntake","mine_load","mine_unload","ore");
    this.tail = route("Tailings","ResidueBay","SpoilTip","tails_load","tails_dump","tailings");
    this.fuel = route("Tanker","LoadPoint","ServicePoint","load","service","propellant");
    this.serviced = false;
    this.flight("Approach");
}
fn advance(me) {
    let p = this.plant;
    let ore = get(p,"cargo_kg");
    let spent = get(p,"tail_cargo_kg");
    this.mine.service_route(p,ore>95.85,ore<0.12);
    this.tail.service_route(p,spent>70.0,spent<0.12);
    if get(this.lander,"piloted") > 0.5 {
        this.fuel.set_valve(p,"");
        if this.phase != "Manual" { this.flight("Manual"); }
        return true;
    }
    let pad_error = get(p,"pad_error");
    let landed = get(this.lander,"contact")>0.5 && get(this.lander,"speed")<0.22 && pad_error<0.9;
    if this.phase == "Manual" {
        this.serviced = false;
        this.flight(if landed {"Waiting"} else if pad_error<0.9 {"Approach"} else {"Return"});
    }
    if this.phase == "Return" && pad_error < 0.8 { this.flight("Approach"); }
    let full = get(p,"fuel_kg")>11.92;
    let needs_service = this.phase=="Waiting" && landed;
    let enough = get(p,"tender_kg") >= 12.02-get(p,"fuel_kg");
    if needs_service && get(p,"transfer_kgs")>0.01 {this.serviced = true;}
    this.fuel.service_route(p,needs_service && enough,needs_service && full);
    if this.phase=="Approach" && landed {
        this.serviced = false;
        this.flight("Waiting");
    } else if needs_service && this.serviced && full && get(p,"clearance")>13.0 {
        this.flight("Liftoff");
    } else if this.phase=="Liftoff" && world_pos(this.lander)[1]>7.0 {
        this.flight("Approach");
    }
    true
}
fn task(me,ctx) {forever(seq([wait(0.15),once(Fn("advance"))]))}
