// @timing presentation
// Attach to a settled geodetic site with no local terrain; pass root in params.
fn simulation_dependencies(me,ctx) {
 #{modelica_entities:[],entity_reads:[],entity_writes:[],query_reads:["QueryUsdPrim","RuntimeDiagnostics","GetReadiness"],required_inputs:[]}
}
fn issue(code) {
 for finding in query("RuntimeDiagnostics").findings {
  if finding.producer=="celestial-terrain-handoff" && finding.code==code {return true;}
 }
 false
}
fn add_flat(root,name) {
 let p=root+"/"+name;
 [#{AddPrim:#{edit_target:"@root@",parent_path:root,name:name,type_name:"Plane",reference:(),reference_prim_path:()}},
  #{SetApiSchemas:#{edit_target:"@root@",path:p,schemas:["LunCoTerrainAPI"]}},
  #{SetAttribute:#{edit_target:"@root@",path:p,name:"lunco:terrain:surfaceRole",type_name:"token",value:"\"flat-site\""}},
  #{SetAttribute:#{edit_target:"@root@",path:p,name:"width",type_name:"double",value:"16"}},
  #{SetAttribute:#{edit_target:"@root@",path:p,name:"length",type_name:"double",value:"16"}},
  #{SetAttribute:#{edit_target:"@root@",path:p,name:"axis",type_name:"token",value:"\"Y\""}}]
}
fn on_start(me,ctx) {
 this.phase=0;this.pending=();this.deadline=0.0;this.checks=[];
 if ctx.root==() {throw "The site-frame test requires its exact root path";}
 this.root=ctx.root;this.doc=query("QueryUsdPrim",#{path:this.root}).doc_id;
 if this.doc==() {throw "The site-frame fixture has no live document";}
 this.started=clock_snapshot().wall_elapsed_s;
}
fn on_tick(me,ctx) {
 if this.phase==99 {return;}
 let now=clock_snapshot().wall_elapsed_s;
 if now-this.started>30.0 {throw "Site-frame terrain ownership test timed out";}
 if this.pending!=() {
  let r=command_result(this.pending);if r.status=="pending" {return;}
  if r.status!="applied" || r.ok!=true {throw "Terrain fixture edit was rejected: "+r;}
  this.pending=();this.deadline=now+0.5;
 }
 if now<this.deadline {return;}
 if this.phase==0 {
  if !query("GetReadiness").ready {return;}
  this.checks.push(t_true(!issue("flat-surface-missing") && !issue("flat-surface-cardinality"),"a site coordinate frame does not invent a local surface requirement"));
  let ops=add_flat(this.root,"OwnershipTestA");for op in add_flat(this.root,"OwnershipTestB") {ops.push(op);}
  let r=cmd("ApplyUsdOps",#{doc_id:this.doc,label:"Test duplicate authored flat surfaces",ops:ops});
  if r==() || r.id==() {throw "The negative terrain fixture was not admitted";}
  this.pending=r.id;this.phase=1;return;
 }
 if this.phase==1 {
  this.checks.push(t_true(issue("flat-surface-cardinality"),"two authored local surfaces produce the exact owner diagnostic"));
  let r=cmd("ApplyUsdOps",#{doc_id:this.doc,label:"Retire temporary terrain test surfaces",ops:[
   #{RemovePrim:#{edit_target:"@root@",path:this.root+"/OwnershipTestA"}},
   #{RemovePrim:#{edit_target:"@root@",path:this.root+"/OwnershipTestB"}}]});
  if r==() || r.id==() {throw "The terrain fixture cleanup was not admitted";}
  this.pending=r.id;this.phase=2;return;
 }
 if this.phase==2 {
  this.checks.push(t_true(!issue("flat-surface-cardinality") && !issue("flat-surface-missing") && query("GetReadiness").ready,"removing the local surfaces restores the valid body-only site frame"));
  report_verdict(this.checks,"SITE FRAME WITHOUT LOCAL TERRAIN","SITE_FRAME_WITHOUT_LOCAL_TERRAIN");this.phase=99;
 }
}
