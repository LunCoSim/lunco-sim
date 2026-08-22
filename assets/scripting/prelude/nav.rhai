// ── Navigation (PURE rhai policy over the world_pos/world_forward reads) ─────
//
// All steering math lives here — Rust only exposes the raw position/heading
// reads + the generic `SetPorts` command (via `drive`/`brake`). Tune or replace
// freely; no Rust rebuild.

/// Signed steer command in [-1, 1] that turns `entity` toward world point
/// `target`, matched to the SKID-drive sign convention: the drive mixes
/// `left = drive + steer`, so `+steer` yaws toward the RIGHT (the slower side).
/// A yaw-plane cross `cy = (heading × goal).y > 0` means the target is to the
/// LEFT, so we steer `-cy` to turn toward it. When the target is BEHIND
/// (`dot < 0`) the cross collapses to ~0, so force a hard turn toward its side —
/// otherwise the rover drives straight past forever. Gain 2.5 + clamp = firm turn.
fn steer_to(entity, target) {
    let p = world_pos(entity);
    let f = world_forward(entity);
    if p == () || f == () { return 0.0; }
    let to  = vnorm(vsub(target, p));
    let fwd = vnorm(f);
    let cy  = vcross(fwd, to)[1];
    let dot = vdot(fwd, to);
    if dot < 0.0 {
        if cy >= 0.0 { -1.0 } else { 1.0 }   // target behind → hard turn toward it
    } else {
        clamp(-cy * 2.5, -1.0, 1.0)          // skid: +steer yaws right, so negate
    }
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
        // A target behind the rover requires a true skid pivot.  Any forward
        // command in that state makes the vehicle trace a large arc and can
        // permanently miss a finite waypoint radius.
        let alignment = if dot < 0.0 { 0.0 } else { clamp(0.25 + 0.75 * dot, 0.25, 1.0) };
        // The approach factor is a semantic part of navigation: the requested
        // acceptance radius is where the vehicle must stop, not merely where a
        // fast vehicle is allowed to pass. Two radii provide a smooth braking
        // distance while retaining a small positive command for final steering.
        let approach = clamp((distance_to_target - radius) / (radius * 2.0), 0.15, 1.0);
        fwd = speed * alignment * approach;
    }
    drive(entity, fwd, steer);
    false
}

// ── Declarative plan / objectives (PURE rhai — objectives live here, not core) ─
//
// A "plan" is just data: an array of world waypoints `[[x,y,z], ...]`. New
// missions should lower that data into a native task tree; `run_plan` remains a
// low-level stateless adapter for focused tests and data consumers. Richer
// objectives (sample, dwell, wait-for-event) compose directly as task nodes.

/// Advance a waypoint plan by one tick. `idx` is the current objective index;
/// returns the next index (incremented when the current waypoint is reached).
/// When `idx >= plan.len()` the plan is complete. Prefer `task(me)` with
/// `seq`/`step` for new mission authoring.
fn run_plan(entity, plan, idx, speed, radius) {
    if idx >= plan.len() { return idx; }
    let wp = plan[idx];
    if nav_to(entity, wp, speed, radius) {
        emit("OBJECTIVE_COMPLETE", idx);
        idx += 1;
        if idx >= plan.len() { emit("PLAN_COMPLETE", true); }
    }
    idx
}
