// Example scenario: a rover patrols a loop of world waypoints.
//
// Attach to a rover entity via `ScriptedModel { language: Rhai, document_id }`.
// The runtime calls `on_start(self)` once, then `on_tick(self)` every
// FixedUpdate, where `self` is the host entity's GlobalEntityId.
//
// State model (important rhai rule):
//   - `const` globals ARE visible inside functions  -> use for config.
//   - top-level `let`s are NOT visible inside functions -> don't use for state.
//   - mutable tick-to-tick state lives on `this` (a per-entity object map).
//
// Helpers used (drive/brake/arrived) come from the embedded prelude.

const WAYPOINTS = [
    [10.0, 0.0,  0.0],
    [10.0, 0.0, 20.0],
    [-5.0, 0.0, 20.0],
];
const ARRIVE_RADIUS = 2.0;   // metres

fn on_start(me) {
    this.idx = 0;
    print("patrol: armed on entity " + me);
}

fn on_tick(me) {
    let wp = WAYPOINTS[this.idx];

    // nav_to() = pure-rhai closed-loop steering (heading error -> steer), built
    // from the world_pos/world_forward reads + DriveRover. Returns true on
    // arrival, where we advance to the next waypoint.
    if nav_to(me, wp, 1.0, ARRIVE_RADIUS) {
        // Announce arrival on the shared bus. Any other script (or the API /
        // YAMCS / ROS2 bridge) can react — see examples/mission.rhai.
        emit("WAYPOINT_REACHED", this.idx);
        this.idx = (this.idx + 1) % WAYPOINTS.len();
        print("patrol: reached waypoint, heading to " + this.idx);
    }
}
