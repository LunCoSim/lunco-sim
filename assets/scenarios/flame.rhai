// Engine-exhaust plume — a SIGNAL consumer.
//
// Scales this cone from the nearest ancestor's `throttle` output (0..1) every
// tick, with a SMOOTHED FLICKER so it reads as a live flame, collapsing to ~zero
// at idle. ALL the mapping lives here — no built-in Rust drives the flame; the
// Modelica engine model only PUBLISHES the throttle signal.
//
// Many cones can share this one script — each mounts it as its own
// `def LuncoProgram "Plume"` child prim with
// `uniform asset lunco:program:sourceAsset = @scenarios/flame.rhai@`. Each program
// prim carries its OWN per-instance config as typed attributes
// (`custom float lunco:param:wmax = 1.05`, `lunco:param:lmax`, `lunco:param:flick`),
// read here through `param(me, key, default)` — `wmax`/`lmax` size, `flick` flicker
// depth — so the script is generic, with NO `name(me)` inference. Works wherever the
// engine is mounted: it WALKS UP to whatever ancestor exposes the `throttle` port
// (the vessel's GNC).

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

    // Smoothed flicker: `rand` is fresh each tick (deterministic), so blend it
    // into a running value — a shimmer, not a strobe.
    let target = 0.8 + rand() * 0.4;
    if this.flick == () { this.flick = 1.0; }
    this.flick = this.flick * 0.7 + target * 0.3;

    // Per-instance config — typed, fast (a HashMap read), no name inference.
    let wmax  = param(me, "wmax", 1.0);    // max width scale
    let lmax  = param(me, "lmax", 3.0);    // max length scale
    let depth = param(me, "flick", 1.0);   // 0 = steady, 1 = full flicker
    let fl = 1.0 + (this.flick - 1.0) * depth;

    // Width blooms quickly with throttle; length tracks throttle and flickers.
    let width = (0.28 + 0.72 * t) * wmax;
    let len = t * lmax * fl;
    if len < 0.02 { len = 0.02; } // avoid a fully-degenerate transform at idle

    set(me, "Transform.scale", [width, len, width]);
    set(me, "Transform.translation", [0.0, -2.7 - len / 2.0, 0.0]);
}
