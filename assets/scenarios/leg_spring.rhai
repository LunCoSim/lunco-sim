// Landing-leg SHOCK ABSORBER — a signal consumer, like `flame.rhai`.
//
// Mounted as a `def LunCoProgram "Spring"` child on each leg strut. On touchdown
// it compresses the strut along its own axis with a damped spring response and
// settles slightly short of full extension — the "landing legs compress,
// soaking up the impact" the episode narration promises, made visible.
//
// WHAT MOVES AND WHAT DOES NOT. The strut is a unit cube whose authored
// `xformOp:scale.y` IS its length (same idiom as the flame cone and the
// altimeter beam: unit mesh, live length in the transform). Compression keeps
// the strut's FOOT end pinned — the pad stays planted where physics put it —
// and shortens the strut upward, so the sliding joint disappears under the
// hull's silhouette instead of opening a visible gap at the ground. The pads
// and their colliders are untouched: the vehicle keeps resting on exactly the
// contact set the solver resolved. This is a visual spring, not a physics
// joint — the compound body stays rigid, which is what keeps it cheap and
// deterministic.
//
// Per-instance geometry arrives as typed params (the script is generic across
// all four legs; nothing here infers a name):
//   bx,by,bz — the strut's FOOT point, in the vessel's local frame
//   ux,uy,uz — unit axis from foot toward hull attach, local frame
//   len      — authored strut length (the unit cube's scale.y)
//   girth    — authored strut thickness (scale.x/z)
//
// Touchdown is detected from the same ancestor ports the episode script flies
// by: `height` (altimeter, reads ≈5 parked) and `velocity_y`. The walk-up is
// `nearest_throttle`'s, generalised.

fn nearest_port(me, key) {
    let cur = parent(me);
    let hops = 0;
    while cur != () && hops < 5 {
        let v = get(cur, key);
        if v != () { return v; }
        cur = parent(cur);
        hops += 1;
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

    let h  = nearest_port(me, "height");
    let vy = nearest_port(me, "velocity_y");

    if this.landed != true {
        // Airborne: remember the sink rate — the impact amplitude is the LAST
        // airborne reading, not the post-contact one the solver has already
        // nulled by the time the altimeter gate trips.
        if vy != () && vy < -0.05 { this.impact_vy = vy; }
        // Touchdown: low AND arriving with real downward speed remembered.
        if h != () && h < 6.2 && this.impact_vy != () && this.impact_vy < -0.3 {
            this.landed = true;
            this.t = 0.0;
            // Stroke fraction of the strut: ~3% floor plus ~1.8%/(m/s) of
            // impact, capped at 10% (0.73 m on this leg) so a hard arrival
            // reads as a deep flex, never as the hull swallowing the legs.
            let iv = -this.impact_vy;
            this.amp = min(0.10, 0.03 + 0.018 * iv);
        }
        return;
    }

    // Damped spring: rises from 0 at contact, overshoots to peak flex in
    // ~0.3 s, rings down and SETTLES at 35% of peak — legs stay visibly
    // loaded under the vehicle's weight, as sprung struts do.
    this.t += 0.02;
    let t = this.t;
    let c = this.amp * (0.35 + 0.65 * (-2.2 * t).exp() * (8.0 * t - 2.14).cos());
    if c < 0.0 { c = 0.0; }

    let live = len * (1.0 - c);
    set(me, "Transform.scale", [girth, live, girth]);
    set(me, "Transform.translation", [
        bx + ux * live * 0.5,
        by + uy * live * 0.5,
        bz + uz * live * 0.5
    ]);
}
