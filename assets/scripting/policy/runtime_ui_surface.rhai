// Generic runtime-surface visibility policy.
//
// The engine supplies a typed fact map. A Twin may replace this hook with its
// own policy when a surface should be always visible, gated by a lesson, or
// shown only after a capability becomes available.
fn preparation_active(facts) {
    if facts.status == "compiling" { return true; }
    let blockers = facts.simulation_progress;
    if blockers == () { return false; }
    for blocker in blockers {
        if blocker.owner == "SceneLifecycle"
            || blocker.owner == "SceneReferences"
            || blocker.owner == "TerrainPreparation"
            || blocker.owner == "DocumentPreparation"
            || blocker.owner == "ModelicaPreparation"
            || blocker.owner == "ScriptPreparation" {
            return true;
        }
    }
    false
}

fn runtime_ui_surface(facts) {
    let visible = if preparation_active(facts) {
        true
    } else if !facts.available {
        false
    } else if facts.visibility_mode == "always" {
        facts.available
    } else if facts.visibility_mode == "possessed" {
        facts.control_owner == "local"
    } else {
        false
    };
    #{ visible: visible }
}
