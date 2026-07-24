// formation.rhai — an example importable TOOL LIBRARY (L3).
//
// Tool libraries are named bundles of higher-level rhai policy, registered as a
// static module namespace. Call them as `formation::fn(...)` from event-driven
// scenario policy. Continuous formation control belongs in Modelica or Rust.
//
// Every function here can freely call the prelude (all_of_type, world_pos,
// vsub, nav_to, …) and the host verbs — they resolve through the engine's
// global namespace at run time. Ship more libraries by dropping a `.rhai` in
// this folder and listing it in tool_libs::built_in_libraries, or register one
// at runtime with the RegisterToolLibrary command (hot-reloadable, no rebuild).

/// The nearest OTHER rover to entity `me`, or () if none. Demonstrates a tool
/// library calling prelude selection helpers across the module boundary.
fn nearest_rover(me) {
    let here = world_pos(me);
    if here == () { return (); }
    let best = nearest_where(here, |e| e.id != me && e.type == "rover");
    if best == () { () } else { best.id }
}

/// Drive `me` to hold `gap` metres line-astern behind `leader` at `speed`.
/// Returns true once in position. Pure rhai over world_pos/world_forward + the
/// prelude nav_to primitive.
fn hold_line(me, leader, gap, speed) {
    let lp = world_pos(leader);
    let lf = world_forward(leader);
    if lp == () || lf == () { return false; }
    // `gap` metres behind the leader, along its backward heading.
    let target = vsub(lp, vscale(vnorm(lf), gap));
    nav_to(me, target, speed, 1.0)
}
