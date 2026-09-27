// Generic runtime UI capture policy.
//
// The recorder asks the active Twin which already-visible surfaces are part of
// a shot. The empty default keeps interactive sessions and captures that do
// not author a UI contract independent of optional HUDs.
fn runtime_ui_recording(facts) {
    if runtime_context.scope != "application"
        || runtime_context.cycle != "ui"
        || runtime_context.phase != "preparation"
        || runtime_context.clock != "application" {
        throw "runtime.ui.recording requires Application/Ui/Preparation context";
    }
    []
}
