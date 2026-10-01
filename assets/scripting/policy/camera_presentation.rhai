// Application-level presentation policy.
//
// CameraTrack and explicit operator/director selections take precedence. With
// neither, prefer the unique local embodiment, then the first authored camera
// in stable USD-path order. Avatar provisioning is an independent policy.
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
    return "none";
}

// Avatar availability does not change director or operator camera ownership.
fn scene_avatar(ctx) {
    if runtime_context.scope != "application"
        || runtime_context.cycle != "presentation"
        || runtime_context.phase != "preparation"
        || runtime_context.clock != "presentation" {
        throw "camera.scene_avatar requires Application/Presentation/Preparation context";
    }
    if ctx.local_avatar_camera_count > 0 { return #{ create: false }; }
    #{ create: true, bindings: [
        ["forward", "forward", 1.0], ["backward", "forward", -1.0],
        ["right", "side", 1.0], ["left", "side", -1.0],
        ["yaw_right", "up", 1.0], ["yaw_left", "up", -1.0],
        ["speed_boost", "speed_boost", 1.0],
    ] }
}
