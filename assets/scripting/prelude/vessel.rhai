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

fn near(value, expected, tolerance) {
    value != () && (value - expected).abs() < tolerance
}

/// Every ECS descendant of `root`, including nested suspension and attachment
/// entities. The production hierarchy is the observable vehicle topology, so
/// callers do not need a second wheel-name convention to inspect it.
fn vessel_descendants(root) {
    let out = [];
    let stack = [];
    for child in children(root) { stack.push(child); }
    let visits = 0;
    while stack.len() > 0 {
        visits += 1;
        if visits > 512 { break; }
        let entity = stack.pop();
        out.push(entity);
        for child in children(entity) { stack.push(child); }
    }
    out
}

/// Count the two production wheel realizations under a vessel and retain their
/// reflected authored parameters. This is intentionally a plain Rhai map: a
/// scenario can decide whether a fleet, a variant pair, or a single vehicle is
/// the subject without adding a Rust test-only query.
fn vessel_wheel_summary(root) {
    let raycast = 0;
    let physical = 0;
    let wheel_count = 0;
    let raycast_radius = ();
    let physical_radius = ();
    let suspension_rest = ();
    let suspension_spring = ();
    let suspension_damping = ();
    for entity in vessel_descendants(root) {
        let ray_radius = get(entity, "WheelRaycast.wheel_radius");
        let physical_radius_value = get(entity, "PhysicalWheel.wheel_radius");
        if ray_radius != () {
            raycast += 1;
            wheel_count += 1;
            if raycast_radius == () { raycast_radius = ray_radius; }
            if suspension_rest == () { suspension_rest = get(entity, "Suspension.rest_length"); }
            if suspension_spring == () { suspension_spring = get(entity, "Suspension.spring_k"); }
            if suspension_damping == () { suspension_damping = get(entity, "Suspension.damping_c"); }
        } else if physical_radius_value != () {
            physical += 1;
            wheel_count += 1;
            if physical_radius == () { physical_radius = physical_radius_value; }
        }
    }
    #{
        raycast: raycast,
        physical: physical,
        wheels: wheel_count,
        raycast_radius: raycast_radius,
        physical_radius: physical_radius,
        suspension_rest: suspension_rest,
        suspension_spring: suspension_spring,
        suspension_damping: suspension_damping,
    }
}

/// Validate a vessel's composed wheel realization and shared raycast-wheel
/// parameters. Asset-specific scenarios supply the expected realization counts
/// and values; the traversal and reflected field names stay centralized here.
fn vessel_wheels_match(root, expected_raycast, expected_physical,
                       radius, suspension_rest, spring_k, damping_c,
                       friction_mu) {
    let summary = vessel_wheel_summary(root);
    if summary.raycast != expected_raycast
        || summary.physical != expected_physical
        || summary.wheels != expected_raycast + expected_physical {
        return false;
    }
    let params_ok = true;
    for entity in vessel_descendants(root) {
        let ray_radius = get(entity, "WheelRaycast.wheel_radius");
        if ray_radius != () {
            params_ok = params_ok
                && near(ray_radius, radius, 0.001)
                && near(get(entity, "WheelRaycast.friction_mu"), friction_mu, 0.001)
                && near(get(entity, "Suspension.rest_length"), suspension_rest, 0.001)
                && near(get(entity, "Suspension.spring_k"), spring_k, 0.1)
                && near(get(entity, "Suspension.damping_c"), damping_c, 0.1);
        } else {
            let physical_radius = get(entity, "PhysicalWheel.wheel_radius");
            if physical_radius != () {
                params_ok = params_ok && near(physical_radius, radius, 0.001);
            }
        }
    }
    params_ok
}

/// Minimum tire friction coefficient over a vessel's wheels, or `()` if it has none.
///
/// **min, not mean**: a vehicle slips at its weakest contact, and averaging would
/// flatter a rover with one bald tire.
fn min_friction(id) {
    if id < 0 { return (); }
    let worst = ();
    for w in vessel_descendants(id) {
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

/// True when `grade_deg` is steeper than this vessel can hold.
///
/// An unknown vessel answers `false`: refuse to call a route undriveable on the
/// strength of a component that failed to load.
fn exceeds_slip(id, grade_deg) {
    let v = slip_limit(id);
    v != () && grade_deg > v
}
