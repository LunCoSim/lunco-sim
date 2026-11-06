// Diagnostic observer: freeze the same authoritative tick across renderer trials.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: [], required_inputs: []}
}
fn on_start(me, ctx) {
    if ctx.freeze_tick <= sim_tick() { throw "graphics freeze tick was already reached"; }
    this.freeze_tick = ctx.freeze_tick;
    this.done = false;
}
fn on_tick(me, ctx) {
    if this.done || sim_tick() < this.freeze_tick { return; }
    this.done = true;
    pause();
    print("GRAPHICS_FIXED_POSE tick=" + sim_tick());
    emit("GRAPHICS_FIXED_POSE", #{tick: sim_tick()});
}
