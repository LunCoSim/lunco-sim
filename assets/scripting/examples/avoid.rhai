// avoid.rhai — SENSING in action: drive to a goal but steer around obstacles.
//
// Demonstrates the sensing half of the bridge — query()-backed spatial probes
// (raycast / obstacle_ahead) — composed with navigation (nav_to). Attach to a
// rover over the API / MCP / UI:
//   RunScenario { target: <rover-gid>, source: <this file> }
//
// All sensing is reads over query("Raycast", …) + world_pos / world_forward;
// all steering is pure-rhai policy in the prelude. Retune freely — no rebuild.

const GOAL = [0.0, 0.0, -40.0];   // a far waypoint, ~straight ahead at start
const SPEED = 1.0;                // m/s setpoint
const ARRIVE_RADIUS = 2.0;        // "arrived" tolerance (m)
const PROBE = 6.0;                // obstacle look-ahead distance (m)
const TURN = 0.6;                 // steer magnitude while avoiding

fn task(me) {
    seq([
        // SENSE and AVOID are one task leaf. The native task kernel invokes the
        // closure once per fixed tick; Rhai only supplies the policy.
        step(
            |m| {
                if obstacle_ahead(m, PROBE) {
                    drive(m, SPEED * 0.5, TURN);
                    emit("AVOIDING", true);
                } else {
                    nav_to(m, GOAL, SPEED, ARRIVE_RADIUS);
                }
            },
            |m| arrived(m, GOAL, ARRIVE_RADIUS),
        ),
        once(|m| {
            emit("GOAL_REACHED", true);
            print("avoid: goal reached");
        }),
        wait(3600000.0), // terminal hold; never re-run the completion action
    ])
}
