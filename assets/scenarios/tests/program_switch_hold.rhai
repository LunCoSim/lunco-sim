// Generic test program used to prove live source switching. It consumes the
// standard `inputs:subject` relationship when present and applies a safe brake;
// it does not know which model owns that subject.

fn subject(me) {
    let scope = usd_path(me);
    if scope == () { return -1; }
    let owner = query("QueryUsdPrim", #{ path: scope, children: true });
    if owner == () || owner.ok == false || owner.children == () { return -1; }
    for path in owner.children {
        let program = query("QueryUsdPrim", #{ path: path, rels: ["inputs:subject"] });
        if program == () || program.ok == false || program.relationships == () { continue; }
        let targets = program.relationships["inputs:subject"];
        if targets != () && targets.len() == 1 { return find_path(targets[0]); }
    }
    -1
}

fn simulation_dependencies(me, ctx) {
    let target = subject(me);
    let entity_writes = [];
    if target >= 0 { entity_writes.push(target); }
    #{ modelica_entities: [], entity_reads: [],
       entity_writes: entity_writes, query_reads: [], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.started = true;
    let target = subject(me);
    if target >= 0 { brake(target); }
    emit("program_switch_hold_started", usd_path(me));
}

fn on_event(me, evt, ctx) { }

fn task(me, ctx) {
    forever(step(|m| {
        let target = subject(m);
        if target >= 0 { brake(target); }
    }, |m| false))
}

fn on_stop(me, ctx) {
    let target = subject(me);
    if target >= 0 { brake(target); }
}
