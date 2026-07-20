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

/// The entity carrying this leg's `LegStrut.mo` ports.
///
/// A `LunCoProgram` prim's ports land on the entity that OWNS the program —
/// the leg — not on a child of it. Scanning `children(me)` (which this did)
/// therefore never found `stroke`, the actuator returned on its first line
/// every tick, and the legs never moved: the green on screen was just the
/// material's authored emissive sitting there statically. `flame.rhai` had
/// this right already — it walks OUTWARD from the prim it decorates until it
/// finds the port. Same shape here: check the owner, then its ancestors, so
/// the actuator works whether it is mounted on the leg or deeper.
fn strut_ports(me) {
    // The model is a `LunCoProgram` CHILD prim of the leg, so its ports are on
    // that child's entity — check children first. The outward walk is the
    // fallback for a leg whose model is applied to the prim itself
    // (`LunCoProgramAPI`), which is how the vessel carries `Lander.mo`.
    for c in children(me) {
        if get(c, "stroke") != () { return c; }
    }
    let cur = me;
    while cur != () {
        if get(cur, "stroke") != () { return cur; }
        cur = parent(cur);
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

    let strut = strut_ports(me);
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

    // ── Load glow: a three-stop ramp on the MODEL's own load fraction ────
    // COLD (unloaded, in flight)  → the bound material's authored emissive:
    //                               a cool white-green "structure idle".
    // WARM (working, settled)     → `warm_*` params: amber.
    // HOT  (rated load, slam)     → `hot_*` params: red.
    //
    // The driving number is `load_frac` — computed INSIDE LegStrut.mo as
    // load/load_rated and clamped there. The strut rates itself; this script
    // owns no threshold and no normalisation, so re-rating the spring in the
    // model re-scales the glow with no edit here. Every colour is authored in
    // USD; this only mixes them. A landing therefore reads as one continuous
    // sweep: white-green on approach, amber as weight comes on, red at the
    // instant of impact, settling back to amber as the spring relaxes.
    let frac = get(strut, "load_frac");
    if frac == () { return; }
    frac = max(0.0, min(1.0, frac));
    if this.cold_r == () {
        let er = get(me, "PbrLook.emissive.red");
        let eg = get(me, "PbrLook.emissive.green");
        let eb = get(me, "PbrLook.emissive.blue");
        if er == () || eg == () || eb == () { return; }
        this.cold_r = er; this.cold_g = eg; this.cold_b = eb;
    }
    // Two segments, meeting at `warm_at` — the fraction where "working" peaks.
    let warm_at = param(me, "warm_at", 0.55);
    let wr = param(me, "warm_r", 4.2);
    let wg = param(me, "warm_g", 2.6);
    let wb = param(me, "warm_b", 0.2);
    let hr = param(me, "hot_r", 5.0);
    let hg = param(me, "hot_g", 0.35);
    let hb = param(me, "hot_b", 0.1);
    let r = 0.0; let g = 0.0; let b = 0.0;
    if frac <= warm_at {
        let t = frac / max(0.001, warm_at);
        r = this.cold_r * (1.0 - t) + wr * t;
        g = this.cold_g * (1.0 - t) + wg * t;
        b = this.cold_b * (1.0 - t) + wb * t;
    } else {
        let t = (frac - warm_at) / max(0.001, 1.0 - warm_at);
        r = wr * (1.0 - t) + hr * t;
        g = wg * (1.0 - t) + hg * t;
        b = wb * (1.0 - t) + hb * t;
    }

    // INTENSITY IS THE LOAD. The hue says how hard the strut is working; this
    // says WHETHER it is working at all. An unloaded strut emits nothing and
    // reads as the plain metal it is — the glow is a highlight that appears
    // under force, not a paint job that is always on. (Constant-brightness
    // green through the whole descent made the legs look like decoration and
    // buried the one moment worth seeing.) `^0.7` lifts the low end so first
    // contact is already visible instead of creeping up from black.
    // `**`, not `pow(..)`: rhai registers f64 exponentiation under the OPERATOR
    // name `**` only (packages/arithmetic.rs — `#[rhai_fn(name = "**")] pow_f_f`);
    // there is no `pow` symbol for floats. `pow(frac, 0.7)` therefore threw
    // "Function not found: pow (f64, f64)" on EVERY tick of EVERY leg, so the
    // three `set(..)` calls below never ran and the struts sat at their authored
    // emissive for the whole descent.
    let lit = frac ** 0.7;
    set(me, "PbrLook.emissive.red",   r * lit);
    set(me, "PbrLook.emissive.green", g * lit);
    set(me, "PbrLook.emissive.blue",  b * lit);
}
