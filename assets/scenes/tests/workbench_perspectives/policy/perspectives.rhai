fn install(policies) {
    let order = [];
    for policy in policies {
        order.push(install_manifest_policy(policy.hook, policy.entry, policy.source,
            policy.policy_file, policy.deterministic, policy.required));
    }
    order
}
fn assemble(facts) {
    let plot = ();
    for panel in facts.instance_panels {
        if panel.id == "modelica_plot" && panel.canonical_instance != () {
            let kind = panel.id;
            let instance = panel.canonical_instance;
            plot = #{ kind: kind, instance: instance, slot: "center" };
        }
    }
    if plot == () { return []; }
    let study = #{
        id: "power-study", title: "Power study", interaction: "editor",
        description: "Compare power-system signals from this Twin in the central graph. Select channels in Telemetry and inspect exact current values in Ports.",
        left: ["telemetry_browser"], right: ["port_inspector"], instances: [plot],
    };
    [study]
}
