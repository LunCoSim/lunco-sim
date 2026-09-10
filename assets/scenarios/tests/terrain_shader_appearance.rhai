// The image and the production render diagnostics are the graphics verdict.
// The camera is authored in USD so near/middle/far coverage is stable and this
// scenario carries no vehicle-specific or renderer-only behavior.
const TEST_KIND = "graphics";

fn on_start(me) {
    notify_kind("Terrain shader appearance: capturing settled near/middle/far relief.", "info");
}
