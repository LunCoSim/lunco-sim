// Engine-plume ILLUMINATION — the light-emitting half of the exhaust plume.
//
// Sibling of `flame.rhai`. That script scales the emissive cones; this one drives
// the actual light source, so the plume stops being glowing geometry that lights
// nothing and starts behaving like the very bright, very warm lamp a running
// rocket engine actually is. Mounted the same way, as a `def LunCoProgram` child
// of a `SphereLight` prim:
//
//   def SphereLight "PlumeLight" {
//       def LunCoProgram "PlumeLight" {
//           uniform asset lunco:program:sourceAsset = @lunco://scenarios/flame_light.rhai@
//           custom float lunco:param:imax = 680000
//       }
//   }
//
// WHY THIS CANNOT DISAGREE WITH THE CONE
// Both scripts derive everything from ONE source of truth: the nearest ancestor
// that exposes a `throttle` port, found by the identical walk-up below. Neither
// holds state about the other and neither is driven by a timer, so a full-scale
// cone with a dark light (or a bright light with no cone) is not a reachable
// state — at any tick both are a pure function of the same `throttle` value.
// The flicker is deliberately NOT shared: it is cosmetic, and a light that
// flickers on exactly the same samples as the mesh reads as a strobing bug
// rather than as combustion. Depth is kept low here for the same reason.
//
// A NOTE ON `set(me, "PointLight.intensity", ...)`
// `me` is the prim that OWNS this program (the `SphereLight`), and USD's
// `SphereLight` is loaded as a Bevy `PointLight` (see `lunco-usd-bevy/src/light.rs`
// -- it becomes a `SpotLight` instead if `UsdLuxShapingAPI` cone attrs are
// authored, which is why this prim must NOT author them). `set` reaches any
// reflect-registered component field, so no engine support was needed for this.

/// Nearest ancestor that exposes a `throttle` port, or 0.0 if none.
/// Identical to `flame.rhai`'s — same signal, same hop budget, deliberately.
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

    // Per-instance config.
    //   imax  — LUMENS (luminous power) at full throttle, not candela: Bevy's
    //           `PointLight::intensity` is total flux, so illuminance at range d
    //           is imax/(4*pi*d^2). Getting this backwards costs a factor of
    //           4*pi ~= 12.6 and reads as "the light is authored but invisible".
    //   rmax  — emitter radius (m) at full throttle; softens the falloff
    //   flick — 0 = steady, 1 = full flicker depth
    let imax  = param(me, "imax", 680000.0);
    let rmax  = param(me, "rmax", 0.6);
    let depth = param(me, "flick", 0.35);

    // Smoothed flicker, same shape as the cone's but shallower by default.
    let target = 0.8 + rand() * 0.4;
    if this.flick == () { this.flick = 1.0; }
    this.flick = this.flick * 0.7 + target * 0.3;
    let fl = 1.0 + (this.flick - 1.0) * depth;

    // Radiant output tracks chamber power, which for a throttled engine is very
    // nearly linear in mass flow — so intensity is linear in throttle rather
    // than eased. The important property is the endpoint: at t = 0 this is
    // EXACTLY 0, not a small residual. A dead engine must contribute no light
    // at all, or every coasting shot picks up a phantom glow from underneath.
    let intensity = t * imax * fl;
    if t <= 0.0 { intensity = 0.0; }

    // The emitter grows with throttle as the cone does. Bevy treats
    // `PointLight::radius` as a physical source size, so this also softens the
    // terminator of the light pool instead of leaving a hard inverse-square dot.
    let radius = 0.06 + t * rmax;

    set(me, "PointLight.intensity", intensity);
    set(me, "PointLight.radius", radius);
}
