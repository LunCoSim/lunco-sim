// Reusable autonomy envelope for an explicitly authored wheeled rover.
//
// A rover opts in through an authored LunCoProgramAPI source or a RunScenario
// command. The empty patrol is intentional: it installs a real
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
