// Landing-leg strut ACTUATOR — the display half of the sprung leg.
//
// The PHYSICS lives in `models/LegStrut.mo`: one Modelica spring-damper per
// leg, wired in `descent_lander.usda` to the vessel's altimeter and vertical
// speed, integrating on the solver clock and publishing TWO ports:
//   stroke — compression (m)   load — axial force (N)
// This script owns NO dynamics and no constants of its own — it applies the
// model's answers to the scene, exactly as `flame.rhai` applies the engine
// model's `throttle` to the plume cone. USD wires + Modelica physics + a rhai
// actuator; no custom Rust anywhere.
//
// WHAT MOVES. The strut is a unit cube whose authored `xformOp:scale.y` IS
// its length (unit-mesh idiom). Compression keeps the FOOT end pinned — the
// pad stays planted where physics put it — and shortens the strut upward,
// sliding it INTO the fixed piston sleeve (`Piston*` cylinders) at the hull
// end: small tube into bigger tube. Colliders untouched; the body stays rigid.
//
// WHAT GLOWS. The strut's bound material (`StrutGlow_Mat`) authors the glow
// colour AT REFERENCE LOAD in USD. On first tick the actuator latches that
// emissive as its chroma (read through the standard material:binding chain,
// the same read `flame.rhai` does), then writes it back each tick scaled by
// the strut model's live `load` — so the light on screen IS the force in the
// equations, the colour is a USD art decision, and the signal is Modelica's.
//
// Mounted as `def LunCoProgram "Actuator"` on each leg. `me` is the LEG prim;
// the strut model is mounted beside it, so "the sibling that publishes
// `stroke`" is the whole lookup — no name inference.
//
// Per-instance params (typed, one set per leg):
//   bx,by,bz — strut FOOT point, vessel-local
//   ux,uy,uz — unit axis foot→hull, vessel-local
//   len      — authored strut length     girth — authored strut thickness
//   load_ref — load (N) at which the glow reaches its authored colour

fn strut_sibling(me) {
    for c in children(me) {
        if get(c, "stroke") != () { return c; }
    }
    ()
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

    let strut = strut_sibling(me);
    if strut == () { return; }

    // No calculation beyond applying the model's answers: LegStrut.mo already
    // clamps `stroke` to [0, stroke_max] — the physics, including its own
    // limits, lives in Modelica.
    let stroke = get(strut, "stroke");
    if stroke == () { return; }
    let live = len - stroke;
    set(me, "Transform.scale", [girth, live, girth]);
    set(me, "Transform.translation", [
        bx + ux * live * 0.5,
        by + uy * live * 0.5,
        bz + uz * live * 0.5
    ]);

    // ── Load glow: green = good, red = heavily compressed ────────────────
    // The bound material's emissive (latched once) is the NOMINAL-load green;
    // `hot_r/g/b` params are the overload red. The live colour lerps between
    // them on load/load_ref (t=0 nominal → t=1 overload) while brightness
    // scales with load — unloaded struts are dark metal, a gentle settle
    // glows green, a slam grades through amber into red. Both endpoints are
    // authored (material + params); this script only mixes them by the
    // strut model's live `load`.
    let load = get(strut, "load");
    if load == () { return; }
    if this.base_r == () {
        let er = get(me, "PbrLook.emissive.red");
        let eg = get(me, "PbrLook.emissive.green");
        let eb = get(me, "PbrLook.emissive.blue");
        if er == () || eg == () || eb == () { return; }
        this.base_r = er; this.base_g = eg; this.base_b = eb;
    }
    let s = load / param(me, "load_ref", 1500.0);
    s = max(0.0, min(1.3, s));
    let t = min(1.0, s);
    let hr = param(me, "hot_r", 4.6);
    let hg = param(me, "hot_g", 0.5);
    let hb = param(me, "hot_b", 0.15);
    set(me, "PbrLook.emissive.red",   (this.base_r * (1.0 - t) + hr * t) * s);
    set(me, "PbrLook.emissive.green", (this.base_g * (1.0 - t) + hg * t) * s);
    set(me, "PbrLook.emissive.blue",  (this.base_b * (1.0 - t) + hb * t) * s);
}
