// Example: a declarative mission plan. The whole scenario is DATA plus one
// task-tree declaration — objectives are authored here in rhai, never compiled
// into core.
//
// Attach to a rover via ScriptedModel { language: Rhai }. Each waypoint emits
// OBJECTIVE_COMPLETE; the last emits PLAN_COMPLETE. External clients see those
// over SubscribeTelemetry. The native task kernel owns progression; there is no
// hand-written on_tick loop.

fn waypoint(wp, index, speed, radius) {
    seq([
        step(|m| nav_to(m, wp, speed, radius),
             |m| arrived(m, wp, radius)),
        once(|m| emit("OBJECTIVE_COMPLETE", index)),
    ])
}

fn task(me) {
    // Keep mission data in the directly-called task function. Rhai does not
    // resolve top-level constants from an indirect task invocation.
    let plan = [
        [ 12.0, 0.0,   0.0],   // 0: drive out
        [ 12.0, 0.0,  25.0],   // 1: traverse
        [-10.0, 0.0,  25.0],   // 2: cross
        [  0.0, 0.0,   0.0],   // 3: return to base
    ];
    let speed = 1.0;
    let radius = 2.0;
    let steps = [];
    for i in 0..plan.len() {
        steps.push(waypoint(plan[i], i, speed, radius));
    }
    steps.push(once(|m| {
        emit("PLAN_COMPLETE", true);
        print("mission_plan: all objectives complete at tick " + sim_tick());
    }));
    steps.push(wait(3600000.0));
    seq(steps)
}
