// @timing presentation
// A completed contact sample belongs to its solved interval, including while
// the admission clock has zero delta. Presentation observes the held boundary.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ReadPorts", "GetReadiness"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.path = ctx.path;
}
fn loaded_boundary(path) {
    let target = find_path(path);
    if target < 0 { return false; }
    let contact = contact_debug(target);
    query("GetReadiness").ready && contact != ()
        && contact.total_normal_impulse > 1.0
}
fn hold(me) {
    this.target = find_path(this.path);
    let result = cmd("StepSimulation", #{hold: true});
    if result.ok != true { throw(result.to_string()); }
}
fn sample(me) {
    this.before = clock_snapshot();
    this.force = get(this.target, "contact_force");
    this.contact = get(this.target, "contact");
    this.impulse = contact_debug(this.target).total_normal_impulse;
}
fn finish(me) {
    let clock = clock_snapshot();
    report_verdict([
        t_true(this.impulse > 1.0 && this.contact == 1.0,
            "the positive fixture contains a solved loaded contact"),
        t_true(this.before.physics_dt_s == 0.0 && clock.physics_dt_s == 0.0,
            "native admission remains at zero delta while held"),
        t_true(clock.sim_tick == this.before.sim_tick
            && clock.physics_elapsed_s == this.before.physics_elapsed_s,
            "holding retains the completed native interval"),
        t_true(get(this.target, "contact") == this.contact,
            "a held boundary retains its contact state"),
        t_true(this.force > 1.0 && get(this.target, "contact_force") == this.force,
            "zero admission delta cannot erase a solved contact load"),
        t_true(get(me, "contact_force") == (),
            "an entity without a collider does not fabricate a contact load")
    ], "CONTACT HOLD", "CONTACT_HOLD");
}
fn task(me, ctx) {
    let path = ctx.path;
    seq([wait_until(|m| loaded_boundary(path)), once(Fn("hold")),
         wait(0.1), once(Fn("sample")), wait(0.1), once(Fn("finish"))])
}
