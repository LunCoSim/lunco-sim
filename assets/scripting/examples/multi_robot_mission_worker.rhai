// Event-driven worker for multi_robot_mission_coordinator.rhai.
//
// A worker never writes a shared assignment table. It listens only for events
// addressed to its own GlobalEntityId, installs a native task tree, and reports
// completion once the task reaches its site.

const SITES = [
    [12.0, 0.0,  0.0],
    [12.0, 0.0, 25.0],
    [-10.0, 0.0, 25.0],
    [  0.0, 0.0,  0.0],
];
const SPEED = 1.0;
const RADIUS = 2.0;

fn on_start(me) {
    subscribe_prefix("MISSION_ASSIGN:");
    emit("ROBOT_READY", true);
    print("worker " + me + ": ready");
}

fn on_event(me, evt) {
    let prefix = "MISSION_ASSIGN:" + me.to_string() + ":";
    if !evt.name.starts_with(prefix) { return; }

    let fields = evt.name.split(":");
    if fields.len() != 3 { return; }
    let site = parse_int(fields[2]);
    if site < 0 || site >= SITES.len() { return; }

    let target = SITES[site];
    this.task = seq([
        step(|m| nav_to(m, target, SPEED, RADIUS),
             |m| arrived(m, target, RADIUS)),
        once(|m| {
            emit("ROBOT_SITE_COMPLETE", site);
            print("worker " + m + ": site " + site + " complete");
        }),
    ]);
}

fn on_stop(me) { brake(me); }
