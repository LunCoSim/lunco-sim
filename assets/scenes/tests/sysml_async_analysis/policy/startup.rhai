fn install_policies(policies) {
    let results = [];
    for policy in policies {
        results.push(install_manifest_policy(
            policy.hook,
            policy.entry,
            policy.source,
            policy.policy_file,
            policy.deterministic,
            policy.required,
        ));
    }
    results
}
