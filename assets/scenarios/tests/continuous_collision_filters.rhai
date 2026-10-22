// Continuous sweeps honor the same authored exclusions as discrete contacts.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
 let ids=[];
 for name in ["FilteredFast","JointFast","FilteredWall","JointWall","FilteredStop","JointStop","A","B"] {
  ids.push(find_path("/World/"+name));
 }
 #{modelica_entities:[],entity_reads:ids,entity_writes:[],query_reads:[],required_inputs:[]}
}
fn on_start(me,ctx) {
 this.start=sim_tick();this.done=false;this.max_a=2.0;this.max_b=0.0;
 this.filtered=find_path("/World/FilteredFast");this.jointed=find_path("/World/JointFast");
}
fn on_tick(me,ctx) {
 if this.done {return;}
 this.max_a=max(this.max_a,world_pos3(find_path("/World/A")).x);
 this.max_b=max(this.max_b,world_pos3(find_path("/World/B")).x);
 if sim_tick()-this.start<30 {return;}
 let filtered=world_pos3(this.filtered).x;let jointed=world_pos3(this.jointed).x;
 let a=world_pos3(find_path("/World/A")).x;let b=world_pos3(find_path("/World/B")).x;
 let checks=[
  t_true(filtered>2.3 && filtered<2.401,"the explicit filtered pair passes its excluded wall and stops at the solid wall"),
  t_true(jointed>2.3 && jointed<2.401,"a collision-disabled joint excludes its partner from continuous sweeps"),
  t_true(contact_debug(this.filtered,find_path("/World/FilteredWall")).touching_pair_count==0,"the filtered pair generates no solid contact"),
  t_true(contact_debug(this.jointed,find_path("/World/JointWall")).touching_pair_count==0,"the joint-disabled pair generates no solid contact"),
  t_true(contact_debug(this.filtered,find_path("/World/FilteredStop")).touching_pair_count>0
    && contact_debug(this.jointed,find_path("/World/JointStop")).touching_pair_count>0,"both admitted solid walls retain real contacts"),
  t_true(this.max_a>2.3 && this.max_a<2.401 && this.max_b>2.0 && this.max_b<2.401,
    "unfiltered high-speed collisions advance and never cross the wall")
 ];
 emit("CONTINUOUS_COLLISION_FILTERS_EVIDENCE",#{filtered_position_x:filtered,jointed_position_x:jointed,control_a_x:a,control_b_x:b});
 this.done=true;report_verdict(checks,"Continuous collision filters","CONTINUOUS_COLLISION_FILTERS");
}
