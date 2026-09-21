// The default containment response for bodies that leave the authored static-
// world bounds or develop a non-finite position or velocity. Rust applies this
// typed action to the live physics graph.
fn escaped_body_policy(ctx) {
    if ctx.kind == "finite_world_exit" || ctx.kind == "non_finite_state" {
        return "pause_object";
    }
    // Unknown states are held at the world boundary until an authored policy
    // explicitly handles them.
    "pause_world"
}
