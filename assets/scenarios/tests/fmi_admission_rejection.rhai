// @timing presentation
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.done = false;
    // Invalid timing and initialization values fail before a communication
    // interval is admitted, without changing the requested physics cadence.
    this.parameter_case = ctx.parameter_case;
    this.expected_detail = if this.parameter_case { "≤ 1" } else { "fixedInternalStepSize" };
    let request = if this.parameter_case {
        #{source: ctx.bounce, name: "Invalid FMI Parameter", communication_ticks: 3, parameters: [["e", 1.5]]}
    } else { #{source: ctx.decay, name: "Incompatible FMI Period"} };
    let admitted = cmd("LoadFmu", request);
    if admitted.ok != true { throw(admitted.to_string()); }
}
fn task(me, ctx) {
    seq([
        wait_until(|m| query("GetReadiness").faulted),
        once(|m| {
            let readiness = query("GetReadiness");
            this.done = true;
            report_verdict([
                t_true(readiness.fault.kind == "fmi-execution-failed",
                    "invalid native admission fails at the FMI owner"),
                t_true(readiness.fault.detail.contains(this.expected_detail),
                    "the diagnostic identifies the rejected owner contract"),
                t_true(!clock_snapshot().simulation_progress.held,
                    "the failed transaction releases its preparation hold")
            ], "FMI ADMISSION REJECTION", "FMI_ADMISSION_REJECTION");
        })
    ])
}
