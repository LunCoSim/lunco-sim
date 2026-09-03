// ── Navigation (PURE rhai policy over the world_pos/world_forward reads) ─────
//
// All steering math lives here — Rust only exposes the raw position/heading
// reads + the generic `SetPorts` command (via `drive`/`brake`). Tune or replace
// freely; no Rust rebuild.

/// Signed steer command in [-1, 1] that turns `entity` toward world point
/// `target`, matched to the SKID-drive sign convention: the drive mixes
/// `left = drive + steer`, so `+steer` yaws toward the RIGHT (the slower side).
/// A yaw-plane cross `cy = (heading × goal).y > 0` means the target is to the
/// LEFT, so we steer `-cy` to turn toward it. When the target is unambiguously
/// BEHIND (`dot < -0.25`), use signed reverse throttle instead of asking every
/// vehicle to pivot in place. The near-sideways band remains forward so an
/// Ackermann rover does not flip direction on a tiny heading error.
fn steer_to(entity, target) {
    let p = world_pos(entity);
    let f = world_forward(entity);
    if p == () || f == () { return 0.0; }
    let to  = vnorm(vsub(target, p));
    let fwd = vnorm(f);
    let cy  = vcross(fwd, to)[1];
    let dot = vdot(fwd, to);
    let travel_sign = if dot < -0.25 { -1.0 } else { 1.0 };
    clamp(-cy * 2.5 * travel_sign, -1.0, 1.0) // +steer yaws right, so negate
}

/// Drive `entity` toward world point `target` at `speed`. Returns true once
/// within `radius` (and brakes). Eases the throttle down while poorly aligned
/// and while approaching the acceptance radius, so the rover can turn into a
/// waypoint and settle instead of carrying full speed through it. The core
/// per-tick navigation primitive. (Named `nav_to`, not `goto` — reserved word.)
fn nav_to(entity, target, speed, radius) {
    if arrived(entity, target, radius) {
        brake(entity);
        return true;
    }
    let steer = steer_to(entity, target);
    let p = world_pos(entity);
    let f = world_forward(entity);
    let fwd = speed;
    if p != () && f != () {
        let to = vsub(target, p);
        let distance_to_target = vlen(to);
        let dot = vdot(vnorm(f), vnorm(to));
        // A clearly behind target uses signed reverse drive.  Do not force a
        // zero-throttle pivot: Ackermann has no generic in-place pivot, and the
        // near-sideways band must keep driving while steering into the goal.
        let travel_sign = if dot < -0.25 { -1.0 } else { 1.0 };
        let alignment = clamp(0.25 + 0.75 * dot.abs(), 0.25, 1.0);
        // The approach factor is a semantic part of navigation: the requested
        // acceptance radius is where the vehicle must stop, not merely where a
        // fast vehicle is allowed to pass. Two radii provide a smooth braking
        // distance while retaining a small positive command for final steering.
        let approach = clamp((distance_to_target - radius) / (radius * 2.0), 0.15, 1.0);
        fwd = speed * travel_sign * alignment * approach;
    }
    drive(entity, fwd, steer);
    false
}
