// Engine-exhaust plume — a SIGNAL consumer, and the plume's LIGHT.
//
// Scales this cone from the nearest ancestor's `throttle` output (0..1) every
// tick, with a SMOOTHED FLICKER so it reads as a live flame, collapsing to ~zero
// at idle. ALL the mapping lives here — no built-in Rust drives the flame; the
// Modelica engine model only PUBLISHES the throttle signal.
//
// Many cones can share this one script — each mounts it as its own
// `def LunCoProgram "Plume"` child prim with
// `uniform asset lunco:program:sourceAsset = @scenarios/flame.rhai@`. Each program
// prim carries its OWN per-instance config as typed attributes
// (`custom float lunco:param:wmax = 1.05`, `lunco:param:lmax`, `lunco:param:flick`),
// read here through `param(me, key, default)` — `wmax`/`lmax` size, `flick` flicker
// depth — so the script is generic, with NO `name(me)` inference. Works wherever the
// engine is mounted: it WALKS UP to whatever ancestor exposes the `throttle` port
// (the vessel's GNC).
//
// ── THE LIGHT IS PART OF THE PLUME, NOT A SECOND THING ───────────────────────
//
// Emissive geometry in a forward renderer illuminates nothing. A descent burn
// therefore left the regolith directly under the vehicle lit only by the sun,
// which on an airless body — hard shadows, no atmospheric scatter to hide it —
// is the most obviously wrong thing in frame.
//
// The fix used to be a SECOND script (`flame_light.rhai`) on a sibling
// `SphereLight`, with its own copy of `nearest_throttle`, its own flicker state,
// and its own hand-tuned `imax` brightness constant. Three ways for the light and
// the flame to disagree, and the invariant "plume visible ⇔ light on" was
// something two files had to remember rather than something the scene enforced.
//
// Now the `SphereLight` is a CHILD of the cone and this one program drives both,
// from the single throttle read and single flicker it already computes. The
// invariant became structural, in three separate ways, none of which is a rule
// anyone has to follow:
//   - delete or unbind the cone and the light goes with it (child of the prim);
//   - hide the cone and Bevy's inherited visibility puts the light out;
//   - the light sits at the cone's LOCAL ORIGIN, so it rides the plume centroid
//     this script positions each tick for free — and, being at the origin, it is
//     immune to the parent's animated non-uniform scale and 180° flip. There is
//     no second transform to keep in sync.
//
// Cones with no child light (the outer shroud) simply find none and skip that
// half, so the script stays generic: it drives whatever light the scene attached,
// if any.

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

/// This prim's child `PointLight`, or `()` if it has none.
///
/// Identified by the COMPONENT it carries, not by its name — a prim has several
/// children (this very program is one of them) and name matching would be exactly
/// the `name(me)` inference the rest of this script avoids. "The child that is a
/// light" is the actual predicate, so it is the one to test.
fn child_light(me) {
    for c in children(me) {
        if get(c, "PointLight.intensity") != () { return c; }
    }
    ()
}

fn on_tick(me) {
    let t = nearest_throttle(me);
    if t == () { t = 0.0; }
    t = clamp(t, 0.0, 1.0);

    // Smoothed flicker: `rand` is fresh each tick (deterministic), so blend it
    // into a running value — a shimmer, not a strobe.
    let target = 0.8 + rand() * 0.4;
    if this.flick == () { this.flick = 1.0; }
    // Keep the PREVIOUS tick's value before overwriting it. This is the light's
    // flicker: not a second state (no second `rand`, no second filter) — just a
    // one-tick delay tap on the same signal, i.e. a phase offset. It matters
    // because a light that brightens on exactly the same samples as the mesh
    // reads as a strobing bug rather than as combustion.
    let flick_prev = this.flick;
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

    // ── Drive the child light, if this cone has one ──────────────────────────
    let light = child_light(me);
    if light == () { return; }

    // A glowing volume's luminous power IS its luminous exitance times its
    // emitting area — so that is what this computes, rather than authoring a
    // brightness that has no connection to how bright the plume actually LOOKS.
    //
    // RADIANCE comes from the material, read through the standard chain. The USD
    // loader resolves `material:binding` → `Material` → `outputs:surface.connect`
    // → `Shader` → `inputs:emissiveColor` (UsdShade / UsdPreviewSurface, via
    // `lunco_usd_bevy::resolve_bound_shader`) and projects the result into
    // `PbrLook.emissive`, so this read IS the authored `inputs:emissiveColor` —
    // one source of truth. Retint the flame in any DCC and the light follows; it
    // can no longer silently disagree with the thing it is the light of.
    let er = get(me, "PbrLook.emissive.red");
    let eg = get(me, "PbrLook.emissive.green");
    let eb = get(me, "PbrLook.emissive.blue");
    // A cone with no bound emissive shader lights nothing — the honest answer,
    // and the same one the renderer gives.
    if er == () || eg == () || eb == () { return; }

    // Rec.709 luma — the standard luminance weighting, and the reason a green
    // flame of the same RGB magnitude lights a scene far more than a blue one.
    // The emissive is LINEAR and un-normalised (values above 1 are the whole
    // point of an emissive), so this is a relative radiance, not a 0..1 colour.
    let lum = 0.2126 * er + 0.7152 * eg + 0.0722 * eb;

    // AREA: the lateral surface of the cone this script just sized. The prim is a
    // unit cone (radius 1, height 1), so after the scale above its radius is
    // `width` and its height is `len`, giving the exact cone formula
    // A = pi * r * sqrt(r^2 + h^2). The plume radiates from its flank, so lateral
    // area — not base area, not volume — is the emitting surface.
    let area = 3.14159265 * width * (width * width + len * len).sqrt();

    // The ONE authored constant: luminous exitance per unit emissive radiance,
    // in lm/m^2. It is the unit conversion between "how bright the shader says
    // this surface is" and photometric output, which nothing in the scene can
    // supply — everything that VARIES is derived above.
    //
    // CALIBRATION. Bevy's `PointLight::intensity` is LUMINOUS POWER IN LUMENS
    // ("the amount of light emitted by this source in all directions",
    // `bevy_light-0.19.0/src/point_light.rs`), not candela — so illuminance at
    // range d is phi/(4*pi*d^2), NOT phi/d^2. That factor of 4*pi ~= 12.6 is easy
    // to get backwards and presents as "the light is authored but invisible".
    // At full throttle on the core cone (`wmax` 0.5, `lmax` 2.5, emissive
    // (6.0, 3.5, 0.9)): width 0.5, len 2.5, A ~= 4.005 m^2, lum ~= 3.844, so
    //   phi = 44200 * 3.844 * 4.005 ~= 680000 lm
    // and regolith ~3 m below the nozzle sees 680000/(4*pi*9) ~= 6000 lux against
    // this scene's 12000 lux sun — a clearly visible pool that reads as a second
    // source without flattening the scene. It falls off fast: ~540 lux at 10 m and
    // ~90 lux at the 25 m `lunco:light:range` cap, under 1% of the sun, so it
    // cannot quietly become scene fill. That 680000 figure is the empirically
    // verified one; a derivation landing far from it is wrong, not the target.
    let lm_per_m2 = param(me, "exitance", 44200.0);

    // Radiant output tracks chamber power, which for a throttled engine is very
    // nearly linear in mass flow. Here it comes out slightly superlinear because
    // the emitting AREA also grows with throttle — the honest answer for a source
    // whose size is part of what you see.
    //
    // The endpoint is the property that matters: at t == 0 this is EXACTLY 0, not
    // a small residual. The area never quite reaches zero (`len` is floored at
    // 0.02 to keep the transform non-degenerate), so without this gate a dead
    // engine would still emit some thousands of lumens and every coasting shot
    // would pick up a phantom glow from underneath.
    let lfl = 1.0 + (flick_prev - 1.0) * param(me, "lightflick", 0.35);
    let intensity = if t <= 0.0 { 0.0 } else { lm_per_m2 * lum * area * lfl };

    // COLOUR is the emissive's chroma — the same read, normalised so the
    // brightest channel is 1.0, because magnitude is already carried by
    // `intensity` and applying it twice would square it. Written ONCE: a
    // material's colour is a static authored fact, unlike the throttle, and
    // re-writing it every tick would also spam the log if the path ever moved.
    // `Color` is an enum, so `.0` indexes the `LinearRgba` variant's payload.
    if this.tinted == () {
        this.tinted = true;
        let peak = max(er, max(eg, eb));
        if peak > 0.0 {
            set(light, "PointLight.color.0.red", er / peak);
            set(light, "PointLight.color.0.green", eg / peak);
            set(light, "PointLight.color.0.blue", eb / peak);
        }
    }

    set(light, "PointLight.intensity", intensity);
    // Emitter radius grows with the plume. Bevy treats `PointLight::radius` as a
    // physical source size, so this softens the terminator of the light pool
    // instead of leaving a hard inverse-square dot.
    set(light, "PointLight.radius", 0.06 + t * param(me, "rmax", 0.6));
}
