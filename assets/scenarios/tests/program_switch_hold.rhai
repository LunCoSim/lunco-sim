// Generic test program used to prove live source switching. It consumes the
// standard `inputs:subject` relationship when present and applies a safe brake;
// it does not know which model owns that subject.

fn subject(me) {
    let path = usd_path(me);
    if path == () { return -1; }
    let doc = query("QueryUsdPrim", #{ path: path, rels: ["inputs:subject"] });
    if doc == () || doc.ok == false || doc.relationships == () { return -1; }
    let targets = doc.relationships["inputs:subject"];
    if targets == () || targets.len() == 0 { return -1; }
    find_path(targets[0])
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
