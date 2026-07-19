// Landing-leg strut ACTUATOR — the display half of the sprung leg.
//
// The PHYSICS lives in `models/LegStrut.mo`: one Modelica spring-damper per
// leg, wired in `descent_lander.usda` to the vessel's altimeter and vertical
// speed, integrating its stroke on the solver clock. This script owns NO
// dynamics — it reads the sibling strut model's `stroke` output each tick and
// applies it to the strut geometry, exactly as `flame.rhai` applies the
// engine model's `throttle` to the plume cone. USD wires + Modelica physics +
// a rhai actuator; no custom Rust anywhere.
//
// WHAT MOVES AND WHAT DOES NOT. The strut is a unit cube whose authored
// `xformOp:scale.y` IS its length (unit-mesh idiom, same as the flame cone).
// Compression keeps the FOOT end pinned — the pad stays planted where physics
// put it — and shortens the strut upward, sliding it INTO the fixed piston
// sleeve (`Piston*` cylinders) anchored at the hull end: small tube into
// bigger tube, the mechanism read the audience expects. The colliders are
// untouched; the compound body stays rigid.
//
// Mounted as `def LunCoProgram "Actuator"` on each leg. `me` is the LEG prim
// (a program's `me` is the prim that carries it); the strut model is mounted
// beside it, so "the sibling that publishes `stroke`" is the whole lookup —
// no name inference, same predicate style as flame.rhai's child_light().
//
// Per-instance geometry (typed params, one set per leg):
//   bx,by,bz — the strut's FOOT point, vessel-local
//   ux,uy,uz — unit axis from foot toward hull attach, vessel-local
//   len      — authored strut length   girth — authored strut thickness

fn sibling_stroke(me) {
    for c in children(me) {
        let s = get(c, "stroke");
        if s != () { return s; }
    }
    0.0
}

fn on_tick(me) {
    let len   = param(me, "len", 7.33);
    let girth = param(me, "girth", 0.15);
    let bx = param(me, "bx", 0.0);
    let by = param(me, "by", -5.0);
    let bz = param(me, "bz", 0.0);
    let ux = param(me, "ux", 0.0);
    let uy = param(me, "uy", 1.0);
    let uz = param(me, "uz", 0.0);

    // No calculation here beyond applying the model's answer: LegStrut.mo
    // already clamps `stroke` to [0, stroke_max] — the physics, including its
    // own limits, lives in Modelica.
    let live = len - sibling_stroke(me);
    set(me, "Transform.scale", [girth, live, girth]);
    set(me, "Transform.translation", [
        bx + ux * live * 0.5,
        by + uy * live * 0.5,
        bz + uz * live * 0.5
    ]);
}
