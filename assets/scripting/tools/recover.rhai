// Vessel recovery — put a rover that ended up on its roof back on its wheels.
//
// NOTHING HERE RUNS BY ITSELF. Recovery used to be automatic: a Rust system
// watched every vessel and righted any that rested overturned for 3 seconds.
// That is gone. A rover on its roof is evidence — about the terrain, the
// suspension, the CoM, or the driving — and a runtime that silently undoes it
// throws that evidence away before anyone sees it. So recovery is now something
// a person (or a scenario that has decided it wants this) asks for.
//
// A tool library binds as a static-module NAMESPACE — call `recover::fn(...)`
// directly, from anywhere including inside `on_tick`. NO `import` statement.
//
//   fn on_event(me, ev) {
//       if ev.name == "stuck" { recover::vessel(ev.entity); }
//   }
//
// This file IS the Recover tool in the editor. Exposing `on_click(id)` is what
// puts a button in the Tools palette — there is no Rust side to it, and no
// registration anywhere. Delete this file and the button goes with it.

/// Editor click handler: arm "Recover" in the Tools palette, click a vessel.
/// `id` is whatever selectable object was under the cursor.
///
/// Deliberately unconditional — it does NOT check `is_overturned` first. You
/// clicked the thing; second-guessing that would resurrect the judgement call
/// the old automatic system got wrong.
fn on_click(id) {
    vessel(id);
}

/// Right one vessel: rotate it (and its whole joint-connected assembly) upright
/// about its own position, reseat it just above the surface, zero its
/// velocities. Heading is approximately preserved — this is a righting, not a
/// teleport, so position and mission progress survive.
///
/// `id` is the entity's id, as reported by `list_entities()`.
fn vessel(id) {
    cmd("RecoverVessel", #{entity_id: id});
}

/// How far a vessel's up-axis may fall before this library calls it overturned:
/// `up · world_up` at or below this is more than ~78 degrees of roll. A vessel
/// on its side reads ~0.0; fully inverted reads -1.0.
const OVERTURNED_UP_DOT = 0.2;

/// Is this vessel overturned? Reads the entity's world rotation and compares its
/// local up against world up.
///
/// Note this asks ONLY about attitude, not about whether the vessel is stuck: a
/// rover mid-tumble down a slope is "overturned" for a second and needs no help.
/// Callers that want "stuck" should also check that it has stopped moving —
/// which is exactly the judgement the old automatic system made on everyone's
/// behalf, and got wrong often enough to be worth leaving to the caller.
fn is_overturned(id) {
    let q = world_rotation(id);
    // No orientation is not "overturned" — an entity that cannot answer must not
    // be recovered on a guess.
    if q == () { return false; }
    let up = qrot(q, [0.0, 1.0, 0.0]);
    if up == () { return false; }
    up[1] <= OVERTURNED_UP_DOT
}

/// Right every overturned vessel in the scene, and return how many were righted.
///
/// The blunt instrument, for a scenario that has genuinely decided it wants one
/// (a soak test, a demo that must not stall). Prefer `vessel(id)` — a script
/// that knows WHICH vessel it is nursing is saying something; one that rights
/// everything on a timer has quietly rebuilt the system this replaced.
fn all_overturned() {
    let n = 0;
    for e in list_entities() {
        if is_overturned(e.id) {
            vessel(e.id);
            n += 1;
        }
    }
    n
}
