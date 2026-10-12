fn simulation_dependencies(me, ctx) {
    let probe = find_path("/World/Probe");
    #{modelica_entities: [probe], entity_reads: [probe], entity_writes: [],
      query_reads: [], required_inputs: []}
}
fn on_start(me, ctx) {
    this.probe = find_path("/World/Probe");
    this.done = false;
    let values = get(this.probe, "ModelicaVariables.values");
    if values == () || values.observed == () || values.observed != 0.5 {
        this.done = true;
        report_verdict(["the checked initial observation is missing or invalid"],
            "MODELICA OBSERVATION FAILURE", "MODELICA_OBSERVATION_FAILURE");
        return;
    }
    expect_runtime_fault("modelica-step-failed");
}
fn on_tick(me, ctx) {
    if this.done { return; }
    if get(this.probe, "ModelicaModel.current_time") < 0.5 { return; }
    this.done = true;
    report_verdict(["a non-finite observation was discarded while the solver kept running"],
        "MODELICA OBSERVATION FAILURE", "MODELICA_OBSERVATION_FAILURE");
}
