// ── Vessel capability ────────────────────────────────────────────────────────
//
// What a vehicle can physically do, derived from its own authored parts rather
// than typed into every mission that uses it.
//
// WHY THIS EXISTS. A rover's slip limit is `atan(mu)` — determined by the tire's
// authored friction coefficient, and therefore not a number anyone should write
// down. It got written down anyway: `26.6` (the medium tier) ended up hand-copied
// into three lesson scripts, two survey documents and a prose comment, while the
// `mu` it derives from was authored once on `worn.usda`. Retuning the tire made
// all six lie, and lie plausibly, because a stale angle still looks like an angle.
//
// NOTHING CACHES THESE. `atan` of a minimum is not worth storing, and a stored
// copy is precisely the staleness this file removes. See
// `docs/architecture/58-vessel-envelope-and-routes.md`.
//
// ⚠ COST: each call walks the vessel's children and does one reflected read per
// wheel. That is fine for configuration — "colour the terrain for this rover",
// "check this route" — and wrong for a per-tick loop. If a task needs it every
// tick, read it ONCE into `this` in `on_start`. The limit does not change while
// you drive.

/// Minimum tire friction coefficient over a vessel's wheels, or `()` if it has none.
///
/// **min, not mean**: a vehicle slips at its weakest contact, and averaging would
/// flatter a rover with one bald tire.
fn min_friction(id) {
    if id < 0 { return (); }
    let worst = ();
    for w in children(id) {
        let mu = get(w, "WheelRaycast.friction_mu");
        if mu != () {
            if worst == () || mu < worst { worst = mu; }
        }
    }
    worst
}

/// Slip limit in DEGREES — `atan(min mu)`. On ground steeper than this the tires
/// break loose no matter how much torque is available.
///
/// Returns `()` for a vessel with no wheels, or before the scene has loaded them.
/// CALLERS MUST HANDLE `()` — defaulting to a literal here would reintroduce
/// exactly the constant this file exists to delete.
fn slip_limit(id) {
    let mu = min_friction(id);
    if mu == () { return (); }
    // Method form + the literal rad->deg constant, matching `tilt_deg` in math.rhai.
    mu.atan() * 57.29577951308232
}

/// ⚠ SHIM — silent substitution.
///
/// TODO(vessel-envelope): a caller cannot tell "this rover's limit is 26.6°" from
/// "I could not read the wheels, here is a plausible number" — so a lesson can
/// paint a hazard threshold that was never derived, and say nothing. That is the
/// untrackable-fallback shape we are trying to remove elsewhere.
///
/// The fix is not a better default: it is for the caller to be able to ASK whether
/// the envelope resolved, and to refuse + report when it did not. `slip_limit`
/// already returns `()` for exactly that; this wrapper throws the distinction away.
/// Keep it only while lessons need to run before the wheels have loaded, and drop
/// it once load ordering is deterministic.
///
/// Slip limit, or `fallback` when the vessel has no wheels.
///
/// For the common shape "colour the terrain by what THIS rover can hold", where a
/// default beats doing nothing — but the default is passed at the call site, where
/// a reader can see it.
fn slip_limit_or(id, fallback) {
    let v = slip_limit(id);
    if v == () { return fallback; }
    v
}

/// True when `grade_deg` is steeper than this vessel can hold.
///
/// An unknown vessel answers `false`: refuse to call a route undriveable on the
/// strength of a component that failed to load.
fn exceeds_slip(id, grade_deg) {
    let v = slip_limit(id);
    v != () && grade_deg > v
}
