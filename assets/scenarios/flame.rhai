// Engine-exhaust plume — a SIGNAL consumer.
//
// Follows the nearest ancestor's `throttle` cosim output (0..1) every tick and
// scales this flame mesh to match: length on Y, width easing in on X/Z, and
// collapsing toward zero (invisible) at zero throttle. ALL the mapping lives
// here — no built-in Rust drives the flame. The Modelica engine model only
// PUBLISHES the throttle signal (a continuous port); turning that signal into a
// visual is scenario/rhai concern, exactly like reading any other port.
//
// `me` is the flame prim — attach this with `lunco:scriptPath` on the flame
// (or on a reusable rocket-engine component that contains it). It works wherever
// the engine is mounted because it WALKS UP to whatever ancestor exposes the
// `throttle` port (the possessed vessel's GNC), rather than hard-coding a path.

/// Nearest ancestor that exposes a `throttle` port, or 0.0 if none. Pure rhai.
fn nearest_throttle(me) {
    let cur = parent(me);
    let hops = 0;
    while cur != () && hops < 5 {
        let t = get(cur, "throttle");
        if t != () { return t; }
        cur = parent(cur);
        hops += 1;
    }
    0.0
}

fn on_tick(me) {
    let t = nearest_throttle(me);
    if t == () { t = 0.0; }
    t = clamp(t, 0.0, 1.0);

    // Width blooms quickly (reads as a lit plume even at low throttle); length
    // tracks throttle linearly. A small floor avoids a fully-degenerate (zero)
    // transform while staying sub-visible at idle.
    let width = (0.35 + 0.65 * t) * 1.4;
    let len = t * 3.0;
    if len < 0.02 { len = 0.02; }

    set(me, "Transform.scale", [width, len, width]);
}
