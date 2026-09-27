// Application-level presentation policy.
//
// The camera runtime has already validated the USD facts before calling this
// function. It only asks which explicit presentation action to realize:
// "avatar", "generated", or "none". Returning "none" leaves the viewport
// empty and lets the owning contract report that no initial presentation was
// selected.
fn default_presentation(ctx) {
    if runtime_context.scope != "application"
        || runtime_context.cycle != "presentation"
        || runtime_context.phase != "preparation"
        || runtime_context.clock != "presentation" {
        throw "camera.default_presentation requires Application/Presentation/Preparation context";
    }
    if ctx.local_avatar_camera_count == 1 {
        return "avatar";
    }
    if ctx.host_requests_generated && ctx.camera_track_count == 0 {
        return "generated";
    }
    return "none";
}
