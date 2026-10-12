// Windowed gate with a declared missing dataset scope supplied by the runner.
const TEST_KIND = "editor";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ReadExposures"], required_inputs: [] }
}

fn consent_surface() {
    let value = query("ReadExposures", #{surface: "asset-consent"});
    if value == () || !value.surfaces.contains("asset-consent") {
        throw "dataset consent surface is required";
    }
    value.surfaces["asset-consent"]
}

fn on_start(me, ctx) {
    if ctx.scope == () || ctx.scope == "" { throw "missing dataset scope is required"; }
    this.scope = ctx.scope;
    this.phase = 0;
    this.ticks = 0;
    this.done = false;
    this.checks = [];
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks % 20 != 0 { return; }
    let surface = consent_surface();
    if this.phase == 0 {
        this.checks.push(t_true(surface.visible, "missing recommended data has a visible consent surface"));
        this.checks.push(t_true(surface.properties.rows.len() > 0, "policy rows remain published"));
        this.checks.push(t_true(surface.properties.actions.len() >= 2, "policy actions remain published"));
        this.initial = surface;
        cmd("DismissDatasetProvisioning", #{scope: "__absent_provisioning_scope__"});
    } else if this.phase == 1 {
        this.checks.push(t_true(surface == this.initial, "unrelated scope dismissal preserves the active view"));
        cmd("DismissDatasetProvisioning", #{scope: this.scope});
    } else {
        this.checks.push(t_true(!surface.visible, "active scope dismissal hides consent"));
        this.checks.push(t_true(surface.properties.len() == 0, "retired policy properties are cleared"));
        this.done = true;
        report_verdict(this.checks, "DATASET PROVISIONING SURFACE", "DATASET_PROVISIONING_SURFACE");
    }
    this.phase += 1;
}
