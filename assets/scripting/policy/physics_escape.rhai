// The containment response when finite-world escape containment is enabled or
// a body develops a non-finite position or velocity. Rust applies this typed
// action to the live physics graph.
fn escaped_body_policy(ctx) {
    if runtime_context.scope != "core"
        || runtime_context.cycle != "simulation"
        || runtime_context.phase != "behavior"
        || runtime_context.clock != "simulation"
        || runtime_context.sequence == ()
        || runtime_context.delta_seconds == () {
        throw("physics.body_escape requires its classified fixed simulation context");
    }
    if ctx.kind == "finite_world_exit" || ctx.kind == "non_finite_state" {
        return "pause_object";
    }
    // Unknown states are held at the world boundary until an authored policy
    // explicitly handles them.
    "pause_world"
}
