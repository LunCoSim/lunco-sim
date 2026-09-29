fn valid_scene_composition_context(context) {
    let no_elapsed_clock = context.clock == "none"
        && context.time_seconds == ()
        && context.delta_seconds == ()
        && context.sequence == ()
        && context.producer == ();
    let lifecycle = (context.scope == "application" || context.scope == "twin")
        && context.cycle == "lifecycle"
        && context.phase == "preparation"
        && no_elapsed_clock;
    let policy_inspection = context.scope == "application"
        && context.cycle == "repl"
        && context.phase == "evaluation"
        && context.clock == "application"
        && context.producer == ();
    lifecycle || policy_inspection
}

fn scene_composition_admission(facts) {
    if !valid_scene_composition_context(runtime_context) {
        throw("usd.scene_composition requires lifecycle preparation or application REPL inspection");
    }

    // Preserve the current semantic default. A Twin policy may replace this
    // hook when its authored composition requirements call for rejection.
    #{ action: "allow_partial" }
}
