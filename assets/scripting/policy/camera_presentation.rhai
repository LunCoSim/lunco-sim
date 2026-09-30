// Application-level presentation policy.
//
// CameraTrack and explicit operator/director selections take precedence. With
// neither, prefer the unique local embodiment, then the first authored camera
// in stable USD-path order. Standalone hosts retain generated framing when the
// active scene has no authored camera.
fn default_presentation(ctx) {
    if runtime_context.scope != "application"
        || runtime_context.cycle != "presentation"
        || runtime_context.phase != "preparation"
        || runtime_context.clock != "presentation" {
        throw "camera.default_presentation requires Application/Presentation/Preparation context";
    }
    if ctx.camera_track_count > 0 || ctx.local_avatar_camera_count > 1 {
        return "none";
    }
    if ctx.local_avatar_camera_count == 1 {
        return "avatar";
    }
    if ctx.authored_camera_count > 0 {
        return "first";
    }
    if ctx.host_requests_generated {
        return "generated";
    }
    return "none";
}
