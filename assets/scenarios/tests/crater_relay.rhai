// Verify the complete production chain through two flights and physical unloading.
fn contact_surfaces() {
    let paths=["Ground","HopperLander/Bus","HopperLander/Foot0","Miner/Payload/CuttingDrum",
        "Tailings/Payload/Floor","Tanker/Payload/Vessel1","PolarWorks/Retort","PolarWorks/OxygenHeader/Pipe3",
        "ProductTank1/Insulation","ProductTank0/Cap2","BatteryBank/Cabinet0","SolarField/Wing0_0","SpoilYard/Heap"];
    for rover in ["Scout","Comet","Miner","Tailings","Tanker"] {
        for wheel in ["FL","FR","RL","RR"] {paths.push(rover+"/Suspension_"+wheel+"/Wheel_"+wheel);}
    }
    paths
}
fn simulation_dependencies(me,ctx) {
    let reads=[];
    for name in ["RelayPlant","HopperLander","Miner","Tailings","Tanker"] { reads.push(find_path("/World/"+name)); }
    for path in contact_surfaces() {reads.push(find_path("/World/"+path));}
    #{modelica_entities:[find_path("/World/RelayPlant")],
      entity_reads:reads,entity_writes:[],query_reads:["QueryPhysicsState"],required_inputs:[]}
}
fn on_start(me,ctx) {
    this.p=find_path("/World/RelayPlant");
    this.l=find_path("/World/HopperLander");
    this.t=0.0; this.next=0.0; this.flights=0; this.landings=0; this.ore=0; this.dumps=0;
    this.fills=0; this.bad=false; this.error=0.0; this.apex=0.0; this.done=false; this.charged=false; this.discharged=false;
}
fn on_event(me,evt,ctx) {
    if evt.name=="polar.delivery" {
        switch evt.value {"ore"=>{this.ore+=1;},"tailings"=>{this.dumps+=1;},"propellant"=>{this.fills+=1;}}
    }
    if evt.name!="crater_relay.phase" {return;}
    if evt.value=="Waiting" {this.landings+=1;}
    if evt.value=="Liftoff" {
        this.flights+=1;
        if get(this.p,"clearance")<12.9 || get(this.p,"fuel_kg")<11.85 {this.bad=true;}
    }
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.t+=dt(); if this.t<0.5 {return;}
    let p=this.p;
    let total=0.0;
    for n in ["stock_ore_kg","cargo_kg","feed_kg","water_kg","h2_kg","o2_kg","spent_kg","samples_kg",
              "tail_cargo_kg","dumped_kg","tender_kg","fuel_kg","burned_kg"] {total+=get(p,n);}
    this.error=max(this.error,(total-12090.68).abs());
    if get(p,"transfer_kgs")>0.02 && (get(this.l,"contact")<0.5 || get(p,"thrust_n")>0.5 || get(p,"berth_distance")>1.1) {this.bad=true;}
    let battery=get(p,"battery_w");
    if battery>10000 {this.charged=true;}
    if battery < -10000 {this.discharged=true;}
    if (get(p,"bus_load_w")+battery-get(p,"field_w")).abs()>2 {this.bad=true;}
    this.apex=max(this.apex,world_pos(this.l)[1]);
    if this.t>=this.next {
        print("Base progress "+[this.t,this.ore,this.dumps,this.fills,this.flights,get(p,"cargo_kg"),get(p,"tail_cargo_kg"),get(p,"tender_kg"),get(p,"fuel_kg"),get(p,"split_kg"),world_pos(find_path("/World/Miner")),world_pos(find_path("/World/Tailings")),world_pos(find_path("/World/Tanker"))]);
        this.next+=25;
    }
    if this.landings<3 && this.t<300 {return;}
    print("Base result "+[this.t,this.ore,this.dumps,this.fills,this.flights,this.apex,get(p,"pad_error"),get(p,"received_kg"),get(p,"split_kg"),get(p,"dumped_kg"),this.error]);
    let missing=[];
    for path in contact_surfaces() {
        let state=query("QueryPhysicsState",#{id:find_path("/World/"+path)});
        if state.collider_present!=true {missing.push(path);}
    }
    report_verdict([
        t_true(missing.is_empty(),"native contact shapes on fleet wheels, processing equipment and receiving surfaces. Missing "+missing),
        t_true(this.ore>=2 && get(p,"received_kg")>180,"two mined loads reach the retort"),
        t_true(this.dumps>=1 && get(p,"dumped_kg")>65,"tailings leave the refinery and reach the spoil bank"),
        t_true(get(p,"split_kg")>6 && get(p,"burned_kg")>4,"water is electrolyzed and product is burned"),
        t_true(this.fills>=2 && this.flights>=2 && this.landings>=3,"separate tanker services two return flights"),
        t_true(this.apex>7 && this.apex<8.4 && get(p,"pad_error")<0.35,"controlled hops return to the surveyed pad"),
        t_true(this.charged && this.discharged && get(p,"battery_soc")>12 && get(p,"battery_soc")<100,"battery both supplies the refinery and accepts the surplus"),
        t_true(get(p,"o2_kpa")>0 && get(p,"h2_kpa")>0 && get(p,"o2_temp_k")>280,"gas vessels report absolute pressure and temperature"),
        t_true(!this.bad && this.error<0.025,"pad and power interlocks and total mass conservation")
    ],"POLAR BASE","CRATER_RELAY"); this.done=true;
}
