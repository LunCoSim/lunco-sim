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

fn on_start(me) {
    this.done = false;
    print("avoid: heading for goal, probing " + PROBE + "m ahead");
}

fn on_tick(me) {
    if this.done { return; }

    // SENSE: is a collider within PROBE metres straight ahead?
    if obstacle_ahead(me, PROBE) {
        // AVOID: keep creeping forward but turn away. (A richer policy would
        // raycast left and right and steer toward the clearer side — see the
        // raycast() verb; this keeps the example short.)
        drive(me, SPEED * 0.5, TURN);
        emit("AVOIDING", true);
        return;
    }

    // CLEAR: navigate toward the goal. nav_to brakes + returns true on arrival.
    if nav_to(me, GOAL, SPEED, ARRIVE_RADIUS) {
        this.done = true;
        emit("GOAL_REACHED", true);
        print("avoid: goal reached");
    }
}
