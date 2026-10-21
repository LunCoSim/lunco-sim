// Independent point-by-point oracle for PropellantMassContracts.mo.
fn simulation_dependencies(me, params) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn on_start(me, params) {this.done=false;this.run=params.contract_run;}
fn on_tick(me, params) {
    if this.done {return;}
    let status=query("RunStatus",#{experiment_id:this.run});
    if status.state=="failed" || status.state=="cancelled" {
        this.done=true;fail_fast(status.to_string(),"PROPELLANT MASS CONTRACTS","PROPELLANT_MASS_CONTRACTS");return;
    }
    if status.state!="done" {return;}
    this.done=true;
    let fields=["vehicle_mass_kg","vehicle_com_x_m","vehicle_com_y_m","vehicle_com_z_m",
        "vehicle_inertia_xx_kg_m2","vehicle_inertia_yy_kg_m2","vehicle_inertia_zz_kg_m2"];
    let names=[];
    for group in ["distributed","point","empty"] {for field in fields {names.push(group+"."+field);}}
    let result=query("GetExperimentResult",#{experiment_id:this.run,variables:names,max_points:64});
    if result==() || result.times==() || result.times.len()!=21
        || abs(result.times[20]-1.0)>1.0e-8 {
        fail_fast("expected complete 21-sample depletion trajectory","PROPELLANT MASS CONTRACTS","PROPELLANT_MASS_CONTRACTS");return;
    }
    let checks=[];
    for row in 0..result.times.len() {
        let t=result.times[row];let fuel=max(0.0,1000.0*(1.0-t));
        let oxidizer=max(0.0,1000.0*(1.0-t)**2);
        // Contrived fixture stations; no species-group reduction is reused.
        let loads=[#{mass:2000.0,p:[0.2,0.1,-0.3]},
            #{mass:fuel/2.0,p:[0.85,1.39,0.0]},#{mass:fuel/2.0,p:[-0.85,1.39,0.0]},
            #{mass:oxidizer/2.0,p:[0.0,1.39,0.85]},#{mass:oxidizer/2.0,p:[0.0,1.39,-0.85]}];
        let mass=0.0;for load in loads {mass+=load.mass;}
        let com=[];
        for i in 0..3 {let moment=0.0;for load in loads {moment+=load.mass*load.p[i];}com.push(moment/mass);}
        let expected=[mass,com[0],com[1],com[2]];
        let dry=[4625.0,6250.0,4625.0];
        for i in 0..3 {
            let inertia=dry[i];
            for load in loads {for j in 0..3 {if j!=i {inertia+=load.mass*(load.p[j]-com[j])**2;}}}
            expected.push(inertia);
        }
        let cases=[#{name:"distributed",expected:expected},
            #{name:"point",expected:[4000.0,0.0,0.0,0.0,5905.0,6250.0,5905.0]},
            #{name:"empty",expected:[2000.0,0.0,0.0,0.0,4625.0,6250.0,4625.0]}];
        for c in cases {for i in 0..fields.len() {
            let values=result.series[c.name+"."+fields[i]];
            let valid=values!=() && values.len()==result.times.len();
            let observed=if valid {values[row]} else {()};
            checks.push(t_true(valid && observed!=() && observed.is_finite()
                && abs(observed-c.expected[i])<1.0e-6,c.name+" "+fields[i]+" at t="+t));
        }}
    }
    report_verdict(checks,"PROPELLANT MASS CONTRACTS","PROPELLANT_MASS_CONTRACTS");
}
