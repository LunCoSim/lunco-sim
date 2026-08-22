// Example: a linear task sequence. Where mission_plan.rhai is a
// pure waypoint list, this mixes step kinds — drive-until-arrived, a timed
// dwell, and wait-for-event — to show real mission sequencing in pure rhai.
//
// Attach to a rover via ScriptedModel { language: Rhai }. The native task
// kernel handles both the cursor and event waits.

const SITE_A = [12.0, 0.0,  0.0];
const SITE_B = [12.0, 0.0, 25.0];
const SPEED  = 1.0;
const RADIUS = 2.0;

fn task(me) {
    seq([
        // 1. Drive to site A, then advance once within RADIUS.
        step(|m| nav_to(m, SITE_A, SPEED, RADIUS), |m| arrived(m, SITE_A, RADIUS)),
        // 2. Dwell 5 s (e.g. take a sample), then advance.
        wait(5.0),
        // 3. Hold until something hails us with a GO event.
        wait_for("GO"),
        // 4. Drive to site B and stop.
        step(|m| nav_to(m, SITE_B, SPEED, RADIUS), |m| arrived(m, SITE_B, RADIUS)),
        once(|m| brake(m)),
    ])
}
