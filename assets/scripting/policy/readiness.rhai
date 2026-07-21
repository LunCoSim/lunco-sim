// ── Readiness policy — what a wait is allowed to freeze ─────────────────────
//
// The engine reports FACTS: "this object's model has not compiled", "the scene
// is still spawning". It does not decide what those facts cost. This file does.
//
// Called once per pending item per frame with:
//
//   kind        "scene_load" | "program_compile" | "participant_init"
//   subject     "world" | "entity"
//   entity      the entity id, or -1 when subject is "world"
//   label       a model name / scene path, for rules that care about one thing
//   elapsed_s   how long this item has been pending (REAL seconds, not sim time)
//   deadline_s  the engine's own give-up point
//
// and must return exactly one of:
//
//   "proceed"      let everything run; the wait is tracked but costs nothing
//   "hold_entity"  freeze THIS OBJECT — no motion, no collisions — world runs on
//   "hold_world"   freeze all physics until it clears
//
// Anything else is refused and the engine's built-in rule applies instead, so a
// typo here degrades to the default rather than to undefined behaviour.
//
// ── Editing this ────────────────────────────────────────────────────────────
//
// Re-registering the hook takes effect on the NEXT FRAME, with items already
// pending — from a scenario:
//
//   register_hook("readiness.action", "readiness_action", `
//       fn readiness_action(ctx) { "proceed" }
//   `);
//
// which is how a test disables holding entirely, or how a demo makes a slow
// compile freeze the whole world so the recording never shows a half-built
// vehicle.

/// Deliberately mirrors `lunco_readiness::Action::builtin`. The two must agree:
/// an app with no scripting runs the Rust rule, and a scene should not behave
/// differently just because scripting is loaded.
fn readiness_action(ctx) {
    // Nothing waits forever. Past the deadline the world runs and the engine
    // logs the overdue item — a frozen app hides the bug, a moving one shows it.
    if ctx.elapsed_s >= ctx.deadline_s {
        return "proceed";
    }

    // A scene mid-spawn has no trustworthy state at all: colliders, terrain and
    // joints are still arriving, and a body integrated against a world that is
    // half there falls through it.
    if ctx.kind == "scene_load" {
        return "hold_world";
    }

    // One object's program is one object's problem. A rover whose script has not
    // compiled must not roll away — but a rover parked next to it, fully ready,
    // has no reason to wait.
    if ctx.kind == "program_compile" || ctx.kind == "participant_init" {
        if ctx.subject == "entity" {
            return "hold_entity";
        }
        // The same wait declared for the world (a co-simulation master coming
        // up) has no single object to freeze.
        return "hold_world";
    }

    // An unrecognised kind is not an emergency. Track it, report it, keep moving.
    "proceed"
}
