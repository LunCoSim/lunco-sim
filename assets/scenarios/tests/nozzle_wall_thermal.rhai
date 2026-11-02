// Production thermal-state and GPU-wire gate; no heat integration in Rhai.
fn simulation_dependencies(me,ctx) {
 #{modelica_entities:[find_path("/World/Wall")],
 entity_reads:[find_path("/World/HotMetal")],entity_writes:[],
 query_reads:["ReadPorts"],required_inputs:[]}
}
fn on_start(me,ctx) {
 this.wall=find_path("/World/Wall");this.metal=find_path("/World/HotMetal");
 this.elapsed=0.0;this.phase=0;this.checks=[];this.previous=();this.done=false;
 this.initial_temperature=();this.previous_heat=();this.integrated_heat_j=0.0;
 cmd("SetTimeTransport",#{playing:true,rate:5.0});cmd("StepSimulation",#{hold:false});
}
fn sample(wall) {
 let r=query("ReadPorts",#{api_id:wall,port_names:["wall_temperature_k","heating_w",
 "radiation_w","conduction_w","stored_heat_rate_w","energy_residual_w","jet_power_w"]});
 if r.contains("ok") && !r.ok {throw "Nozzle thermal state unavailable: "+r;}
 let v=#{};for p in r.ports {v[p.name]=p.value;}v
}
fn on_tick(me,ctx) {
 if this.done {return;}this.elapsed+=dt();let v=sample(this.wall);
 if this.initial_temperature==() {this.initial_temperature=v.wall_temperature_k;}
 if this.previous_heat!=() {
  this.integrated_heat_j+=0.5*(this.previous_heat+v.stored_heat_rate_w)*dt();
 }
 this.previous_heat=v.stored_heat_rate_w;
 if !v.wall_temperature_k.is_finite() || abs(v.energy_residual_w)>1.0e-6
  || v.heating_w<0.0 || v.heating_w>0.03*v.jet_power_w+1.0e-6 {
  throw "Nozzle wall violated temperature, heat-transfer bound or energy closure: "+v;
 }
 if this.phase==0 && this.elapsed>=0.25 {
  this.checks.push(t_true(abs(v.wall_temperature_k-293.15)<1.0,"cold metal begins near its authored temperature"));
  this.checks.push(t_true(v.heating_w==0.0,"idle engine supplies no gas heating"));
  let r=cmd("SetPorts",#{target:this.wall,writes:[["propellant_flow_kgs",0.8],
   ["exit_temperature_k",600.0],["stagnation_temperature_k",3000.0],["jet_power_w",2000000.0]]});
  if !r.ok {throw "Nozzle warm-up stimulus rejected: "+r;}
  this.phase=1;this.elapsed=0.0;
 }else if this.phase==1 && this.elapsed>=20.0 {
  this.hot=v.wall_temperature_k;
  this.checks.push(t_true(this.hot>1000.0 && this.hot<3000.0,"delivered gas heats the wall below recovery temperature"));
  let gpu=query("ReadPorts",#{api_id:this.metal,port_names:["temperature_k","emissivity"]});
  let temp=();let emissivity=();for p in gpu.ports {
   if p.name=="temperature_k" {temp=p.value;}if p.name=="emissivity" {emissivity=p.value;}
  }
  this.checks.push(t_true(abs(temp-this.hot)<2.0,"solved wall temperature reaches the actual PBR shader"));
  this.checks.push(t_true(abs(emissivity-0.8)<1.0e-6,"thermal balance and shader use the same emissivity"));
  print("NOZZLE_THERMAL_HOT|"+v);
  let r=cmd("SetPorts",#{target:this.wall,writes:[["propellant_flow_kgs",0.0],
   ["exit_temperature_k",0.0],["stagnation_temperature_k",0.0],["jet_power_w",0.0]]});
  if !r.ok {throw "Nozzle shutdown stimulus rejected: "+r;}
  this.phase=2;this.elapsed=0.0;
 }else if this.phase==2 {
  if this.elapsed>0.1 && this.previous!=() && v.wall_temperature_k>this.previous+1.0e-6 {
   throw "Nozzle warmed after shutoff in a cold environment";
  }
  this.previous=v.wall_temperature_k;
  if this.elapsed>=0.25 && this.elapsed<0.25+dt() {
   this.checks.push(t_true(v.wall_temperature_k>0.9*this.hot && v.heating_w==0.0,
    "shutdown stops gas heating without instantly extinguishing wall heat"));
  }
  if this.elapsed>=20.0 {
   this.checks.push(t_true(v.wall_temperature_k<this.hot && v.wall_temperature_k>293.15,
    "wall cools over time while retaining stored heat"));
   let stored_change_j=750.0*(v.wall_temperature_k-this.initial_temperature);
   this.checks.push(t_true(abs(this.integrated_heat_j-stored_change_j)<0.005*abs(stored_change_j),
    "independently integrated net heat agrees with the wall's stored energy change"));
   print("NOZZLE_THERMAL_COOL|"+v);
   this.done=true;cmd("SetTimeTransport",#{playing:true,rate:1.0});
   report_verdict(this.checks,"NOZZLE WALL THERMAL","NOZZLE_WALL_THERMAL");
  }
 }
}
