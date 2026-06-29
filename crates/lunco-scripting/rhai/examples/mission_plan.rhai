// Example: a declarative mission plan. The whole scenario is DATA (the PLAN
// array) plus one call to the prelude's run_plan executor — objectives are
// authored here in rhai, never compiled into core.
//
// Attach to a rover via ScriptedModel { language: Rhai }. Each waypoint emits
// OBJECTIVE_COMPLETE; the last emits PLAN_COMPLETE. External clients see those
// over SubscribeTelemetry; other scripts can react via on_event.

const PLAN = [
    [ 12.0, 0.0,   0.0],   // 0: drive out
    [ 12.0, 0.0,  25.0],   // 1: traverse
    [-10.0, 0.0,  25.0],   // 2: cross
    [  0.0, 0.0,   0.0],   // 3: return to base
];
const SPEED = 1.0;
const RADIUS = 2.0;

fn on_start(me) {
    this.idx = 0;
    print("mission_plan: " + PLAN.len() + " objectives queued");
}

fn on_tick(me) {
    if "done_at" in this { return; }   // latch when finished

    // run_plan is stateless — `this` holds the index (only the hook has `this`).
    this.idx = run_plan(me, PLAN, this.idx, SPEED, RADIUS);

    if this.idx >= PLAN.len() {
        this.done_at = sim_tick();
        print("mission_plan: all objectives complete at tick " + this.done_at);
    }
}
