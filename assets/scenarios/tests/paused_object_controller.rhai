// Test instrumentation: a suspended controller must stop producing events.
const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: [], required_inputs: []}
}

fn on_tick(me, ctx) {
    emit("test:object-controller-tick", sim_tick());
}
