// @timing presentation
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["ListOpenDocuments","InspectUsdDocument","QueryUsdPrim","QueryPhysicsState",
        "GetReadiness","RuntimeDiagnostics","CompileStatus","GetDocumentSource"],required_inputs:[]}
}
fn attr(path,name,kind,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}
}
fn prim(parent,name,kind,schemas) {
    [#{AddPrim:#{edit_target:"@runtime@",parent_path:parent,name:name,type_name:kind,reference:()}},
     #{SetApiSchemas:#{edit_target:"@runtime@",path:parent+"/"+name,schemas:schemas}}]
}
fn native(path) {query("QueryPhysicsState",#{id:find_path(path)})}
fn terminal(value) {
    if value!=() && value.status=="pending" && value.id!=() {return command_result(value.id);}
    value
}
fn finish(state,checks) {
    if state.scratch!=() {editor::close_scratch(state.scratch);}
    state.done=true;report_verdict(checks,"SIMULATION CORE SECOND PASS","SIMULATION_CORE_SECOND_PASS");state
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,phase:0,scratch:(),checks:[],test_case:ctx.test_case,doc:ctx.doc_id,
        stable_tick:(),stable_frames:0};
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>900 {this=finish(this,["second-pass case timed out: "+this.test_case+" phase "+this.phase]);return;}
    try {
        if this.phase==0 {
            if this.test_case=="axis-z" {
                let direction=vec3_array(get(find_path("/World/Source/AuditMount"),"ForceActuator.direction_local"));
                this=finish(this,[t_true(abs(direction[0])<0.000001 && abs(direction[1])<0.000001
                    && abs(direction[2]+1.0)<0.000001,"Z-up actuator direction must be canonical -Z"),
                    t_true(!query("GetReadiness").faulted,"actuation fixture must remain admitted")]);return;
            }
            if this.test_case=="unknown-gid" {
                let raw=query("QueryUsdPrim",#{path:"/World/ForceBody",topology:true}).topology.binding.entity;
                let body=find_path("/World/ForceBody");let before=native("/World/ForceBody").mass_kg;
                let result=cmd("ReleasePort",#{target:raw,name:"unknown_identity_probe"});
                this=finish(this,[t_true(raw!=body,"fixture needs distinct local bits and global identity"),
                    t_true(result.ok==false && result.error.contains("unknown global entity ID"),
                        "unregistered global identity must reject before dispatch: "+result),
                    t_true(native("/World/ForceBody").mass_kg==before,"rejected identity must preserve the body"),
                    t_true(!query("GetReadiness").faulted,"identity rejection must preserve the application")]);return;
            }
            if this.test_case=="number-overflow" {
                let created=editor::new_scratch("model FiniteNumberProbe end FiniteNumberProbe;","FiniteNumberProbe");
                if !created.ok {throw("scratch creation failed: "+created);}
                this.scratch=created.doc_id;this.phase=10;return;
            }
            let ops=[];
            if this.test_case=="live-units" {
                ops=[#{SetStageMassUnits:#{edit_target:"@runtime@",kilograms_per_unit:0.001}}];
                this.before=native("/World/Target");
            }
            if this.test_case=="scale-small-valid" {
                ops=[attr("/World/ForceBody","physics:mass","float","1e10"),
                    #{SetScale:#{edit_target:"@runtime@",path:"/World/ForceBody",value:[1e-13,1e-13,1e-13]}}];
            }
            if this.test_case=="scale-tiny" {
                ops=[#{RemoveAttribute:#{edit_target:"@root@",path:"/World/ForceBody",name:"physics:mass"}},
                    #{SetScale:#{edit_target:"@runtime@",path:"/World/ForceBody",value:[3e-9,3e-9,3e-9]}}];
            }
            if this.test_case=="inertia-tiny" {
                ops=[attr("/World/ForceBody","physics:diagonalInertia","float3","(1e-45, 1e-45, 1e-45)")];
            }
            if this.test_case=="mass-negative" || this.test_case=="mass-tiny" {
                ops=[attr("/World/ForceBody","physics:mass","float",if this.test_case=="mass-tiny" {"1e-45"} else {"-1"})];
            }
            if this.test_case=="mass-sentinels" {
                ops=[attr("/World/ForceBody","physics:mass","float","0"),
                    attr("/World/ForceBody","physics:density","float","0"),
                    attr("/World/ForceBody","physics:diagonalInertia","float3","(0, 0, 0)"),
                    attr("/World/ForceBody","physics:principalAxes","quatf","(0, 0, 0, 0)")];
            }
            if this.test_case=="implicit-density" {
                ops=[#{RemoveAttribute:#{edit_target:"@root@",path:"/World/ForceBody",name:"physics:mass"}}];
            }
            if this.test_case=="held-delete" {
                this.pending=cmd("SetPorts",#{target:find_path("/World/ForceBody"),writes:[["force_x",12.0]]});
                this.phase=2;return;
            }
            if this.test_case=="empty-body" {
                ops=prim("/World","ProbeBody","Xform",["PhysicsRigidBodyAPI","PhysicsMassAPI"]);
                ops.push(attr("/World/ProbeBody","physics:kinematicEnabled","bool","true"));
                ops.push(attr("/World/ProbeBody","physics:centerOfMass","point3f","(1, 0, 0)"));
            }
            if this.test_case=="compound" || this.test_case=="compound-reset" || this.test_case=="child-mass"
                || this.test_case=="child-root-mass" || this.test_case=="child-scaled" {
                ops=prim("/World","ProbeBody","Xform",["PhysicsRigidBodyAPI","PhysicsMassAPI"]);
                ops.push(attr("/World/ProbeBody","physics:kinematicEnabled","bool","true"));
                let parent="/World/ProbeBody";
                if this.test_case=="child-root-mass" {ops.push(attr(parent,"physics:mass","float","14"));}
                if this.test_case=="compound-reset" {
                    ops.push(attr(parent,"physics:mass","float","1"));
                    ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:parent,value:[2.0,0.0,0.0]}});
                }
                if this.test_case=="compound" {
                    ops.push(attr(parent,"physics:mass","float","1"));
                    for op in prim(parent,"Offset","Xform",[]) {ops.push(op);}
                    parent+="/Offset";
                    ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:parent,value:[100000000.0,0.0,0.0]}});
                }
                for op in prim(parent,"Shape","Cube",["PhysicsCollisionAPI","PhysicsMassAPI"]) {ops.push(op);}
                ops.push(attr(parent+"/Shape","size","double","1"));
                if this.test_case=="compound-reset" {
                    ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:parent+"/Shape",value:[5.0,0.0,0.0]}});
                    ops.push(attr(parent+"/Shape","xformOpOrder","token[]",`["!resetXformStack!", "xformOp:translate"]`));
                }
            if this.test_case=="compound" {
                    ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:parent+"/Shape",value:[-99999999.0,0.0,0.0]}});
                } else {
                    ops.push(attr(parent+"/Shape","physics:mass","float","7"));
                    ops.push(attr(parent+"/Shape","physics:density","float","100"));
                    if this.test_case=="child-scaled" {
                        ops.push(attr(parent+"/Shape","physics:diagonalInertia","float3","(1, 2, 3)"));
                        ops.push(attr(parent+"/Shape","physics:centerOfMass","point3f","(1, 0, 0)"));
                        ops.push(attr(parent+"/Shape","physics:principalAxes","quatf","(0.70710678, 0, 0, 0.70710678)"));
                        ops.push(#{SetRotate:#{edit_target:"@runtime@",path:parent+"/Shape",value:[0.0,0.0,90.0]}});
                        ops.push(#{SetScale:#{edit_target:"@runtime@",path:parent+"/Shape",value:[2.0,3.0,4.0]}});
                    }
                }
            }
            if this.test_case=="wheel-live-owner" {
                this.pending=cmd("SetPorts",#{target:find_path("/World/Raycast"),writes:[
                    ["mass",1300.0],["inertia_xx",5000.0],["inertia_yy",6000.0],["inertia_zz",7000.0],
                    ["com_x",0.25],["com_y",0.125],["com_z",0.0]]});
                this.phase=20;return;
            }
            if this.test_case=="wheel-mass" {
                this.before=native("/World/Raycast").mass_kg;
                ops=[attr("/World/Raycast/Suspension_FL/Wheel_FL","physxVehicleWheel:mass","float","35")];
            }
            if ops.is_empty() {throw("unknown second-pass case: "+this.test_case);}
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops});this.phase=1;return;
        }
        if this.phase==10 {
            if query("CompileStatus",#{doc_id:this.scratch}).ast_parsed!=true {return;}
            let before=query("GetDocumentSource",#{doc_id:this.scratch});
            for result in [cmd("AddModelicaComponent",#{doc_id:this.scratch,class:"FiniteNumberProbe",
                    type_name:"Real",name:"overflow",x:1e300,y:0.0}),
                cmd("ApplyModelicaOps",#{doc_id:this.scratch,ops:[#{AddIconGraphic:#{class:"FiniteNumberProbe",
                    graphic:#{kind:"Text",x1:0.0,y1:0.0,x2:1.0,y2:1.0,text:"overflow",font_size:1e300}}}]})] {
                this.checks.push(t_true(result.ok==false,"overflow must return a terminal rejection: "+result));
            }
            let after=query("GetDocumentSource",#{doc_id:this.scratch});
            this.checks.push(t_true(before.generation==after.generation && before.source==after.source,
                "neither direct nor tagged numeric overflow may mutate source"));
            this.checks.push(t_true(!query("GetReadiness").faulted,"numeric rejection must preserve the application"));
            this=finish(this,this.checks);return;
        }
        if this.phase==2 {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            if result.ok!=true {throw("held-input admission failed: "+result);}
            if native("/World/ForceBody").linear_velocity_mps[0]<=0.0 {return;}
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[#{RemovePrim:#{edit_target:"@root@",path:"/World/ForceBody"}}]});
            this.phase=1;return;
        }
        if this.phase==20 {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            if result.ok!=true {throw("live mass property admission failed: "+result);}
            this.before=native("/World/Raycast");
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[
                attr("/World/Raycast/Suspension_FL/Wheel_FL","physxVehicleWheel:mass","float","35")]});
            this.phase=1;return;
        }
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {throw("typed document edit rejected: "+result);}
        let ready=query("GetReadiness");
        if this.test_case=="live-units" || this.test_case=="mass-negative" || this.test_case=="mass-tiny" || this.test_case=="inertia-tiny" || this.test_case=="scale-tiny" {
            if !ready.faulted {return;}
            if this.stable_tick==() {this.stable_tick=clock_snapshot().sim_tick;return;}
            this.stable_frames+=1;if this.stable_frames<3 {return;}
            let kind=if this.test_case=="live-units" {"usd-stage-projection-reset"} else {"usd-rigid-body-mass-invalid"};
            let checks=[t_true(ready.application_available,"invalid physical source must preserve application access"),
                t_true(ready.fault.kind==kind,"source failure must identify its owner: "+ready.fault),
                t_true(ready.fault.detail!="" && ready.fault.subject!="","source failure needs subject and cause"),
                t_true(clock_snapshot().sim_tick==this.stable_tick,"terminal source failure must hold integration")];
            if this.test_case=="live-units" {
                checks.push(t_true(ready.fault.detail.contains("kilogramsPerUnit"),"mass convention failure must identify units"));
                checks.push(t_true(find_path("/World/Target")>=0 && native("/World/Target").mass_kg==this.before.mass_kg,
                    "invalid replacement must preserve the admitted body"));
            } else {
                checks.push(t_true(ready.fault.detail.contains(if this.test_case=="inertia-tiny" || this.test_case=="scale-tiny" {"physics:diagonalInertia"} else {"physics:mass"}),
                    "native admission failure must identify the rejected property"));
            }
            this=finish(this,checks);return;
        }
        if ready.faulted {throw("valid live edit faulted: "+ready);}
        if !ready.ready || ready.world_hold {return;}
        let checks=[];
        if this.test_case=="held-delete" {
            if find_path("/World/ForceBody")>=0 {return;}
            if this.stable_tick==() {this.stable_tick=clock_snapshot().sim_tick;return;}
            if clock_snapshot().sim_tick<=this.stable_tick+2 {return;}
            checks=[t_true(!ready.faulted,"retiring held endpoint must not fault surviving exchange"),
                t_true(native("/World/Target").mass_kg>0.0,"surviving body must remain admitted"),
                t_true(clock_snapshot().sim_tick>this.stable_tick+2,"surviving simulation must continue")];
        }
        if this.test_case=="implicit-density" || this.test_case=="mass-sentinels" {
            let body=native("/World/ForceBody");
            if abs(body.mass_kg-1000.0)>0.001 {return;}
            checks=[t_true(abs(body.mass_kg-1000.0)<0.001,"unit-cube implicit USD mass must be 1000kg"),
                t_true(body.computed_mass_finite && body.computed_inertia_finite,"implicit mass tensor must be finite")];
        }
        if this.test_case=="scale-small-valid" {
            let body=native("/World/ForceBody");
            if abs(body.mass_kg-1e10)>100.0 {return;}
            checks=[t_true(body.computed_mass_finite && body.computed_inertia_finite,
                "a representable final tensor must survive underflow of intermediate native shape inertia")];
            for axis in 0..3 {
                checks.push(t_true(abs(body.inertia_tensor_kgm2[axis][axis]-1e10*1e-26/6.0)<1e-22,
                    "tiny-scale explicit mass must use the complete f64 geometry reduction"));
            }
        }
        if this.test_case=="wheel-live-owner" {
            if this.stable_tick==() {this.stable_tick=clock_snapshot().sim_tick;return;}
            if clock_snapshot().sim_tick<=this.stable_tick+4 {return;}
            let body=native("/World/Raycast");
            checks=[t_true(abs(body.mass_kg-1300.0)<0.001,"source refold must preserve authoritative total mass")];
            for axis in 0..3 {
                checks.push(t_true(abs(body.center_of_mass_m[axis]-[0.25,0.125,0.0][axis])<0.00001,
                    "source refold must preserve authoritative total COM"));
                checks.push(t_true(abs(body.inertia_tensor_kgm2[axis][axis]-[5000.0,6000.0,7000.0][axis])<0.01,
                    "source refold must preserve authoritative total inertia"));
            }
        }
        if this.test_case=="wheel-mass" {
            let mass=native("/World/Raycast").mass_kg;
            if abs(mass-this.before-if this.phase==4 {0.0} else {10.0})>0.001 {return;}
            if this.stable_tick==() {this.stable_tick=clock_snapshot().sim_tick;return;}
            if clock_snapshot().sim_tick<=this.stable_tick+3 {return;}
            if this.phase==1 {
                this.checks.push(t_true(abs(mass-this.before-10.0)<0.001,"wheel edit must add exactly the changed 10kg"));
                this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[attr("/World/Raycast/Suspension_FL/Wheel_FL","physxVehicleWheel:mass","float","35")]});
                this.phase=3;this.stable_tick=();return;
            }
            if this.phase==3 {
                this.checks.push(t_true(abs(mass-this.before-10.0)<0.001,"repeated wheel edit must not fold twice"));
                this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[attr("/World/Raycast/Suspension_FL/Wheel_FL","physxVehicleWheel:mass","float","25")]});
                this.phase=4;this.stable_tick=();return;
            }
            checks=this.checks;
            checks.push(t_true(abs(mass-this.before)<0.001,"restored wheel mass must restore the independent carrier baseline"));
            checks.push(t_true(native("/World/Raycast").computed_inertia_finite,"recomputed wheel tensor must remain finite"));
        }
        if checks.is_empty() {
            if find_path("/World/ProbeBody")<0 {return;}
            let body=native("/World/ProbeBody");
            if body.colliders.len()==0 && this.test_case!="empty-body" {return;}
            checks=[t_true(body.computed_mass_finite && body.computed_inertia_finite,"compound mass must remain finite")];
            if this.test_case=="compound" || this.test_case=="compound-reset" {
                let expected=if this.test_case=="compound-reset" {3.0} else {1.0};
                checks.push(t_true(abs(body.colliders[0].body_local_translation_m[0]-expected)<0.000001,
                    "large cancelling transforms must retain the final one-metre mount"));
            }
            if this.test_case=="empty-body" {
                checks.push(t_true(abs(body.mass_kg-1.0)<0.00001,"body without volumes must use standard unit mass"));
                for axis in 0..3 {
                    checks.push(t_true(abs(body.center_of_mass_m[axis]-if axis==0 {1.0} else {0.0})<0.000001,
                        "empty-body approximation must retain its authored COM"));
                    let row_valid=true;
                    for column in 0..3 {
                        if abs(body.inertia_tensor_kgm2[axis][column]-if axis==column {0.004} else {0.0})>0.00000001 {
                            row_valid=false;
                        }
                    }
                    checks.push(t_true(row_valid,"empty-body inertia must be isotropic about the authored COM"));
                }
            }
            if this.test_case=="child-mass" || this.test_case=="child-root-mass" || this.test_case=="child-scaled" {
                checks.push(t_true(abs(body.mass_kg-if this.test_case=="child-root-mass" {14.0} else {7.0})<0.00001,
                    "explicit root and child mass must preserve standard precedence"));
                if this.test_case=="child-scaled" {
                    for axis in 0..3 {
                        checks.push(t_true(abs(body.center_of_mass_m[axis]-if axis==1 {2.0} else {0.0})<0.00001,
                            "authored child COM must include scale and mount rotation"));
                        checks.push(t_true(abs(body.inertia_tensor_kgm2[axis][axis]-(axis+1).to_float())<0.0001,
                            "child principal frame and mount must compose once"));
                    }
                }
            }
        }
        this=finish(this,checks);
    } catch(error) {this=finish(this,["second-pass observation failed: "+error]);}
}
