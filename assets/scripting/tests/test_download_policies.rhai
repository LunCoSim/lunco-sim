// Production Rhai coverage for the generic download and consent seams.
// Rust supplies facts and low-level effects; Rhai owns the returned policy and
// presentation decisions.

let checks = [];
let hooks = list_hooks();
let provisioning_hook = ();
let download_prepare_hook = ();
for hook in hooks {
    if hook.id == "assets.provision" { provisioning_hook = hook; }
    if hook.id == "assets.download.prepare" { download_prepare_hook = hook; }
}

checks.push(t_true(provisioning_hook != ()
        && provisioning_hook.declared == true
        && provisioning_hook.parameters.len() == 1
        && provisioning_hook.parameters[0].type == "map"
        && provisioning_hook.output == "map",
    "consent has a typed authored view-model contract"));
let provisioning = invoke_hook("assets.provision", [#{
    scope: "twin",
    scope_id: "twin:test",
    owner: "test",
    interactive: true,
    show_on_start: true,
    suppressed: false,
    suppressible: true,
    datasets: [#{
        id: "twin/test/texture",
        key: "texture",
        name: "Texture",
        recommended: true,
        processed: false,
        state: #{ kind: "missing", detail: () },
    }],
}]);
checks.push(t_true(provisioning.ok == true
        && provisioning.value.visible == true
        && provisioning.value.rows.len() == 1
        && provisioning.value.rows[0].action == "request"
        && provisioning.value.actions[0].action == "request_all"
        && provisioning.value.actions[1].action == "dismiss"
        && provisioning.value.actions[2].action == "suppress",
    "Rhai owns consent visibility, rows, and semantic actions"));

checks.push(t_true(download_prepare_hook != ()
        && download_prepare_hook.declared == true
        && download_prepare_hook.parameters.len() == 1
        && download_prepare_hook.parameters[0].type == "map"
        && download_prepare_hook.output == "map",
    "download preparation has a typed generic policy contract"));
let download_policy = invoke_hook("assets.download.prepare", [#{
    key: "texture",
    name: "Texture",
    url: "https://example.invalid/texture.zip",
    destination: "textures/texture.zip",
    twin_scoped: true,
    shared_cache: false,
    destination_exists: false,
    version: (),
    expected_sha256: (),
    has_process: false,
}]);
checks.push(t_true(download_policy.ok == true
        && download_policy.value.allow_network == true
        && download_policy.value.cache == "use_existing"
        && download_policy.value.install_mode == "keep_existing"
        && download_policy.value.backup == "discard_after_commit"
        && download_policy.value.retain_max_count == 0,
    "Rhai owns the default download mode"));

t_report(checks);
