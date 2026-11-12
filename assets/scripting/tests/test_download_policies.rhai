// Production Rhai coverage for the generic download and consent seams.
// Rust supplies facts and low-level effects; Rhai owns the returned policy and
// presentation decisions.
fn provisioned_view(facts) {
    let result = invoke_hook("assets.provision", [facts]);
    if !result.ok { throw "assets.provision failed: " + result.error; }
    result.value
}

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
    datasets: [#{
        id: "twin/test/texture",
        key: "texture",
        name: "Texture",
        required_for_scene: true, recommended: false,
        processed: false,
        inputs_ready: false,inputs:[#{id:"source",name:"Texture",url:"https://example.invalid/texture.png",state:"missing",error:()}],
        state: #{ kind: "missing", detail: () },
    }],
}]);
checks.push(t_true(provisioning.ok == true
        && provisioning.value.visible == true
        && provisioning.value.rows.len() == 1
        && provisioning.value.rows[0].action == "request"
        && provisioning.value.actions.len() == 1
        && provisioning.value.actions[0].action == "dismiss",
    "Rhai owns consent visibility, rows, and semantic actions"));

let facts = #{
    scope: "twin", scope_id: "twin:test", owner: "test", interactive: true,
    datasets: [],
};
for state in [
    #{kind: "queued", detail: ()},
    #{kind: "downloading", detail: (), bytes_done: 1048576, bytes_total: 4194304},
    #{kind: "processing", detail: "dem"},
    #{kind: "cancelling", detail: ()},
    #{kind: "failed", detail: "checksum mismatch"},
    #{kind: "cancelled", detail: ()},
] {
    facts.datasets = [#{id: "twin/test/data", key: "data", name: "Terrain",
        required_for_scene: true, recommended: false, processed: true, inputs_ready:false,inputs:[#{id:"source",name:"Terrain",url:"https://example.invalid/terrain.tif",state:"missing",error:()}], state: state}];
    let view = provisioned_view(facts);
    checks.push(t_true(view.visible && view.rows.len() == 1,
        "consent retains " + state.kind + " work without another missing row"));
    checks.push(t_eq(view.rows[0].scope, "twin:test", "row actions retain their exact scope"));
    if state.kind == "downloading" {
        checks.push(t_true(view.rows[0].status.contains("25%")
            && view.rows[0].action == "cancel", "download shows real progress and cancellation"));
    }
    if state.kind == "failed" {
        checks.push(t_true(view.rows[0].detail.contains("checksum mismatch")
            && view.rows[0].action_label == "Retry download", "failure retains its cause and retry action"));
    }
}
facts.datasets[0].inputs_ready=true;
facts.datasets[0].inputs[0].state="ready";
facts.datasets[0].state = #{kind: "missing", detail: ()};
let pending=provisioned_view(facts).rows[0];
checks.push(t_true(pending.status.contains("Downloaded") && pending.action=="process" && pending.action_label=="Prepare" && pending.rebuild_action=="", "stale or missing output offers preparation without a duplicate rebuild action"));
checks.push(t_eq(pending.source_url,"https://example.invalid/terrain.tif","the source link preserves the authored address"));
facts.datasets[0].state = #{kind: "installed", detail: ()};
let complete = provisioned_view(facts);
checks.push(t_true(complete.visible && complete.rows.len() == 1
    && complete.title == "Data ready" && complete.actions[0].action == "dismiss",
    "completed downloads retain an explicit completion view"));
checks.push(t_true(complete.rows[0].stages_display=="none" && complete.rows[0].min_height=="80px","ready rows use compact summaries"));
checks.push(t_eq(complete.rows[0].rebuild_action,"rebuild","only an installed processed artifact offers forced rebuild"));
facts.simulation=#{pending:[],world_hold:false,blockers:[],playing:false,fault:()};
let paused=provisioned_view(facts);
checks.push(t_true(paused.simulation_status=="Simulation paused" && paused.actions[0].action=="dismiss","completion preserves intentional pause and never starts transport"));
facts.datasets[0].state=#{kind:"missing",detail:()};
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("Source files are downloaded"),"downloaded files wait for preparation without claiming work is running"));
facts.datasets[0].inputs_ready=false;
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("Download the required files"),"missing files explain the required action"));
facts.datasets[0].state=#{kind:"queued",detail:()};
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("continue in the background"),"active work leaves the application available"));
facts.datasets[0].state=#{kind:"failed",detail:"checksum mismatch"};
checks.push(t_true(provisioned_view(facts).simulation_detail.contains("then retry"),"failed required data retains an explicit recovery step"));
facts.datasets[0].inputs_ready=true;
facts.datasets[0].state=#{kind:"installed",detail:()};
facts.simulation.pending=["collision support"];
facts.simulation.world_hold=true;
let held=provisioned_view(facts);
checks.push(t_true(held.title=="Preparing simulation" && held.simulation_status.contains("held"),"prepared data is distinct from physics readiness"));
facts.simulation.fault=#{subject:"Physics",detail:"Initialization failed"};
checks.push(t_true(provisioned_view(facts).title=="Simulation needs attention","a simulation failure is not reported ready merely because data is ready"));
facts.simulation=();
checks.push(t_true(provisioned_view(facts).visible,
    "nonrecommended data remains mandatory when referenced by the scene"));
facts.datasets[0].required_for_scene=false;
facts.datasets[0].recommended=true;
checks.push(t_true(!provisioned_view(facts).visible && provisioned_view(facts).rows.len()==0,
    "recommendations for other scenes do not open consent"));
facts.include_optional=true;
let optional=provisioned_view(facts);
checks.push(t_true(optional.rows.len()==1 && optional.rows[0].label.starts_with("Recommended"), "optional Twin data remains available in Settings"));
facts.datasets[0].name="DejaVu Sans Regular";
facts.datasets[0].recommended=false;
let optional_font=provisioned_view(facts);
checks.push(t_true(optional_font.rows.len()==1
    && optional_font.rows[0].label.starts_with("Optional · DejaVu Sans Regular"),
    "the fallback font uses the ordinary explicit-download label"));
facts.include_optional=false;
facts.datasets[0].required_for_scene=true;
facts.datasets[0].state = #{kind: "downloading", detail: (), bytes_done: 1048576, bytes_total: 0};
let unknown_total = provisioned_view(facts).rows[0].status;
checks.push(t_true(unknown_total.contains("MiB received") && !unknown_total.contains("%"),
    "unknown download length does not invent a percentage"));
facts.datasets[0].state = #{kind: "invalid", detail: ()};
checks.push(t_true(invoke_hook("assets.provision", [facts]).ok == false,
    "an invalid dataset phase is rejected visibly"));

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
