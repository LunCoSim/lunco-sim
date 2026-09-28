// Rust resolves the uniquely marked policy manifest and its compiled hooks;
// this script returns the installation order as manifest hook identifiers.
fn install_policies(policies) {
    let order = [];
    for policy in policies {
        order.push(#{ hook: policy.hook });
    }
    order
}
