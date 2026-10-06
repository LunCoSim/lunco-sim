// TEST-ONLY: run on a loaded Connections view with an expanded inferred group.
// Source identity and member paths come from the production query, never fixtures.
fn simulation_dependencies(me,ctx) {#{modelica_entities:[],entity_reads:[],entity_writes:[],query_reads:["InspectConnectionDiagram"],required_inputs:[]}}
fn on_start(me,ctx) {this.phase=0;this.elapsed=0.0;this.done=false;this.checks=[];}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.elapsed+=dt();
    if this.elapsed>20.0 {this.checks.push("group runtime gate timed out at phase "+this.phase);this.done=true;report_verdict(this.checks,"DIAGRAM GROUP RUNTIME","DIAGRAM_GROUP_RUNTIME");return;}
    let q=query("InspectConnectionDiagram",#{});
    if q==() || q.built!=true || q.groups==() {return;}
    if this.phase==0 {
        if q.groups.len()==0 {return;}
        let group=q.groups[0];for candidate in q.groups {if candidate.internal_links>group.internal_links {group=candidate;}}if group.collapsed {return;}
        this.group=group;this.scope=q.scope;this.view_id=q.view_id;this.count=q.connection_count;this.positions=#{};
        for node in q.nodes {if group.members.contains(node.key) {this.positions[node.key]=#{x:node.x,y:node.y};}}
        this.checks.push(t_true(this.positions.len()==group.members.len(),"expanded group exposes every member"));
        cmd("SetConnectionGroupCollapsed",#{view_id:this.view_id,group_id:group.id,collapsed:true});this.phase=1;return;
    }
    let current=();for group in q.groups {if group.id==this.group.id {current=group;}}
    let summary=();for node in q.nodes {if node.key==this.group.key {summary=node;}}
    if this.phase==1 {
        if summary==() {return;}
        this.checks.push(t_true(q.connection_count==this.count,"collapse preserves full USD connection count"));
        this.checks.push(t_true(q.rendered_connection_count==this.count-this.group.internal_links,"collapse accounts for every internal and feedback edge"));
        for node in q.nodes {this.checks.push(t_true(!this.group.members.contains(node.key),"collapsed member is represented by summary"));}
        for port in summary.ports {let exact=false;for path in this.group.members {if port.name.starts_with(path+".") {exact=true;}}this.checks.push(t_true(exact,"summary ports retain exact member property identity"));}
        cmd("MoveConnectionGroup",#{view_id:this.view_id,group_id:this.group.id,dx:80.0,dy:40.0});this.phase=2;return;
    }
    if this.phase==2 {
        let placed=#{};for definition in q.definitions {if definition.name==q.view {for p in definition.positions {placed[p.path]=p;}}}
        let ready=true;for path in this.group.members {if !placed.contains(path) {ready=false;}else {let original=this.positions[path];if (placed[path].x-original.x-80.0).abs()>0.01 || (placed[path].y-original.y-40.0).abs()>0.01 {ready=false;}}}
        if !ready || summary==() || current==() {return;}
        this.checks.push(t_true(ready,"one group move persists every member displacement"));
        cmd("OpenConnectionNode",#{view_id:this.view_id,key:this.group.key});this.phase=3;return;
    }
    if this.phase==3 {
        if summary!=() || current==() || current.collapsed {return;}
        let visible=0;for node in q.nodes {if this.group.members.contains(node.key) {visible+=1;}}
        if visible!=this.group.members.len() {return;}
        this.checks.push(t_true(q.connection_count==this.count && q.scope==this.scope,"expanding keeps USD topology and scope"));
        let rejected=cmd("SetConnectionGroup",#{view_id:this.view_id,group_id:"invalid-test",label:"Invalid",members:[this.group.members[0],"/absent/group/member"]});
        this.checks.push(t_true(rejected.ok==false,"missing USD membership is rejected"));
        cmd("UndoConnectionView",#{view_id:this.view_id,redo:false});this.phase=4;return;
    }
    if this.phase==4 {if summary==() {return;}cmd("UndoConnectionView",#{view_id:this.view_id,redo:false});this.phase=5;return;}
    if this.phase==5 {
        let moved=false;for definition in q.definitions {if definition.name==q.view {for p in definition.positions {if this.positions.contains(p.path) && (p.x-this.positions[p.path].x-80.0).abs()<0.01 {moved=true;}}}}
        if moved || current==() || summary==() {return;}cmd("UndoConnectionView",#{view_id:this.view_id,redo:false});this.phase=6;return;
    }
    if this.phase==6 {
        if summary!=() || current==() || current.collapsed {return;}
        this.checks.push(t_true(q.connection_count==this.count,"view undo leaves USD topology intact"));
        this.done=true;report_verdict(this.checks,"DIAGRAM GROUP RUNTIME","DIAGRAM_GROUP_RUNTIME");
    }
}
