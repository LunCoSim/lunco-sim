// Run on a celestial DEM scene through the production scenario API.
const TEST_KIND = "graphics";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["TerrainLodStatus"], required_inputs: [] }
}

fn terminal_ack(value) {
    if value != () && value.status == "pending" && value.id != () {
        return command_result(value.id);
    }
    value
}

fn finish(state, failures) {
    state.done = true;
    report_verdict(failures, "SHADER SOURCE LIFECYCLE", "SHADER_SOURCE_LIFECYCLE");
    state
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.phase = "ready";
    this.samples = 0;
    this.failures = [];
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 20.0 {
        this = finish(this, ["shader source lifecycle timed out in " + this.phase]); return;
    }
    let status = query("TerrainLodStatus", #{});
    if status == () || status.continuations == () || status.continuations.len() != 1 {
        this = finish(this, ["the scene must expose one terrain continuation"]); return;
    }
    let collar = status.continuations[0];
    if this.phase == "ready" {
        if !collar.source_reflected || collar.source_valid != true || collar.hidden || !collar.ready {
            return;
        }
        this.source_shader = collar.source_shader;
        this.source = collar.source;
        this.pending = cmd("ReloadShader", #{path: "lunco://shaders/inactive-source-lifecycle-test.wesl"});
        this.phase = "reject"; return;
    }
    if this.phase == "reject" {
        let ack = terminal_ack(this.pending);
        if ack == () || ack.status == "pending" { return; }
        this.failures.push(t_true(ack.ok == false, "inactive source reload must be rejected"));
        this.pending = cmd("ReloadShader", #{path: this.source_shader});
        this.phase = "reload"; return;
    }
    if this.phase == "reload" {
        let ack = terminal_ack(this.pending);
        if ack == () || ack.status == "pending" { return; }
        if ack.ok != true {
            this = finish(this, ["active source reload was rejected"]); return;
        }
        this.failures.push(t_true(true, "active source reload was accepted"));
        this.phase = "observe";
    }
    if this.phase != "observe" { return; }
    if collar.source != this.source || collar.source_shader != this.source_shader
        || !collar.source_reflected || collar.source_valid != true {
        this = finish(this, ["valid source reflection was lost while its material reloaded"]); return;
    }
    this.samples += 1;
    if this.samples < 180 { return; }
    this.failures.push(t_true(collar.ready && !collar.hidden,
        "the continuation must be drawable after source reload"));
    this.failures.push(t_true(this.samples == 180,
        "source identity and reflection persisted across 180 admitted observations"));
    this = finish(this, this.failures);
}
