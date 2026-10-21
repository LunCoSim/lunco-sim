// Production observation of continuous contacts and fixed-joint attachment.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[find_path("/World/A"),find_path("/World/B"),find_path("/World/Wall"),find_path("/World/Pivot"),find_path("/World/Link"),find_path("/World/ArcObstacle"),find_path("/World/Touching"),find_path("/World/Overlapping"),find_path("/World/TouchingWall"),find_path("/World/OverlappingWall")],entity_writes:[],query_reads:["QueryPhysicsState"],required_inputs:[]}
}
fn on_start(me,ctx) {this.start=sim_tick();this.max_anchor_error=0.0;this.max_rotation_error=0.0;this.max_penetration=0.0;this.max_arc_penetration=0.0;this.samples=0;this.done=false;this.maximum_initial_x=2.45;this.maximum_a_x=2.0;this.maximum_b_x=0.0;this.minimum_gap=2.0;}
fn on_tick(me,ctx) {
    if this.done {return;}
    let a=find_path("/World/A");let b=find_path("/World/B");let wall=find_path("/World/Wall");
    let pa=world_pos3(a);let pb=world_pos3(b);
    this.maximum_a_x=max(this.maximum_a_x,pa.x);
    this.maximum_b_x=max(this.maximum_b_x,pb.x);
    this.minimum_gap=min(this.minimum_gap,pa.x-pb.x);
    for name in ["Touching","Overlapping"] {this.maximum_initial_x=max(this.maximum_initial_x,world_pos3(find_path("/World/"+name)).x);}
    let pivot=find_path("/World/Pivot");let link=find_path("/World/Link");
    let anchor=world_pos3(pivot)+qrot(world_rotation_quat(pivot),vec3(1.0,0.0,0.0));
    this.max_anchor_error=max(this.max_anchor_error,vlen(anchor-world_pos3(link)));
    for axis in [vec3(1.0,0.0,0.0),vec3(0.0,1.0,0.0),vec3(0.0,0.0,1.0)] {
        this.max_rotation_error=max(this.max_rotation_error,vlen(qrot(world_rotation_quat(pivot),axis)-qrot(world_rotation_quat(link),axis)));
    }
    let pair=contact_debug(a,b);if pair==() {throw("continuous contact facts unavailable");}
    this.max_arc_penetration=max(this.max_arc_penetration,contact_debug(link,find_path("/World/ArcObstacle")).max_penetration);
    this.max_penetration=max(this.max_penetration,pair.max_penetration);this.samples+=1;
    if sim_tick()-this.start<60 {return;}
    let body=query("QueryPhysicsState",#{id:a});
    let overlapping=query("QueryPhysicsState",#{id:find_path("/World/Overlapping")});
    this.done=true;
    report_verdict([
        t_true(this.samples>=60,"every admitted physics tick was observed"),
        t_true(body.physics_state_ready==true && body.collider_present==true,"dynamic collider admission is complete"),
        t_true(this.maximum_a_x<=2.4001 && this.maximum_b_x<=2.4001 && this.minimum_gap>=0.1999,"coupled contacts preserve body order and the fixed wall"),
        t_true(this.maximum_a_x>2.3 && this.maximum_b_x>2.0,"the admitted high-speed bodies actually advanced"),
        t_true(this.max_anchor_error<0.001,"the rotating fixed joint preserves its offset anchor"),
        t_true(this.max_rotation_error<0.001,"the fixed joint preserves its relative orientation"),
        t_true(this.max_penetration<0.001,"the exact moving pair stays within contact tolerance"),
        t_true(this.max_arc_penetration<0.001,"the rotating link and obstacle retain their native contact tolerance"),
        t_true(contact_debug(a,pivot).touching_pair_count==0,"pair evidence excludes an unrelated body"),
        t_true(this.maximum_initial_x<3.4,"initial touching and overlap do not cross the far wall face"),
        t_true(world_pos3(find_path("/World/Touching")).x<=2.401 && contact_debug(find_path("/World/Touching"),find_path("/World/TouchingWall")).max_penetration<0.001,"the native discrete solver recovers initial touching contact"),
        t_true(overlapping.physics_initialization_paused && overlapping.rigid_body_disabled && overlapping.collider_disabled && (world_pos3(find_path("/World/Overlapping")).x-2.45).abs()<1e-6,"invalid initial overlap is paused in its authored pose"),
    ],"Continuous contacts","CONTINUOUS_CONTACTS");
    print("CONTINUOUS CONTACTS "+#{a:vec3_array(pa),b:vec3_array(pb),maximum_anchor_error_m:this.max_anchor_error,maximum_penetration_m:this.max_penetration,maximum_initial_contact_x:this.maximum_initial_x,maximum_a_x:this.maximum_a_x,maximum_b_x:this.maximum_b_x,minimum_pair_gap:this.minimum_gap});
}
