// @timing presentation

fn on_start(me, ctx) {
    this.done = false;
    let receipt = query("CosimStatus").last_coupled_initialization;
    this.checks = [t_true(receipt != () && receipt.scene_generation.to_float() > 0.0
        && receipt.solver_id != "", "the completed initialization retains its scene and solver identity")];
    let result = cmd("LoadScene", #{path: ctx.replacement_scene});
    if result.ok != true { throw result.to_string(); }
}

fn on_tick(me, ctx) {
    if this.done || find_path(ctx.replacement_marker) < 0 || !query("GetReadiness").ready { return; }
    let full = query("CosimStatus");
    let compact = query("CosimStatus", #{include_values: false, include_entities: false});
    this.checks.push(t_true(full.last_coupled_initialization == ()
        && compact.last_coupled_initialization == (),
        "scene replacement clears the initialization receipt in both API views"));
    this.done = true;
    report_verdict(this.checks, "MODELICA INITIALIZATION RECEIPT", "MODELICA_INITIALIZATION_RECEIPT");
}
