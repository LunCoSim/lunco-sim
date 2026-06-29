// Example: a step SEQUENCE (Layer 1 sequencer). Where mission_plan.rhai is a
// pure waypoint list, this mixes step kinds — drive-until-arrived, a timed
// dwell, and wait-for-event — to show real mission sequencing in pure rhai.
//
// Attach to a rover via ScriptedModel { language: Rhai }. Each step emits
// STEP_COMPLETE(idx); the last emits SEQUENCE_COMPLETE(len). External clients
// see those over SubscribeTelemetry; other scripts react via on_event.

const SITE_A = [12.0, 0.0,  0.0];
const SITE_B = [12.0, 0.0, 25.0];
const SPEED  = 1.0;
const RADIUS = 2.0;

// Built once in on_start and held on `this` (cheaper than rebuilding per tick).
fn build_steps(me) {
    [
        // 1. Drive to site A, then advance once within RADIUS.
        step(|m| nav_to(m, SITE_A, SPEED, RADIUS), |m| arrived(m, SITE_A, RADIUS)),
        // 2. Dwell 5 s (e.g. take a sample), then advance.
        wait(5.0),
        // 3. Hold until something hails us with a GO event.
        wait_for("GO"),
        // 4. Drive to site B and stop.
        step(|m| nav_to(m, SITE_B, SPEED, RADIUS), |m| arrived(m, SITE_B, RADIUS)),
    ]
}

fn on_start(me) {
    this.cur = seq_init();
    this.steps = build_steps(me);
    print("sequence: " + this.steps.len() + " steps queued");
}

fn on_tick(me) {
    this.cur = run_steps(me, this.steps, this.cur);
}

fn on_event(me, evt) {
    // Feed events to the cursor so wait_for("GO") can complete.
    this.cur = seq_note_event(this.cur, evt);
}
