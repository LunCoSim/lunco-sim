// A loaded support and its unloaded control on the same Coulomb-friction slope.
// No control input, pose overwrite or velocity correction is applied.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
 let ids=[];for name in ["Load","Foot","Control","Slider"] {ids.push(find_path("/World/"+name));}
 #{modelica_entities:[],entity_reads:ids,entity_writes:[],query_reads:["QueryPhysicsState"],required_inputs:[]}
}
fn on_start(me,ctx) {
 this.start=sim_tick();this.samples=0;this.done=false;
 this.max_relative_rate=0.0;this.max_relative_angle=0.0;
 this.loaded_drift=0.0;this.control_drift=0.0;
 this.load=find_path("/World/Load");this.foot=find_path("/World/Foot");
 this.control=find_path("/World/Control");this.slider=find_path("/World/Slider");
}
fn on_tick(me,ctx) {
 if this.done {return;}
 let tick=sim_tick()-this.start;
 if tick<60 {return;}
 let lp=world_pos3(this.foot);let cp=world_pos3(this.control);
 if this.samples==0 {this.loaded_start=lp;this.control_start=cp;}
 this.samples+=1;
 this.loaded_drift=max(this.loaded_drift,vlen(lp-this.loaded_start));
 this.control_drift=max(this.control_drift,vlen(cp-this.control_start));
 let load=query("QueryPhysicsState",#{id:this.load});
 let foot=query("QueryPhysicsState",#{id:this.foot});
 let relative_rate=vlen(vec3_from(foot.angular_velocity_radps)-vec3_from(load.angular_velocity_radps));
 let q=quat_inverse(world_rotation_quat(this.load))*world_rotation_quat(this.foot);
 let relative_angle=2.0*acos(min(1.0,abs(q.w)));
 this.max_relative_rate=max(this.max_relative_rate,relative_rate);
 this.max_relative_angle=max(this.max_relative_angle,relative_angle);
 if tick<360 {return;}
 this.done=true;
 let control=query("QueryPhysicsState",#{id:this.control});
 let evidence=#{samples:this.samples,loaded_drift_m:this.loaded_drift,control_drift_m:this.control_drift,
  maximum_relative_angular_rate_radps:this.max_relative_rate,maximum_relative_angle_rad:this.max_relative_angle,
  final_foot_velocity_mps:foot.linear_velocity_mps,final_control_velocity_mps:control.linear_velocity_mps,
  slider_displacement_m:get(this.slider,"displacement"),slider_velocity_mps:get(this.slider,"velocity"),
  slider_force_N:get(this.slider,"force"),
  loaded_contact_force_N:port(this.foot,"contact_force"),control_contact_force_N:port(this.control,"contact_force"),loaded_contact:contact_debug(this.foot),control_contact:contact_debug(this.control)};
 emit("LOADED_SLIDER_SLOPE_EVIDENCE",evidence);
 report_verdict([
  t_true(this.control_drift<0.002,"an unloaded support sticks on a subcritical friction slope"),
  t_true(vlen(vec3_from(control.linear_velocity_mps))<0.001,"the unloaded support settles without a controller"),
  t_true(this.loaded_drift<0.002,"joint-transmitted load preserves static friction"),
  t_true(this.max_relative_angle<0.0001,"the slider locks relative rotation"),
  t_true(this.max_relative_rate<0.001,"the completed slider velocities respect locked relative rotation"),
  t_true(port(this.foot,"contact_force")!=() &&
    abs(port(this.foot,"contact_force")/((4000.0+180.0)*1.62*cos(0.12))-1.0)<0.02,
    "the contact reaction equals the supported weight independently of solver pass count"),
  t_true(port(this.control,"contact_force")!=() &&
    abs(port(this.control,"contact_force")/(180.0*1.62*cos(0.12))-1.0)<0.02,
    "the control reaction uses the same native physical impulse units")
 ],"Loaded slider slope","LOADED_SLIDER_SLOPE");
}
