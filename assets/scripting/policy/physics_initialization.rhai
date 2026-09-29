// A validated terrain penetration pauses only its authored articulated object.
// The engine retains the authored pose, removes the object from admission, and
// lets independent scene physics and readiness continue.
fn physics_initialization_policy(facts) {
    let context = runtime_context;
    if context.scope != "twin" || context.cycle != "lifecycle"
        || context.phase != "preparation" || context.clock != "none"
        || context.generation == () || context.sequence != ()
        || context.time_seconds != () || context.delta_seconds != () {
        throw "physics initialization requires its Twin lifecycle preparation context";
    }
    if facts.policy != "strict-authored" {
        throw "the application initialization policy received an unknown selector";
    }
    if facts.status != "terrain_penetration"
        || facts.penetration_m == () || facts.penetration_m <= 0.0
        || facts.subject == () || facts.position.len() != 3
        || facts.assembly_member_count == () {
        throw "the application initialization policy requires measured terrain penetration facts";
    }
    "pause"
}
