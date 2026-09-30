// Exercise the production Modelica/library connection boundary. No visual state
// is calculated here: only physical inputs, observations and independent ratios.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities:[find_path("/World/Engine")],
      entity_reads:[],entity_writes:[],query_reads:["QueryUsdPrim"],required_inputs:[]}
}
fn on_start(me, ctx) {
    // Observe the generated network boundary, not its fused member entity.
    // This catches defaults that initialize correctly but reset on first step.
    this.engine=find_path("/World/Engine");
    this.phase=0;this.elapsed=0.0;this.failures=[];this.done=false;
}
fn write(engine,writes) {
    let result=cmd("SetPorts",#{target:engine,writes:writes});
    if !result.ok {throw("physical input write rejected");}
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.elapsed+=dt();if this.elapsed<0.25 {return;}this.elapsed=0.0;
    let throttle=get(this.engine,"render_throttle");
    let intensity=get(this.engine,"intensity");
    let length=get(this.engine,"full_throttle_length_m");
    let radius=get(this.engine,"envelope_radius_m");
    let color_r=get(this.engine,"color_r");let color_b=get(this.engine,"color_b");
    if this.phase==0 {
        this.failures.push(t_true(throttle==0.0 && intensity==0.0,"an idle engine emitted light"));
        this.failures.push(t_true(length>0.0 && radius>0.0,"default nozzle/thrust values did not produce an envelope"));
        write(this.engine,[["throttle",1.0],["thrust_n",1000.0],["propellant_flow_kgs",1000.0/2600.0],["exhaust_velocity_mps",2600.0]]);
    } else if this.phase==1 {
        this.failures.push(t_true(throttle>0.99 && intensity>0.0,"delivered thrust did not reveal the plume"));
        this.length=length;this.radius=radius;
        write(this.engine,[["maximum_thrust_n",4000.0],["thrust_n",4000.0],["propellant_flow_kgs",4000.0/2600.0]]);
    } else if this.phase==2 {
        this.failures.push(t_true((length/this.length-2.0).abs()<0.002,"four times thrust did not double the pressure-law length"));
        this.high_length=length;
        write(this.engine,[["nozzle_exit_radius_m",0.20]]);
    } else if this.phase==3 {
        this.failures.push(t_true((radius/this.radius-2.0).abs()<0.002,"twice the nozzle radius did not double plume width"));
        this.failures.push(t_true((length/this.high_length-1.0).abs()<0.002,"momentum/area length invariance was lost"));
        write(this.engine,[["fuel_family",1.0],["mixture_mode",-1.0]]);
    } else if this.phase==4 {
        this.failures.push(t_true(color_r>color_b,"fuel-rich hydrocarbon palette was not warm"));
        this.rich_blue=color_b;write(this.engine,[["mixture_mode",1.0]]);
    } else if this.phase==5 {
        this.failures.push(t_true(color_b>this.rich_blue,"oxidizer-rich palette did not shift away from warm soot colour"));
        write(this.engine,[["fuel_family",2.0],["mixture_mode",0.0]]);
    } else if this.phase==6 {
        this.failures.push(t_true(color_b>color_r,"hydrogen palette was not distinct from hydrocarbon"));
        write(this.engine,[["propellant_flow_kgs",0.0]]);
    } else {
        this.failures.push(t_true(throttle==0.0 && intensity==0.0 && get(this.engine,"visual_length_fraction")==0.0,
            "zero fuel flow left a plume with a stale positive thrust signal"));
        let mesh=query("QueryUsdPrim",#{path:"/World/Engine/Exhaust/Flame",connections:true,relationships:true,attrs:[]});
        this.failures.push(t_true(mesh.type_name=="Cone","library engine did not inherit its exhaust geometry"));
        this.done=true;
        report_verdict(this.failures,"ROCKET ENGINE PLUME DEFAULTS","ROCKET_ENGINE_PLUME_DEFAULTS");
        return;
    }
    this.phase+=1;
}
