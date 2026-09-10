// Negative control for the multi-rover stress matrix. The fixture deliberately
// contains three rovers; this scenario passes only when the production roster
// exposes that unsupported cardinality instead of treating it as a stress run.

fn on_start(me) {
    this.done = false;
    this.rovers = [];
    let group = find("/MultiRoverStress/Rovers");
    if group != () && group >= 0 {
        for rover in children(group) {
            if world_pos(rover) != () { this.rovers.push(rover); }
        }
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.done = true;
    report_verdict([
        t_true(this.rovers.len() == 3,
            "negative fixture roster changed; expected exactly three rovers"),
        t_true(this.rovers.len() < 4,
            "unsupported three-rover matrix was accepted as supported"),
    ], "MULTI-ROVER STRESS NEGATIVE", "MULTI_ROVER_STRESS_NEGATIVE");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["negative multi-rover fixture stopped before its cardinality observation"],
            "MULTI-ROVER STRESS NEGATIVE", "MULTI_ROVER_STRESS_NEGATIVE");
    }
}
