// Example scenario: a rover patrols a loop of world waypoints.
//
// Attach to a rover entity via `ScriptedModel { language: Rhai, document_id }`.
// The native task kernel owns the fixed-tick cursor; this policy has no
// hand-written on_tick function and no mutable `this.idx` state.

const WAYPOINTS = [
    [10.0, 0.0,  0.0],
    [10.0, 0.0, 20.0],
    [-5.0, 0.0, 20.0],
];
const ARRIVE_RADIUS = 2.0;   // metres

fn patrol_leg(wp, index) {
    seq([
        step(|m| nav_to(m, wp, 1.0, ARRIVE_RADIUS),
             |m| arrived(m, wp, ARRIVE_RADIUS)),
        once(|m| {
            emit("WAYPOINT_REACHED", index);
            print("patrol: reached waypoint " + index);
        }),
    ])
}

fn task(me) {
    let legs = [];
    for i in 0..WAYPOINTS.len() {
        legs.push(patrol_leg(WAYPOINTS[i], i));
    }
    forever(seq(legs))
}
