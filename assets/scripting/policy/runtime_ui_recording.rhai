// Generic runtime UI capture policy.
//
// The recorder asks the active Twin which already-visible surfaces are part of
// a shot. The empty default keeps interactive sessions and captures that do
// not author a UI contract independent of optional HUDs.
fn runtime_ui_recording(facts) {
    []
}
