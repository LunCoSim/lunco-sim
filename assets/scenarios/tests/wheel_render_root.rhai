// Wheel presentation follows the mechanical axle independently of mesh basis.
import "lunco://scripting/tools/asset_preflight" as preflight;
const ROOT="/WheelRenderRoot";
const INVALID="lunco://scenes/tests/wheel_render_root_invalid.usda";
fn simulation_dependencies(me,ctx) {
    let reads=[];
    for kind in ["RoverPhysical","RoverRaycast"] {
        for entity in vessel_descendants(find_path(ROOT+"/"+kind)) {reads.push(entity);}
    }
    #{modelica_entities:[find_path(ROOT+"/RoverPhysical"),find_path(ROOT+"/RoverRaycast")],
      entity_reads:reads,entity_writes:[find_path(ROOT+"/RoverPhysical"),find_path(ROOT+"/RoverRaycast")],
      query_reads:["ValidateAsset"],required_inputs:[]}
}
fn relative(a,b) {
    // conjugate(a) * b; both public world rotations use x,y,z,w.
    [-a[0]*b[3]+a[3]*b[0]-a[1]*b[2]+a[2]*b[1],
     -a[1]*b[3]+a[3]*b[1]-a[2]*b[0]+a[0]*b[2],
     -a[2]*b[3]+a[3]*b[2]-a[0]*b[1]+a[1]*b[0],
      a[3]*b[3]+a[0]*b[0]+a[1]*b[1]+a[2]*b[2]]
}
fn on_start(me,ctx) {
    this.time=0.0;this.done=false;this.phase="motion";this.checks=[];
    this.preflight=preflight::create([INVALID]);
    for kind in ["RoverPhysical","RoverRaycast"] {drive(find_path(ROOT+"/"+kind),1.0,0.0);}
}
fn on_tick(me,ctx) {
    if this.done {return;}this.time+=dt();
    if this.time>30.0 {
        this.done=true;fail_fast("Wheel render contract timed out at "+this.phase,"WHEEL RENDER ROOT","WHEEL_RENDER_ROOT");return;
    }
    if this.phase=="lint" {
        this.preflight=preflight::advance(this.preflight);
        if this.preflight.error!=() {
            this.done=true;fail_fast(this.preflight.error,"WHEEL RENDER ROOT","WHEEL_RENDER_ROOT");return;
        }
        if !this.preflight.done {return;}
        let report=this.preflight.reports[INVALID];
        for entry in [
            [ROOT+"/RoverPhysical/Suspension_FL/Wheel_FL","overlap"],
            [ROOT+"/RoverRaycast/Suspension_FL/Wheel_FL","one render root"],
            [ROOT+"/RoverRaycast/Suspension_FR/Wheel_FR","centered on the axle"],
        ] {
            let found=false;
            for finding in report.findings {
                if finding.rule=="wheel-visual-contract" && finding.severity=="error"
                    && finding.subject==entry[0] && finding.message.contains(entry[1]) {found=true;}
            }
            this.checks.push(t_true(found,"invalid wheel visual rejected visibly: "+entry[1]));
        }
        let disabled_ok=true;
        for finding in report.findings {
            if finding.rule=="raycast-wheel-collision-contract"
                && finding.subject==ROOT+"/RoverRaycast/Suspension_RR/Wheel_RR" {disabled_ok=false;}
        }
        this.checks.push(t_true(disabled_ok,"explicitly disabled collision metadata is valid on query-only wheels"));
        this.done=true;
        report_verdict(this.checks,"WHEEL RENDER ROOT","WHEEL_RENDER_ROOT");return;
    }
    if this.time<1.0 {return;}
    let checks=[];
    for kind in ["RoverPhysical","RoverRaycast"] {
        let wheel=find_path(ROOT+"/"+kind+"/Suspension_FL/Wheel_FL");
        let visual=find_path(ROOT+"/"+kind+"/Suspension_FL/Wheel_FL/Render");
        let component=if kind=="RoverPhysical" {"PhysicalWheel"} else {"WheelRaycast"};
        checks.push(t_true(get(wheel,component+".visual_entity").to_string()!="None",
            kind+": authored render root is bound to wheel presentation"));
        checks.push(t_true(get(wheel,"Visibility").to_string().contains("Hidden"),
            kind+": analytic proxy stays hidden"));
        if kind=="RoverRaycast" {
            let spin=get(wheel,"WheelRaycast.spin_angle");
            let q=relative(world_rotation(wheel),world_rotation(visual));
            checks.push(t_true(abs(get(wheel,"WheelRaycast.spin_velocity"))>0.1,
                "raycast drivetrain produces measured shaft motion"));
            checks.push(t_true(abs(abs(q[3])-abs(cos(spin/2.0)))<0.0001
                && abs(q[1])<0.0001 && abs(q[2])<0.0001 && abs(q[0])>0.01,
                "X-axis visual mesh rolls about the authored axle with its measured spin"));
        }
        drive(find_path(ROOT+"/"+kind),0.0,0.0);
    }
    this.checks=checks;this.phase="lint";
}
