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
// GEOMETRY IS NOT A PARAMETER. The foot point, the strut axis, the rest
// length and the girth are all already authored — they ARE the leg prim's
// `xformOp:translate` / `rotateXYZ` / `scale`. So this script reads them off
// the leg's own transform instead of taking them as numbers a human has to
// keep in step: move or re-angle a leg in USD and the spring follows it, with
// nothing left to desync.
//
//   axis   u = the leg's local +Y in vessel space (its rotation applied to
//              (0,1,0)) — the strut's own long axis, by construction
//   foot   b = translate − u · scale.y/2, the low end of the unit cube once
//              scaled and rotated. INVARIANT under compression: the script
//              only ever moves the hull end, so re-deriving `b` from the live
//              transform gives the same point at any stroke.
//   rest     = scale.y + the model's current stroke, so a live re-compile
//              mid-compression still recovers the uncompressed length.
//
// Captured once in `on_start` (the transform is authored-fresh there, and a
// per-tick re-derive would slowly random-walk through f32 rounding).

fn sibling_stroke(me) {
    for c in children(me) {
        let s = get(c, "stroke");
        if s != () { return s; }
    }
    0.0
}

// The leg's local +Y in vessel space — column 1 of the rotation matrix built
// from the quaternion `get()` hands back as [x, y, z, w].
fn axis_up(q) {
    let x = q[0];
    let y = q[1];
    let z = q[2];
    let w = q[3];
    let ax = 2.0 * (x * y - w * z);
    let ay = 1.0 - 2.0 * (x * x + z * z);
    let az = 2.0 * (y * z + w * x);
    [ax, ay, az]
}

fn on_start(me) {
    let t = get(me, "Transform.translation");
    let s = get(me, "Transform.scale");
    let q = get(me, "Transform.rotation");
    // Positive-form guard: only proceed when all three reads produced arrays.
    if type_of(t) == "array" && type_of(s) == "array" && type_of(q) == "array" {
        let u = axis_up(q);
        let half = s[1] * 0.5;
        this.girth = s[0];
        this.rest = s[1] + sibling_stroke(me);
        this.ux = u[0];
        this.uy = u[1];
        this.uz = u[2];
        this.bx = t[0] - u[0] * half;
        this.by = t[1] - u[1] * half;
        this.bz = t[2] - u[2] * half;
    }
}

fn on_tick(me) {
    // Nothing captured (the transform was not readable at start) — stay put
    // rather than write a garbage pose.
    if type_of(this.rest) != "f64" { return; }

    // No calculation here beyond applying the model's answer: LegStrut.mo
    // already clamps `stroke` to [0, stroke_max] — the physics, including its
    // own limits, lives in Modelica.
    let live = this.rest - sibling_stroke(me);
    let half = live * 0.5;
    set(me, "Transform.scale", [this.girth, live, this.girth]);
    set(me, "Transform.translation", [
        this.bx + this.ux * half,
        this.by + this.uy * half,
        this.bz + this.uz * half
    ]);
}
