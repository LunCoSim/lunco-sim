// @timing presentation
// Continuous ground-level camera translation through multiple atlas chunks.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [params.camera_id],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}
fn view_for(status, params) {
    for view in status.camera_cover {
        if view.camera == params.camera && view.source == params.source { return view; }
    }
    ()
}
fn contact_keys(status) {
    let keys = [];
    for row in status.contacts {
        if row.ready { keys.push([row.source,row.depth,row.x,row.z,row.resolution,row.min_wavelength_m]); }
    }
    keys
}
fn advance(me) {
    if this.done { return; }
    let i=this.index;
    let result=cmd("SetCameraLookAt", #{camera:this.params.camera_id,eye:this.params.eyes[i],target:this.params.targets[i]});
    this.command_ok=this.command_ok && result.ok;
    this.index+=1;
    this.step_start=clock_snapshot().wall_elapsed_s;
}
fn elapsed_step(me) { this.done || clock_snapshot().wall_elapsed_s-this.step_start>=this.params.step_seconds }
fn settled(me) { this.done }
fn on_start(me,params) {
    let status=query("TerrainLodStatus", #{});
    let view=view_for(status,params);
    if view==() || view.relief==() || !view.relief.current || !view.ready || params.eyes.len()<16 { throw "traversal requires settled initial relief and a multi-step path"; }
    this.params=params;
    this.start=view.position_local_m;
    this.atlas=view.relief.atlas;
    this.initial_center=view.relief.center_m;
    this.initial_batches=if view.relief.patch_batches==() {0} else {view.relief.patch_batches};
    this.contacts=contact_keys(status);
    this.cover_ok=true;
    this.relief_ok=true;
    this.atlas_ok=true;
    this.command_ok=true;
    this.samples=0;
    this.index=0;
    this.done=false;
    this.started=clock_snapshot().wall_elapsed_s;
    this.step_start=this.started;
}
fn on_tick(me,params) {
    if this.done==() || this.done { return; }
    this.samples+=1;
    let status=query("TerrainLodStatus", #{});
    let view=view_for(status,params);
    let complete=false;
    let moved=false;
    let patched=false;
    if view!=() {
        this.cover_ok=this.cover_ok && view.drawn>0 && view.presented_ready;
        this.relief_ok=this.relief_ok && view.relief!=();
        if view.relief!=() {
            this.atlas_ok=this.atlas_ok && view.relief.atlas==this.atlas;
            let dx=view.position_local_m[0]-this.start[0];
            let dz=view.position_local_m[2]-this.start[2];
            moved=dx*dx+dz*dz>=params.minimum_distance_m*params.minimum_distance_m;
            patched=view.relief.patch_batches!=() && view.relief.patch_batches>this.initial_batches
                && view.relief.chunks_reused>0 && view.relief.center_m!=this.initial_center;
            complete=this.index==params.eyes.len() && moved && view.ready && view.relief.current && patched;
        }
    } else {this.cover_ok=false;this.relief_ok=false;}
    if complete || !this.command_ok || clock_snapshot().wall_elapsed_s-this.started>params.timeout_s {
        this.done=true;
        let contacts=contact_keys(status);
        let same=contacts.len()==this.contacts.len() && contacts.len()>0;
        for contact in contacts {same=same && this.contacts.contains(contact);}
        report_verdict([
            t_true(this.command_ok && moved && this.index==params.eyes.len(),"every continuous camera motion step is admitted"),
            t_true(this.cover_ok && this.samples>=params.eyes.len(),"a complete drawable terrain cover persists throughout traversal"),
            t_true(this.relief_ok && this.atlas_ok,"relief stays present in the same GPU atlas"),
            t_true(complete && patched,"continuous motion completes with GPU subregion updates and a current window"),
            t_true(same,"visual traversal preserves physical contact lattices")
        ],"TERRAIN RELIEF TRAVERSE","TERRAIN_RELIEF_TRAVERSE");
    }
}
fn task(me,params) {
    let steps=[];
    for eye in params.eyes {
        steps.push(once(Fn("advance")));
        steps.push(wait_until(Fn("elapsed_step")));
    }
    steps.push(wait_until(Fn("settled")));
    seq(steps)
}
