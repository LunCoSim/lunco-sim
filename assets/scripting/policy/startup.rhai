// The single application policy bootstrap. Rust resolves index.toml and passes
// this function the typed, storage-resolved policy records; this script owns
// the installation order and reports each result without throwing.
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
