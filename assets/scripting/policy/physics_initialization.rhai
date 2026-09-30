// Small wheel contact overlap at authored rest height is accepted by policy;
// deeper support overlap pauses only that authored articulated object. The
// engine retains the paused pose and lets independent scene physics continue.
const MAX_ACCEPTED_INITIAL_SUPPORT_OVERLAP_M = 0.05;

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
    if facts.penetration_m > MAX_ACCEPTED_INITIAL_SUPPORT_OVERLAP_M {
        return "pause";
    }
    "accept"
}
