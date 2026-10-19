// Production Modelica conservation and optical-budget acceptance. Rhai supplies
// boundary conditions and observes laws; it never computes a vehicle's plume.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[find_path("/World/Engine")],entity_reads:[find_path("/World/Engine/Exhaust/Flame")],entity_writes:[],
      query_reads:["QueryUsdPrim","GetPort"],required_inputs:[]}
}
fn write(engine,writes) {
    let result=cmd("SetPorts",#{target:engine,writes:writes});
    if !result.ok {throw("plume boundary write rejected: "+result);}
}
fn value(engine,name) {get(engine,name)}
fn station(engine,kind,i) {value(engine,"profile_"+kind+"_"+i)}
fn conservation(engine,mdot) {
    let checks=[];
    let h=value(engine,"total_specific_energy_j_kg");
    let gamma=value(engine,"gas_gamma");let gas_r=value(engine,"gas_constant_j_kg_k");
    for i in 0..5 {
        let r=station(engine,"radius_m",i);let rho=station(engine,"density_kg_m3",i);
        let u=station(engine,"speed_mps",i);let temp=station(engine,"temperature_k",i);
        checks.push(t_true((rho*u*PI()*r*r-mdot).abs()<1.0e-8,"station "+i+" conserves mass"));
        checks.push(t_true((u*u/2.0+gamma*gas_r/(gamma-1.0)*temp-h).abs()<1.0e-5,
            "station "+i+" conserves total specific energy"));
        if i>0 {
            checks.push(t_true(r>station(engine,"radius_m",i-1) && rho<station(engine,"density_kg_m3",i-1)
                && temp<station(engine,"temperature_k",i-1) && u>station(engine,"speed_mps",i-1),
                "station "+i+" expands, dilutes, cools and accelerates"));
        }
    }
    // Independent composite Simpson integration of the published, log-linearly
    // interpolated emissivity field. This does not call the Modelica normalization formula.
    let integral=0.0;let length=value(engine,"envelope_length_m");
    for i in 0..4 {
        let z0=length*(i*i).to_float()/16.0;
        let z1=length*((i+1)*(i+1)).to_float()/16.0;
        let sum=0.0;
        for j in 0..129 {
            let f=j.to_float()/128.0;
            let radius=station(engine,"radius_m",i)*(1.0-f)+station(engine,"radius_m",i+1)*f;
            let emission=exp(ln(station(engine,"emission_w_m3",i))*(1.0-f)+ln(station(engine,"emission_w_m3",i+1))*f);
            let weight=if j==0 || j==128 {1.0} else if j%2==0 {2.0} else {4.0};
            sum+=weight*emission*PI()*radius*radius;
        }
        integral+=sum*(z1-z0)/384.0;
    }
    checks.push(t_true((integral-value(engine,"radiant_power_w")).abs()<1.0e-8*value(engine,"radiant_power_w"),
        "integrated emissivity closes the radiative power budget"));
    checks.push(t_true(value(engine,"radiant_power_w")>=0.0 && value(engine,"radiant_power_w")<=value(engine,"jet_power_w"),
        "radiation does not exceed delivered jet energy"));
    checks.push(t_true((value(engine,"intensity")-value(engine,"radiant_power_w")*value(engine,"luminous_efficacy_lm_w")).abs()<1.0e-6,
        "local light uses the same optical power and efficacy"));
    checks.push(t_true(value(engine,"mass_residual_kgs")<1.0e-9 && value(engine,"energy_residual_w")<1.0e-5
        && value(engine,"thrust_residual_n")<1.0e-7,"published conservation residuals close"));
    checks
}
fn on_start(me,ctx) {
    this.engine=find_path("/World/Engine");this.phase=0;this.elapsed=0.0;
    this.checks=[];this.done=false;
}
fn on_tick(me,ctx) {
    if this.done {return;}this.elapsed+=dt();if this.elapsed<0.25 {return;}this.elapsed=0.0;
    if this.phase==0 {
        this.checks.push(t_true(value(this.engine,"render_throttle")==0.0 && value(this.engine,"intensity")==0.0,
            "zero delivered flow/thrust starts dark"));
        this.checks.push(t_true(value(this.engine,"envelope_radius_m")>value(this.engine,"nozzle_exit_radius_m"),
            "the observation envelope accommodates expansion"));
        write(this.engine,[["thrust_n",1000.0],["propellant_flow_kgs",1000.0/2600.0],
            ["exhaust_velocity_mps",2600.0]]);
    } else if this.phase==1 {
        this.checks += conservation(this.engine,1000.0/2600.0);
        let flame=find_path("/World/Engine/Exhaust/Flame");
        this.checks.push(t_true(abs(get(flame,"flow_phase_sin")-value(this.engine,"flow_phase_sin"))<1.0e-6
            && abs(get(flame,"flow_phase_cos")-value(this.engine,"flow_phase_cos"))<1.0e-6,
            "both bounded phase channels reach the shader through the GPU boundary"));
        this.checks.push(t_true(value(this.engine,"exit_speed_mps")<2600.0,"gas velocity excludes the pressure-thrust term"));
        this.power=value(this.engine,"radiant_power_w");this.rho=station(this.engine,"density_kg_m3",0);
        this.temp=station(this.engine,"temperature_k",0);this.emission=station(this.engine,"emission_w_m3",0);
        this.radius=station(this.engine,"radius_m",4);this.phase_m=value(this.engine,"transported_distance_m");
        write(this.engine,[["thrust_n",2000.0],["maximum_thrust_n",2000.0],
            ["propellant_flow_kgs",2000.0/2600.0]]);
    } else if this.phase==2 {
        this.checks.push(t_true((value(this.engine,"radiant_power_w")/this.power-2.0).abs()<1.0e-6,
            "fixed-Isp doubling of flow doubles available radiation"));
        this.checks.push(t_true((station(this.engine,"density_kg_m3",0)/this.rho-2.0).abs()<1.0e-6
            && (station(this.engine,"temperature_k",0)-this.temp).abs()<1.0e-6,
            "fixed-Isp flow changes density without inventing a temperature change"));
        this.checks.push(t_true((value(this.engine,"transported_distance_m")-this.phase_m).abs()>1.0e-6,"bounded transport advances with the simulation clock"));
        write(this.engine,[["thrust_n",0.0],["propellant_flow_kgs",0.0]]);
    } else if this.phase==3 {
        this.checks.push(t_true(value(this.engine,"radiant_power_w")==0.0 && value(this.engine,"intensity")==0.0
            && value(this.engine,"render_throttle")==0.0,"zero delivered energy extinguishes both volume and light"));
        this.stopped_phase=value(this.engine,"transported_distance_m");
    } else if this.phase==4 {
        this.checks.push(t_true(value(this.engine,"transported_distance_m")==this.stopped_phase,"transport stops with the delivered flow"));
        write(this.engine,[["nozzle_exit_radius_m",0.2],["thrust_n",4000.0],
            ["maximum_thrust_n",4000.0],["propellant_flow_kgs",4000.0/2600.0]]);
    } else if this.phase==5 {
        this.checks += conservation(this.engine,4000.0/2600.0);
        this.checks.push(t_true((station(this.engine,"radius_m",4)/this.radius-2.0).abs()<1.0e-6,
            "geometric similarity doubles the profile radii"));
        this.checks.push(t_true((station(this.engine,"density_kg_m3",0)-this.rho).abs()<1.0e-9
            && (station(this.engine,"emission_w_m3",0)/this.emission-0.5).abs()<1.0e-6,
            "geometric similarity preserves mass flux and scales volumetric emission"));
        write(this.engine,[["thrust_n",0.0],["propellant_flow_kgs",0.0]]);
    } else {
        let mesh=query("QueryUsdPrim",#{path:"/World/Engine/Exhaust/Flame",attrs:[]});
        this.checks.push(t_true(mesh.type_name=="Cylinder","expanding gas uses a non-tapering proxy"));
        this.done=true;report_verdict(this.checks,"PLUME CONSERVATION","ROCKET_ENGINE_PLUME_DEFAULTS");return;
    }
    this.phase+=1;
}
