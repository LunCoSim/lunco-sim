// Example: a declarative mission plan. The whole scenario is DATA (the PLAN
// array) plus one call to the prelude's run_plan executor — objectives are
// authored here in rhai, never compiled into core.
//
// Attach to a rover via ScriptedModel { language: Rhai }. Each waypoint emits
// OBJECTIVE_COMPLETE; the last emits PLAN_COMPLETE. External clients see those
// over SubscribeTelemetry. The native task kernel owns progression; there is no
// hand-written on_tick loop.

const PLAN = [
    [ 12.0, 0.0,   0.0],   // 0: drive out
    [ 12.0, 0.0,  25.0],   // 1: traverse
    [-10.0, 0.0,  25.0],   // 2: cross
    [  0.0, 0.0,   0.0],   // 3: return to base
];
const SPEED = 1.0;
const RADIUS = 2.0;

fn waypoint(wp, index) {
    seq([
        step(|m| nav_to(m, wp, SPEED, RADIUS),
             |m| arrived(m, wp, RADIUS)),
        once(|m| emit("OBJECTIVE_COMPLETE", index)),
    ])
}

fn task(me) {
    let steps = [];
    for i in 0..PLAN.len() {
        steps.push(waypoint(PLAN[i], i));
    }
    steps.push(once(|m| {
        emit("PLAN_COMPLETE", true);
        print("mission_plan: all objectives complete at tick " + sim_tick());
    }));
    steps.push(wait(3600000.0));
    seq(steps)
}
