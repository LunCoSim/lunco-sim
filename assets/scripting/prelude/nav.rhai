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
/// within `radius` (and brakes). Eases the throttle down while poorly aligned so
/// the rover PIVOTS toward the goal instead of arcing wide past it. The core
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
        let dot = vdot(vnorm(f), vnorm(vsub(target, p)));
        fwd = speed * clamp(0.25 + 0.75 * dot, 0.25, 1.0);
    }
    drive(entity, fwd, steer);
    false
}

// ── Declarative plan / objectives (PURE rhai — objectives live here, not core) ─
//
// A "plan" is just data: an array of world waypoints `[[x,y,z], ...]`. run_plan
// advances one objective at a time, emitting OBJECTIVE_COMPLETE per waypoint and
// PLAN_COMPLETE at the end. The step index IS the resumable state (no coroutines
// needed). IMPORTANT rhai rule: `this` is bound ONLY in the hook that the engine
// calls (on_tick), NOT in functions the hook calls. So run_plan is STATELESS —
// it takes the current index and returns the next; the hook holds the index on
// `this`. Richer objectives (sample, dwell, wait-for-event) compose the same way.

/// Advance a waypoint plan by one tick. `idx` is the current objective index;
/// returns the next index (incremented when the current waypoint is reached).
/// When `idx >= plan.len()` the plan is complete. Typical use in on_tick:
///   `this.idx = run_plan(me, PLAN, this.idx, SPEED, RADIUS);`
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

