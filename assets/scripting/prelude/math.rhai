// ── Math on the [x,y,z] arrays world_pos returns ───────────────────────────

fn vsub(a, b) { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn vadd(a, b) { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn vlen(a)    { (a[0]*a[0] + a[1]*a[1] + a[2]*a[2]).sqrt() }
fn vdot(a, b) { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }
fn vcross(a, b) {
    [ a[1]*b[2] - a[2]*b[1],
      a[2]*b[0] - a[0]*b[2],
      a[0]*b[1] - a[1]*b[0] ]
}
fn vnorm(a) {
    let l = vlen(a);
    if l <= 0.0 { a } else { [a[0]/l, a[1]/l, a[2]/l] }
}
fn clamp(x, lo, hi) { if x < lo { lo } else if x > hi { hi } else { x } }

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

fn vscale(a, s) { [a[0]*s, a[1]*s, a[2]*s] }

// ── Orientation, all derived from world_rotation(id) → [x,y,z,w] ────────────
// The core exposes ONE orientation accessor (the quaternion); every world axis
// and tilt below is derived here in the prelude, so there is no per-axis Rust.

/// Rotate a local vector `v` ([x,y,z]) by quaternion `q` ([x,y,z,w]) into world
/// space: v' = v + w·(2·qv×v) + qv×(2·qv×v).
fn qrot(q, v) {
    let qv = [q[0], q[1], q[2]];
    let t = vscale(vcross(qv, v), 2.0);
    vadd(v, vadd(vscale(t, q[3]), vcross(qv, t)))
}

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
    // world up is [0,1,0], so dot(u, worldUp) == u[1]; u is unit already.
    clamp(u[1], -1.0, 1.0).acos() * 57.29577951308232
}

/// True when `entity` has tipped past `max_deg` from upright (a rover on the
/// edge of rolling). Returns false if orientation is unavailable.
fn is_tipped(id, max_deg) {
    let t = tilt_deg(id);
    t >= 0.0 && t > max_deg
}

