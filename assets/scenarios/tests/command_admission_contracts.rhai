// @timing presentation
// Observe command rejection even while a document edit holds simulation time.
const PROBE="/World/CommandAdmissionProbe";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["ListOpenDocuments","InspectUsdDocument","QueryUsdPrim","GetReadiness"],required_inputs:[]}
}
fn generation(doc) {query("InspectUsdDocument",#{doc_id:doc}).generation}
fn value(doc) {query("QueryUsdPrim",#{doc_id:doc,path:PROBE,attrs:["inputs:probe"]}).attrs["inputs:probe"]}
fn attribute(value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:PROBE,name:"inputs:probe",type_name:"double",value:value}}
}
fn mission() {
    [get_setting("MissionClock.mission_tick0"),get_setting("MissionClock.mission_epoch0_jd"),
     get_setting("MissionClock.anchor.tick0"),get_setting("MissionClock.anchor.epoch0_jd")]
}
fn transport() {[get_setting("TimeTransport.mode").to_string(),get_setting("TimeTransport.rate")]}
fn on_start(me,ctx) {this.frames=0;this.done=false;this.phase=0;this.doc=();this.checks=[];}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>600 {
        this.checks.push("command admission timed out at phase "+this.phase);
        this.done=true;report_verdict(this.checks,"COMMAND ADMISSION CONTRACTS","COMMAND_ADMISSION_CONTRACTS");return;
    }
    try {
        if this.doc==() {
            for document in query("ListOpenDocuments").open_documents {
                if document.kind=="usd" && document.title=="simulation_core_contracts" {this.doc=document.doc_id;}
            }
            if this.doc==() {return;}
        }
        if this.phase==0 {
            this.before=generation(this.doc);
            // reference_prim_path is omitted; reference is explicitly null.
            let ticket=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[
                #{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:"CommandAdmissionProbe",
                    type_name:"Scope",reference:()}},attribute("7")]});
            this.checks.push(t_true(ticket!=() && ticket.id!=(),"omitted optional field lost its command receipt"));
            if ticket.id==() {throw("no authoring receipt: "+ticket);}
            this.pending=ticket.id;this.phase=1;return;
        }
        if this.phase==1 {
            let completed=command_result(this.pending);
            if completed.status=="pending" {return;}
            if completed.ok!=true {throw("valid optional-field authoring failed: "+completed);}
            this.checks.push(t_true(completed.status=="applied","valid typed authoring did not apply"));
            this.phase=2;return;
        }
        let entity=find_path(PROBE);
        if entity<0 {return;}
        if value(this.doc)!=7.0 {return;}
        this.checks.push(t_true(generation(this.doc)>this.before,"valid authoring did not advance its document"));
        this.checks.push(t_true(value(this.doc)==7.0 && entity>=0,
            "the admitted prim and scalar did not project into the live scene"));
        let before=generation(this.doc);
        let malformed=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[attribute("99"),
            #{SetAttribute:#{edit_target:"@runtime@",path:PROBE,name:"inputs:probe",type_name:17,value:"13"}}]});
        this.checks.push(t_true(malformed.ok==false,"malformed nested field type was accepted"));
        this.checks.push(t_true(generation(this.doc)==before,"malformed nested field changed document generation"));
        this.checks.push(t_true(value(this.doc)==7.0,"malformed batch committed its valid prefix"));
        let unknown=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[attribute("99"),
            #{SetAttribute:#{edit_target:"@runtime@",path:PROBE,name:"inputs:probe",type_name:"double",
                value:"13",unexpected_field:true}}]});
        this.checks.push(t_true(unknown.ok==false,"unknown nested field was accepted"));
        this.checks.push(t_true(generation(this.doc)==before,"unknown nested field changed document generation"));
        this.checks.push(t_true(value(this.doc)==7.0,"unknown-field batch committed its valid prefix"));
        let original_mission=mission();let original_transport=transport();
        let bad_epoch=cmd("SetMissionEpoch",#{epoch_jd:0.0});
        this.checks.push(t_true(bad_epoch.ok==false,"invalid mission epoch returned success"));
        this.checks.push(t_true(mission()==original_mission,"invalid mission epoch changed the clock mapping"));
        let bad_animation=cmd("ControlAnimation",#{target:find_path("/World/Target"),seek_secs:3.0});
        this.checks.push(t_true(bad_animation.ok==false,"absent animation target returned success"));
        this.checks.push(t_true(transport()==original_transport,"rejected clock commands changed simulation transport"));
        this.checks.push(t_true(cmd("ControlAnimation",#{playing:false}).ok==true,
            "default animation pause with omitted options failed"));
        this.checks.push(t_true(cmd("ControlAnimation",#{playing:true}).ok==true,
            "default animation resume with omitted options failed"));
        this.checks.push(t_true(mission()==original_mission && transport()==original_transport,
            "animation-only commands changed the mission clock or transport"));
        this.checks.push(t_true(query("GetReadiness").faulted==false,
            "terminal user-command rejection faulted the application"));
        this.done=true;report_verdict(this.checks,"COMMAND ADMISSION CONTRACTS","COMMAND_ADMISSION_CONTRACTS");
    } catch(error) {
        this.checks.push("command admission observation failed: "+error);
        this.done=true;report_verdict(this.checks,"COMMAND ADMISSION CONTRACTS","COMMAND_ADMISSION_CONTRACTS");
    }
}
