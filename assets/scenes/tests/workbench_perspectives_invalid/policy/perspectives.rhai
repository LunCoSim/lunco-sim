fn install(policies) {
    let order = [];
    for policy in policies {
        order.push(install_manifest_policy(policy.hook, policy.entry, policy.source,
            policy.policy_file, policy.deterministic, policy.required));
    }
    order
}
fn assemble(facts) {
    let invalid = #{
        id: "invalid", title: "Invalid study", interaction: "editor",
        description: "This negative fixture requests an unavailable workbench capability.",
        center: ["missing-panel"],
    };
    [invalid]
}
