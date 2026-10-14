import "lunco://scenarios/tests/modelica_initialization_network" as network;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "CosimStatus"], required_inputs: []}
}
fn on_start(me, ctx) { this = network::start_case(true); }
fn on_tick(me, ctx) { this = network::step_case(this); }
