// Generic runtime-surface visibility policy.
//
// The engine supplies a typed fact map. A Twin may replace this hook with its
// own policy when a surface should be always visible, gated by a lesson, or
// shown only after a capability becomes available.
fn runtime_ui_surface(facts) {
    let visible = if facts.visibility_mode == "always" {
        facts.available
    } else if facts.visibility_mode == "possessed" {
        facts.available && facts.control_owner == "local"
    } else {
        false
    };
    #{ visible: visible }
}
