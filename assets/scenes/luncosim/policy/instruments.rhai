fn install(policies) {
    let entries = [];
    for policy in policies {
        entries.push(install_manifest_policy(policy.hook, policy.entry, policy.source,
            policy.policy_file, policy.deterministic, policy.required));
    }
    entries
}

fn instruments(facts) {
    if !facts.scene_mounted || !facts.instance_kinds.contains("signal_grid") { return []; }
    [#{id:"instruments", title:"Instruments", visible:false, restore:false,
       description:"Live measurements from the active vehicle or packing line.",
       scene_visible:true, interaction:"simulation",
       floating:[#{kind:"signal_grid", instance:1, corner:"top_right",
                   width:285, height:196, inset:12}]}]
}
