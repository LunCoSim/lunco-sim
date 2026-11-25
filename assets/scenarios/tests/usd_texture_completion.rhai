// @timing presentation
// A real file with no image loader exercises a terminal texture dependency.
fn connect_texture(state, sources) {
    let edit = cmd("ApplyUsdOp", #{doc_id:state.doc, op:#{SetConnection:#{
        edit_target:"@root@", path:state.root + "/Mat/Preview", name:"inputs:diffuseColor",
        type_name:"color3f", sources:sources
    }}});
    if !edit.ok {throw edit.error;}
}

fn on_start(me, ctx) {
    this.doc=ctx.doc_id;
    this.root=ctx.root;
    this.shape=find_path_in_document(this.doc, this.root+"/Shape");
    this.checks=[];
    this.phase="failed_texture";
    this.revision=();
    let before = usd_document_generation(this.doc);
    let rgb = this.root+"/Mat/InvalidImage.outputs:rgb";
    for pair in [
        ["float", rgb], ["double3", rgb], ["float3[]", rgb],
        ["point3f", this.root+"/Mat/Preview.inputs:diffuseColor"]
    ] {
        let rejected = cmd("ApplyUsdOp", #{doc_id:this.doc, op:#{SetConnection:#{
            edit_target:"@root@", path:this.root+"/Mat/Preview", name:"inputs:invalidAgreement",
            type_name:pair[0], sources:[pair[1]]
        }}});
        this.checks.push(t_true(rejected.status=="rejected" && usd_document_generation(this.doc)==before,
            "native USD agreement rejects an incompatible connection to " + pair[0]));
    }
    connect_texture(this,[this.root+"/Mat/InvalidImage.outputs:rgb"]);
}

fn on_event(me, evt, ctx) {
    if this.phase=="done" || !evt.name.starts_with("usd.") {return;}
    let event=evt.value;
    if event.doc_id!=this.doc {return;}
    if evt.name=="usd.document.projected" {this.revision=event.generation; return;}
    if evt.name!="usd.projection.visuals_ready" && evt.name!="usd.projection.visuals_failed" {return;}
    if this.revision!=event.generation {return;}
    let ready=evt.name=="usd.projection.visuals_ready";
    let failure=get(this.shape,"FailedPbrLook");
    if this.phase=="failed_texture" {
        this.checks.push(t_true(!ready && failure!=() && event.detail.contains(this.root+"/Shape"),
            "a failed bound texture reports its affected surface and revision"));
        this.checks.push(t_true(!query("GetReadiness").faulted && get(this.shape,"Mesh3d")!=(),
            "a texture rejection preserves geometry and simulation admission"));
        this.phase="unrelated";
        this.revision=();
        let edit=cmd("ApplyUsdOp", #{doc_id:this.doc, op:#{SetAttribute:#{
            edit_target:"@root@", path:this.root+"/Unrelated", name:"primvars:displayColor",
            type_name:"color3f[]", value:"[(0.1, 0.2, 0.3)]"
        }}});
        if !edit.ok {throw edit.error;}
    } else if this.phase=="unrelated" {
        this.checks.push(t_true(ready && failure!=(),
            "an unrelated in-place appearance completes while the failed texture remains diagnosed"));
        this.phase="repair";
        this.revision=();
        connect_texture(this,[]);
    } else {
        this.checks.push(t_true(ready && failure==() && get(this.shape,"PendingPbrLook")==()
            && find_path_in_document(this.doc,this.root+"/Shape")==this.shape,
            "removing the failed connection restores the same admitted draw entity"));
        this.phase="done";
        report_verdict(this.checks,"USD TEXTURE COMPLETION","USD_TEXTURE_COMPLETION");
    }
}
