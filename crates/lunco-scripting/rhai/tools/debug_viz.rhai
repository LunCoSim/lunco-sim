// Debug visualization tools — toggle joint / wheel-force gizmos from any
// Rhai scenario or hook. Mirrors the `ToggleJointViz` typed command via
// the `cmd()` bus, so the same toggle is reachable from UI, HTTP API,
// MCP, and scripts.
//
// Usage (in a scenario `on_start` or interactively):
//
//   import "debug_viz" as viz;
//   viz.show_joints(true);          // anchor dots + axis lines
//   viz.show_wheel_forces(true);    // force box + arrow at each wheel
//   viz.disable_all();              // turn everything off

/// Toggle joint visualization (anchor dots + axis lines for every
/// Avian joint in the scene).
fn show_joints(on) {
    cmd("ToggleJointViz", #{show_joints: on, show_wheel_forces: false});
}

/// Toggle wheel-force visualization (wireframe box + force/velocity
/// arrows at every `PhysicalWheel` or `WheelRaycast`).
fn show_wheel_forces(on) {
    cmd("ToggleJointViz", #{show_joints: false, show_wheel_forces: on});
}

/// Enable or disable both layers at once.
fn set_all(joints, wheel_forces) {
    cmd("ToggleJointViz", #{show_joints: joints, show_wheel_forces: wheel_forces});
}

/// Turn off all debug visualization.
fn disable_all() {
    set_all(false, false);
}

/// Enable everything (quick "turn on debug" shortcut).
fn enable_all() {
    set_all(true, true);
}
