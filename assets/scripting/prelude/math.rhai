// ── Math on the [x,y,z] arrays world_pos returns ───────────────────────────
//
// `vadd` `vsub` `vscale` `vlen` `vdot` `vcross` `vnorm` `clamp` `angle_deg`
// `yaw_delta_deg` are NATIVE — registered from Rust in
// `lunco_scripting::rhai_math`, on the same `[x, y, z]` float arrays. They are
// not defined here because `acos` is a partial function: a dot product of two
// unit vectors leaves its domain by an ulp whenever the vectors are nearly
// identical, and in rhai that surfaced as a NaN every later comparison read as
// false — a poisoned accumulator that looks like a passing test. In Rust the
// domain is handled once, and an unmeasurable angle comes back as `()`.
//
// What stays here is composition over those primitives, and the entity-aware
// helpers that need a world read first.

/// Distance between two entities in metres, or -1.0 if either lacks a position.
fn distance(a, b) {
    let pa = world_pos(a);
    let pb = world_pos(b);
    if pa == () || pb == () { return -1.0; }
    vlen(vsub(pa, pb))
}

/// True once `entity` is within `radius` metres of world point `target` ([x,y,z]).
fn arrived(entity, target, radius) {
    let p = world_pos(entity);
    if p == () { return false; }
    vlen(vsub(p, target)) <= radius
}

// ── Orientation, all derived from world_rotation(id) → [x,y,z,w] ────────────
// The core exposes ONE orientation accessor (the quaternion) and one native
// `qrot`; every world axis and tilt below is a composition of those, which is
// what the prelude is for.

/// World up-vector of an entity ([x,y,z]), or () if it has no orientation.
fn world_up(id) {
    let q = world_rotation(id);
    if q == () { return (); }
    qrot(q, [0.0, 1.0, 0.0])
}

/// World right-vector of an entity ([x,y,z]), or ().
fn world_right(id) {
    let q = world_rotation(id);
    if q == () { return (); }
    qrot(q, [1.0, 0.0, 0.0])
}

/// Tilt of an entity from upright, in DEGREES (angle between its up-vector and
/// world up), or -1.0 if it has no orientation. 0 = level, 90 = on its side.
fn tilt_deg(id) {
    let u = world_up(id);
    if u == () { return -1.0; }
    // Through `angle_deg` so the domain of `acos` is enforced in one place. A
    // body whose orientation is degenerate has no tilt to report, and -1.0 is
    // this function's existing "unavailable" answer — the same one an entity
    // with no orientation gets. Returning NaN instead would compare false
    // against every threshold and read as "not tipped".
    let deg = angle_deg(u, [0.0, 1.0, 0.0]);
    if deg == () { return -1.0; }
    deg
}

/// True when `entity` has tipped past `max_deg` from upright (a rover on the
/// edge of rolling). Returns false if orientation is unavailable.
fn is_tipped(id, max_deg) {
    let t = tilt_deg(id);
    t >= 0.0 && t > max_deg
}

