// Rust computes and verifies content identity. This policy only chooses what
// to do with a verified hit; source, recipe, dependency, compiler, and
// parameter changes always remain misses and rebuild through the owner.
fn artifact_cache_policy(facts) {
    if facts.cache_hit {
        return #{
            action: "reuse",
            reason: "the requested content key matches a validated artifact"
        };
    }
    #{
        action: "rebuild",
        reason: "no validated artifact exists for the requested content key"
    }
}
