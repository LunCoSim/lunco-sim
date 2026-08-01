// Canonical autonomy envelope for every wheeled rover.
//
// The runtime attaches this file by the authored throttle/steer/brake + drive
// actuator surface. The empty patrol is intentional: it installs a real
// behaviour-tree-backed autopilot that holds safely until a route is supplied.
// Alt+LMB on an authored rover edits USD; Alt+LMB on a spawned rover extends the
// mirrored patrol spec. Both routes therefore drive the same BT/Rust mechanism.
//
// No on_tick: continuous movement belongs to the autopilot fixed-step driver.
fn on_start(me) {
    engage_patrol(me, []);
}

fn on_stop(me) {
    cmd("DisengageAutopilot", #{ vessel: me });
}
