// Example scenario: a rover patrols a loop of world waypoints.
//
// Attach to a rover entity via `ScriptedModel { language: Rhai, document_id }`.
// The native task kernel owns the fixed-tick cursor; this policy has no
// hand-written on_tick function and no mutable `this.idx` state.

fn patrol_leg(wp, index, radius) {
    seq([
        step(|m| nav_to(m, wp, 1.0, radius),
             |m| arrived(m, wp, radius)),
        once(|m| {
            emit("WAYPOINT_REACHED", index);
            print("patrol: reached waypoint " + index);
        }),
    ])
}

fn task(me) {
    // Configuration is local to the directly-called task function. Rhai does
    // not resolve top-level constants from an indirect task invocation.
    let waypoints = [
        [10.0, 0.0,  0.0],
        [10.0, 0.0, 20.0],
        [-5.0, 0.0, 20.0],
    ];
    let radius = 2.0;
    let legs = [];
    for i in 0..waypoints.len() {
        legs.push(patrol_leg(waypoints[i], i, radius));
    }
    forever(seq(legs))
}
