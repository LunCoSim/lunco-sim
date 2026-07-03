//! Lander & Rover Mission — standalone tutorial orchestrator (one source).
//!
//! The lesson IS this script: it loads its own multi-vehicle scene and coaches
//! the student. Run by the launcher on a host entity; no `lunco:scriptPath`.

fn on_start(me) {
    load_scene("scenes/sandbox/lander_test.usda");
    hint("A powered descent lands a rover. Watch the touchdown, then take control (click it or press F) and drive the deployed rover.");
    notify_kind("Tutorial: Lander & Rover Mission", "info");
}
