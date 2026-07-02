// survey.rhai — an example custom TOOL LIBRARY you author yourself.
//
// A tool library is a named bundle of reusable policy. Drop a `.rhai` in this
// folder (or register one at runtime with the RegisterToolLibrary command —
// hot-reloadable, no rebuild) and its functions are callable as `survey::fn(...)`
// from ANY scenario hook. No `import` statement is needed: tool libraries bind as
// static module namespaces, which (unlike script imports) resolve inside the pure
// hook functions the engine calls. Functions here may freely call the prelude
// (world_pos, nearest_where, vsub, …) and the host verbs.
//
//   fn on_start(me) { this.leg = 0; this.visited = []; }
//   fn on_tick(me)  {
//       let p = survey::lawnmower_target(me, 20.0, 5.0, this.leg);
//       if nav_to(me, p, 1.0, 2.0) { this.leg += 1; }
//   }

/// A waypoint on a boustrophedon ("lawnmower") survey pattern. `leg` selects the
/// row (rows are `spacing` metres apart, each `width` long, alternating
/// direction). Pure data+math — the rover follows the returned point with the
/// prelude's nav_to. Returns () if `me` has no position.
fn lawnmower_target(me, width, spacing, leg) {
    let here = world_pos(me);
    if here == () { return (); }
    let row = leg / 2;                          // integer row index
    let x = if leg % 2 == 0 { width } else { -width };
    [x, here[1], here[2] + row.to_float() * spacing]
}

/// The nearest registered entity NOT in the `visited` id list, or () if every
/// nearby entity has been visited. Shows a tool composing prelude selection
/// helpers (nearest_where) across the module boundary.
fn nearest_unvisited(me, visited) {
    let here = world_pos(me);
    if here == () { return (); }
    let best = nearest_where(here, |e| e.id != me && !visited.contains(e.id));
    if best == () { () } else { best.id }
}
