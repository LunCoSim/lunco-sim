// ── Readiness policy — what a wait is allowed to freeze ─────────────────────
//
// The engine reports FACTS: "this object's model has not compiled", "the scene
// is still spawning". It does not decide what those facts cost. This file does.
//
// Called once per pending item per frame with:
//
//   kind        "scene_load" | "program_compile" | "program_failed" | "participant_init"
//   subject     "world" | "entity"
//   entity      the entity id, or -1 when subject is "world"
//   label       a model name / scene path, for rules that care about one thing
//   elapsed_ticks  how long this item has been pending, in FIXED ticks — the
//                  authoritative age. Ticks, not wall-clock, so the terminal
//                  verdict lands on the same tick on every machine and a replay
//                  reproduces. A paused clock does not burn the deadline down.
//   deadline_ticks the engine's terminal-fault threshold, same units
//   elapsed_s      elapsed_ticks x the fixed timestep, for rules that prefer
//                  seconds. Derived, so it is always a whole number of ticks
//   deadline_s     deadline_ticks x the fixed timestep; reaching it faults and
//                  holds the world before this policy may authorize proceed
//   default_action  the persisted default selected by the engine
//   ignore_failed_models explicit best-effort setting for failed Modelica
//
// and must return exactly one of:
//
//   "proceed"      let everything run before the terminal deadline; the wait is
//                  tracked but costs nothing
//   "hold_entity"  freeze THIS OBJECT — no motion, no collisions — world runs on
//   "hold_world"   freeze all physics until it clears
//
// Anything else is refused and the engine's built-in rule applies instead. Once
// the deadline is reached, Rust raises a terminal readiness fault and overrides
// every policy answer, including "proceed".
//
// ── Editing this ────────────────────────────────────────────────────────────
//
// Re-registering the hook takes effect on the NEXT FRAME, with items already
// pending — from a scenario:
//
//   bind_policy("readiness.action", "readiness_action", source);
//
// which is how a test disables holding entirely before the terminal threshold,
// or how a demo makes a slow compile freeze the whole world so the recording
// never shows a half-built vehicle.

/// Deliberately mirrors `lunco_readiness::Action::builtin`. The two must agree:
/// an app with no scripting runs the Rust rule, and a scene should not behave
/// differently just because scripting is loaded.
fn valid_readiness_context(context) {
    context.scope == "core"
        && context.cycle == "simulation"
        && context.phase == "behavior"
        && context.clock == "simulation"
        && context.time_seconds != ()
        && context.delta_seconds != ()
        && context.sequence != ()
        && context.producer == ()
}

fn readiness_action(ctx) {
    if !valid_readiness_context(runtime_context) {
        throw("readiness.action requires its core fixed-simulation behavior context");
    }

    // A failure is not a slow compile. The default is fail-closed: no physics
    // runs against a vehicle whose controller or generated island failed. The
    // persisted best-effort setting deliberately changes the default to
    // `proceed`; scenes may still override it here for an authored exercise.
    if ctx.kind == "program_failed" {
        return ctx.default_action;
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
