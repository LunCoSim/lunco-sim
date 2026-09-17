// Generic runtime-UI action policy.
//
// Rust owns only the HUI action transport and dropdown state. Twin/scenario
// policy decides what an action means. Camera actions are shared engine
// capabilities, so they live here instead of inside one route program; other
// actions remain available on the typed runtime.ui.action event bus to the
// authored Twin policies that own them.

fn on_action(action) {
    if action == () || action == "" { return #{ ok: true, handled: false }; }
    if handle_camera_ui_action(action) {
        return #{ ok: true, handled: true };
    }
    #{ ok: true, handled: false }
}
