fn simulation_dependencies(me, ctx) {
    let engine = find_path("/RocketEngineObservables/Engine");
    #{ modelica_entities: [engine], entity_reads: [engine], entity_writes: [engine],
       query_reads: [], required_inputs: [] }
}

fn on_start(me, ctx) {
    let engine = find_path("/RocketEngineObservables/Engine");
    let variables = get(engine, "ModelicaVariables.values");
    if variables == () || variables.len() == 0 {
        report_verdict([t_true(false, "the initialized participant has no required variable state")],
                       "MODELICA VARIABLES MISSING", "MODELICA_VARIABLES_MISSING");
        return;
    }
    expect_runtime_fault("modelica-variables-missing");
    if !remove(engine, "ModelicaVariables") {
        report_verdict([t_true(false, "the test could not remove the required variable component")],
                       "MODELICA VARIABLES MISSING", "MODELICA_VARIABLES_MISSING");
    }
}
