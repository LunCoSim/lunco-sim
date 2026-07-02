// Debug visualization tools — toggle joint / wheel-force gizmos from any
// Rhai scenario or hook. Mirrors the `ToggleJointViz` typed command via
// the `cmd()` bus, so the same toggle is reachable from UI, HTTP API,
// MCP, and scripts.
//
// A tool library binds as a static-module NAMESPACE — call `debug_viz::fn(...)`
// directly, from anywhere including inside `on_tick`. NO `import` statement
// (script-level `import` aliases are invisible to rhai's pure hook functions;
// static modules are not).
//
//   fn on_start(me) {
//       debug_viz::show_joints(true);        // anchor dots + axis lines
//       debug_viz::show_wheel_forces(true);  // force box + arrow at each wheel
//   }
//   fn on_stop(me) { debug_viz::disable_all(); }   // turn everything off

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
