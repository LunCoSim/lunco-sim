// Production gate attached to an independent scene observer. Exercises the
// default live waypoint actions through the same tool and sensor owners used by
// an interactive route, while preserving the authored document generation.
const TEST_KIND = "editor";
import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;

fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],required_inputs:[],
        query_reads:["QueryUsdPrim","QueryUsdPrims","ResolveUsdTarget",
            "QueryRuntimeMarkers","InspectUsdCurveView","SensorOccupants","InspectSelection","QueryPhysicsState"]}
}

fn finish(state) {
    report_verdict(state.checks,"LIVE ROUTE PLAN","LIVE_ROUTE_PLAN");
    state.done=true;
    state
}
fn check(state, ok, message) {
    state.checks.push(t_true(ok,message));
    if !ok {state=finish(state);}
    state
}
fn on_start(me,ctx) {
    this.done=false;
    this.checks=[];
    this.phase=0;
    this.ticks=0;
    this.doc=ctx.doc_id;
    this.route=ctx.route_path;
    this.root=ctx.scene_root_path;
    this.source=waypoint_editor::route_positions(this.doc,this.route);
    if this.source.ok!=true || this.source.paths.len()<2 {this=check(this,false,"authored route fixture unavailable");return;}
    this.generation=usd_document_generation(this.doc);
    this.first=this.source.paths[0];
    this.first_id=find_path_in_document(this.doc,this.first);
    this.owner=find_path_in_document(this.doc,this.route);
    track_entity_read(this.first_id);
    this.original_position=world_pos(this.first_id);
    this.target=vadd(this.original_position,[9.0,0.0,8.0]);
    this.moved_target=vadd(this.target,[2.0,0.0,0.0]);
    this.source_move=vadd(this.original_position,[3.0,0.0,2.0]);
}
fn source_unchanged(state) {
    let current=waypoint_editor::route_positions(state.doc,state.route);
    current.ok==true && current.paths==state.source.paths && current.points==state.source.points
        && usd_document_generation(state.doc)==state.generation
}
fn query_markers(state) {
    track_entity_read(state.owner);
    query("QueryRuntimeMarkers",#{owner:state.owner}).markers
}
fn view(state) {
    let id=waypoint_editor::ribbon_entity(state.doc,state.route);
    if id<0 {return ();}
    query("InspectUsdCurveView",#{doc_id:state.doc,entity_id:id})
}
fn matches(p,q) {p!=() && q!=() && vlen(vsub(p,q))<0.05}
fn move_point(state,key,position) {
    waypoint_editor::on_move(#{doc_id:state.doc,route_path:state.route,route_point_key:key,
        target_path:key,hit_path:key,prim_paths:[key],world_position:point3(position,"active_physics")})
}
fn delete_point(state,key) {
    waypoint_editor::on_delete(#{doc_id:state.doc,route_path:state.route,route_point_key:key,
        selected_path:key,target_path:key,hit_path:key,prim_paths:[key]})
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.ticks+=1;
    if this.ticks>1800 {this=check(this,false,"live route plan timed out in phase "+this.phase.to_string());return;}
    let plan=waypoint_editor::live_route_snapshot(this.doc,this.route);
    if plan.ok!=true {return;}
    let writer=waypoint_editor::live_route_state(this.doc,this.route);
    if writer==() || writer.live_route_pending!=() {return;}
    let curve=view(this);
    if curve==() || curve.state!="ready" {return;}
    if this.phase==0 {
        this.curve_id=waypoint_editor::ribbon_entity(this.doc,this.route);
        let add=waypoint_editor::on_click(#{button:"primary",doc_id:this.doc,route_path:this.route,
            scene_root_path:this.root,surface_world_position:point3(this.target,"active_physics")});
        this=check(this,add.ok==true && add.intent=="Interactive","default terrain add admits a live route change");
        this.phase=1;return;
    }
    if this.phase==1 {
        if plan.paths.len()!=this.source.paths.len()+1 || to_int(curve.result_segment_count)!=plan.paths.len()-1 {return;}
        this.key=plan.paths[plan.paths.len()-1];
        this.binding=plan.bindings[this.key];
        let native=query_markers(this)[this.binding.native_key];
        this=check(this,native!=() && native.marker_id.to_int()==this.binding.point_id
            && native.sensor_id.to_int()==this.binding.sensor_id,"native added waypoint has independent visual and sensor identities");
        this=check(this,source_unchanged(this) && waypoint_editor::ribbon_entity(this.doc,this.route)==this.curve_id,
            "live addition keeps USD and the existing curve identity unchanged");
        track_entity_write(this.binding.point_id);
        this.direct=cmd("DeleteEntity",#{target:this.binding.point_id});
        track_entity_write(this.owner);
        this.competing=cmd("SetRuntimeMarker",#{owner:this.owner,key:this.binding.native_key,marker:()});
        this.phase=10;return;
    }
    if this.phase==10 {
        if this.direct.status=="pending" {let r=command_result(this.direct.id);if r!=(){this.direct=r;}}
        if this.competing.status=="pending" {let r=command_result(this.competing.id);if r!=(){this.competing=r;}}
        if this.direct==() || this.competing==() || this.direct.status=="pending" || this.competing.status=="pending" {return;}
        this=check(this,this.direct.ok==false && this.direct.error.contains("SetRuntimeMarker"),"generic deletion cannot bypass native marker ownership");
        if this.done {return;}
        this=check(this,this.competing.ok==false && this.competing.error.contains("different scenario"),"a competing actor cannot retire another route's marker");
        if this.done {return;}
        let moved=move_point(this,this.key,this.moved_target);
        this=check(this,moved.ok==true,"native point move is admitted through the default tool");
        this.phase=2;return;
    }
    if this.phase==2 {
        let native=query_markers(this)[this.binding.native_key];
        if native==() || !matches(native.center,this.moved_target) {return;}
        this=check(this,native.marker_id.to_int()==this.binding.point_id && native.sensor_id.to_int()==this.binding.sensor_id,
            "moving a live point preserves both marker and sensor identity");
        track_entity_read(this.binding.sensor_id);
        this=check(this,matches(world_pos(this.binding.sensor_id),this.moved_target),"live overlap sensor follows its waypoint");
        this=check(this,source_unchanged(this),"native point movement leaves authored route unchanged");
        let moved=move_point(this,this.first,this.source_move);
        this=check(this,moved.ok==true,"existing authored waypoint can receive a live move");
        this.phase=3;return;
    }
    if this.phase==3 {
        if !matches(world_pos(this.first_id),this.source_move) {return;}
        this=check(this,source_unchanged(this),"existing waypoint movement does not author its pose");
        this.color_revision=curve.requested_revision;
        let color=waypoint_editor::on_set_color(#{doc_id:this.doc,route_path:this.route,color_path:this.key,color:[0.6,0.1,0.2]});
        this=check(this,color.ok==true,"point color is live by default");
        this.phase=4;return;
    }
    if this.phase==4 {
        if plan.bindings[this.key].color!=[0.6,0.1,0.2] {return;}
        this=check(this,curve.requested_revision==this.color_revision && source_unchanged(this),
            "live marker color does not rebuild the ribbon or edit USD");
        let deleted=delete_point(this,this.key);
        this=check(this,deleted.ok==true,"native waypoint deletion is admitted");
        this.phase=5;return;
    }
    if this.phase==5 {
        if plan.paths.contains(this.key) || to_int(curve.result_segment_count)!=plan.paths.len()-1 {return;}
        this=check(this,!query_markers(this).contains(this.binding.native_key) && source_unchanged(this),
            "deleting a live waypoint retires its native objects only");
        let deleted=delete_point(this,this.first);
        this=check(this,deleted.ok==true,"authored waypoint can be removed from the live plan without authoring");
        this.phase=6;return;
    }
    if this.phase==6 {
        if plan.paths.contains(this.first) || to_int(curve.result_segment_count)!=plan.paths.len()-1 {return;}
        this=check(this,source_unchanged(this),"removing an authored point from the live plan does not delete its source");
        let reset=waypoint_editor::on_reset_live_route(#{doc_id:this.doc,route_path:this.route});
        this=check(this,reset.ok==true,"explicit reset of the live plan is admitted");
        this.phase=7;return;
    }
    if this.phase==7 {
        if writer.live_route!=() || plan.paths!=this.source.paths || !matches(world_pos(this.first_id),this.original_position)
            || to_int(curve.result_segment_count)!=plan.paths.len()-1 {return;}
        this=check(this,query_markers(this).len()==0 && source_unchanged(this)
            && waypoint_editor::ribbon_entity(this.doc,this.route)==this.curve_id,
            "reset restores source poses and retires runtime markers without any document projection");
        this=finish(this);
    }
}
