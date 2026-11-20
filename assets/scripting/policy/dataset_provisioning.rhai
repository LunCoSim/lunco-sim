// assets.provision receives one scope's authoritative dataset state. Rhai owns
// visibility, wording, and actions; the host validates and publishes this model.
fn dataset_download_status(state) {
    let done = state.bytes_done.to_float();
    let total = state.bytes_total.to_float();
    let mib = ((done / 1048576.0) * 10.0).round() / 10.0;
    if total <= 0.0 {
        return "Downloading · " + mib.to_string() + " MiB received";
    }
    let percentage = done / total * 100.0;
    let percent = percentage.floor().to_int();
    let total_mib = ((total / 1048576.0) * 10.0).round() / 10.0;
    let status = "Downloading · " + percent.to_string() + "% · ";
    status += mib.to_string() + " / " + total_mib.to_string() + " MiB";
    status
}

fn source_file_label(state) {
    if state=="ready" {return "Downloaded";}
    if state=="invalid" {return "Invalid";}
    "Not downloaded"
}
fn provision_choice(condition,yes,no) {if condition {return yes;} no}
fn preparation_color(ready,working) {
    if ready {return "var(--ok-color)";}
    if working {return "var(--caution-color)";}
    "var(--danger-color)"
}
fn dataset_working(state) {
    ["queued","downloading","processing","cancelling"].contains(state)
}
fn preparation_title(complete,failed,busy,downloads_needed) {
    if complete {return "Data ready";}
    if failed>0 {return "Data preparation needs attention";}
    if busy>0 {return "Preparing data";}
    if downloads_needed==0 {return "Ready to prepare data";}
    "Required data is missing"
}
fn dataset_source_summary(dataset) {
    let verified=0;
    let url=();
    let lines="";
    let errors="";
    for input in dataset.inputs {
        if input.id=="source" {url=input.url;}
        if input.state=="ready" {verified+=1;}
        if lines!="" {lines+="\n";}
        lines+=source_file_label(input.state)+" · "+input.name;
        if input.error!=() {errors+=input.name+": "+input.error+"\n";}
    }
    if url==() {throw "Dataset has no declared primary source: "+dataset.id;}
    let status="Source files · "+verified.to_string()+" / ";
    status+=dataset.inputs.len().to_string()+" available\n"+lines;
    #{url:url,status:status,files:lines,count:verified.to_string()+" / "+dataset.inputs.len().to_string(),errors:errors}
}
fn preparation_error_detail(detail) {
    if detail.starts_with("processing failed: ") {return detail.sub_string(19);}
    detail
}
fn dataset_preparation_row(dataset,scope) {
    let state=dataset.state.kind;
    if !["installed","queued","downloading","processing","cancelling","missing","failed","cancelled"].contains(state) {throw "Unknown dataset state: "+state;}
    let sources=dataset_source_summary(dataset);
    let can_prepare=dataset.inputs_ready && dataset.processed;
    let row=#{id:dataset.id,parameter_id:dataset.id,scope:scope,label:dataset.name,
        status:provision_choice(dataset.inputs_ready,"Downloaded · ready to prepare","Not downloaded"),
        status_color:preparation_color(state=="installed",dataset_working(state)),
        download_status:"1  Source files · "+sources.count+" downloaded",
        source_files:sources.files,
        details_display:provision_choice(dataset.expanded==true,"flex","none"),
        details_label:provision_choice(dataset.expanded==true,"Hide details","Details"),
        min_height:provision_choice(dataset.expanded==true,"220px",provision_choice(state=="installed","80px","112px")),
        stages_display:provision_choice(state=="installed" && dataset.expanded!=true,"none","flex"),
        action_display:provision_choice(state=="installed","none","flex"),
        rebuild_action:"",rebuild_label:"",ready:state=="installed",
        preparation_status:provision_choice(dataset.processed,"2  Prepare artifacts · not started","2  Install · no processing required"),
        detail:sources.errors,source_url:sources.url,
        action:provision_choice(can_prepare,"process","request"),
        action_label:provision_choice(can_prepare,"Prepare",provision_choice(dataset.processed,"Download & prepare","Download"))};
    if state=="missing" && sources.errors!="" {row.status="Source validation failed";row.action_label="Download again";}
    if state=="installed" {
        row.status="Installed";row.preparation_status="2  Prepare artifacts · installed";
        row.action="dismiss";row.action_label="Done";
    }
    if state=="queued" {row.status="Queued · waiting for a preparation slot";row.action="cancel";row.action_label="Cancel";}
    if state=="downloading" {
        row.status=dataset_download_status(dataset.state);
        if dataset.state.source!=() {row.download_status="Source file · "+dataset.state.source;}
        row.action="cancel";row.action_label="Cancel";
        row.preparation_status=provision_choice(dataset.processed,"Preparation · waiting for source files","No preparation required");
    }
    if state=="processing" {
        row.status="Preparing";row.preparation_status="2  Prepare artifacts · "+dataset.state.detail;
        row.action="cancel";row.action_label="Cancel";
    }
    if state=="cancelling" {row.status="Stopping…";row.action="dismiss";row.action_label="Hide";}
    if state=="failed" {
        row.status=provision_choice(can_prepare,"Preparation failed","Download failed");
        if can_prepare {row.preparation_status="2  Prepare artifacts · failed";}
        row.detail=preparation_error_detail(dataset.state.detail);
        row.action_label=provision_choice(can_prepare,"Retry preparation","Retry download");
    }
    if state=="cancelled" {row.status="Cancelled";row.action_label=provision_choice(can_prepare,"Resume preparation","Resume download");}
    if can_prepare && state=="installed" {row.rebuild_action="rebuild";row.rebuild_label="Rebuild";}
    if row.detail!="" {row.min_height="180px";}
    row
}
fn simulation_preparation(facts) {
    if facts==() {return #{ready:false,fault:false,status:"",detail:"",terrain:()};}
    if facts.fault!=() {return #{ready:false,fault:true,status:"Simulation needs attention",detail:facts.fault.subject+": "+facts.fault.detail,terrain:()};}
    let terrain=();
    if facts.terrain_maps_pending==true {
        terrain=#{title:"Building terrain maps",detail:"The required files are installed. Terrain will appear once the maps and camera views are ready."};
    } else if facts.terrain_views_pending==true {
        terrain=#{title:"Preparing terrain views",detail:"The required files are installed. Terrain is still loading into the scene’s camera views."};
    }
    if facts.pending==() || facts.blockers==() || facts.world_hold==() {return #{ready:false,fault:false,status:"Waiting for simulation readiness",detail:"",terrain:terrain};}
    if facts.world_hold || facts.pending.len()>0 || facts.blockers.len()>0 {
        return #{ready:false,fault:false,status:"Simulation held during preparation",
            detail:"Preparing terrain collision support and scene models.",terrain:terrain};
    }
    let status=provision_choice(facts.playing==false,"Simulation paused","Simulation ready");
    #{ready:true,fault:false,status:status,detail:"",terrain:terrain}
}
fn startup_assets_view(ctx) {
    let rows=[];
    let ready=0;
    let working=false;
    let failed=false;
    for dataset in ctx.datasets {
        let state=dataset.state.kind;
        let row=dataset_preparation_row(dataset,ctx.scope_id);
        if row.ready {ready+=1;row.status="Ready";}
        if state=="queued" {row.status="Queued";}
        if state=="processing" {row.status="Preparing for the simulator";}
        if state=="cancelling" {row.action_display="none";}
        if dataset_working(state) {working=true;}
        if state=="failed" || row.status=="Source validation failed" {failed=true;}
        rows.push(row);
    }
    let complete=ready==rows.len();
    let title="Preparing built-in assets";
    let description="These downloads run once. You can keep using LunCoSim.";
    let dismiss="Continue in background";
    if complete {
        title="Built-in assets ready";
        description="These files are saved on this device for future launches.";
        dismiss="Done";
    } else if failed {
        title="Built-in assets need attention";
        description="Retry here or in Settings → Data & libraries.";
        dismiss=provision_choice(working,"Continue in background","Close");
    } else if !working {
        title="Built-in asset setup paused";
        description="Resume here or return later in Settings → Data & libraries.";
        dismiss="Close";
    }
    let color=provision_choice(!complete && !failed && !working,"var(--muted-color)",
        preparation_color(complete,working && !failed));
    #{visible:ctx.interactive && ctx.scene_notice_visible!=true && !rows.is_empty(),
      title:title,description:description,phase_color:color,
      activity_display:provision_choice(working,"flex","none"),
      scope_label:ready.to_string()+" / "+rows.len().to_string()+" ready",
      simulation_status:"",simulation_detail:"",rows:rows,
      actions:[#{key:"dismiss",label:dismiss,action:"dismiss",scope:"startup-assets"}]}
}

fn dataset_provisioning(ctx) {
    if ctx.startup==true {return startup_assets_view(ctx);}
    let settings=ctx.include_optional==true;
    let rows=[];
    let optional=[];
    let ready_rows=[];
    let installed_count=0;
    let missing=0;
    let busy=0;
    let failed=0;
    let recommended=0;
    let downloads_needed=0;
    for dataset in ctx.datasets {
        if type_of(dataset.required_for_scene)!="bool" {throw "Missing typed scene requirement fact: "+dataset.id;}
        if dataset.required_for_scene!=true && !settings {continue;}
        recommended+=1;
        let state=dataset.state.kind;
        let idle=["missing","failed","cancelled"].contains(state);
        if idle {missing+=1;}
        if idle && !dataset.inputs_ready {downloads_needed+=1;}
        if dataset_working(state) {busy+=1;}
        let row=dataset_preparation_row(dataset,ctx.scope_id);
        if state=="failed" || row.status=="Source validation failed" {failed+=1;}
        if settings {
            let prefix="Optional · ";
            if dataset.required_for_scene==true {prefix="Required by scene · ";}
            else if dataset.selected_for_startup==true {prefix="Built-in · ";}
            else if dataset.recommended {prefix="Recommended · ";}
            row.label=prefix+row.label;
        }
        if row.ready {installed_count+=1;}
        if settings && dataset.required_for_scene!=true {optional.push(row);}
        else if !settings && row.ready {ready_rows.push(row);}
        else {rows.push(row);}
    }
    for row in ready_rows {rows.push(row);}
    for row in optional {rows.push(row);}
    let complete=missing==0 && busy==0;
    let simulation=simulation_preparation(ctx.simulation);
    if !complete && !settings && !simulation.fault {
        simulation.status="Simulation waiting for required data";
        simulation.detail="The application remains available. ";
        if failed>0 {simulation.detail+="Resolve the failed download or preparation above, then retry.";}
        else if busy>0 {simulation.detail+="Downloads and preparation continue in the background.";}
        else if downloads_needed>0 {simulation.detail+="Download the required files to continue this scene.";}
        else {simulation.detail+="Source files are downloaded. Prepare the required artifacts to continue.";}
    }
    let actions=[];
    if missing>0 && rows.len()>1 {
        let label=provision_choice(downloads_needed>0,"Download and prepare","Prepare required data");
        actions.push(#{key:"request-all",label:label,action:"request_required",scope:ctx.scope_id});
    }
    let description=provision_choice(complete,"All required files are installed.","Required for this scene. Simulation waits for downloads, preparation and physics. Other Twin data can be downloaded later in Settings.");
    let title=preparation_title(complete,failed,busy,downloads_needed);
    if complete && !simulation.ready && ctx.simulation!=() && !simulation.fault {
        title="Preparing simulation";
        description="The required files are installed. Scene preparation is still in progress.";
    }
    if complete && simulation.terrain!=() {
        title=simulation.terrain.title;
        description=simulation.terrain.detail;
    }
    if simulation.fault {title="Simulation needs attention";}
    if ctx.startup_working==true {description+=" Built-in downloads continue in the background.";}
    let working=busy>0 || (complete && ctx.simulation!=() && !simulation.fault
        && (!simulation.ready || simulation.terrain!=()));
    let dismiss=provision_choice(complete,"Close","Keep scene paused");
    if working {dismiss="Continue in background";}
    actions.push(#{key:"dismiss",label:dismiss,action:"dismiss",scope:ctx.scope_id});
    #{visible:ctx.interactive && recommended>0,
      title:title,phase_color:preparation_color(complete && !working && !simulation.fault,working && failed==0 && !simulation.fault),
      activity_display:provision_choice(working,"flex","none"),description:description,simulation_status:simulation.status,simulation_detail:simulation.detail,
      scope_label:ctx.owner+" · "+installed_count.to_string()+" / "+recommended.to_string()+" datasets installed",rows:rows,actions:actions}
}
