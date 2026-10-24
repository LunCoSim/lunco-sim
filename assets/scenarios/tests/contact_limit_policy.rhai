// The test vehicle's authored reaction. The Modelica envelope owns detection.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [find_path(usd_path(me) + "/ContactValidity")],
      entity_reads: [me], entity_writes: [me],
      query_reads: ["ReadPorts"], required_inputs: []}
}
fn on_start(me, ctx) { subscribe("test.contact-speed-limit"); }
fn on_event(me, evt, ctx) {
    let model = find_path(usd_path(me) + "/ContactValidity");
    if evt.name != "test.contact-speed-limit" || evt.source != model { return; }
    let ports = query("ReadPorts", #{api_id: model, port_names: ["speed_limit_mps"]}).ports;
    if ports.len() != 1 { throw("Contact validity limit is unavailable"); }
    let limit = ports[0].value;
    let result = cmd("PauseSimulationObject", #{target: me,
        condition: "test:contact-speed-outside-validity",
        detail: "Impact normal approach speed=" + (evt.value * limit).to_string()
          + " m/s exceeds the authored contact-model validity limit=" + limit.to_string()
          + " m/s at tick " + evt.sim_tick.to_string()
          + ". Coupled simulation paused; impact results beyond this limit are not meaningful."});
    if result.ok != true { throw("Contact validity pause rejected: " + result); }
}
