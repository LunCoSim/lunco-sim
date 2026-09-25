// The application startup policy installs this function once. The USD scene
// lifecycle invokes it once after the composed stage and scene projections
// settle, then releases time-dependent consumers with the selected epoch.
fn valid_scene_time_context(context) {
    let no_elapsed_clock = context.clock == "none"
        && context.time_seconds == ()
        && context.delta_seconds == ()
        && context.sequence == ()
        && context.producer == ();
    let scene_lifecycle = context.scope == "twin"
        && context.cycle == "lifecycle"
        && context.phase == "preparation"
        && context.generation != ()
        && no_elapsed_clock;
    let policy_evaluation = context.scope == "application"
        && context.cycle == "repl"
        && context.phase == "evaluation"
        && context.clock == "application"
        && context.producer == ();
    scene_lifecycle || policy_evaluation
}

fn select_scene_time(facts) {
    if !valid_scene_time_context(runtime_context) {
        throw("scene.time.select requires its Twin lifecycle preparation context or an application REPL evaluation");
    }

    if facts.epoch_api == true && facts.epoch_status == "valid" {
        return #{
            source: "authored",
            epoch_jd: facts.authored_epoch_jd,
            warning: (),
        };
    }

    let warning = ();
    if facts.epoch_api == true {
        warning = "Scene root LunCoEpochAPI has a " + facts.epoch_status
            + " lunco:time:epochJd; using current computer UTC converted to TDB."
            + " Author a non-zero epoch for repeatable celestial time.";
    } else if facts.has_celestial_source == true {
        warning = "Scene has celestial sources but no root LunCoEpochAPI; using"
            + " current computer UTC converted to TDB. Author an epoch for"
            + " repeatable celestial time.";
    }

    #{
        source: "computer_time",
        epoch_jd: facts.computer_time_tdb_jd,
        warning: warning,
    }
}
